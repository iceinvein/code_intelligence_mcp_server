//! Code Intelligence MCP Server - Main entry point

#![allow(unexpected_cfgs)]

use rust_mcp_sdk::{
    error::{McpSdkError, SdkResult},
    schema::{
        Implementation, InitializeResult, ProtocolVersion, ServerCapabilities,
        ServerCapabilitiesTools,
    },
};
use std::sync::Arc;
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use code_intelligence_mcp_server::cli;
use code_intelligence_mcp_server::embeddings::{
    create_embedder, default_embedding_dim, DeferredEmbedder, Embedder, TruncatingEmbedder,
};

/// Type alias for the (embedder, optional-deferred-slot) pair returned by embedder creation.
type EmbedderWithSlot = (
    Box<dyn Embedder + Send>,
    Option<std::sync::Arc<std::sync::Mutex<Option<Box<dyn Embedder + Send>>>>>,
);

/// Optionally wrap an embedder with Matryoshka truncation.
fn maybe_truncate(
    embedder: Box<dyn Embedder + Send>,
    truncate_dim: Option<usize>,
) -> anyhow::Result<Box<dyn Embedder + Send>> {
    match truncate_dim {
        Some(dim) => Ok(Box::new(TruncatingEmbedder::new(embedder, dim)?)),
        None => Ok(embedder),
    }
}

#[tokio::main]
async fn main() -> SdkResult<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let cli_args = cli::parse_args(&args);

    if cli_args.help {
        cli::print_help();
        return Ok(());
    }
    if cli_args.version {
        cli::print_version();
        return Ok(());
    }

    // Lifecycle subcommands run synchronously without booting the daemon.
    if !matches!(
        cli_args.command,
        code_intelligence_mcp_server::cli::Command::Run
    ) {
        return dispatch_subcommand(cli_args.command).map_err(|e| McpSdkError::Internal {
            description: e.to_string(),
        });
    }

    // Set up file logging to global ~/.code-intelligence/logs directory
    let global_dir = code_intelligence_mcp_server::config::get_data_dir();
    let logs_dir = global_dir.join("logs");

    // Create logs directory if it doesn't exist
    std::fs::create_dir_all(&logs_dir).map_err(|err| McpSdkError::Internal {
        description: format!("Failed to create logs directory: {}", err),
    })?;

    // Clean up log files older than 7 days
    code_intelligence_mcp_server::logging::cleanup_old_logs(&logs_dir, 7);

    // Create a daily rotating file appender for global server log
    let file_appender = tracing_appender::rolling::daily(&logs_dir, "server.log");
    let (non_blocking_file, _guard) = tracing_appender::non_blocking(file_appender);

    // Create a daily rotating file appender for MCP access log
    let access_appender = tracing_appender::rolling::daily(&logs_dir, "access.log");
    let (non_blocking_access, _access_guard) = tracing_appender::non_blocking(access_appender);

    // Set up layered subscriber with stderr, file, access log, and in-process
    // broadcast for the SSE /api/logs/stream endpoint.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let log_broadcaster = code_intelligence_mcp_server::log_broadcast::LogBroadcaster::new();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_ansi(true))
        .with(fmt::layer().with_writer(non_blocking_file).with_ansi(false))
        .with(
            fmt::layer()
                .with_writer(non_blocking_access)
                .with_ansi(false)
                .with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target("mcp_access", tracing::Level::INFO),
                ),
        )
        .with(
            fmt::layer()
                .with_writer(log_broadcaster.make_writer())
                .with_ansi(false),
        )
        .init();

    // Keep guards alive for the duration of the program
    std::mem::forget(_guard);
    std::mem::forget(_access_guard);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        logs_dir = %logs_dir,
        "Starting code-intelligence-mcp-server"
    );

    // Raise the soft FD limit toward the hard limit so parallel indexing
    // (Tantivy mmap, LanceDB fragments, pooled SQLite WAL connections, plus
    // the embedding/reranker/LLM models) doesn't trip the macOS default of
    // 256 open files.
    match code_intelligence_mcp_server::os::raise_fd_limit_to_hard() {
        Ok((old_soft, new_soft, hard)) if new_soft > old_soft => {
            info!(old_soft, new_soft, hard, "Raised RLIMIT_NOFILE soft limit");
        }
        Ok((soft, _, hard)) => {
            debug!(soft, hard, "RLIMIT_NOFILE already at or above hard limit");
        }
        Err(err) => {
            tracing::warn!(error = %err, "Failed to raise RLIMIT_NOFILE; indexing may hit FD limits under load");
        }
    }

    run_standalone(
        cli_args.host.as_deref(),
        cli_args.port,
        cli_args.discovery_port,
        log_broadcaster,
    )
    .await
}

fn dispatch_subcommand(cmd: code_intelligence_mcp_server::cli::Command) -> anyhow::Result<()> {
    use code_intelligence_mcp_server::cli::Command;
    use code_intelligence_mcp_server::install;
    match cmd {
        Command::Run => unreachable!("dispatch_subcommand should not see Run"),
        Command::Install(opts) => install::handle_install(opts),
        Command::Uninstall => install::handle_uninstall(),
        Command::Start => install::handle_start(),
        Command::Stop => install::handle_stop(),
        Command::Status => install::handle_status(),
        Command::Migrate(opts) => install::handle_migrate(opts),
    }
}

async fn run_standalone(
    host: Option<&str>,
    port: Option<u16>,
    discovery_port: Option<u16>,
    log_broadcaster: code_intelligence_mcp_server::log_broadcast::LogBroadcaster,
) -> SdkResult<()> {
    let standalone_config =
        code_intelligence_mcp_server::config::StandaloneConfig::load(host, port, discovery_port)
            .map_err(|e| McpSdkError::Internal {
                description: e.to_string(),
            })?;

    // Ensure data directories exist
    let data_dir = &standalone_config.data_dir;
    std::fs::create_dir_all(data_dir.join("repos").as_std_path()).map_err(|e| {
        McpSdkError::Internal {
            description: format!("Failed to create data dir: {}", e),
        }
    })?;
    std::fs::create_dir_all(data_dir.join("logs").as_std_path()).map_err(|e| {
        McpSdkError::Internal {
            description: format!("Failed to create logs dir: {}", e),
        }
    })?;

    // Create shared embedder (loaded once, shared across all repos).
    // For llamacpp backend, use DeferredEmbedder so the HTTP server starts immediately.
    let (embedder, standalone_deferred_slot): EmbedderWithSlot =
        match standalone_config.embeddings_backend {
            code_intelligence_mcp_server::config::EmbeddingsBackend::Hash => {
                let base = create_embedder(
                    standalone_config.embeddings_backend,
                    standalone_config.embeddings_model_dir.as_deref(),
                    standalone_config.embeddings_device,
                    standalone_config.hash_embedding_dim,
                )
                .map_err(|e| McpSdkError::Internal {
                    description: format!("Failed to create embedder: {}", e),
                })?;
                let e = maybe_truncate(base, standalone_config.embedding_truncate_dim).map_err(
                    |e| McpSdkError::Internal {
                        description: format!("Failed to create truncating embedder: {}", e),
                    },
                )?;
                info!("Created hash embedder with dimension: {}", e.dim());
                (e, None)
            }
            code_intelligence_mcp_server::config::EmbeddingsBackend::LlamaCpp => {
                let dim = default_embedding_dim(
                    standalone_config.embeddings_backend,
                    standalone_config.hash_embedding_dim,
                    standalone_config.embedding_truncate_dim,
                    None,
                );
                let deferred = DeferredEmbedder::new(dim);
                let slot = deferred.inner_slot();
                info!(
                    dim,
                    "Created deferred embedder — model will load in background"
                );
                (Box::new(deferred), Some(slot))
            }
        };

    // Create registry and session manager
    let registry = code_intelligence_mcp_server::registry::RepoRegistry::new(
        data_dir.join("repos/registry.json"),
        data_dir.join("repos"),
    );

    let job_registry = code_intelligence_mcp_server::server::jobs::new_job_registry();
    code_intelligence_mcp_server::server::jobs::spawn_job_eviction_loop(job_registry.clone());

    let session_manager = code_intelligence_mcp_server::session::SessionManager::new(
        standalone_config.clone(),
        registry,
        embedder,
        Some(job_registry.clone()),
    )
    .await
    .map_err(|e| McpSdkError::Internal {
        description: e.to_string(),
    })?;
    let session_manager = Arc::new(session_manager);
    session_manager.spawn_eviction_loop();

    let server_details = InitializeResult {
        server_info: Implementation {
            name: "code-intelligence".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Code Intelligence MCP (Standalone)".into()),
            description: Some("Multi-repo code intelligence server".into()),
            icons: vec![],
            website_url: None,
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            tasks: Some(code_intelligence_mcp_server::server::task_capabilities()),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: Some(code_intelligence_mcp_server::server::server_instructions().into()),
        meta: None,
    };

    let session_repos = code_intelligence_mcp_server::server::standalone::new_session_repos();
    let pending_repos = code_intelligence_mcp_server::server::mcp_proxy::new_pending_repos();
    let bound_repos = code_intelligence_mcp_server::server::mcp_proxy::new_bound_repos();
    code_intelligence_mcp_server::server::standalone::spawn_session_eviction_loop(
        session_repos.clone(),
        bound_repos.clone(),
    );
    let handler = code_intelligence_mcp_server::server::standalone::StandaloneHandler::new(
        session_manager.clone(),
        server_details.clone(),
        session_repos.clone(),
        pending_repos.clone(),
        bound_repos.clone(),
    );
    let bind_host = standalone_config.host.clone();
    let bind_port = standalone_config.port;
    // The SDK owns the axum router and exposes no hook to read the request
    // URI from inside the MCP transport. We bind it to an internal loopback
    // port and run a small proxy on the public port that captures `?repo=`
    // URL bindings on the way through. Offset by +100 so it never collides
    // with discovery/api ports (+1 and +2).
    let internal_mcp_port = bind_port.saturating_add(100);

    // Use SDK's hyper server for Streamable HTTP transport
    use rust_mcp_sdk::mcp_server::{hyper_server, HyperServerOptions, ToMcpServerHandler};
    let server = hyper_server::create_server(
        server_details,
        handler.to_mcp_server_handler(),
        HyperServerOptions {
            host: "127.0.0.1".to_string(),
            port: internal_mcp_port,
            task_store: Some(Arc::new(rust_mcp_sdk::task_store::InMemoryTaskStore::<
                rust_mcp_sdk::schema::schema_utils::ClientJsonrpcRequest,
                rust_mcp_sdk::schema::schema_utils::ResultFromServer,
            >::new(None))),
            client_task_store: None,
            ..Default::default()
        },
    );

    // Spawn background embedder loading for LlamaCpp backend in standalone mode.
    if let Some(slot) = standalone_deferred_slot {
        let model_dir = standalone_config.embeddings_model_dir.clone();
        let device = standalone_config.embeddings_device;
        let hash_dim = standalone_config.hash_embedding_dim;
        let truncate_dim = standalone_config.embedding_truncate_dim;
        tokio::spawn(async move {
            info!("Starting background embedding model download/load (standalone)...");
            let result = tokio::task::spawn_blocking(move || {
                let base = create_embedder(
                    code_intelligence_mcp_server::config::EmbeddingsBackend::LlamaCpp,
                    model_dir.as_deref(),
                    device,
                    hash_dim,
                )?;
                let embedder = maybe_truncate(base, truncate_dim)?;
                Ok::<_, anyhow::Error>(embedder)
            })
            .await;
            match result {
                Ok(Ok(real_embedder)) => {
                    let mut guard = slot.lock().expect("DeferredEmbedder mutex poisoned");
                    *guard = Some(real_embedder);
                    info!("Embedding model loaded — vector search is now available (standalone)");
                }
                Ok(Err(e)) => {
                    error!("Failed to load embedding model: {}. Vector search will remain unavailable.", e);
                }
                Err(e) => {
                    error!("Embedding model loading task panicked: {}. Vector search will remain unavailable.", e);
                }
            }
        });
    }

    // Spawn discovery endpoint (always in standalone mode)
    {
        let discovery_port = standalone_config
            .discovery_port
            .unwrap_or(bind_port.saturating_add(1));
        if let Err(e) = code_intelligence_mcp_server::server::discovery::spawn_discovery_server(
            &bind_host,
            bind_port,
            discovery_port,
        )
        .await
        {
            error!(
                "Failed to start discovery server: {}. Discovery endpoint will not be available.",
                e
            );
        }
    }

    // Spawn the MCP proxy on the public port. It forwards POST/GET/DELETE
    // /mcp to the internal SDK listener (port +100), reading `?repo=` URL
    // bindings on the way through and pairing them with the SDK-assigned
    // mcp-session-id on the way back.
    {
        let pr = pending_repos.clone();
        let br = bound_repos.clone();
        if let Err(e) = code_intelligence_mcp_server::server::mcp_proxy::spawn_mcp_proxy(
            &bind_host,
            bind_port,
            internal_mcp_port,
            "/mcp",
            pr,
            br,
        )
        .await
        {
            error!(
                "Failed to start MCP proxy: {}. ?repo= URL binding will not be available.",
                e
            );
        }
    }

    // Spawn JSON API endpoint on mcp_port + 2 (or +3 if collision with discovery).
    {
        let api_port = bind_port.saturating_add(2);
        if let Err(e) = code_intelligence_mcp_server::server::api::spawn_api_server(
            &bind_host,
            api_port,
            session_manager.clone(),
            session_repos.clone(),
            log_broadcaster.clone(),
            job_registry.clone(),
        )
        .await
        {
            error!(
                "Failed to start API server: {}. /api/* endpoints will not be available.",
                e
            );
        }
    }

    info!(
        host = %bind_host,
        port = bind_port,
        data_dir = %standalone_config.data_dir,
        "Starting standalone server on http://{}:{}",
        bind_host, bind_port,
    );

    server.start().await
}

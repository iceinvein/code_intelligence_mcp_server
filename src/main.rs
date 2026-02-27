//! Code Intelligence MCP Server - Main entry point

#![allow(unexpected_cfgs)]

use rust_mcp_sdk::{
    error::{McpSdkError, SdkResult},
    mcp_server::{server_runtime, McpServerOptions, ToMcpServerHandler},
    schema::{
        Implementation, InitializeResult, ProtocolVersion, ServerCapabilities,
        ServerCapabilitiesTools,
    },
    McpServer, StdioTransport, TransportOptions,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use code_intelligence_mcp_server::cli;
use code_intelligence_mcp_server::config::Config;
use code_intelligence_mcp_server::embeddings::{create_embedder, default_embedding_dim, DeferredEmbedder, Embedder, TruncatingEmbedder};

/// Type alias for the (embedder, optional-deferred-slot) pair returned by embedder creation.
type EmbedderWithSlot = (Box<dyn Embedder + Send>, Option<std::sync::Arc<std::sync::Mutex<Option<Box<dyn Embedder + Send>>>>>);

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
use code_intelligence_mcp_server::handlers::AppState;
use code_intelligence_mcp_server::indexer::pipeline::IndexPipeline;
use code_intelligence_mcp_server::leader::{LeaderElection, Role};
use code_intelligence_mcp_server::metrics::{spawn_metrics_server, MetricsRegistry};
use code_intelligence_mcp_server::path::Utf8Path;
use code_intelligence_mcp_server::reranker::create_reranker;
use code_intelligence_mcp_server::llm::create_llm_generator;
use code_intelligence_mcp_server::retrieval::hyde::HypotheticalCodeGenerator;
use code_intelligence_mcp_server::retrieval::Retriever;
use code_intelligence_mcp_server::server::CodeIntelligenceHandler;
use code_intelligence_mcp_server::storage::sqlite::SqliteStore;
use code_intelligence_mcp_server::storage::tantivy::TantivyIndex;
use code_intelligence_mcp_server::storage::vector::LanceDbStore;

#[cfg(feature = "web-ui")]
mod web_ui;

#[cfg(feature = "web-ui")]
fn env_true(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "y"))
        .unwrap_or(false)
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

    // Set up layered subscriber with stderr, file, and access log output
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(true)
        )
        .with(
            fmt::layer()
                .with_writer(non_blocking_file)
                .with_ansi(false)
        )
        .with(
            fmt::layer()
                .with_writer(non_blocking_access)
                .with_ansi(false)
                .with_filter(tracing_subscriber::filter::Targets::new()
                    .with_target("mcp_access", tracing::Level::INFO))
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

    // Branch into standalone or embedded mode
    if cli_args.standalone {
        return run_standalone(
            cli_args.host.as_deref(),
            cli_args.port,
            cli_args.discovery_port,
        ).await;
    }

    if let Err(err) = run_embedded().await {
        error!(error = %err, "Server exited with error");
        return Err(err);
    }
    Ok(())
}

async fn run_standalone(
    host: Option<&str>,
    port: Option<u16>,
    discovery_port: Option<u16>,
) -> SdkResult<()> {
    let standalone_config = code_intelligence_mcp_server::config::StandaloneConfig::load(host, port, discovery_port)
        .map_err(|e| McpSdkError::Internal { description: e.to_string() })?;

    // Ensure data directories exist
    let data_dir = &standalone_config.data_dir;
    std::fs::create_dir_all(data_dir.join("repos").as_std_path())
        .map_err(|e| McpSdkError::Internal { description: format!("Failed to create data dir: {}", e) })?;
    std::fs::create_dir_all(data_dir.join("logs").as_std_path())
        .map_err(|e| McpSdkError::Internal { description: format!("Failed to create logs dir: {}", e) })?;

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
                ).map_err(|e| McpSdkError::Internal { description: format!("Failed to create embedder: {}", e) })?;
                let e = maybe_truncate(base, standalone_config.embedding_truncate_dim)
                    .map_err(|e| McpSdkError::Internal { description: format!("Failed to create truncating embedder: {}", e) })?;
                info!("Created hash embedder with dimension: {}", e.dim());
                (e, None)
            }
            code_intelligence_mcp_server::config::EmbeddingsBackend::LlamaCpp => {
                let dim = default_embedding_dim(standalone_config.embeddings_backend, standalone_config.hash_embedding_dim, standalone_config.embedding_truncate_dim);
                let deferred = DeferredEmbedder::new(dim);
                let slot = deferred.inner_slot();
                info!(dim, "Created deferred embedder — model will load in background");
                (Box::new(deferred), Some(slot))
            }
        };

    // Create registry and session manager
    let registry = code_intelligence_mcp_server::registry::RepoRegistry::new(
        data_dir.join("repos/registry.json"),
        data_dir.join("repos"),
    );

    let session_manager = code_intelligence_mcp_server::session::SessionManager::new(
        standalone_config.clone(), registry, embedder,
    )
    .await
    .map_err(|e| McpSdkError::Internal { description: e.to_string() })?;
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
        instructions: None,
        meta: None,
    };

    let handler = code_intelligence_mcp_server::server::standalone::StandaloneHandler::new(
        session_manager, server_details.clone(),
    );
    let bind_host = standalone_config.host.clone();
    let bind_port = standalone_config.port;

    // Use SDK's hyper server for Streamable HTTP transport
    use rust_mcp_sdk::mcp_server::{hyper_server, HyperServerOptions, ToMcpServerHandler};
    let server = hyper_server::create_server(
        server_details,
        handler.to_mcp_server_handler(),
        HyperServerOptions {
            host: bind_host.clone(),
            port: bind_port,
            task_store: Some(Arc::new(
                rust_mcp_sdk::task_store::InMemoryTaskStore::<
                    rust_mcp_sdk::schema::schema_utils::ClientJsonrpcRequest,
                    rust_mcp_sdk::schema::schema_utils::ResultFromServer,
                >::new(None),
            )),
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
            }).await;
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
            error!("Failed to start discovery server: {}. Discovery endpoint will not be available.", e);
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

async fn run_embedded() -> SdkResult<()> {
    let config = Config::from_env().map_err(|err| McpSdkError::Internal {
        description: err.to_string(),
    })?;

    // Ensure per-repo data directory exists
    std::fs::create_dir_all(config.db_path.parent().unwrap_or(&config.db_path))
        .map_err(|err| McpSdkError::Internal {
            description: format!("Failed to create repo data directory: {}", err),
        })?;

    // Clean up per-repo log files older than 7 days
    {
        let repo_logs_dir = config.db_path.parent()
            .unwrap_or(&config.db_path)
            .join("logs");
        code_intelligence_mcp_server::logging::cleanup_old_logs(&repo_logs_dir, 7);
    }

    // Register this repo in the shared registry (non-fatal on error)
    {
        let data_dir = code_intelligence_mcp_server::config::get_data_dir();
        let registry = code_intelligence_mcp_server::registry::RepoRegistry::new(
            data_dir.join("repos/registry.json"),
            data_dir.join("repos"),
        );
        if let Err(e) = registry.register(config.base_dir.as_str()) {
            tracing::warn!("Failed to register repo in registry: {}", e);
        }
    }

    // Hint about legacy data directories
    let legacy_cimcp = std::path::Path::new(&std::env::var("HOME").unwrap_or_default())
        .join(".cimcp");
    if legacy_cimcp.exists() {
        info!(
            path = %legacy_cimcp.display(),
            "Legacy ~/.cimcp directory detected. Data now stored under ~/.code-intelligence/. \
             You can safely delete ~/.cimcp/ after verifying the new location works."
        );
    }

    let sqlite = SqliteStore::open(&config.db_path).map_err(|err| McpSdkError::Internal {
        description: err.to_string(),
    })?;
    sqlite.init().map_err(|err| McpSdkError::Internal {
        description: err.to_string(),
    })?;

    // Create embedder — for hash backend (instant), load synchronously.
    // For llamacpp backend (downloads ~531 MB on first run), use a DeferredEmbedder
    // so the MCP server starts immediately and degrades to BM25-only until ready.
    let (embedder, deferred_slot): EmbedderWithSlot =
        match config.embeddings_backend {
            code_intelligence_mcp_server::config::EmbeddingsBackend::Hash => {
                let base = create_embedder(
                    config.embeddings_backend,
                    config.embeddings_model_dir.as_deref(),
                    config.embeddings_device,
                    config.hash_embedding_dim,
                )
                .map_err(|err| McpSdkError::Internal {
                    description: format!("Failed to create embedder: {}", err),
                })?;
                let e = maybe_truncate(base, config.embedding_truncate_dim)
                    .map_err(|err| McpSdkError::Internal { description: format!("Failed to create truncating embedder: {}", err) })?;
                info!("Created hash embedder with dimension: {}", e.dim());
                (e, None)
            }
            code_intelligence_mcp_server::config::EmbeddingsBackend::LlamaCpp => {
                let dim = default_embedding_dim(config.embeddings_backend, config.hash_embedding_dim, config.embedding_truncate_dim);
                let deferred = DeferredEmbedder::new(dim);
                let slot = deferred.inner_slot();
                info!(dim, "Created deferred embedder — model will load in background");
                (Box::new(deferred), Some(slot))
            }
        };

    // --- Leader election ---
    let (is_leader_flag, role_rx, mut _leader_guard) = if config.leader_election_enabled {
        let repo_data_dir = config.db_path.parent().unwrap_or(&config.db_path);
        let mut election = LeaderElection::new(
            Utf8Path::new(repo_data_dir.as_str()),
            config.leader_heartbeat_interval_ms,
            config.leader_ttl_seconds,
        );
        let role = election.try_acquire().map_err(|err| McpSdkError::Internal {
            description: format!("Leader election failed: {}", err),
        })?;
        info!(role = %role, "Leader election result");
        let flag = election.is_leader_flag();
        let rx = election.role_receiver();
        (flag, rx, Some(election))
    } else {
        let flag = Arc::new(AtomicBool::new(true));
        let (_, rx) = tokio::sync::watch::channel(Role::Leader);
        (flag, rx, None)
    };
    let is_leader = is_leader_flag.load(Ordering::SeqCst);

    // --- Tantivy initialization (leader vs follower) ---
    let tantivy = if is_leader {
        TantivyIndex::open_or_create(&config.tantivy_index_path)
    } else {
        // Follower: retry open_readonly up to 10s in case leader hasn't created the index yet
        let mut last_err = None;
        let mut opened = None;
        for attempt in 0..5 {
            match TantivyIndex::open_readonly(&config.tantivy_index_path) {
                Ok(t) => { opened = Some(t); break; }
                Err(e) => {
                    last_err = Some(e);
                    if attempt < 4 {
                        info!(attempt = attempt + 1, "Tantivy index not ready, retrying in 2s...");
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    }
                }
            }
        }
        opened.ok_or_else(|| last_err.unwrap_or_else(|| anyhow::anyhow!("Failed to open Tantivy index")))
    }.map_err(|err| McpSdkError::Internal {
        description: err.to_string(),
    })?;

    let vector_dim = embedder.dim();
    let embedder: Arc<Mutex<Box<dyn Embedder + Send>>> = Arc::new(Mutex::new(embedder));

    let lancedb = LanceDbStore::connect(&config.vector_db_path)
        .await
        .map_err(|err| McpSdkError::Internal {
            description: err.to_string(),
        })?;

    // Migrate vector table if dimensions have changed (e.g., 384 -> 768)
    let needs_reindex = lancedb
        .migrate_vector_table("symbols", vector_dim)
        .await
        .map_err(|err| McpSdkError::Internal {
            description: format!("Failed to migrate vector table: {}", err),
        })?;

    let vectors = lancedb
        .open_or_create_table("symbols", vector_dim)
        .await
        .map_err(|err| McpSdkError::Internal {
            description: err.to_string(),
        })?;

    let config = Arc::new(config);
    let tantivy = Arc::new(tantivy);
    let vectors = Arc::new(vectors);

    // Create metrics registry
    let metrics = Arc::new(MetricsRegistry::new().map_err(|err| McpSdkError::Internal {
        description: format!("Failed to create metrics registry: {}", err),
    })?);

    // Spawn metrics server if enabled (non-fatal — server works without metrics)
    let _metrics_handle = if config.metrics_enabled {
        match spawn_metrics_server(Arc::clone(&metrics), config.metrics_port).await {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!("Metrics server failed to start: {}. Continuing without metrics.", e);
                None
            }
        }
    } else {
        None
    };

    // Create reranker when enabled and model is available
    let reranker = create_reranker(
        config.reranker_enabled,
        config.reranker_model_path.as_deref(),
        config.reranker_cache_dir.as_deref(),
        config.reranker_top_k,
    )
    .map_err(|err| McpSdkError::Internal {
        description: format!("Failed to create reranker: {}", err),
    })?;

    // Create HyDE generator if enabled.
    //
    // For the "local" backend we load the same Qwen2.5-Coder-1.5B model used
    // by the description worker. Model loading is blocking (~1-2 s on warm
    // cache; downloads ~1.1 GB on first run), so we hand it off to
    // `spawn_blocking`. A failure to load the model is non-fatal: the server
    // starts normally and HyDE searches silently degrade to BM25+vector without
    // the hypothetical-document step.
    let hyde_generator = if config.hyde_enabled {
        let mut gen = HypotheticalCodeGenerator::new(
            config.hyde_llm_backend.clone(),
            config.hyde_api_key.clone(),
            config.hyde_max_tokens,
        );

        if config.hyde_llm_backend == "local" {
            let llm_config = config.clone();
            match tokio::task::spawn_blocking(move || create_llm_generator(&llm_config)).await {
                Ok(Ok(Some(llm))) => {
                    tracing::info!("HyDE local LLM loaded — on-device hypothetical code generation enabled");
                    gen = gen.with_local_llm(llm);
                }
                Ok(Ok(None)) => {
                    tracing::warn!(
                        "HyDE local LLM not available (LLM_ENABLED=false or model not found). Disabling HyDE."
                    );
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        "Failed to create HyDE local LLM: {}. Disabling HyDE.", e
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "HyDE local LLM loading task panicked: {}. Disabling HyDE.", e
                    );
                }
            }
            // If the local LLM wasn't attached, don't create a broken generator
            if !gen.has_local_llm() {
                None
            } else {
                Some(gen)
            }
        } else {
            Some(gen)
        }
    } else {
        None
    };

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        Arc::clone(&metrics),
    );
    let retriever = Retriever::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        reranker,
        hyde_generator,
        Arc::clone(&metrics),
    );

    let state = Arc::new(AppState {
        config: config.clone(),
        indexer,
        retriever,
        sqlite: Arc::new(sqlite),
        is_leader: is_leader_flag.clone(),
        role_rx,
        mcp_runtime: Arc::new(once_cell::sync::OnceCell::new()),
    });

    // Trigger automatic re-index if vector dimension migration occurred.
    // Runs in background so the MCP server starts immediately — large repos
    // (500+ files) can take 60+ seconds to re-index, which would exceed
    // Claude Code's MCP connection timeout if done synchronously.
    if needs_reindex {
        tracing::info!(
            "Vector table migration completed. Clearing fingerprints and similarity clusters to force full re-index..."
        );
        // Clear fingerprints so the indexer treats all files as new
        if let Err(e) = state.sqlite.clear_all_file_fingerprints() {
            tracing::error!("Failed to clear fingerprints after vector migration: {}", e);
        }
        // Clear similarity clusters so embeddings are regenerated for all symbols
        if let Err(e) = state.sqlite.clear_similarity_clusters() {
            tracing::error!("Failed to clear similarity clusters after vector migration: {}", e);
        }
        let reindex_state = state.clone();
        tokio::spawn(async move {
            tracing::info!("Starting background re-index after vector dimension migration...");
            match reindex_state.indexer.index_all().await {
                Ok(stats) => {
                    tracing::info!(
                        "Automatic re-index completed successfully: indexed {} symbols from {} files",
                        stats.symbols_indexed,
                        stats.files_indexed
                    );
                }
                Err(err) => {
                    tracing::error!(
                        "Automatic re-index failed: {}. Please run 'refresh_index' manually.",
                        err
                    );
                }
            }
        });
    }

    // Recovery: regenerate vectors for orphaned symbols (e.g. after LanceDB data loss).
    // Runs in background so it doesn't block MCP server startup.
    if !needs_reindex {
        let orphan_check_state = state.clone();
        tokio::spawn(async move {
            let orphan_count = orphan_check_state
                .sqlite
                .list_symbols_without_similarity_clusters(1)
                .map(|v| v.len())
                .unwrap_or(0);
            if orphan_count > 0 {
                tracing::warn!(
                    "Found symbols without embeddings. Regenerating vectors in background..."
                );
                if let Err(e) = orphan_check_state
                    .indexer
                    .generate_embeddings_for_orphaned_symbols()
                    .await
                {
                    tracing::error!("Background vector regeneration failed: {}", e);
                } else {
                    tracing::info!("Background vector regeneration completed");
                }
            }
        });
    }

    // --- Background tasks gated on leader/follower role ---
    let cancel_token = tokio_util::sync::CancellationToken::new();

    if is_leader {
        // Leader: spawn watch loop and LLM description worker
        if state.config.watch_mode {
            state.indexer.spawn_watch_loop(cancel_token.clone());
        }

        if config.llm_enabled {
            let llm_config = config.clone();
            let llm_indexer = state.indexer.clone();
            let sampling_enabled = config.sampling_descriptions_enabled;
            let mcp_runtime_cell = state.mcp_runtime.clone();
            tokio::spawn(async move {
                let local_generator = match tokio::task::spawn_blocking(move || {
                    code_intelligence_mcp_server::llm::create_llm_generator(&llm_config)
                }).await {
                    Ok(Ok(Some(llm))) => llm,
                    Ok(Ok(None)) => {
                        tracing::debug!("LLM descriptions not available, skipping description worker");
                        return;
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("Failed to create LLM generator: {}", e);
                        return;
                    }
                    Err(e) => {
                        tracing::warn!("LLM generator task panicked: {}", e);
                        return;
                    }
                };

                // Wrap in FallbackLlmGenerator if sampling is enabled.
                // The mcp_runtime OnceCell may not be set yet (it's populated on the
                // first tool call from the client). FallbackLlmGenerator stores the
                // Arc<OnceCell<...>> and checks it on each generate() call, so once
                // the runtime becomes available, subsequent descriptions use sampling.
                let generator: std::sync::Arc<dyn code_intelligence_mcp_server::llm::LlmGenerator> = if sampling_enabled {
                    tracing::info!(
                        "MCP sampling enabled, FallbackLlmGenerator will use sampling once client connects"
                    );
                    std::sync::Arc::new(
                        code_intelligence_mcp_server::llm::sampling::FallbackLlmGenerator::new(
                            mcp_runtime_cell,
                            local_generator,
                        ),
                    )
                } else {
                    tracing::info!("MCP sampling descriptions disabled, using local LLM only");
                    local_generator
                };

                let desc_cancel = tokio_util::sync::CancellationToken::new();
                let _desc_handle = llm_indexer.spawn_description_worker(generator, desc_cancel);
                tracing::info!("LLM description worker spawned");
            });
        }

        // Leader: spawn heartbeat writer
        if let Some(ref guard) = _leader_guard {
            guard.spawn_heartbeat_writer(cancel_token.clone());
        }
    } else {
        // Follower: spawn periodic Tantivy reader reload (every 5s)
        let follower_tantivy = tantivy.clone();
        let follower_cancel = cancel_token.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = follower_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        if let Err(e) = follower_tantivy.reload_reader() {
                            tracing::warn!("Follower Tantivy reader reload failed: {}", e);
                        }
                    }
                }
            }
        });

        // Follower: spawn monitor for stale leader heartbeat
        if let Some(ref mut guard) = _leader_guard {
            guard.spawn_follower_monitor(cancel_token.clone());
        }

        // Follower: spawn promotion reactor
        let mut promotion_rx = state.role_rx.clone();
        let promotion_state = state.clone();
        tokio::spawn(async move {
            while promotion_rx.changed().await.is_ok() {
                if *promotion_rx.borrow() == Role::Leader {
                    tracing::warn!(
                        "Promoted to leader! Hot Tantivy writer upgrade not supported in v1. \
                         Recommend restarting this instance for full leader capabilities."
                    );
                    // Start watch loop on promotion if enabled
                    if promotion_state.config.watch_mode {
                        let watch_cancel = tokio_util::sync::CancellationToken::new();
                        promotion_state.indexer.spawn_watch_loop(watch_cancel);
                        tracing::info!("Watch loop started after leader promotion");
                    }
                    break;
                }
            }
        });
    }

    #[cfg(feature = "web-ui")]
    if env_true("WEB_UI") {
        web_ui::spawn(state.clone())
            .await
            .map_err(|err| McpSdkError::Internal {
                description: err.to_string(),
            })?;
    }

    let db_path_rel = config.path_relative_to_base(&config.db_path).ok();
    let vector_db_path_rel = config.path_relative_to_base(&config.vector_db_path).ok();
    let tantivy_index_path_rel = config
        .path_relative_to_base(&config.tantivy_index_path)
        .ok();

    debug!(
        base_dir = %config.base_dir,
        db_path = %config.db_path,
        db_path_rel = ?db_path_rel,
        vector_db_path = %config.vector_db_path,
        vector_db_path_rel = ?vector_db_path_rel,
        tantivy_index_path = %config.tantivy_index_path,
        tantivy_index_path_rel = ?tantivy_index_path_rel,
        embeddings_backend = ?config.embeddings_backend,
        embeddings_model_dir = ?config.embeddings_model_dir.as_ref().map(|p| p.to_string()),
        embeddings_device = ?config.embeddings_device,
        embedding_batch_size = config.embedding_batch_size,
        hash_embedding_dim = config.hash_embedding_dim,
        vector_search_limit = config.vector_search_limit,
        hybrid_alpha = config.hybrid_alpha,
        rank_vector_weight = config.rank_vector_weight,
        rank_keyword_weight = config.rank_keyword_weight,
        rank_exported_boost = config.rank_exported_boost,
        rank_index_file_boost = config.rank_index_file_boost,
        rank_test_penalty = config.rank_test_penalty,
        rank_popularity_weight = config.rank_popularity_weight,
        rank_popularity_cap = config.rank_popularity_cap,
        watch_mode = config.watch_mode,
        watch_debounce_ms = config.watch_debounce_ms,
        watch_min_index_interval_ms = config.watch_min_index_interval_ms,
        max_context_bytes = config.max_context_bytes,
        index_node_modules = config.index_node_modules,
        repo_roots = ?config.repo_roots,
        "Loaded config"
    );

    // Spawn background embedder loading for LlamaCpp backend.
    // The deferred_slot is None for hash backend (loaded synchronously above).
    if let Some(slot) = deferred_slot {
        let model_dir = config.embeddings_model_dir.clone();
        let device = config.embeddings_device;
        let hash_dim = config.hash_embedding_dim;
        let truncate_dim = config.embedding_truncate_dim;
        tokio::spawn(async move {
            info!("Starting background embedding model download/load...");
            let result = tokio::task::spawn_blocking(move || {
                let base = create_embedder(
                    code_intelligence_mcp_server::config::EmbeddingsBackend::LlamaCpp,
                    model_dir.as_deref(),
                    device,
                    hash_dim,
                )?;
                let embedder = maybe_truncate(base, truncate_dim)?;
                Ok::<_, anyhow::Error>(embedder)
            }).await;
            match result {
                Ok(Ok(real_embedder)) => {
                    let mut guard = slot.lock().expect("DeferredEmbedder mutex poisoned");
                    *guard = Some(real_embedder);
                    info!("Embedding model loaded — vector search is now available");
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

    info!(
        embeddings_backend = ?config.embeddings_backend,
        watch_mode = config.watch_mode,
        "Initialized components"
    );

    let server_details = InitializeResult {
        server_info: Implementation {
            name: "code-intelligence".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Code Intelligence MCP".into()),
            description: Some("Local code intelligence MCP server".into()),
            icons: vec![],
            website_url: None,
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            tasks: Some(code_intelligence_mcp_server::server::task_capabilities()),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: None,
        meta: None,
    };

    let transport = StdioTransport::new(TransportOptions::default())?;
    let handler = CodeIntelligenceHandler { state }.to_mcp_server_handler();

    let server = server_runtime::create_server(McpServerOptions {
        server_details,
        transport,
        handler,
        task_store: Some(Arc::new(
            rust_mcp_sdk::task_store::InMemoryTaskStore::<
                rust_mcp_sdk::schema::schema_utils::ClientJsonrpcRequest,
                rust_mcp_sdk::schema::schema_utils::ResultFromServer,
            >::new(None),
        )),
        client_task_store: None,
    });

    info!("Starting MCP stdio server");
    server.start().await
}

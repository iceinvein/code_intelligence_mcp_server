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
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use code_intelligence_mcp_server::cli;
use code_intelligence_mcp_server::config::Config;
use code_intelligence_mcp_server::embeddings::{create_embedder, Embedder};
use code_intelligence_mcp_server::handlers::AppState;
use code_intelligence_mcp_server::indexer::pipeline::IndexPipeline;
use code_intelligence_mcp_server::metrics::{spawn_metrics_server, MetricsRegistry};
use code_intelligence_mcp_server::reranker::create_reranker;
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
        return run_standalone(cli_args.host.as_deref(), cli_args.port).await;
    }

    if let Err(err) = run_embedded().await {
        error!(error = %err, "Server exited with error");
        return Err(err);
    }
    Ok(())
}

async fn run_standalone(host: Option<&str>, port: Option<u16>) -> SdkResult<()> {
    let standalone_config = code_intelligence_mcp_server::config::StandaloneConfig::load(host, port)
        .map_err(|e| McpSdkError::Internal { description: e.to_string() })?;

    // Ensure data directories exist
    let data_dir = &standalone_config.data_dir;
    std::fs::create_dir_all(data_dir.join("repos").as_std_path())
        .map_err(|e| McpSdkError::Internal { description: format!("Failed to create data dir: {}", e) })?;
    std::fs::create_dir_all(data_dir.join("logs").as_std_path())
        .map_err(|e| McpSdkError::Internal { description: format!("Failed to create logs dir: {}", e) })?;

    // Create shared embedder (loaded once, shared across all repos)
    let embedder = create_embedder(
        standalone_config.embeddings_backend,
        standalone_config.embeddings_model_dir.as_deref(),
        standalone_config.embeddings_model_repo.as_deref(),
        standalone_config.embeddings_device,
        standalone_config.embedding_max_threads,
        standalone_config.hash_embedding_dim,
    ).map_err(|e| McpSdkError::Internal { description: format!("Failed to create embedder: {}", e) })?;

    info!("Shared embedder loaded with dimension: {}", embedder.dim());

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
            ..Default::default()
        },
    );

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

    let embedder = create_embedder(
        config.embeddings_backend,
        config.embeddings_model_dir.as_deref(),
        config.embeddings_model_repo.as_deref(),
        config.embeddings_device,
        config.embedding_max_threads,
        config.hash_embedding_dim,
    )
    .map_err(|err| McpSdkError::Internal {
        description: format!("Failed to create embedder: {}", err),
    })?;

    info!("Created embedder with dimension: {}", embedder.dim());

    let tantivy = TantivyIndex::open_or_create(&config.tantivy_index_path).map_err(|err| {
        McpSdkError::Internal {
            description: err.to_string(),
        }
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

    // Create reranker if model path is configured
    let reranker = create_reranker(
        config.reranker_model_path.as_deref(),
        config.reranker_cache_dir.as_deref(),
        config.reranker_top_k,
    )
    .map_err(|err| McpSdkError::Internal {
        description: format!("Failed to create reranker: {}", err),
    })?;

    // Create HyDE generator if enabled
    let hyde_generator = if config.hyde_enabled {
        Some(HypotheticalCodeGenerator::new(
            config.hyde_llm_backend.clone(),
            config.hyde_api_key.clone(),
            config.hyde_max_tokens,
        ))
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
    });

    // Trigger automatic re-index if vector dimension migration occurred
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
        match state.indexer.index_all().await {
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
    }

    if state.config.watch_mode {
        let watch_cancel = tokio_util::sync::CancellationToken::new();
        state.indexer.spawn_watch_loop(watch_cancel);
    }

    // Spawn LLM description worker in background — model download (potentially
    // 1+ GB) must not block MCP server startup on stdio transport.
    if config.llm_enabled {
        let llm_config = config.clone();
        let llm_indexer = state.indexer.clone();
        tokio::spawn(async move {
            let generator = match tokio::task::spawn_blocking(move || {
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
            let cancel = tokio_util::sync::CancellationToken::new();
            let _desc_handle = llm_indexer.spawn_description_worker(generator, cancel);
            tracing::info!("LLM description worker spawned");
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
        task_store: None,
        client_task_store: None,
    });

    info!("Starting MCP stdio server");
    server.start().await
}

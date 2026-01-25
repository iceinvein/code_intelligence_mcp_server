//! Code Intelligence MCP Server - Main entry point

#![allow(unexpected_cfgs)]

use rust_mcp_sdk::{
    error::{McpSdkError, SdkResult},
    mcp_server::{server_runtime, McpServerOptions, ToMcpServerHandler},
    schema::{Implementation, InitializeResult, ProtocolVersion, ServerCapabilities, ServerCapabilitiesTools},
    McpServer, StdioTransport, TransportOptions,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;

use code_intelligence_mcp_server::config::Config;
use code_intelligence_mcp_server::embeddings::{create_embedder, Embedder};
use code_intelligence_mcp_server::handlers::AppState;
use code_intelligence_mcp_server::indexer::pipeline::IndexPipeline;
use code_intelligence_mcp_server::metrics::{MetricsRegistry, spawn_metrics_server};
use code_intelligence_mcp_server::reranker::create_reranker;
use code_intelligence_mcp_server::retrieval::hyde::HypotheticalCodeGenerator;
use code_intelligence_mcp_server::retrieval::Retriever;
use code_intelligence_mcp_server::server::CodeIntelligenceHandler;
use code_intelligence_mcp_server::storage::sqlite::SqliteStore;
use code_intelligence_mcp_server::storage::tantivy::TantivyIndex;
use code_intelligence_mcp_server::storage::vector::LanceDbStore;

mod cli;

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
    if cli::wants_help(&args) {
        cli::print_help();
        return Ok(());
    }
    if cli::wants_version(&args) {
        cli::print_version();
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "Starting code-intelligence-mcp-server"
    );

    if let Err(err) = run().await {
        error!(error = %err, "Server exited with error");
        return Err(err);
    }
    Ok(())
}

async fn run() -> SdkResult<()> {
    let config = Config::from_env().map_err(|err| McpSdkError::Internal {
        description: err.to_string(),
    })?;

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
        config.hash_embedding_dim,
    )
    .map_err(|err| McpSdkError::Internal {
        description: format!("Failed to create embedder: {}", err),
    })?;

    info!(
        "Created embedder with dimension: {}",
        embedder.dim()
    );

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
    lancedb
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
    let metrics = Arc::new(
        MetricsRegistry::new()
            .map_err(|err| McpSdkError::Internal {
                description: format!("Failed to create metrics registry: {}", err),
            })?
    );

    // Spawn metrics server if enabled
    let _metrics_handle = if config.metrics_enabled {
        let handle = spawn_metrics_server(Arc::clone(&metrics), config.metrics_port)
            .await
            .map_err(|err| McpSdkError::Internal {
                description: format!("Failed to spawn metrics server: {}", err),
            })?;
        Some(handle)
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
    });

    if state.config.watch_mode {
        state.indexer.spawn_watch_loop();
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
        base_dir = %config.base_dir.display(),
        db_path = %config.db_path.display(),
        db_path_rel = ?db_path_rel,
        vector_db_path = %config.vector_db_path.display(),
        vector_db_path_rel = ?vector_db_path_rel,
        tantivy_index_path = %config.tantivy_index_path.display(),
        tantivy_index_path_rel = ?tantivy_index_path_rel,
        embeddings_backend = ?config.embeddings_backend,
        embeddings_model_dir = ?config.embeddings_model_dir.as_ref().map(|p| p.display().to_string()),
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

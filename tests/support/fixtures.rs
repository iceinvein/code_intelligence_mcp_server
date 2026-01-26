//! rstest fixtures for integration tests
//!
//! This module provides reusable rstest fixtures for constructing AppState
//! and its dependencies in integration tests. Fixtures use dependency injection
//! to automatically provide required dependencies.
//!
//! # Usage
//!
//! ```rust
//! use crate::support::fixtures::*;
//!
//! #[rstest]
//! #[tokio::test]
//! async fn my_test(app_state: AppState) {
//!     // app_state is automatically constructed with all dependencies
//!     assert!(app_state.config.base_dir.exists());
//! }
//! ```

use code_intelligence_mcp_server::{
    config::{Config, EmbeddingsBackend, EmbeddingsDevice},
    embeddings::hash::HashEmbedder,
    handlers::AppState,
    indexer::pipeline::IndexPipeline,
    metrics::MetricsRegistry,
    retrieval::Retriever,
    storage::{
        sqlite::SqliteStore,
        tantivy::TantivyIndex,
        vector::LanceDbStore,
    },
};
use rstest::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

/// Unique counter for creating isolated test directories
static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Creates a unique temporary directory for test isolation
///
/// This fixture generates a unique temporary directory path using an atomic
/// counter combined with nanosecond timestamp. The directory is created
/// automatically.
///
/// Each call to this fixture creates a new unique directory, ensuring
/// test isolation even when tests run in parallel.
#[fixture]
pub fn tmp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let c = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("cimcp-fixture-test-{nanos}-{c}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Creates a test configuration pointing to the temporary directory
///
/// This fixture depends on `tmp_dir` and constructs a complete Config
/// with Hash embedder for fast testing (no model downloads).
///
/// Configuration uses:
/// - Hash embeddings (fast, no model download)
/// - CPU embeddings device
/// - Minimal indexing patterns
/// - Test-friendly settings (no watch mode, metrics disabled)
#[fixture]
pub fn test_config(tmp_dir: PathBuf) -> Config {
    let base_dir = tmp_dir.canonicalize().unwrap_or_else(|_| tmp_dir.clone());
    Config {
        db_path: base_dir.join("code-intelligence.db"),
        vector_db_path: base_dir.join("vectors"),
        tantivy_index_path: base_dir.join("tantivy-index"),
        base_dir: base_dir.clone(),
        embeddings_backend: EmbeddingsBackend::Hash,
        embeddings_model_dir: None,
        embeddings_model_url: None,
        embeddings_model_sha256: None,
        embeddings_auto_download: false,
        embeddings_model_repo: None,
        embeddings_model_revision: None,
        embeddings_model_hf_token: None,
        embeddings_device: EmbeddingsDevice::Cpu,
        embedding_batch_size: 32,
        hash_embedding_dim: 32,
        vector_search_limit: 20,
        hybrid_alpha: 0.7,
        rank_vector_weight: 0.7,
        rank_keyword_weight: 0.3,
        rank_exported_boost: 0.1,
        rank_index_file_boost: 0.05,
        rank_test_penalty: 0.1,
        rank_popularity_weight: 0.05,
        rank_popularity_cap: 50,
        index_patterns: vec![
            "**/*.ts".to_string(),
            "**/*.tsx".to_string(),
            "**/*.rs".to_string(),
        ],
        exclude_patterns: vec![],
        watch_mode: false,
        watch_debounce_ms: 100,
        max_context_bytes: 200_000,
        index_node_modules: false,
        repo_roots: vec![base_dir],
        reranker_model_path: None,
        reranker_top_k: 20,
        reranker_cache_dir: None,
        learning_enabled: false,
        learning_selection_boost: 0.1,
        learning_file_affinity_boost: 0.05,
        max_context_tokens: 8192,
        token_encoding: "o200k_base".to_string(),
        parallel_workers: 4,
        embedding_cache_enabled: true,
        embedding_max_threads: 0,
        pagerank_damping: 0.85,
        pagerank_iterations: 20,
        synonym_expansion_enabled: true,
        acronym_expansion_enabled: true,
        rrf_enabled: true,
        rrf_k: 60.0,
        rrf_keyword_weight: 1.0,
        rrf_vector_weight: 1.0,
        rrf_graph_weight: 0.5,
        hyde_enabled: false,
        hyde_llm_backend: "openai".to_string(),
        hyde_api_key: None,
        hyde_max_tokens: 512,
        metrics_enabled: false,
        metrics_port: 9090,
        package_detection_enabled: false,
    }
}

/// Creates a TantivyIndex for full-text search
///
/// This fixture depends on `test_config` and opens or creates
/// a Tantivy index at the configured path.
#[fixture]
fn tantivy_index(test_config: Config) -> Arc<TantivyIndex> {
    Arc::new(
        TantivyIndex::open_or_create(&test_config.tantivy_index_path)
            .unwrap()
    )
}

/// Creates a HashEmbedder for fast embedding generation
///
/// This fixture depends on `test_config` and creates a HashEmbedder
/// wrapped in an AsyncMutex for thread-safe async access.
///
/// Hash embeddings are used for testing because they're fast and
/// require no model downloads.
#[fixture]
pub fn hash_embedder(test_config: Config) -> Arc<AsyncMutex<Box<dyn code_intelligence_mcp_server::embeddings::Embedder + Send>>> {
    Arc::new(AsyncMutex::new(
        Box::new(HashEmbedder::new(test_config.hash_embedding_dim)) as _
    ))
}

/// Creates a LanceDB vector store for semantic search
///
/// This fixture depends on `test_config` and connects to LanceDB,
/// then opens or creates the symbols table for vector embeddings.
///
/// Note: LanceDB connection is async, but we block here since rstest
/// doesn't properly support mixing async and non-async fixtures.
#[fixture]
fn vector_store(
    test_config: Config,
) -> Arc<code_intelligence_mcp_server::storage::vector::LanceVectorTable> {
    // Use tokio runtime to block on the async call
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let lancedb = LanceDbStore::connect(&test_config.vector_db_path)
            .await
            .unwrap();
        Arc::new(
            lancedb
                .open_or_create_table("symbols", test_config.hash_embedding_dim)
                .await
                .unwrap(),
        )
    })
}

/// Creates a MetricsRegistry for collecting telemetry
///
/// This fixture creates a new metrics registry for each test,
/// ensuring isolated metrics between tests.
#[fixture]
pub fn metrics() -> Arc<MetricsRegistry> {
    Arc::new(MetricsRegistry::new().unwrap())
}

/// Creates a complete AppState with all dependencies
///
/// This is the main fixture that composes all other fixtures into
/// a fully functional AppState for testing. It depends on:
/// - `test_config`: Configuration pointing to temp directory
/// - `tantivy_index`: Full-text search index
/// - `hash_embedder`: Embedding generator
/// - `vector_store`: Vector database table
/// - `metrics`: Metrics registry
///
/// It also creates:
/// - SqliteStore: Initialized with schema
/// - IndexPipeline: For indexing operations
/// - Retriever: For search/retrieval operations
///
/// # Example
///
/// ```rust
/// #[rstest]
/// #[tokio::test]
/// async fn test_search(app_state: AppState) {
///     // Use app_state.config, app_state.retriever, etc.
/// }
/// ```
#[fixture]
pub fn app_state(
    test_config: Config,
    tantivy_index: Arc<TantivyIndex>,
    hash_embedder: Arc<AsyncMutex<Box<dyn code_intelligence_mcp_server::embeddings::Embedder + Send>>>,
    vector_store: Arc<code_intelligence_mcp_server::storage::vector::LanceVectorTable>,
    metrics: Arc<MetricsRegistry>,
) -> AppState {
    // Wrap config in Arc for shared use
    let config = Arc::new(test_config);

    // Initialize SQLite store
    let sqlite = Arc::new(SqliteStore::open(&config.db_path).unwrap());
    sqlite.init().unwrap();

    // Create index pipeline
    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy_index.clone(),
        vector_store.clone(),
        hash_embedder.clone(),
        metrics.clone(),
    );

    // Create retriever
    let retriever = Retriever::new(
        config.clone(),
        tantivy_index,
        vector_store,
        hash_embedder,
        None,  // No reranker for basic tests
        None,  // No HyDE generator for basic tests
        metrics,
    );

    AppState {
        config,
        indexer,
        retriever,
        sqlite,
    }
}

// ============================================================================
// Smoke tests to verify fixtures work correctly
// ============================================================================

#[cfg(test)]
mod smoke_tests {
    use super::*;

    /// Verifies that tmp_dir fixture creates a valid directory
    #[rstest]
    fn tmp_dir_creates_directory(tmp_dir: PathBuf) {
        assert!(tmp_dir.exists());
        assert!(tmp_dir.is_dir());
    }

    /// Verifies that test_config fixture creates valid configuration
    #[rstest]
    fn test_config_has_valid_paths(tmp_dir: PathBuf, test_config: Config) {
        assert_eq!(test_config.base_dir, tmp_dir.canonicalize().unwrap_or_else(|_| tmp_dir.clone()));
        assert!(test_config.db_path.starts_with(&test_config.base_dir));
        assert!(test_config.vector_db_path.starts_with(&test_config.base_dir));
        assert!(test_config.tantivy_index_path.starts_with(&test_config.base_dir));
    }

    /// Verifies that tantivy_index fixture can be created
    #[rstest]
    fn tantivy_index_fixture_works(tantivy_index: Arc<TantivyIndex>) {
        // If we got here, the fixture was created successfully
        assert!(true, "tantivy_index fixture created successfully");
    }

    /// Verifies that vector_store fixture can be created
    #[rstest]
    #[tokio::test]
    async fn vector_store_fixture_works(vector_store: Arc<code_intelligence_mcp_server::storage::vector::LanceVectorTable>) {
        // If we got here, the fixture was created successfully
        assert!(true, "vector_store fixture created successfully");
    }

    /// Verifies that the complete app_state fixture can be constructed
    ///
    /// This is the most important smoke test - it verifies that all fixtures
    /// can be injected together and the complete AppState can be built.
    #[rstest]
    #[tokio::test]
    async fn fixtures_smoke(app_state: AppState) {
        // Verify all components are present
        assert!(app_state.config.base_dir.exists());
        assert!(app_state.config.db_path.exists());
        // Verify sqlite was initialized by checking we can query it
        let count = app_state.sqlite.count_symbols().unwrap();
        assert_eq!(count, 0); // Empty database
    }
}

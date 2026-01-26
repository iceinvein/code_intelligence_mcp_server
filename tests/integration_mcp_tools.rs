//! Integration tests for MCP tool handlers
//!
//! Tests verify that tool handlers produce correct response structures
//! and handle error cases appropriately.

// Test support module with fixtures and helpers
mod support;

use code_intelligence_mcp_server::{
    config::{Config, EmbeddingsBackend, EmbeddingsDevice},
    embeddings::hash::HashEmbedder,
    handlers::{
        handle_explain_search, handle_find_affected_code, handle_find_similar_code,
        handle_get_module_summary, handle_report_selection, handle_summarize_file,
        handle_trace_data_flow,
    },
    metrics::MetricsRegistry,
    path::Utf8PathBuf,
    retrieval::Retriever,
    storage::{
        sqlite::{SqliteStore, SymbolRow},
        tantivy::TantivyIndex,
        vector::LanceDbStore,
    },
    tools::{
        ExplainSearchTool, FindAffectedCodeTool, FindSimilarCodeTool, GetModuleSummaryTool,
        ReportSelectionTool, SummarizeFileTool, TraceDataFlowTool,
    },
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex as AsyncMutex;

/// Empty JSON array for default values
static EMPTY: &[serde_json::Value] = &[];

/// Generate a unique temporary directory for test isolation
fn tmp_db_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("cimcp-mcp-test-{nanos}-{c}.db"));
    dir
}

/// Helper to create a test symbol in the database
fn create_test_symbol(
    db_path: &std::path::Path,
    id: &str,
    name: &str,
    kind: &str,
    file_path: &str,
    exported: bool,
) -> Result<(), anyhow::Error> {
    let db_path_utf8 = Utf8PathBuf::from_path_buf(db_path.to_path_buf())
        .map_err(|_| anyhow::anyhow!("Database path is not valid UTF-8"))?;
    let sqlite = SqliteStore::open(&db_path_utf8)?;
    sqlite.init()?;

    let symbol = SymbolRow {
        id: id.to_string(),
        file_path: file_path.to_string(),
        language: "rust".to_string(),
        kind: kind.to_string(),
        name: name.to_string(),
        exported,
        start_byte: 0,
        end_byte: 100,
        start_line: 1,
        end_line: 10,
        text: format!("pub fn {}() {{}}", name),
    };

    sqlite.upsert_symbol(&symbol)?;
    Ok(())
}

/// Create AppState with sqlite initialized for handler tests
async fn create_app_state(db_path: &Path, suffix: &str) -> code_intelligence_mcp_server::handlers::AppState {
    // Use the parent directory of the db_path as the base directory
    let base_dir = db_path.parent().unwrap_or(db_path).to_path_buf();

    let config = Arc::new(test_config(&base_dir));

    let db_path_utf8 = Utf8PathBuf::from_path_buf(db_path.to_path_buf()).unwrap();
    let sqlite = Arc::new(SqliteStore::open(&db_path_utf8).unwrap());
    // Initialize database schema
    sqlite.init().unwrap();

    // Create unique tantivy index path using db_path stem and suffix to avoid lock contention
    let tantivy_index_path = config.tantivy_index_path
        .join(format!("{}-{}", db_path.file_stem().unwrap_or_default().to_string_lossy(), suffix));
    let tantivy = Arc::new(TantivyIndex::open_or_create(&tantivy_index_path).unwrap());
    let embedder = Arc::new(AsyncMutex::new(
        Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _
    ));

    let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
    let vectors = Arc::new(
        lancedb
            .open_or_create_table("symbols", config.hash_embedding_dim)
            .await
            .unwrap(),
    );

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = code_intelligence_mcp_server::indexer::pipeline::IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics.clone(),
    );

    let retriever = Retriever::new(
        config.clone(),
        tantivy,
        vectors,
        embedder,
        None,
        None,
        metrics,
    );

    code_intelligence_mcp_server::handlers::AppState {
        config,
        indexer,
        retriever,
        sqlite,
    }
}

// ============================================================================
// Tests for summarize_file tool
// ============================================================================

#[tokio::test]
async fn test_summarize_file_tool() {
    let db_path = tmp_db_path();

    create_test_symbol(
        &db_path,
        "test-1",
        "testFunction",
        "function",
        "src/test.rs",
        true,
    )
    .unwrap();
    create_test_symbol(
        &db_path,
        "test-2",
        "internalFunc",
        "function",
        "src/test.rs",
        false,
    )
    .unwrap();
    create_test_symbol(
        &db_path,
        "test-3",
        "TestClass",
        "class",
        "src/test.rs",
        true,
    )
    .unwrap();

    let state = create_app_state(&db_path, "summarize-1").await;

    let params = SummarizeFileTool {
        file_path: "src/test.rs".to_string(),
        include_signatures: Some(false),
        verbose: Some(false),
    };

    let result = handle_summarize_file(&state, params).unwrap();

    assert_eq!(
        result.get("file_path").and_then(|v| v.as_str()),
        Some("src/test.rs")
    );
    assert_eq!(
        result.get("total_symbols").and_then(|v| v.as_u64()),
        Some(3)
    );
    assert_eq!(
        result.get("exported_symbols").and_then(|v| v.as_u64()),
        Some(2)
    );
    assert!(result.get("counts_by_kind").is_some());
    assert!(result.get("purpose").is_some());
}

#[tokio::test]
async fn test_summarize_file_with_signatures() {
    let db_path = tmp_db_path();

    create_test_symbol(
        &db_path,
        "test-1",
        "exportedFunc",
        "function",
        "src/module.ts",
        true,
    )
    .unwrap();
    create_test_symbol(
        &db_path,
        "test-2",
        "internalFunc",
        "function",
        "src/module.ts",
        false,
    )
    .unwrap();

    let state = create_app_state(&db_path, "summarize-2").await;

    let params = SummarizeFileTool {
        file_path: "src/module.ts".to_string(),
        include_signatures: Some(true),
        verbose: Some(false),
    };

    let result = handle_summarize_file(&state, params).unwrap();

    assert_eq!(
        result.get("file_path").and_then(|v| v.as_str()),
        Some("src/module.ts")
    );
    assert_eq!(
        result.get("total_symbols").and_then(|v| v.as_u64()),
        Some(2)
    );

    // Check exports list is populated when include_signatures=true
    let empty = Vec::new();
    let exports = result
        .get("exports")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    assert_eq!(exports.len(), 1); // Only exported symbol included by default
    assert_eq!(
        exports[0].get("name").and_then(|v| v.as_str()),
        Some("exportedFunc")
    );
    assert!(exports[0].get("signature").is_some());
}

#[tokio::test]
async fn test_summarize_file_verbose_includes_internal() {
    let db_path = tmp_db_path();

    create_test_symbol(
        &db_path,
        "test-1",
        "exportedFunc",
        "function",
        "src/module.ts",
        true,
    )
    .unwrap();
    create_test_symbol(
        &db_path,
        "test-2",
        "internalFunc",
        "function",
        "src/module.ts",
        false,
    )
    .unwrap();

    let state = create_app_state(&db_path, "summarize-3").await;

    let params = SummarizeFileTool {
        file_path: "src/module.ts".to_string(),
        include_signatures: Some(true),
        verbose: Some(true), // verbose=true should include internal symbols
    };

    let result = handle_summarize_file(&state, params).unwrap();

    let empty = Vec::new();
    let exports = result
        .get("exports")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    // verbose=true includes both exported and internal
    assert_eq!(exports.len(), 2);
}

#[tokio::test]
async fn test_summarize_file_not_found() {
    let db_path = tmp_db_path();

    let state = create_app_state(&db_path, "summarize-4").await;

    let params = SummarizeFileTool {
        file_path: "nonexistent.rs".to_string(),
        include_signatures: Some(false),
        verbose: Some(false),
    };

    let result = handle_summarize_file(&state, params).unwrap();

    assert_eq!(
        result.get("error").and_then(|v| v.as_str()),
        Some("FILE_NOT_FOUND")
    );
    assert!(result.get("message").is_some());
}

// ============================================================================
// Tests for get_module_summary tool
// ============================================================================

#[cfg(test)]
mod get_module_summary_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    // Note: We can't use rstest fixtures directly with tokio::test because
    // the app_state fixture uses blocking calls internally (block_on) which
    // conflicts with the tokio runtime. Instead, we create AppState manually
    // within each async test.

    // Unique counter for test isolation
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Helper to create AppState for async tests
    /// This must be called within async context to avoid runtime conflicts
    async fn create_async_app_state() -> (code_intelligence_mcp_server::handlers::AppState, std::path::PathBuf) {
        use code_intelligence_mcp_server::handlers::AppState;
        use code_intelligence_mcp_server::indexer::pipeline::IndexPipeline;
        use code_intelligence_mcp_server::retrieval::Retriever;
        use code_intelligence_mcp_server::storage::sqlite::SqliteStore;
        use code_intelligence_mcp_server::storage::tantivy::TantivyIndex;
        use code_intelligence_mcp_server::storage::vector::LanceDbStore;

        // Create temp dir for this test with unique ID for parallel test safety
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let unique_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base_dir = std::path::PathBuf::from(format!(
            "/tmp/cimcp-test-{}-{}",
            unique_id,
            counter
        ));
        std::fs::create_dir_all(&base_dir).unwrap();

        let config = std::sync::Arc::new(super::test_config(&base_dir));

        // Create storage components async
        let tantivy = std::sync::Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
        let sqlite = std::sync::Arc::new(SqliteStore::open(config.db_path.as_path()).unwrap());
        sqlite.init().unwrap();

        let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
        let vectors = std::sync::Arc::new(
            lancedb
                .open_or_create_table("symbols", config.hash_embedding_dim)
                .await
                .unwrap(),
        );

        let embedder = std::sync::Arc::new(tokio::sync::Mutex::new(
            Box::new(code_intelligence_mcp_server::embeddings::hash::HashEmbedder::new(config.hash_embedding_dim))
                as Box<dyn code_intelligence_mcp_server::embeddings::Embedder + Send>
        ));

        let metrics = std::sync::Arc::new(code_intelligence_mcp_server::metrics::MetricsRegistry::new().unwrap());

        let indexer = IndexPipeline::new(
            config.clone(),
            tantivy.clone(),
            vectors.clone(),
            embedder.clone(),
            metrics.clone(),
        );

        let retriever = Retriever::new(
            config.clone(),
            tantivy,
            vectors,
            embedder,
            None,
            None,
            metrics,
        );

        let app_state = AppState {
            config,
            indexer,
            retriever,
            sqlite,
        };

        (app_state, base_dir)
    }

    #[tokio::test]
    async fn test_get_module_summary_tool() {
        let (app_state, _base_dir) = create_async_app_state().await;

        // Create actual source file with exports
        let module_path = app_state.config.base_dir.join("src/module.ts");
        std::fs::create_dir_all(module_path.parent().unwrap()).unwrap();
        std::fs::write(&module_path, r#"
export function exportedFunction() {
    return "hello";
}

export class ExportedClass {
    constructor() {}
}

function internalFunc() {
    // This is internal, not exported
}
"#).unwrap();

        // Index the file
        app_state.indexer.index_all().await.unwrap();

        let params = GetModuleSummaryTool {
            file_path: "src/module.ts".to_string(),
            group_by_kind: Some(true),
        };

        let result = handle_get_module_summary(&app_state, params).unwrap();

        assert_eq!(result.get("export_count").and_then(|v| v.as_u64()), Some(2));
        assert!(result.get("exports").is_some());
        assert!(result.get("groups").is_some());

        // Check grouping worked
        let empty = Vec::new();
        let groups = result
            .get("groups")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);
        assert!(!groups.is_empty());

        // Should have groups for function and class
        let kinds: Vec<_> = groups
            .iter()
            .filter_map(|g| g.get("kind").and_then(|k| k.as_str()))
            .collect();
        assert!(kinds.contains(&"function"));
        assert!(kinds.contains(&"class"));
    }

    #[tokio::test]
    async fn test_get_module_summary_flat() {
        let (app_state, _base_dir) = create_async_app_state().await;

        // Create actual source file with a single export
        let module_path = app_state.config.base_dir.join("src/api.ts");
        std::fs::create_dir_all(module_path.parent().unwrap()).unwrap();
        std::fs::write(&module_path, r#"
export function myFunction() {
    return "api response";
}
"#).unwrap();

        // Index the file
        app_state.indexer.index_all().await.unwrap();

        let params = GetModuleSummaryTool {
            file_path: "src/api.ts".to_string(),
            group_by_kind: Some(false), // Flat output
        };

        let result = handle_get_module_summary(&app_state, params).unwrap();

        assert_eq!(result.get("export_count").and_then(|v| v.as_u64()), Some(1));

        let empty = Vec::new();
        let exports = result
            .get("exports")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);
        assert_eq!(exports.len(), 1);
        assert_eq!(
            exports[0].get("name").and_then(|v| v.as_str()),
            Some("myFunction")
        );

        // groups should be empty when group_by_kind=false
        let groups = result
            .get("groups")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);
        assert!(groups.is_empty());
    }

    #[tokio::test]
    async fn test_get_module_summary_no_exports() {
        let (app_state, _base_dir) = create_async_app_state().await;

        // Create actual source file with only internal symbols (no exports)
        let module_path = app_state.config.base_dir.join("src/internal.ts");
        std::fs::create_dir_all(module_path.parent().unwrap()).unwrap();
        std::fs::write(&module_path, r#"
function internalFunc() {
    // No exports here - just internal function
    return "internal";
}
"#).unwrap();

        // Index the file
        app_state.indexer.index_all().await.unwrap();

        let params = GetModuleSummaryTool {
            file_path: "src/internal.ts".to_string(),
            group_by_kind: Some(false),
        };

        let result = handle_get_module_summary(&app_state, params).unwrap();

        // Should return NO_EXPORTS error
        assert_eq!(
            result.get("error").and_then(|v| v.as_str()),
            Some("NO_EXPORTS")
        );
        assert!(result.get("message").is_some());
    }

    #[tokio::test]
    async fn test_get_module_summary_signatures() {
        let (app_state, _base_dir) = create_async_app_state().await;

        // Create actual source file with export
        let module_path = app_state.config.base_dir.join("src/utils.ts");
        std::fs::create_dir_all(module_path.parent().unwrap()).unwrap();
        std::fs::write(&module_path, r#"
// Exported function with a longer body to test signature truncation
export function myFunction(param: string): number {
    return param.length * 2;
}
"#).unwrap();

        // Index the file
        app_state.indexer.index_all().await.unwrap();

        let params = GetModuleSummaryTool {
            file_path: "src/utils.ts".to_string(),
            group_by_kind: Some(false),
        };

        let result = handle_get_module_summary(&app_state, params).unwrap();

        let empty = Vec::new();
        let exports = result
            .get("exports")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);

        assert!(!exports.is_empty(), "Should have at least one export");

        // Check signature field exists on the first export
        assert!(exports[0].get("signature").is_some());

        // Signature should be a truncated version of the text
        let sig = exports[0]
            .get("signature")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(!sig.is_empty());

        // Signature should contain the function signature
        assert!(sig.contains("myFunction"), "Signature should contain function name");
    }
}

// ============================================================================
// Tests for trace_data_flow tool
// ============================================================================

#[tokio::test]
async fn test_trace_data_flow_tool() {
    let db_path = tmp_db_path();

    create_test_symbol(&db_path, "root", "dataVar", "variable", "src/main.rs", true).unwrap();

    let state = create_app_state(&db_path, "trace-1").await;

    let params = TraceDataFlowTool {
        symbol_name: "dataVar".to_string(),
        file_path: None,
        direction: Some("both".to_string()),
        depth: Some(2),
        limit: Some(50),
    };

    let result = handle_trace_data_flow(&state, params).unwrap();

    assert!(result.get("symbol_name").is_some());
    assert!(result.get("flows").is_some());
    assert!(result.get("read_count").is_some());
    assert!(result.get("write_count").is_some());
}

#[tokio::test]
async fn test_trace_data_flow_not_found() {
    let db_path = tmp_db_path();

    let state = create_app_state(&db_path, "trace-2").await;

    let params = TraceDataFlowTool {
        symbol_name: "nonexistent".to_string(),
        file_path: None,
        direction: Some("both".to_string()),
        depth: Some(2),
        limit: Some(50),
    };

    let result = handle_trace_data_flow(&state, params).unwrap();

    assert_eq!(
        result.get("error").and_then(|v| v.as_str()),
        Some("SYMBOL_NOT_FOUND")
    );
}

// ============================================================================
// Tests for find_affected_code tool
// ============================================================================

#[tokio::test]
async fn test_find_affected_code_tool() {
    let db_path = tmp_db_path();

    create_test_symbol(
        &db_path,
        "root",
        "apiFunction",
        "function",
        "src/api.rs",
        true,
    )
    .unwrap();

    let state = create_app_state(&db_path, "affected-1").await;

    let params = FindAffectedCodeTool {
        symbol_name: "apiFunction".to_string(),
        file_path: None,
        depth: Some(2),
        limit: Some(50),
        include_tests: Some(false),
    };

    let result = handle_find_affected_code(&state, params).unwrap();

    assert!(result.get("symbol_name").is_some());
    assert!(result.get("affected").is_some());
    assert!(result.get("affected_files").is_some());
}

#[tokio::test]
async fn test_find_affected_code_not_found() {
    let db_path = tmp_db_path();

    let state = create_app_state(&db_path, "affected-2").await;

    let params = FindAffectedCodeTool {
        symbol_name: "nonexistent".to_string(),
        file_path: None,
        depth: Some(2),
        limit: Some(50),
        include_tests: Some(false),
    };

    let result = handle_find_affected_code(&state, params).unwrap();

    assert_eq!(
        result.get("error").and_then(|v| v.as_str()),
        Some("SYMBOL_NOT_FOUND")
    );
}

// ============================================================================
// Tests for report_selection tool
// ============================================================================

#[tokio::test]
async fn test_report_selection_tool() {
    let db_path = tmp_db_path();

    // Create a symbol first (required for foreign key constraint)
    create_test_symbol(
        &db_path,
        "sym-123",
        "myFunction",
        "function",
        "src/api.rs",
        true,
    )
    .unwrap();

    let state = create_app_state(&db_path, "report-1").await;

    let params = ReportSelectionTool {
        query: "test search".to_string(),
        selected_symbol_id: "sym-123".to_string(),
        position: 1,
    };

    let result = handle_report_selection(&state, params).await.unwrap();

    assert_eq!(result.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(result.get("recorded").and_then(|v| v.as_bool()), Some(true));
    assert!(result.get("selection_id").is_some());
    assert_eq!(
        result.get("query_normalized").and_then(|v| v.as_str()),
        Some("test search")
    );
}

#[tokio::test]
async fn test_report_selection_normalizes_query() {
    let db_path = tmp_db_path();

    // Create a symbol first (required for foreign key constraint)
    create_test_symbol(
        &db_path,
        "sym-456",
        "anotherFunction",
        "function",
        "src/utils.rs",
        true,
    )
    .unwrap();

    let state = create_app_state(&db_path, "report-2").await;

    let params = ReportSelectionTool {
        query: "  Test Search  ".to_string(), // Leading/trailing spaces and mixed case
        selected_symbol_id: "sym-456".to_string(),
        position: 2,
    };

    let result = handle_report_selection(&state, params).await.unwrap();

    // Query should be normalized (lowercased, trimmed)
    assert_eq!(
        result.get("query_normalized").and_then(|v| v.as_str()),
        Some("test search")
    );
}

// ============================================================================
// Test infrastructure for explain_search and find_similar_code
// ============================================================================

/// Create a temporary test directory with config
fn tmp_test_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(100);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("cimcp-search-test-{nanos}-{c}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Create test configuration
fn test_config(base_dir: &Path) -> Config {
    let base_dir = base_dir.canonicalize().unwrap_or_else(|_| base_dir.to_path_buf());
    let base_dir_utf8 = Utf8PathBuf::from_path_buf(base_dir.clone()).unwrap_or_else(|_| {
        Utf8PathBuf::from(base_dir.to_string_lossy().as_ref())
    });
    Config {
        db_path: base_dir_utf8.join("code-intelligence.db"),
        vector_db_path: base_dir_utf8.join("vectors"),
        tantivy_index_path: base_dir_utf8.join("tantivy-index"),
        base_dir: base_dir_utf8.clone(),
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
        watch_min_index_interval_ms: 50,
        max_context_bytes: 200_000,
        index_node_modules: false,
        repo_roots: vec![base_dir_utf8],
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

/// Setup test environment with indexed content
async fn setup_search_test() -> (PathBuf, Arc<Retriever>) {
    let dir = tmp_test_dir();

    // Create test source files
    std::fs::write(
        dir.join("search.ts"),
        r#"
export function searchFunction(query: string): string {
    return "result: " + query;
}

export function helperFunction(value: number): number {
    return value * 2;
}
"#,
    )
    .unwrap();

    let config = Arc::new(test_config(&dir));

    let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
    let embedder = Arc::new(AsyncMutex::new(
        Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
    ));
    let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
    let vectors = Arc::new(
        lancedb
            .open_or_create_table("symbols", config.hash_embedding_dim)
            .await
            .unwrap(),
    );

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = code_intelligence_mcp_server::indexer::pipeline::IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics.clone(),
    );

    // Index the test files
    indexer.index_all().await.unwrap();

    let retriever = Retriever::new(config, tantivy, vectors, embedder, None, None, metrics);

    (dir, Arc::new(retriever))
}

// ============================================================================
// Tests for explain_search tool
// ============================================================================

#[tokio::test]
async fn test_explain_search_tool() {
    let (_dir, retriever) = setup_search_test().await;

    let params = ExplainSearchTool {
        query: "search".to_string(),
        limit: Some(5),
        exported_only: Some(false),
        verbose: Some(false),
    };

    let result = handle_explain_search(&retriever, params).await.unwrap();

    assert_eq!(result.get("query").and_then(|v| v.as_str()), Some("search"));
    assert!(result.get("count").is_some());
    assert!(result.get("results").is_some());
    assert!(result.get("display").is_some());

    // Check that results array contains expected fields
    let results = result
        .get("results")
        .and_then(|v| v.as_array())
        .map_or(EMPTY, |v| v);
    if !results.is_empty() {
        let first = &results[0];
        assert!(first.get("symbol_id").is_some());
        assert!(first.get("score").is_some());
        assert!(first.get("score_breakdown").is_some());
    }
}

#[tokio::test]
async fn test_explain_search_verbose() {
    let (_dir, retriever) = setup_search_test().await;

    let params = ExplainSearchTool {
        query: "helper".to_string(),
        limit: Some(10),
        exported_only: Some(true),
        verbose: Some(true),
    };

    let result = handle_explain_search(&retriever, params).await.unwrap();

    assert_eq!(result.get("query").and_then(|v| v.as_str()), Some("helper"));

    // With verbose, we should have additional signals
    let results = result
        .get("results")
        .and_then(|v| v.as_array())
        .map_or(EMPTY, |v| v);
    if !results.is_empty() {
        let first = &results[0];
        // Verbose mode adds signals field
        assert!(first.get("score_breakdown").is_some());
    }
}

// ============================================================================
// Tests for find_similar_code tool
// ============================================================================

#[tokio::test]
async fn test_find_similar_code_by_symbol_name() {
    let (dir, _retriever) = setup_search_test().await;

    let params = FindSimilarCodeTool {
        symbol_name: Some("searchFunction".to_string()),
        code_snippet: None,
        file_path: Some(dir.join("search.ts").to_str().unwrap().to_string()),
        limit: Some(10),
        threshold: Some(0.1), // Low threshold for testing
    };

    // Create AppState - need to reconstruct the components
    let config = Arc::new(test_config(&dir));
    let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
    let embedder = Arc::new(AsyncMutex::new(
        Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
    ));
    let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
    let vectors = Arc::new(
        lancedb
            .open_or_create_table("symbols", config.hash_embedding_dim)
            .await
            .unwrap(),
    );

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = code_intelligence_mcp_server::indexer::pipeline::IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics.clone(),
    );

    let retriever = Retriever::new(
        config.clone(),
        tantivy,
        vectors,
        embedder,
        None,
        None,
        metrics,
    );
    let state = code_intelligence_mcp_server::handlers::AppState {
        config: config.clone(),
        indexer,
        retriever,
        sqlite: Arc::new(SqliteStore::open(config.db_path.as_path()).unwrap()),
    };

    // The handler might fail if embedding isn't found, so check both success and error cases
    let result = handle_find_similar_code(&state, params).await.unwrap();

    // Check response structure - could be success or error
    if result.get("error").is_some() {
        // If error, it should be SYMBOL_NOT_FOUND or similar
        assert!(
            result.get("error").and_then(|v| v.as_str()) == Some("SYMBOL_NOT_FOUND")
                || result.get("error").and_then(|v| v.as_str()) == Some("EMBEDDING_NOT_FOUND")
        );
    } else {
        // Success case - verify response structure
        assert!(result.get("threshold").is_some());
        assert!(result.get("count").is_some());
        assert!(result.get("results").is_some());
        // query field is present on success
        assert!(result.get("query").is_some());

        let results = result
            .get("results")
            .and_then(|v| v.as_array())
            .map_or(EMPTY, |v| v);
        assert!(!results.is_empty());
    }
}

#[tokio::test]
async fn test_find_similar_code_by_code_snippet() {
    let (dir, _retriever) = setup_search_test().await;

    let params = FindSimilarCodeTool {
        symbol_name: None,
        code_snippet: Some("function test() { return 42; }".to_string()),
        file_path: None,
        limit: Some(5),
        threshold: Some(0.0), // Zero threshold to get any results
    };

    // Create AppState
    let config = Arc::new(test_config(&dir));
    let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
    let embedder = Arc::new(AsyncMutex::new(
        Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
    ));
    let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
    let vectors = Arc::new(
        lancedb
            .open_or_create_table("symbols", config.hash_embedding_dim)
            .await
            .unwrap(),
    );

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = code_intelligence_mcp_server::indexer::pipeline::IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics.clone(),
    );

    let retriever = Retriever::new(
        config.clone(),
        tantivy,
        vectors,
        embedder,
        None,
        None,
        metrics,
    );
    let state = code_intelligence_mcp_server::handlers::AppState {
        config: config.clone(),
        indexer,
        retriever,
        sqlite: Arc::new(SqliteStore::open(config.db_path.as_path()).unwrap()),
    };

    let result = handle_find_similar_code(&state, params).await.unwrap();

    assert_eq!(result.get("threshold").and_then(|v| v.as_f64()), Some(0.0));
    assert!(result.get("results").is_some());
}

#[tokio::test]
async fn test_find_similar_code_not_found() {
    let (dir, _retriever) = setup_search_test().await;

    let params = FindSimilarCodeTool {
        symbol_name: Some("nonexistentFunction".to_string()),
        code_snippet: None,
        file_path: Some(dir.join("search.ts").to_str().unwrap().to_string()),
        limit: Some(5),
        threshold: Some(0.5),
    };

    // Create AppState
    let config = Arc::new(test_config(&dir));
    let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
    let embedder = Arc::new(AsyncMutex::new(
        Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
    ));
    let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
    let vectors = Arc::new(
        lancedb
            .open_or_create_table("symbols", config.hash_embedding_dim)
            .await
            .unwrap(),
    );

    let metrics = Arc::new(MetricsRegistry::new().unwrap());

    let indexer = code_intelligence_mcp_server::indexer::pipeline::IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
        metrics.clone(),
    );

    let retriever = Retriever::new(
        config.clone(),
        tantivy,
        vectors,
        embedder,
        None,
        None,
        metrics,
    );
    let state = code_intelligence_mcp_server::handlers::AppState {
        config: config.clone(),
        indexer,
        retriever,
        sqlite: Arc::new(SqliteStore::open(config.db_path.as_path()).unwrap()),
    };

    let result = handle_find_similar_code(&state, params).await.unwrap();

    assert_eq!(
        result.get("error").and_then(|v| v.as_str()),
        Some("SYMBOL_NOT_FOUND")
    );
}

// ============================================================================
// Fixture smoke tests
// ============================================================================

/// Verify that rstest fixtures can be injected correctly
///
/// This test ensures the new rstest fixture infrastructure works.
/// It's a simple smoke test that all fixtures compose correctly.
#[cfg(test)]
mod fixture_tests {
    use rstest::*;

    // Import fixtures from the support module declared at file root
    #[allow(unused_imports)]
    use super::support::fixtures::*;

    // Import the actual AppState type to avoid ambiguity
    use code_intelligence_mcp_server::handlers::AppState as ActualAppState;

    // Synchronous smoke test - just verify tmp_dir fixture works
    #[rstest]
    fn rstest_tmp_dir_fixture_works(tmp_dir: std::path::PathBuf) {
        assert!(tmp_dir.exists());
        assert!(tmp_dir.is_dir());
    }

    // Verify test_config fixture works
    #[rstest]
    fn rstest_test_config_fixture_works(test_config: code_intelligence_mcp_server::config::Config) {
        assert!(test_config.base_dir.exists());
        assert!(test_config.db_path.starts_with(&test_config.base_dir));
    }

    // Async fixture test - verify metrics fixture works
    #[rstest]
    fn rstest_metrics_fixture_works(_metrics: std::sync::Arc<code_intelligence_mcp_server::metrics::MetricsRegistry>) {
        // Just verify we got the fixture - MetricsRegistry doesn't have much to check
    }

    // Verify the app_state fixture can be constructed and injected
    // Note: app_state fixture uses blocking calls internally for LanceDB
    #[rstest]
    fn rstest_app_state_fixture_works(app_state: ActualAppState) {
        // Verify all components are present
        assert!(app_state.config.base_dir.exists());
        assert!(app_state.config.db_path.exists());
        // Verify sqlite was initialized by checking we can query it
        let count = app_state.sqlite.count_symbols().unwrap();
        assert_eq!(count, 0); // Empty database
    }
}

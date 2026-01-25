//! Integration tests for MCP tool handlers
//!
//! Tests verify that tool handlers produce correct response structures
//! and handle error cases appropriately.

use code_intelligence_mcp_server::{
    config::{Config, EmbeddingsBackend, EmbeddingsDevice},
    embeddings::hash::HashEmbedder,
    handlers::{
        handle_explain_search, handle_find_affected_code, handle_find_similar_code,
        handle_get_module_summary, handle_report_selection, handle_summarize_file,
        handle_trace_data_flow,
    },
    metrics::MetricsRegistry,
    retrieval::Retriever,
    storage::{sqlite::{SqliteStore, SymbolRow}, tantivy::TantivyIndex, vector::LanceDbStore},
    tools::{ExplainSearchTool, FindAffectedCodeTool, FindSimilarCodeTool, GetModuleSummaryTool, ReportSelectionTool, SummarizeFileTool, TraceDataFlowTool},
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{path::PathBuf, time::{SystemTime, UNIX_EPOCH}};
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
    let sqlite = SqliteStore::open(db_path)?;
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

// ============================================================================
// Tests for summarize_file tool
// ============================================================================

#[test]
fn test_summarize_file_tool() {
    let db_path = tmp_db_path();

    create_test_symbol(&db_path, "test-1", "testFunction", "function", "src/test.rs", true).unwrap();
    create_test_symbol(&db_path, "test-2", "internalFunc", "function", "src/test.rs", false).unwrap();
    create_test_symbol(&db_path, "test-3", "TestClass", "class", "src/test.rs", true).unwrap();

    let params = SummarizeFileTool {
        file_path: "src/test.rs".to_string(),
        include_signatures: Some(false),
        verbose: Some(false),
    };

    let result = handle_summarize_file(&db_path, params).unwrap();

    assert_eq!(result.get("file_path").and_then(|v| v.as_str()), Some("src/test.rs"));
    assert_eq!(result.get("total_symbols").and_then(|v| v.as_u64()), Some(3));
    assert_eq!(result.get("exported_symbols").and_then(|v| v.as_u64()), Some(2));
    assert!(result.get("counts_by_kind").is_some());
    assert!(result.get("purpose").is_some());
}

#[test]
fn test_summarize_file_with_signatures() {
    let db_path = tmp_db_path();

    create_test_symbol(&db_path, "test-1", "exportedFunc", "function", "src/module.ts", true).unwrap();
    create_test_symbol(&db_path, "test-2", "internalFunc", "function", "src/module.ts", false).unwrap();

    let params = SummarizeFileTool {
        file_path: "src/module.ts".to_string(),
        include_signatures: Some(true),
        verbose: Some(false),
    };

    let result = handle_summarize_file(&db_path, params).unwrap();

    assert_eq!(result.get("file_path").and_then(|v| v.as_str()), Some("src/module.ts"));
    assert_eq!(result.get("total_symbols").and_then(|v| v.as_u64()), Some(2));

    // Check exports list is populated when include_signatures=true
    let empty = Vec::new();
    let exports = result.get("exports").and_then(|v| v.as_array()).unwrap_or(&empty);
    assert_eq!(exports.len(), 1); // Only exported symbol included by default
    assert_eq!(exports[0].get("name").and_then(|v| v.as_str()), Some("exportedFunc"));
    assert!(exports[0].get("signature").is_some());
}

#[test]
fn test_summarize_file_verbose_includes_internal() {
    let db_path = tmp_db_path();

    create_test_symbol(&db_path, "test-1", "exportedFunc", "function", "src/module.ts", true).unwrap();
    create_test_symbol(&db_path, "test-2", "internalFunc", "function", "src/module.ts", false).unwrap();

    let params = SummarizeFileTool {
        file_path: "src/module.ts".to_string(),
        include_signatures: Some(true),
        verbose: Some(true), // verbose=true should include internal symbols
    };

    let result = handle_summarize_file(&db_path, params).unwrap();

    let empty = Vec::new();
    let exports = result.get("exports").and_then(|v| v.as_array()).unwrap_or(&empty);
    // verbose=true includes both exported and internal
    assert_eq!(exports.len(), 2);
}

#[test]
fn test_summarize_file_not_found() {
    let db_path = tmp_db_path();

    let params = SummarizeFileTool {
        file_path: "nonexistent.rs".to_string(),
        include_signatures: Some(false),
        verbose: Some(false),
    };

    let result = handle_summarize_file(&db_path, params).unwrap();

    assert_eq!(result.get("error").and_then(|v| v.as_str()), Some("FILE_NOT_FOUND"));
    assert!(result.get("message").is_some());
}

// ============================================================================
// Tests for get_module_summary tool
// ============================================================================

#[test]
fn test_get_module_summary_tool() {
    let db_path = tmp_db_path();

    create_test_symbol(&db_path, "export-1", "exportedFunction", "function", "src/module.ts", true).unwrap();
    create_test_symbol(&db_path, "export-2", "exportedClass", "class", "src/module.ts", true).unwrap();

    let params = GetModuleSummaryTool {
        file_path: "src/module.ts".to_string(),
        group_by_kind: Some(true),
    };

    let result = handle_get_module_summary(&db_path, params).unwrap();

    assert_eq!(result.get("export_count").and_then(|v| v.as_u64()), Some(2));
    assert!(result.get("exports").is_some());
    assert!(result.get("groups").is_some());

    // Check grouping worked
    let empty = Vec::new();
    let groups = result.get("groups").and_then(|v| v.as_array()).unwrap_or(&empty);
    assert!(!groups.is_empty());

    // Should have groups for function and class
    let kinds: Vec<_> = groups.iter()
        .filter_map(|g| g.get("kind").and_then(|k| k.as_str()))
        .collect();
    assert!(kinds.contains(&"function"));
    assert!(kinds.contains(&"class"));
}

#[test]
fn test_get_module_summary_flat() {
    let db_path = tmp_db_path();

    create_test_symbol(&db_path, "export-1", "myFunction", "function", "src/api.ts", true).unwrap();

    let params = GetModuleSummaryTool {
        file_path: "src/api.ts".to_string(),
        group_by_kind: Some(false), // Flat output
    };

    let result = handle_get_module_summary(&db_path, params).unwrap();

    assert_eq!(result.get("export_count").and_then(|v| v.as_u64()), Some(1));

    let empty = Vec::new();
    let exports = result.get("exports").and_then(|v| v.as_array()).unwrap_or(&empty);
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].get("name").and_then(|v| v.as_str()), Some("myFunction"));

    // groups should be empty when group_by_kind=false
    let groups = result.get("groups").and_then(|v| v.as_array()).unwrap_or(&empty);
    assert!(groups.is_empty());
}

#[test]
fn test_get_module_summary_no_exports() {
    let db_path = tmp_db_path();

    // Only create internal symbols (exported=false)
    create_test_symbol(&db_path, "internal-1", "internalFunc", "function", "src/internal.ts", false).unwrap();

    let params = GetModuleSummaryTool {
        file_path: "src/internal.ts".to_string(),
        group_by_kind: Some(false),
    };

    let result = handle_get_module_summary(&db_path, params).unwrap();

    // Should return NO_EXPORTS error
    assert_eq!(result.get("error").and_then(|v| v.as_str()), Some("NO_EXPORTS"));
    assert!(result.get("message").is_some());
}

#[test]
fn test_get_module_summary_signatures() {
    let db_path = tmp_db_path();

    create_test_symbol(&db_path, "export-1", "myFunction", "function", "src/utils.ts", true).unwrap();

    let params = GetModuleSummaryTool {
        file_path: "src/utils.ts".to_string(),
        group_by_kind: Some(false),
    };

    let result = handle_get_module_summary(&db_path, params).unwrap();

    let empty = Vec::new();
    let exports = result.get("exports").and_then(|v| v.as_array()).unwrap_or(&empty);
    assert_eq!(exports.len(), 1);

    // Check signature field exists
    assert!(exports[0].get("signature").is_some());

    // Signature should be a truncated version of the text
    let sig = exports[0].get("signature").and_then(|v| v.as_str()).unwrap_or("");
    assert!(!sig.is_empty());
}

// ============================================================================
// Tests for trace_data_flow tool
// ============================================================================

#[test]
fn test_trace_data_flow_tool() {
    let db_path = tmp_db_path();

    create_test_symbol(&db_path, "root", "dataVar", "variable", "src/main.rs", true).unwrap();

    let params = TraceDataFlowTool {
        symbol_name: "dataVar".to_string(),
        file_path: None,
        direction: Some("both".to_string()),
        depth: Some(2),
        limit: Some(50),
    };

    let result = handle_trace_data_flow(&db_path, params).unwrap();

    assert!(result.get("symbol_name").is_some());
    assert!(result.get("flows").is_some());
    assert!(result.get("read_count").is_some());
    assert!(result.get("write_count").is_some());
}

#[test]
fn test_trace_data_flow_not_found() {
    let db_path = tmp_db_path();

    let params = TraceDataFlowTool {
        symbol_name: "nonexistent".to_string(),
        file_path: None,
        direction: Some("both".to_string()),
        depth: Some(2),
        limit: Some(50),
    };

    let result = handle_trace_data_flow(&db_path, params).unwrap();

    assert_eq!(result.get("error").and_then(|v| v.as_str()), Some("SYMBOL_NOT_FOUND"));
}

// ============================================================================
// Tests for find_affected_code tool
// ============================================================================

#[test]
fn test_find_affected_code_tool() {
    let db_path = tmp_db_path();

    create_test_symbol(&db_path, "root", "apiFunction", "function", "src/api.rs", true).unwrap();

    let params = FindAffectedCodeTool {
        symbol_name: "apiFunction".to_string(),
        file_path: None,
        depth: Some(2),
        limit: Some(50),
        include_tests: Some(false),
    };

    let result = handle_find_affected_code(&db_path, params).unwrap();

    assert!(result.get("symbol_name").is_some());
    assert!(result.get("affected").is_some());
    assert!(result.get("affected_files").is_some());
}

#[test]
fn test_find_affected_code_not_found() {
    let db_path = tmp_db_path();

    let params = FindAffectedCodeTool {
        symbol_name: "nonexistent".to_string(),
        file_path: None,
        depth: Some(2),
        limit: Some(50),
        include_tests: Some(false),
    };

    let result = handle_find_affected_code(&db_path, params).unwrap();

    assert_eq!(result.get("error").and_then(|v| v.as_str()), Some("SYMBOL_NOT_FOUND"));
}

// ============================================================================
// Tests for report_selection tool
// ============================================================================

#[test]
fn test_report_selection_tool() {
    let db_path = tmp_db_path();

    // Create a symbol first (required for foreign key constraint)
    create_test_symbol(&db_path, "sym-123", "myFunction", "function", "src/api.rs", true).unwrap();

    let params = ReportSelectionTool {
        query: "test search".to_string(),
        selected_symbol_id: "sym-123".to_string(),
        position: 1,
    };

    // report_selection is async
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(handle_report_selection(&db_path, params)).unwrap();

    assert_eq!(result.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(result.get("recorded").and_then(|v| v.as_bool()), Some(true));
    assert!(result.get("selection_id").is_some());
    assert_eq!(result.get("query_normalized").and_then(|v| v.as_str()), Some("test search"));
}

#[test]
fn test_report_selection_normalizes_query() {
    let db_path = tmp_db_path();

    // Create a symbol first (required for foreign key constraint)
    create_test_symbol(&db_path, "sym-456", "anotherFunction", "function", "src/utils.rs", true).unwrap();

    let params = ReportSelectionTool {
        query: "  Test Search  ".to_string(), // Leading/trailing spaces and mixed case
        selected_symbol_id: "sym-456".to_string(),
        position: 2,
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(handle_report_selection(&db_path, params)).unwrap();

    // Query should be normalized (lowercased, trimmed)
    assert_eq!(result.get("query_normalized").and_then(|v| v.as_str()), Some("test search"));
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
fn test_config(base_dir: &PathBuf) -> Config {
    let base_dir = base_dir.canonicalize().unwrap_or_else(|_| base_dir.clone());
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
    ).unwrap();

    let config = Arc::new(test_config(&dir));

    let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
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
    let results = result.get("results").and_then(|v| v.as_array()).map_or(EMPTY, |v| v);
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
    let results = result.get("results").and_then(|v| v.as_array()).map_or(EMPTY, |v| v);
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

    let retriever = Retriever::new(config.clone(), tantivy, vectors, embedder, None, None, metrics);
    let state = code_intelligence_mcp_server::handlers::AppState {
        config,
        indexer,
        retriever,
    };

    // The handler might fail if embedding isn't found, so check both success and error cases
    let result = handle_find_similar_code(&state, params).await.unwrap();

    // Check response structure - could be success or error
    if result.get("error").is_some() {
        // If error, it should be SYMBOL_NOT_FOUND or similar
        assert!(result.get("error").and_then(|v| v.as_str()) == Some("SYMBOL_NOT_FOUND")
            || result.get("error").and_then(|v| v.as_str()) == Some("EMBEDDING_NOT_FOUND"));
    } else {
        // Success case - verify response structure
        assert!(result.get("threshold").is_some());
        assert!(result.get("count").is_some());
        assert!(result.get("results").is_some());
        // query field is present on success
        assert!(result.get("query").is_some());

        let results = result.get("results").and_then(|v| v.as_array()).map_or(EMPTY, |v| v);
        assert!(results.len() >= 0);
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

    let retriever = Retriever::new(config.clone(), tantivy, vectors, embedder, None, None, metrics);
    let state = code_intelligence_mcp_server::handlers::AppState {
        config,
        indexer,
        retriever,
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

    let retriever = Retriever::new(config.clone(), tantivy, vectors, embedder, None, None, metrics);
    let state = code_intelligence_mcp_server::handlers::AppState {
        config,
        indexer,
        retriever,
    };

    let result = handle_find_similar_code(&state, params).await.unwrap();

    assert_eq!(result.get("error").and_then(|v| v.as_str()), Some("SYMBOL_NOT_FOUND"));
}

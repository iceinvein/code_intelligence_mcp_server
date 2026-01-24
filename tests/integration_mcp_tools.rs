//! Integration tests for MCP tool handlers
//!
//! Tests verify that tool handlers produce correct response structures
//! and handle error cases appropriately.

use code_intelligence_mcp_server::{
    handlers::{
        handle_find_affected_code, handle_get_module_summary, handle_report_selection,
        handle_summarize_file, handle_trace_data_flow,
    },
    storage::sqlite::{SqliteStore, SymbolRow},
    tools::{FindAffectedCodeTool, GetModuleSummaryTool, ReportSelectionTool, SummarizeFileTool, TraceDataFlowTool},
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

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

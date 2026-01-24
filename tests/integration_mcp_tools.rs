//! Integration tests for MCP tool handlers
//!
//! Tests verify that tool handlers produce correct response structures
//! and handle error cases appropriately.

use code_intelligence_mcp_server::{
    handlers::{
        handle_find_affected_code, handle_get_module_summary, handle_report_selection,
        handle_summarize_file, handle_trace_data_flow,
    },
    storage::sqlite::SqliteStore,
    tools::{FindAffectedCodeTool, GetModuleSummaryTool, ReportSelectionTool, SummarizeFileTool, TraceDataFlowTool},
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{path::PathBuf, time::SystemTime, UNIX_EPOCH};

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

    let symbol = crate::storage::sqlite::SymbolRow {
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

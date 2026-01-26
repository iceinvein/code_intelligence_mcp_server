//! Test helper functions for integration tests
//!
//! This module provides procedural helper functions for common test operations.
//! These are not rstest fixtures - they're utility functions that can be called
//! directly in tests for setup and teardown operations.

use anyhow::Result;
use code_intelligence_mcp_server::path::Utf8PathBuf;
use code_intelligence_mcp_server::storage::sqlite::{SqliteStore, SymbolRow};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Generate a unique temporary database path for test isolation
///
/// Uses an atomic counter combined with nanosecond timestamp to ensure
/// unique paths across parallel test executions.
///
/// # Returns
///
/// A `PathBuf` pointing to a unique temp database file location
pub fn tmp_db_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("cimcp-mcp-test-{nanos}-{c}.db"));
    dir
}

/// Generate a unique temporary directory for test isolation
///
/// Creates a temporary directory with a unique name for test isolation.
/// The directory is created immediately and the path is returned.
///
/// Note: This is a procedural helper function, not a rstest fixture.
/// For rstest tests, use the `tmp_dir` fixture from fixtures.rs instead.
///
/// # Returns
///
/// A `PathBuf` pointing to a newly created temporary directory
pub fn tmp_test_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("cimcp-test-{nanos}-{c}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Create a test symbol in the SQLite database
///
/// This helper function creates a symbol with minimal required fields for testing.
/// It opens the database, initializes the schema if needed, and inserts the symbol.
///
/// # Arguments
///
/// * `db_path` - Path to the SQLite database file
/// * `id` - Unique identifier for the symbol
/// * `name` - Display name of the symbol
/// * `kind` - Type/kind of symbol (e.g., "function", "class", "variable")
/// * `file_path` - Source file path where symbol is defined
/// * `exported` - Whether the symbol is exported from its module
///
/// # Returns
///
/// `Ok(())` if the symbol was created successfully
///
/// # Errors
///
/// Returns an error if database operations fail
pub fn create_test_symbol(
    db_path: &Path,
    id: &str,
    name: &str,
    kind: &str,
    file_path: &str,
    exported: bool,
) -> Result<()> {
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

/// Create a test symbol with custom text content
///
/// Similar to `create_test_symbol` but allows specifying the source text
/// for the symbol, useful for testing search and content extraction.
///
/// # Arguments
///
/// * `db_path` - Path to the SQLite database file
/// * `id` - Unique identifier for the symbol
/// * `name` - Display name of the symbol
/// * `kind` - Type/kind of symbol
/// * `file_path` - Source file path
/// * `exported` - Whether the symbol is exported
/// * `text` - Source code text for the symbol
///
/// # Returns
///
/// `Ok(())` if the symbol was created successfully
#[allow(dead_code)]
pub fn create_test_symbol_with_text(
    db_path: &Path,
    id: &str,
    name: &str,
    kind: &str,
    file_path: &str,
    exported: bool,
    text: &str,
) -> Result<()> {
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
        end_byte: text.len() as u32,
        start_line: 1,
        end_line: text.lines().count() as u32,
        text: text.to_string(),
    };

    sqlite.upsert_symbol(&symbol)?;
    Ok(())
}

/// Create a test symbol for a specific programming language
///
/// Creates a symbol with the specified language field, useful for
/// testing language-specific indexing and extraction.
///
/// # Arguments
///
/// * `db_path` - Path to the SQLite database file
/// * `id` - Unique identifier for the symbol
/// * `name` - Display name of the symbol
/// * `kind` - Type/kind of symbol
/// * `file_path` - Source file path
/// * `language` - Programming language (e.g., "typescript", "python")
/// * `exported` - Whether the symbol is exported
///
/// # Returns
///
/// `Ok(())` if the symbol was created successfully
#[allow(dead_code)]
pub fn create_test_symbol_with_language(
    db_path: &Path,
    id: &str,
    name: &str,
    kind: &str,
    file_path: &str,
    language: &str,
    exported: bool,
) -> Result<()> {
    let db_path_utf8 = Utf8PathBuf::from_path_buf(db_path.to_path_buf())
        .map_err(|_| anyhow::anyhow!("Database path is not valid UTF-8"))?;
    let sqlite = SqliteStore::open(&db_path_utf8)?;
    sqlite.init()?;

    let symbol = SymbolRow {
        id: id.to_string(),
        file_path: file_path.to_string(),
        language: language.to_string(),
        kind: kind.to_string(),
        name: name.to_string(),
        exported,
        start_byte: 0,
        end_byte: 100,
        start_line: 1,
        end_line: 10,
        text: format!("// {} language\nexport function {}() {{}}", language, name),
    };

    sqlite.upsert_symbol(&symbol)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tmp_db_path_generates_unique_paths() {
        let path1 = tmp_db_path();
        let path2 = tmp_db_path();

        assert_ne!(path1, path2);
        assert!(path1.to_string_lossy().contains("cimcp-mcp-test-"));
        assert!(path2.to_string_lossy().contains("cimcp-mcp-test-"));
    }

    #[test]
    fn test_tmp_test_dir_creates_directory() {
        let dir = tmp_test_dir();
        assert!(dir.exists());
        assert!(dir.is_dir());
    }

    #[test]
    fn test_create_test_symbol() {
        let db_path = tmp_db_path();
        let result = create_test_symbol(
            &db_path,
            "test-id",
            "testFunction",
            "function",
            "src/test.rs",
            true,
        );

        assert!(result.is_ok());

        // Verify symbol was created
        let db_path_utf8 = Utf8PathBuf::from_path_buf(db_path).unwrap();
        let sqlite = SqliteStore::open(&db_path_utf8).unwrap();
        let symbols = sqlite.search_symbols_by_exact_name("testFunction", None, 10).unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "testFunction");
        assert!(symbols[0].exported);
    }
}

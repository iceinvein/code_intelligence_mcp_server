use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::storage::sqlite::schema::TestLinkRow;

/// `(symbol_id, symbol_name, file_path, start_line, edge_type)` for a test caller.
pub type TestCallerRow = (String, String, String, u32, String);

/// Determine if a file path is a test file.
///
/// Delegates to the canonical implementation in `crate::classify`.
pub fn is_test_file(path: &str) -> bool {
    crate::classify::is_test_file(path)
}

/// Infer source file path from test file path.
///
/// Strips a leading test-directory segment AND a test extension marker
/// (e.g. `src/foo/__tests__/bar.test.ts` -> `src/foo/bar.ts`). The previous
/// implementation returned after the first match, so files that combined
/// both a `__tests__/` directory and a `.test.` extension produced a
/// non-existent source path.
pub fn infer_source_from_test(test_path: &str) -> Option<String> {
    if !is_test_file(test_path) {
        return None;
    }

    let mut path = test_path.to_string();

    // Step 1: collapse a test-directory segment, leaving the trailing slash.
    let lower = path.to_lowercase();
    for pat in ["/__tests__/", "/tests/", "/test/"] {
        if let Some(pos) = lower.find(pat) {
            let mut buf = String::with_capacity(path.len());
            buf.push_str(&path[..pos]);
            buf.push('/');
            buf.push_str(&path[pos + pat.len()..]);
            path = buf;
            break;
        }
    }

    // Step 2: strip extension marker on the resulting path.
    let lower = path.to_lowercase();
    if lower.contains(".test.") {
        path = path.replacen(".test.", ".", 1);
    } else if lower.contains(".spec.") {
        path = path.replacen(".spec.", ".", 1);
    } else if let Some(ext) = [".ts", ".tsx", ".js", ".jsx", ".rs", ".go", ".py"]
        .into_iter()
        .find(|ext| lower.ends_with(&format!("_test{ext}")))
    {
        let trim_to = path.len() - format!("_test{ext}").len();
        path = format!("{}{ext}", &path[..trim_to]);
    }

    if path == test_path {
        None
    } else {
        Some(path)
    }
}

/// Create or update a test-source link
pub fn upsert_test_link(conn: &Connection, link: &TestLinkRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO test_links (test_file_path, source_file_path, link_direction, created_at)
VALUES (?1, ?2, ?3, unixepoch())
ON CONFLICT(test_file_path, source_file_path) DO UPDATE SET
  link_direction=excluded.link_direction
"#,
        params![
            link.test_file_path,
            link.source_file_path,
            link.link_direction,
        ],
    )
    .context("Failed to upsert test link")?;
    Ok(())
}

/// Auto-create test links when indexing a test file
pub fn create_test_links_for_file(conn: &Connection, test_file_path: &str) -> Result<()> {
    if !is_test_file(test_file_path) {
        return Ok(());
    }

    if let Some(source_path) = infer_source_from_test(test_file_path) {
        let link = TestLinkRow {
            test_file_path: test_file_path.to_string(),
            source_file_path: source_path,
            link_direction: "bidirectional".to_string(),
            created_at: 0,
        };
        upsert_test_link(conn, &link)?;
    }

    Ok(())
}

/// Get all test files that test a given source file
pub fn get_tests_for_source(conn: &Connection, source_path: &str) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT test_file_path
FROM test_links
WHERE source_file_path = ?1
ORDER BY test_file_path ASC
"#,
        )
        .context("Failed to prepare get_tests_for_source")?;

    let mut rows = stmt.query(params![source_path])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

/// Get all source files that a test file tests
pub fn get_sources_for_test(conn: &Connection, test_path: &str) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT source_file_path
FROM test_links
WHERE test_file_path = ?1
ORDER BY source_file_path ASC
"#,
        )
        .context("Failed to prepare get_sources_for_test")?;

    let mut rows = stmt.query(params![test_path])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

/// Delete test links for a file (either test or source)
pub fn delete_test_links_for_file(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM test_links WHERE test_file_path = ?1 OR source_file_path = ?1",
        params![file_path],
    )
    .context("Failed to delete test links for file")?;
    Ok(())
}

/// Get test links for symbols in a file (links test file to symbols)
pub fn get_symbols_with_tests(conn: &Connection, file_path: &str) -> Result<Vec<(String, String)>> {
    // Returns list of (symbol_name, test_file_path)
    let mut stmt = conn
        .prepare(
            r#"
SELECT s.name, tl.test_file_path
FROM test_links tl
JOIN symbols s ON s.file_path = tl.source_file_path
WHERE tl.source_file_path = ?1
ORDER BY s.name ASC
"#,
        )
        .context("Failed to prepare get_symbols_with_tests")?;

    let mut rows = stmt.query(params![file_path])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push((row.get(0)?, row.get(1)?));
    }
    Ok(out)
}

/// Find test symbols that have call/reference edges to a target symbol.
///
/// Returns tuples of `(symbol_id, symbol_name, file_path, start_line, edge_type)` for
/// every symbol whose file is one of `test_file_paths` and that has a `call` or
/// `reference` edge pointing at `target_symbol_id`.
///
/// # Errors
///
/// Returns an error if the SQLite query fails.
pub fn find_test_symbols_calling(
    conn: &Connection,
    test_file_paths: &[String],
    target_symbol_id: &str,
    limit: usize,
) -> Result<Vec<TestCallerRow>> {
    if test_file_paths.is_empty() {
        return Ok(Vec::new());
    }

    // Build positional placeholders for the IN clause: ?1, ?2, …, ?N
    // The target symbol id occupies ?N+1 and the limit occupies ?N+2.
    let placeholders: Vec<String> = (1..=test_file_paths.len())
        .map(|i| format!("?{i}"))
        .collect();
    let in_clause = placeholders.join(", ");
    let target_param_idx = test_file_paths.len() + 1;
    let limit_param_idx = test_file_paths.len() + 2;

    let sql = format!(
        r#"
SELECT s.id, s.name, s.file_path, s.start_line,
       GROUP_CONCAT(DISTINCT e.edge_type) AS edge_types
FROM symbols s
JOIN edges e ON e.from_symbol_id = s.id
WHERE s.file_path IN ({in_clause})
  AND e.to_symbol_id = ?{target_param_idx}
  AND e.edge_type IN ('call', 'reference')
GROUP BY s.id, s.name, s.file_path, s.start_line
ORDER BY s.file_path, s.start_line
LIMIT ?{limit_param_idx}
"#
    );

    let mut stmt = conn
        .prepare(&sql)
        .context("Failed to prepare find_test_symbols_calling")?;

    // Build the parameter list: test file paths first, then target id, then limit.
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = test_file_paths
        .iter()
        .map(|p| -> Box<dyn rusqlite::types::ToSql> { Box::new(p.clone()) })
        .collect();
    param_values.push(Box::new(target_symbol_id.to_string()));
    param_values.push(Box::new(limit as i64));

    let mut rows = stmt
        .query(rusqlite::params_from_iter(param_values.iter()))
        .context("Failed to execute find_test_symbols_calling")?;

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let start_line: i64 = row.get(3)?;
        out.push((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            start_line as u32,
            row.get(4)?,
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod infer_source_tests {
    use super::infer_source_from_test;

    #[test]
    fn maps_test_dot_extension_to_source_extension() {
        assert_eq!(
            infer_source_from_test("src/foo/bar.test.ts").as_deref(),
            Some("src/foo/bar.ts")
        );
        assert_eq!(
            infer_source_from_test("src/foo/bar.spec.tsx").as_deref(),
            Some("src/foo/bar.tsx")
        );
    }

    #[test]
    fn maps_underscore_test_suffix_per_language() {
        assert_eq!(
            infer_source_from_test("crates/foo/src/bar_test.rs").as_deref(),
            Some("crates/foo/src/bar.rs")
        );
        assert_eq!(
            infer_source_from_test("internal/parser_test.go").as_deref(),
            Some("internal/parser.go")
        );
    }

    #[test]
    fn collapses_tests_subdirectory_into_source_path() {
        assert_eq!(
            infer_source_from_test("src/main/tests/foo.ts").as_deref(),
            Some("src/main/foo.ts")
        );
    }

    /// Regression: previously the `.test.` check fired first and returned
    /// `src/main/__tests__/pr-review-dedupe.ts` (a path that does not exist
    /// on disk), so the test_links row pointed nowhere and
    /// `find_tests_for_symbol` could never resolve the link.
    #[test]
    fn collapses_underscored_tests_dir_alongside_dot_test_extension() {
        assert_eq!(
            infer_source_from_test("src/main/__tests__/pr-review-dedupe.test.ts").as_deref(),
            Some("src/main/pr-review-dedupe.ts")
        );
        assert_eq!(
            infer_source_from_test("packages/ui/__tests__/Button.spec.tsx").as_deref(),
            Some("packages/ui/Button.tsx")
        );
    }

    #[test]
    fn returns_none_for_non_test_paths() {
        assert_eq!(infer_source_from_test("src/main/pr-review-dedupe.ts"), None);
    }
}

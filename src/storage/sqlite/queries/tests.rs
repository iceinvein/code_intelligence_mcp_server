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

/// Infer source file path from test file path
pub fn infer_source_from_test(test_path: &str) -> Option<String> {
    let lower = test_path.to_lowercase();

    // .test.ts -> .ts
    if lower.contains(".test.") {
        return Some(test_path.replace(".test.", ".").replace(".TEST.", "."));
    }

    // .spec.ts -> .ts
    if lower.contains(".spec.") {
        return Some(test_path.replace(".spec.", "."));
    }

    // _test.ts -> .ts
    if lower.ends_with("_test.ts") {
        return Some(test_path.replace("_test.ts", ".ts"));
    }
    if lower.ends_with("_test.tsx") {
        return Some(test_path.replace("_test.tsx", ".tsx"));
    }
    if lower.ends_with("_test.js") {
        return Some(test_path.replace("_test.js", ".js"));
    }
    if lower.ends_with("_test.rs") {
        return Some(test_path.replace("_test.rs", ".rs"));
    }

    // /test/ or /tests/ directories
    if let Some(pos) = lower.find("/test/") {
        // /path/to/test/module.ts -> /path/to/module.ts
        let before = &test_path[..pos];
        let after = &test_path[pos + 5..]; // Skip "/test/"
        return Some(format!("{}{}", before, after));
    }

    if let Some(pos) = lower.find("/tests/") {
        let before = &test_path[..pos];
        let after = &test_path[pos + 6..]; // Skip "/tests/"
        return Some(format!("{}{}", before, after));
    }

    if let Some(pos) = lower.find("/__tests__/") {
        let before = &test_path[..pos];
        let after = &test_path[pos + 10..]; // Skip "/__tests__/"
        return Some(format!("{}{}", before, after));
    }

    None
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

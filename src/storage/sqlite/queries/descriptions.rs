//! SQLite queries for LLM-generated symbol descriptions cache.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// Symbol data needed for description generation.
pub struct SymbolForDescription {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub text: String,
    pub exported: bool,
    pub start_line: u32,
    pub end_line: u32,
}

/// Get cached description if content hash matches (symbol unchanged).
/// Returns None if no description exists or if content changed.
pub fn get_description(conn: &Connection, symbol_id: &str, content_hash: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT description FROM descriptions WHERE symbol_id = ?1 AND content_hash = ?2"
    )?;
    let result = stmt
        .query_row(params![symbol_id, content_hash], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .context("Failed to query description cache")?;
    Ok(result)
}

/// Get description for a symbol regardless of content hash.
/// Used when re-upserting a symbol to Tantivy (we want whatever description exists).
pub fn get_description_for_symbol(conn: &Connection, symbol_id: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT description FROM descriptions WHERE symbol_id = ?1"
    )?;
    let result = stmt
        .query_row(params![symbol_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .context("Failed to query description for symbol")?;
    Ok(result)
}

/// Store or update a description.
pub fn upsert_description(
    conn: &Connection,
    symbol_id: &str,
    content_hash: &str,
    description: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO descriptions (symbol_id, content_hash, description, generated_at)
         VALUES (?1, ?2, ?3, strftime('%s', 'now'))
         ON CONFLICT(symbol_id) DO UPDATE SET
            content_hash = excluded.content_hash,
            description = excluded.description,
            generated_at = excluded.generated_at",
        params![symbol_id, content_hash, description],
    )
    .context("Failed to upsert description")?;
    Ok(())
}

/// Get all symbols that don't have descriptions yet.
/// Joins symbols table with descriptions to find gaps.
pub fn get_undescribed_symbols(conn: &Connection) -> Result<Vec<SymbolForDescription>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.name, s.kind, s.file_path, s.text, s.exported, s.start_line, s.end_line
         FROM symbols s
         LEFT JOIN descriptions d ON s.id = d.symbol_id
         WHERE d.symbol_id IS NULL
           AND s.kind != 'file'
         ORDER BY s.exported DESC, (s.end_line - s.start_line) DESC"
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SymbolForDescription {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                file_path: row.get(3)?,
                text: row.get(4)?,
                exported: row.get::<_, i64>(5)? != 0,
                start_line: row.get::<_, i64>(6)? as u32,
                end_line: row.get::<_, i64>(7)? as u32,
            })
        })
        .context("Failed to query undescribed symbols")?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to collect undescribed symbols")
}

/// Get a batch of symbols that don't have descriptions yet.
///
/// Uses LIMIT without OFFSET — described symbols drop out of the LEFT JOIN
/// result set automatically, so each call returns the next batch.
/// File-kind symbols are excluded at the SQL level; further filtering
/// (test symbols, tiny private helpers) happens in the Rust caller.
pub fn get_undescribed_symbols_batch(conn: &Connection, limit: usize) -> Result<Vec<SymbolForDescription>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.name, s.kind, s.file_path, s.text, s.exported, s.start_line, s.end_line
         FROM symbols s
         LEFT JOIN descriptions d ON s.id = d.symbol_id
         WHERE d.symbol_id IS NULL
           AND s.kind != 'file'
         ORDER BY s.exported DESC, (s.end_line - s.start_line) DESC
         LIMIT ?1"
    )?;
    let rows = stmt
        .query_map(params![limit], |row| {
            Ok(SymbolForDescription {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                file_path: row.get(3)?,
                text: row.get(4)?,
                exported: row.get::<_, i64>(5)? != 0,
                start_line: row.get::<_, i64>(6)? as u32,
                end_line: row.get::<_, i64>(7)? as u32,
            })
        })
        .context("Failed to query undescribed symbols batch")?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to collect undescribed symbols batch")
}

/// Count symbols that don't have descriptions yet.
/// Excludes file-kind symbols which are never described.
pub fn count_undescribed_symbols(conn: &Connection) -> Result<usize> {
    let count: usize = conn.query_row(
        "SELECT COUNT(*) FROM symbols s
         LEFT JOIN descriptions d ON s.id = d.symbol_id
         WHERE d.symbol_id IS NULL
           AND s.kind != 'file'",
        [],
        |row| row.get(0),
    ).context("Failed to count undescribed symbols")?;
    Ok(count)
}

/// Delete descriptions for symbols that no longer exist.
pub fn cleanup_orphaned_descriptions(conn: &Connection) -> Result<usize> {
    let count = conn.execute(
        "DELETE FROM descriptions WHERE symbol_id NOT IN (SELECT id FROM symbols)",
        [],
    ).context("Failed to cleanup orphaned descriptions")?;
    Ok(count)
}

/// Row returned by `list_descriptions_with_symbol_data`.
pub struct DescriptionWithSymbolData {
    pub symbol_id: String,
    pub content_hash: String,
    pub description: String,
    pub name: String,
    pub kind: String,
    pub text: String,
    pub file_path: String,
}

/// Fetch all descriptions joined with their symbol data for stale-detection.
/// Returns (symbol_id, stored_content_hash, description, name, kind, text, file_path).
/// Optionally filtered by file_path prefix.
pub fn list_descriptions_with_symbol_data(
    conn: &Connection,
    file_path: Option<&str>,
    limit: usize,
) -> Result<Vec<DescriptionWithSymbolData>> {
    let sql = if file_path.is_some() {
        "SELECT d.symbol_id, d.content_hash, d.description, s.name, s.kind, s.text, s.file_path
         FROM descriptions d
         JOIN symbols s ON s.id = d.symbol_id
         WHERE s.file_path LIKE (?1 || '%')
         ORDER BY s.file_path ASC, s.name ASC
         LIMIT ?2"
    } else {
        "SELECT d.symbol_id, d.content_hash, d.description, s.name, s.kind, s.text, s.file_path
         FROM descriptions d
         JOIN symbols s ON s.id = d.symbol_id
         ORDER BY s.file_path ASC, s.name ASC
         LIMIT ?2"
    };

    let mut stmt = conn.prepare(sql)?;

    fn map_row(row: &rusqlite::Row) -> rusqlite::Result<DescriptionWithSymbolData> {
        Ok(DescriptionWithSymbolData {
            symbol_id: row.get(0)?,
            content_hash: row.get(1)?,
            description: row.get(2)?,
            name: row.get(3)?,
            kind: row.get(4)?,
            text: row.get(5)?,
            file_path: row.get(6)?,
        })
    }

    if let Some(fp) = file_path {
        stmt.query_map(params![fp, limit], map_row)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to list descriptions with symbol data")
    } else {
        stmt.query_map(params!["", limit], map_row)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to list descriptions with symbol data")
    }
}

/// Row returned by `find_undocumented_symbols_filtered`.
pub struct UndocumentedSymbol {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub exported: bool,
    pub line_count: u32,
}

/// Find symbols without descriptions, with filtering options.
///
/// Filters:
/// - `min_lines`: minimum (end_line - start_line) (default: 3)
/// - `exported_only`: if true, only include exported symbols
/// - `file_path`: if set, filter by file path prefix
/// - `limit`: max results
///
/// Always excludes file-kind symbols. Ordered by exported DESC, line_count DESC.
pub fn find_undocumented_symbols_filtered(
    conn: &Connection,
    min_lines: u32,
    exported_only: bool,
    file_path: Option<&str>,
    limit: usize,
) -> Result<Vec<UndocumentedSymbol>> {
    // Build query dynamically based on filters
    let mut sql = String::from(
        "SELECT s.id, s.name, s.kind, s.file_path, s.exported, (s.end_line - s.start_line) AS line_count
         FROM symbols s
         LEFT JOIN descriptions d ON s.id = d.symbol_id
         WHERE d.symbol_id IS NULL
           AND s.kind != 'file'
           AND (s.end_line - s.start_line) >= ?1"
    );

    if exported_only {
        sql.push_str(" AND s.exported = 1");
    }

    if file_path.is_some() {
        sql.push_str(" AND s.file_path LIKE (?3 || '%')");
    }

    sql.push_str(" ORDER BY s.exported DESC, line_count DESC LIMIT ?2");

    let mut stmt = conn.prepare(&sql)?;

    fn map_row(row: &rusqlite::Row) -> rusqlite::Result<UndocumentedSymbol> {
        Ok(UndocumentedSymbol {
            id: row.get(0)?,
            name: row.get(1)?,
            kind: row.get(2)?,
            file_path: row.get(3)?,
            exported: row.get::<_, i64>(4)? != 0,
            line_count: row.get::<_, i64>(5)? as u32,
        })
    }

    if let Some(fp) = file_path {
        stmt.query_map(params![min_lines, limit, fp], map_row)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to find undocumented symbols")
    } else {
        stmt.query_map(params![min_lines, limit], map_row)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to find undocumented symbols")
    }
}

/// Get count of described symbols (for stats/logging).
pub fn count_descriptions(conn: &Connection) -> Result<usize> {
    let count: usize = conn.query_row(
        "SELECT COUNT(*) FROM descriptions",
        [],
        |row| row.get(0),
    ).context("Failed to count descriptions")?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::storage::sqlite::schema::SCHEMA_SQL).unwrap();
        // Insert a test symbol
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES ('sym1', 'src/foo.rs', 'rust', 'function', 'do_stuff', 1, 0, 100, 1, 10, 'fn do_stuff() {}')",
            [],
        ).unwrap();
        conn
    }

    #[test]
    fn get_description_returns_none_when_empty() {
        let conn = setup_test_db();
        let result = get_description(&conn, "sym1", "hash123").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn upsert_and_get_description() {
        let conn = setup_test_db();
        upsert_description(&conn, "sym1", "hash123", "Does stuff with things").unwrap();
        let result = get_description(&conn, "sym1", "hash123").unwrap();
        assert_eq!(result, Some("Does stuff with things".to_string()));
    }

    #[test]
    fn get_description_returns_none_on_hash_mismatch() {
        let conn = setup_test_db();
        upsert_description(&conn, "sym1", "hash_old", "Old description").unwrap();
        let result = get_description(&conn, "sym1", "hash_new").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_description_for_symbol_ignores_hash() {
        let conn = setup_test_db();
        upsert_description(&conn, "sym1", "hash_old", "Old description").unwrap();
        let result = get_description_for_symbol(&conn, "sym1").unwrap();
        assert_eq!(result, Some("Old description".to_string()));
    }

    #[test]
    fn upsert_overwrites_existing() {
        let conn = setup_test_db();
        upsert_description(&conn, "sym1", "hash1", "First").unwrap();
        upsert_description(&conn, "sym1", "hash2", "Second").unwrap();
        let result = get_description(&conn, "sym1", "hash2").unwrap();
        assert_eq!(result, Some("Second".to_string()));
    }

    #[test]
    fn get_undescribed_symbols_finds_gaps() {
        let conn = setup_test_db();
        let undescribed = get_undescribed_symbols(&conn).unwrap();
        assert_eq!(undescribed.len(), 1);
        assert_eq!(undescribed[0].id, "sym1");
    }

    #[test]
    fn get_undescribed_excludes_described() {
        let conn = setup_test_db();
        upsert_description(&conn, "sym1", "hash", "Described").unwrap();
        let undescribed = get_undescribed_symbols(&conn).unwrap();
        assert_eq!(undescribed.len(), 0);
    }

    #[test]
    fn cleanup_orphaned_removes_stale() {
        let conn = setup_test_db();
        // Insert description for non-existent symbol (no FK on descriptions table)
        conn.execute(
            "INSERT INTO descriptions (symbol_id, content_hash, description, generated_at) VALUES ('sym_gone', 'hash', 'Orphan', 0)",
            [],
        ).unwrap();
        upsert_description(&conn, "sym1", "hash", "Valid").unwrap();
        let removed = cleanup_orphaned_descriptions(&conn).unwrap();
        assert_eq!(removed, 1);
        assert!(get_description_for_symbol(&conn, "sym1").unwrap().is_some());
        assert!(get_description_for_symbol(&conn, "sym_gone").unwrap().is_none());
    }

    #[test]
    fn count_descriptions_works() {
        let conn = setup_test_db();
        assert_eq!(count_descriptions(&conn).unwrap(), 0);
        upsert_description(&conn, "sym1", "h", "d").unwrap();
        assert_eq!(count_descriptions(&conn).unwrap(), 1);
    }

    #[test]
    fn get_undescribed_batch_respects_limit() {
        let conn = setup_test_db();
        // setup_test_db inserts sym1; add two more
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES ('sym2', 'src/bar.rs', 'rust', 'function', 'bar', 1, 0, 50, 1, 5, 'fn bar() {}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES ('sym3', 'src/baz.rs', 'rust', 'function', 'baz', 1, 0, 50, 1, 5, 'fn baz() {}')",
            [],
        ).unwrap();

        let batch = get_undescribed_symbols_batch(&conn, 2).unwrap();
        assert_eq!(batch.len(), 2);

        // Describe one, next batch should return remaining
        upsert_description(&conn, &batch[0].id, "h", "d").unwrap();
        let batch2 = get_undescribed_symbols_batch(&conn, 2).unwrap();
        assert_eq!(batch2.len(), 2); // sym that was described drops out, 2 remain
    }

    #[test]
    fn count_undescribed_symbols_works() {
        let conn = setup_test_db();
        assert_eq!(count_undescribed_symbols(&conn).unwrap(), 1);
        upsert_description(&conn, "sym1", "h", "d").unwrap();
        assert_eq!(count_undescribed_symbols(&conn).unwrap(), 0);
    }

    #[test]
    fn list_descriptions_with_symbol_data_returns_joined_rows() {
        let conn = setup_test_db();
        upsert_description(&conn, "sym1", "hash_abc", "Does stuff").unwrap();

        let rows = list_descriptions_with_symbol_data(&conn, None, 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol_id, "sym1");
        assert_eq!(rows[0].content_hash, "hash_abc");
        assert_eq!(rows[0].description, "Does stuff");
        assert_eq!(rows[0].name, "do_stuff");
        assert_eq!(rows[0].kind, "function");
        assert_eq!(rows[0].file_path, "src/foo.rs");
    }

    #[test]
    fn list_descriptions_with_symbol_data_filters_by_file_path() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES ('sym2', 'lib/bar.rs', 'rust', 'function', 'bar_fn', 1, 0, 50, 1, 5, 'fn bar_fn() {}')",
            [],
        ).unwrap();
        upsert_description(&conn, "sym1", "h1", "Desc 1").unwrap();
        upsert_description(&conn, "sym2", "h2", "Desc 2").unwrap();

        let rows = list_descriptions_with_symbol_data(&conn, Some("src/"), 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "do_stuff");

        let rows_all = list_descriptions_with_symbol_data(&conn, None, 100).unwrap();
        assert_eq!(rows_all.len(), 2);
    }

    #[test]
    fn list_descriptions_with_symbol_data_respects_limit() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES ('sym2', 'src/bar.rs', 'rust', 'function', 'bar_fn', 1, 0, 50, 1, 5, 'fn bar_fn() {}')",
            [],
        ).unwrap();
        upsert_description(&conn, "sym1", "h1", "Desc 1").unwrap();
        upsert_description(&conn, "sym2", "h2", "Desc 2").unwrap();

        let rows = list_descriptions_with_symbol_data(&conn, None, 1).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn find_undocumented_symbols_filtered_basic() {
        let conn = setup_test_db();
        // sym1 has no description and spans lines 1-10 (9 lines)
        let undoc = find_undocumented_symbols_filtered(&conn, 3, false, None, 100).unwrap();
        assert_eq!(undoc.len(), 1);
        assert_eq!(undoc[0].name, "do_stuff");
        assert_eq!(undoc[0].line_count, 9); // end_line(10) - start_line(1)
    }

    #[test]
    fn find_undocumented_excludes_described_symbols() {
        let conn = setup_test_db();
        upsert_description(&conn, "sym1", "h", "d").unwrap();
        let undoc = find_undocumented_symbols_filtered(&conn, 1, false, None, 100).unwrap();
        assert_eq!(undoc.len(), 0);
    }

    #[test]
    fn find_undocumented_filters_by_min_lines() {
        let conn = setup_test_db();
        // sym1 has 9 lines (end_line=10, start_line=1)
        let undoc = find_undocumented_symbols_filtered(&conn, 20, false, None, 100).unwrap();
        assert_eq!(undoc.len(), 0); // 9 < 20

        let undoc = find_undocumented_symbols_filtered(&conn, 5, false, None, 100).unwrap();
        assert_eq!(undoc.len(), 1); // 9 >= 5
    }

    #[test]
    fn find_undocumented_filters_exported_only() {
        let conn = setup_test_db();
        // sym1 is exported (1), add a non-exported symbol
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES ('sym_priv', 'src/priv.rs', 'rust', 'function', 'private_fn', 0, 0, 200, 1, 20, 'fn private_fn() {}')",
            [],
        ).unwrap();

        let all = find_undocumented_symbols_filtered(&conn, 1, false, None, 100).unwrap();
        assert_eq!(all.len(), 2);

        let exported = find_undocumented_symbols_filtered(&conn, 1, true, None, 100).unwrap();
        assert_eq!(exported.len(), 1);
        assert_eq!(exported[0].name, "do_stuff");
    }

    #[test]
    fn find_undocumented_filters_by_file_path() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES ('sym2', 'lib/other.rs', 'rust', 'function', 'other_fn', 1, 0, 100, 1, 10, 'fn other_fn() {}')",
            [],
        ).unwrap();

        let src_only = find_undocumented_symbols_filtered(&conn, 1, false, Some("src/"), 100).unwrap();
        assert_eq!(src_only.len(), 1);
        assert_eq!(src_only[0].name, "do_stuff");

        let lib_only = find_undocumented_symbols_filtered(&conn, 1, false, Some("lib/"), 100).unwrap();
        assert_eq!(lib_only.len(), 1);
        assert_eq!(lib_only[0].name, "other_fn");
    }

    #[test]
    fn find_undocumented_excludes_file_kind() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES ('sym_file', 'src/mod.rs', 'rust', 'file', 'mod.rs', 1, 0, 500, 1, 50, '// file contents')",
            [],
        ).unwrap();

        let undoc = find_undocumented_symbols_filtered(&conn, 1, false, None, 100).unwrap();
        // Should only have sym1 (do_stuff), not the file symbol
        assert_eq!(undoc.len(), 1);
        assert_eq!(undoc[0].name, "do_stuff");
    }

    #[test]
    fn find_undocumented_respects_limit() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES ('sym2', 'src/bar.rs', 'rust', 'function', 'bar_fn', 1, 0, 100, 1, 10, 'fn bar_fn() {}')",
            [],
        ).unwrap();

        let undoc = find_undocumented_symbols_filtered(&conn, 1, false, None, 1).unwrap();
        assert_eq!(undoc.len(), 1);
    }

    #[test]
    fn find_undocumented_orders_exported_first() {
        let conn = setup_test_db();
        // sym1 is exported. Add a bigger private symbol.
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES ('sym_big_priv', 'src/big.rs', 'rust', 'function', 'big_private', 0, 0, 1000, 1, 100, 'fn big_private() { /* 100 lines */ }')",
            [],
        ).unwrap();

        let undoc = find_undocumented_symbols_filtered(&conn, 1, false, None, 100).unwrap();
        assert_eq!(undoc.len(), 2);
        // Exported (do_stuff) should come first despite smaller line count
        assert_eq!(undoc[0].name, "do_stuff");
        assert!(undoc[0].exported);
        assert_eq!(undoc[1].name, "big_private");
        assert!(!undoc[1].exported);
    }
}

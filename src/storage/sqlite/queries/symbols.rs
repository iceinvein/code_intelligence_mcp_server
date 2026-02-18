use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::{SymbolHeaderRow, SymbolRow};

pub fn upsert_symbol(conn: &Connection, symbol: &SymbolRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO symbols (
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, unixepoch())
ON CONFLICT(id) DO UPDATE SET
  file_path=excluded.file_path,
  language=excluded.language,
  kind=excluded.kind,
  name=excluded.name,
  exported=excluded.exported,
  start_byte=excluded.start_byte,
  end_byte=excluded.end_byte,
  start_line=excluded.start_line,
  end_line=excluded.end_line,
  text=excluded.text,
  updated_at=unixepoch()
"#,
        params![
            symbol.id,
            symbol.file_path,
            symbol.language,
            symbol.kind,
            symbol.name,
            if symbol.exported { 1 } else { 0 },
            symbol.start_byte,
            symbol.end_byte,
            symbol.start_line,
            symbol.end_line,
            symbol.text
        ],
    )
    .with_context(|| {
        format!(
            "Failed to upsert symbol: id={}, file_path={}, name={}",
            symbol.id, symbol.file_path, symbol.name
        )
    })?;
    Ok(())
}

pub fn batch_upsert_symbols(conn: &Connection, symbols: &[SymbolRow]) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        r#"
INSERT INTO symbols (
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, unixepoch())
ON CONFLICT(id) DO UPDATE SET
  file_path=excluded.file_path,
  language=excluded.language,
  kind=excluded.kind,
  name=excluded.name,
  exported=excluded.exported,
  start_byte=excluded.start_byte,
  end_byte=excluded.end_byte,
  start_line=excluded.start_line,
  end_line=excluded.end_line,
  text=excluded.text,
  updated_at=unixepoch()
"#,
    )?;
    for s in symbols {
        stmt.execute(params![
            s.id,
            s.file_path,
            s.language,
            s.kind,
            s.name,
            if s.exported { 1 } else { 0 },
            s.start_byte,
            s.end_byte,
            s.start_line,
            s.end_line,
            s.text
        ])
        .with_context(|| format!("Failed to batch upsert symbol: id={}", s.id))?;
    }
    Ok(())
}

pub fn delete_symbols_by_file(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM symbols WHERE file_path = ?1",
        params![file_path],
    )
    .with_context(|| format!("Failed to delete symbols for file: {file_path}"))?;
    Ok(())
}

pub fn count_symbols(conn: &Connection) -> Result<u64> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
        .context("Failed to count symbols: table=symbols, COUNT(*)")?;
    Ok(count.max(0) as u64)
}

pub fn most_recent_symbol_update(conn: &Connection) -> Result<Option<i64>> {
    let ts: Option<i64> = conn
        .query_row("SELECT MAX(updated_at) FROM symbols", [], |row| row.get(0))
        .optional()
        .context("Failed to query most recent symbol update: table=symbols, MAX(updated_at)")?
        .flatten();
    Ok(ts)
}

pub fn search_symbols_by_exact_name(
    conn: &Connection,
    name: &str,
    file_path: Option<&str>,
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    let mut out = Vec::new();

    match file_path {
        Some(fp) => {
            let mut stmt = conn
                .prepare(
                    r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE name = ?1 AND file_path = ?2
ORDER BY exported DESC, start_byte ASC
LIMIT ?3
"#,
                )
                .context("Failed to prepare search_symbols_by_exact_name (file)")?;
            let mut rows = stmt.query(params![name, fp, limit as i64])?;
            while let Some(row) = rows.next()? {
                out.push(SymbolRow {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    language: row.get(2)?,
                    kind: row.get(3)?,
                    name: row.get(4)?,
                    exported: row.get::<_, i64>(5)? != 0,
                    start_byte: row.get::<_, i64>(6)? as u32,
                    end_byte: row.get::<_, i64>(7)? as u32,
                    start_line: row.get::<_, i64>(8)? as u32,
                    end_line: row.get::<_, i64>(9)? as u32,
                    text: row.get(10)?,
                });
            }
        }
        None => {
            let mut stmt = conn
                .prepare(
                    r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE name = ?1
ORDER BY exported DESC, file_path ASC, start_byte ASC
LIMIT ?2
"#,
                )
                .context("Failed to prepare search_symbols_by_exact_name")?;
            let mut rows = stmt.query(params![name, limit as i64])?;
            while let Some(row) = rows.next()? {
                out.push(SymbolRow {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    language: row.get(2)?,
                    kind: row.get(3)?,
                    name: row.get(4)?,
                    exported: row.get::<_, i64>(5)? != 0,
                    start_byte: row.get::<_, i64>(6)? as u32,
                    end_byte: row.get::<_, i64>(7)? as u32,
                    start_line: row.get::<_, i64>(8)? as u32,
                    end_line: row.get::<_, i64>(9)? as u32,
                    text: row.get(10)?,
                });
            }
        }
    }

    Ok(out)
}

pub fn search_symbols_by_text_substr(
    conn: &Connection,
    needle: &str,
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE instr(text, ?1) > 0
ORDER BY exported DESC, file_path ASC, start_byte ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare search_symbols_by_text_substr")?;

    let mut rows = stmt.query(params![needle, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SymbolRow {
            id: row.get(0)?,
            file_path: row.get(1)?,
            language: row.get(2)?,
            kind: row.get(3)?,
            name: row.get(4)?,
            exported: row.get::<_, i64>(5)? != 0,
            start_byte: row.get::<_, i64>(6)? as u32,
            end_byte: row.get::<_, i64>(7)? as u32,
            start_line: row.get::<_, i64>(8)? as u32,
            end_line: row.get::<_, i64>(9)? as u32,
            text: row.get(10)?,
        });
    }
    Ok(out)
}

pub fn get_symbol_by_id(conn: &Connection, id: &str) -> Result<Option<SymbolRow>> {
    conn.query_row(
        r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE id = ?1
"#,
        params![id],
        |row| {
            Ok(SymbolRow {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                name: row.get(4)?,
                exported: row.get::<_, i64>(5)? != 0,
                start_byte: row.get::<_, i64>(6)? as u32,
                end_byte: row.get::<_, i64>(7)? as u32,
                start_line: row.get::<_, i64>(8)? as u32,
                end_line: row.get::<_, i64>(9)? as u32,
                text: row.get(10)?,
            })
        },
    )
    .optional()
    .with_context(|| format!("Failed to query symbol by id: table=symbols, id={}", id))
}

pub fn list_symbol_headers_by_file(
    conn: &Connection,
    file_path: &str,
    exported_only: bool,
) -> Result<Vec<SymbolHeaderRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line
FROM symbols
WHERE file_path = ?1 AND (?2 = 0 OR exported = ?2)
ORDER BY start_byte ASC
"#,
        )
        .context("Failed to prepare list_symbol_headers_by_file")?;

    let mut rows = stmt.query(params![file_path, if exported_only { 1 } else { 0 }])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SymbolHeaderRow {
            id: row.get(0)?,
            file_path: row.get(1)?,
            language: row.get(2)?,
            kind: row.get(3)?,
            name: row.get(4)?,
            exported: row.get::<_, i64>(5)? != 0,
            start_byte: row.get::<_, i64>(6)? as u32,
            end_byte: row.get::<_, i64>(7)? as u32,
            start_line: row.get::<_, i64>(8)? as u32,
            end_line: row.get::<_, i64>(9)? as u32,
        });
    }

    // Log diagnostic info for empty results
    if out.is_empty() {
        tracing::warn!(
            file_path = %file_path,
            exported_only = exported_only,
            "No symbols found for file path"
        );

        // Try to find similar paths for debugging
        let mut similar_stmt = conn
            .prepare("SELECT DISTINCT file_path FROM symbols WHERE file_path LIKE ?1 LIMIT 5")
            .context("Failed to prepare similar path query")?;

        let pattern = if let Some(parent) = file_path.rsplit('/').next() {
            format!("%{}%", parent)
        } else {
            format!("%{}%", file_path)
        };

        let similar_paths = {
            let mut rows = similar_stmt.query(params![pattern])?;
            let mut paths = Vec::new();
            while let Some(row) = rows.next().ok().flatten() {
                if let Ok(path) = row.get::<_, String>(0) {
                    paths.push(path);
                }
            }
            paths
        };

        if !similar_paths.is_empty() {
            tracing::warn!(
                file_path = %file_path,
                similar_paths = ?similar_paths,
                "Found similar file paths in database"
            );
        }
    }

    Ok(out)
}

pub fn list_symbol_id_name_pairs(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare("SELECT id, name FROM symbols ORDER BY name ASC")
        .context("Failed to prepare list_symbol_id_name_pairs")?;

    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push((row.get(0)?, row.get(1)?));
    }
    Ok(out)
}

pub fn list_symbols_by_file(conn: &Connection, file_path: &str) -> Result<Vec<SymbolRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE file_path = ?1
ORDER BY start_byte ASC
"#,
        )
        .context("Failed to prepare list_symbols_by_file")?;

    let mut rows = stmt.query(params![file_path])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SymbolRow {
            id: row.get(0)?,
            file_path: row.get(1)?,
            language: row.get(2)?,
            kind: row.get(3)?,
            name: row.get(4)?,
            exported: row.get::<_, i64>(5)? != 0,
            start_byte: row.get::<_, i64>(6)? as u32,
            end_byte: row.get::<_, i64>(7)? as u32,
            start_line: row.get::<_, i64>(8)? as u32,
            end_line: row.get::<_, i64>(9)? as u32,
            text: row.get(10)?,
        });
    }
    Ok(out)
}

pub fn search_symbols_by_name_prefix(
    conn: &Connection,
    prefix: &str,
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE name LIKE (?1 || '%')
ORDER BY name ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare search_symbols_by_name_prefix")?;

    let mut rows = stmt.query(params![prefix, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SymbolRow {
            id: row.get(0)?,
            file_path: row.get(1)?,
            language: row.get(2)?,
            kind: row.get(3)?,
            name: row.get(4)?,
            exported: row.get::<_, i64>(5)? != 0,
            start_byte: row.get::<_, i64>(6)? as u32,
            end_byte: row.get::<_, i64>(7)? as u32,
            start_line: row.get::<_, i64>(8)? as u32,
            end_line: row.get::<_, i64>(9)? as u32,
            text: row.get(10)?,
        });
    }
    Ok(out)
}

/// Batch lookup line counts for symbols by ID.
///
/// Returns HashMap mapping symbol_id to line_count (end_line - start_line + 1).
/// Symbols not found in the database are omitted from the result.
pub fn batch_get_symbol_line_counts(
    conn: &Connection,
    symbol_ids: &[String],
) -> Result<std::collections::HashMap<String, u32>> {
    if symbol_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let placeholders = symbol_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(",");

    let query = format!(
        "SELECT id, (end_line - start_line + 1) AS line_count FROM symbols WHERE id IN ({})",
        placeholders
    );

    let mut stmt = conn
        .prepare(&query)
        .context("Failed to prepare batch_get_symbol_line_counts")?;

    let params: Vec<&dyn rusqlite::ToSql> = symbol_ids
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql)
        .collect();

    let mut rows = stmt.query(params.as_slice())?;
    let mut out = std::collections::HashMap::new();
    while let Some(row) = rows.next()? {
        let symbol_id: String = row.get(0)?;
        let line_count: i64 = row.get(1)?;
        out.insert(symbol_id, line_count.max(1) as u32);
    }
    Ok(out)
}

/// Batch-fetch the body text (source code) for a set of symbols.
///
/// Used by the term_coverage signal to check whether query terms appear
/// in a symbol's actual source code, not just its name or file path.
pub fn batch_get_symbol_texts(
    conn: &Connection,
    symbol_ids: &[String],
) -> Result<std::collections::HashMap<String, String>> {
    if symbol_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let placeholders = symbol_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(",");

    let query = format!(
        "SELECT id, text FROM symbols WHERE id IN ({})",
        placeholders
    );

    let mut stmt = conn
        .prepare(&query)
        .context("Failed to prepare batch_get_symbol_texts")?;

    let params: Vec<&dyn rusqlite::ToSql> = symbol_ids
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql)
        .collect();

    let mut rows = stmt.query(params.as_slice())?;
    let mut out = std::collections::HashMap::new();
    while let Some(row) = rows.next()? {
        let symbol_id: String = row.get(0)?;
        let text: String = row.get(1)?;
        out.insert(symbol_id, text);
    }
    Ok(out)
}

/// Batch check which symbols are test code (inside `mod tests` blocks or annotated with `#[test]`).
///
/// Returns a HashSet of symbol IDs that are detected as test code.
/// Detection criteria:
/// 1. Symbol is inside a `mod tests` block (byte range containment in same file)
/// 2. Symbol text contains `#[test]` attribute
/// 3. Symbol name starts with `test_` in a Rust file (naming convention)
pub fn batch_check_test_symbols(
    conn: &Connection,
    symbol_ids: &[String],
) -> Result<std::collections::HashSet<String>> {
    if symbol_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }

    let placeholders = symbol_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(",");

    // Find symbols that are inside a `mod tests` block (byte range containment),
    // have #[test] in their text, or are members of a Mock/Test/Fake class.
    let query = format!(
        r#"
SELECT DISTINCT s.id
FROM symbols s
WHERE s.id IN ({placeholders})
  AND (
    -- Criterion 1: inside a module named "tests" in the same file
    EXISTS (
      SELECT 1 FROM symbols m
      WHERE m.file_path = s.file_path
        AND m.kind = 'module'
        AND m.name = 'tests'
        AND m.start_byte <= s.start_byte
        AND m.end_byte >= s.end_byte
        AND m.id != s.id
    )
    -- Criterion 2: has #[test] attribute in source text (but not file symbols,
    -- whose text spans the entire file and would false-positive on any file with tests)
    OR (s.kind != 'file' AND instr(s.text, '#[test]') > 0)
    -- Criterion 3: member of a Mock/Test/Fake/Stub class (e.g., MockTransaction
    -- methods like _commit, _rollback that have normal names but are test infra).
    -- Uses line range containment to detect class membership.
    OR EXISTS (
      SELECT 1 FROM symbols parent
      WHERE parent.file_path = s.file_path
        AND parent.kind IN ('class', 'interface')
        AND parent.start_line <= s.start_line
        AND parent.end_line >= s.end_line
        AND parent.id != s.id
        AND (
          parent.name LIKE 'Mock%'
          OR parent.name LIKE '%Mock'
          OR parent.name LIKE 'Fake%'
          OR parent.name LIKE 'Stub%'
        )
    )
  )
"#
    );

    let mut stmt = conn
        .prepare(&query)
        .context("Failed to prepare batch_check_test_symbols")?;

    let params: Vec<&dyn rusqlite::ToSql> = symbol_ids
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql)
        .collect();

    let mut rows = stmt.query(params.as_slice())?;
    let mut out = std::collections::HashSet::new();
    while let Some(row) = rows.next()? {
        let symbol_id: String = row.get(0)?;
        out.insert(symbol_id);
    }
    Ok(out)
}

pub fn search_symbols_by_name_substr(
    conn: &Connection,
    needle: &str,
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE instr(name, ?1) > 0
ORDER BY name ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare search_symbols_by_name_substr")?;

    let mut rows = stmt.query(params![needle, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SymbolRow {
            id: row.get(0)?,
            file_path: row.get(1)?,
            language: row.get(2)?,
            kind: row.get(3)?,
            name: row.get(4)?,
            exported: row.get::<_, i64>(5)? != 0,
            start_byte: row.get::<_, i64>(6)? as u32,
            end_byte: row.get::<_, i64>(7)? as u32,
            start_line: row.get::<_, i64>(8)? as u32,
            end_line: row.get::<_, i64>(9)? as u32,
            text: row.get(10)?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::SqliteStore;

    #[test]
    fn batch_upsert_symbols_inserts_multiple() {
        let store = SqliteStore::open_in_memory().unwrap();
        store.init().unwrap();
        let conn = store.read().unwrap();

        let symbols = vec![
            SymbolRow {
                id: "s1".into(),
                file_path: "a.rs".into(),
                language: "rust".into(),
                kind: "function".into(),
                name: "foo".into(),
                exported: true,
                start_byte: 0,
                end_byte: 10,
                start_line: 1,
                end_line: 3,
                text: "fn foo() {}".into(),
            },
            SymbolRow {
                id: "s2".into(),
                file_path: "a.rs".into(),
                language: "rust".into(),
                kind: "function".into(),
                name: "bar".into(),
                exported: false,
                start_byte: 11,
                end_byte: 20,
                start_line: 4,
                end_line: 6,
                text: "fn bar() {}".into(),
            },
        ];

        batch_upsert_symbols(&conn, &symbols).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}

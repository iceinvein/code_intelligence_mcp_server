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
    .context("Failed to upsert symbol")?;
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
        .context("Failed to count symbols")?;
    Ok(count.max(0) as u64)
}

pub fn most_recent_symbol_update(conn: &Connection) -> Result<Option<i64>> {
    let ts: Option<i64> = conn
        .query_row("SELECT MAX(updated_at) FROM symbols", [], |row| row.get(0))
        .optional()
        .context("Failed to query most recent symbol update")?
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
    .context("Failed to query symbol by id")
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

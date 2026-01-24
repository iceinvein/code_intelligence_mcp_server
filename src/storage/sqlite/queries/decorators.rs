use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::storage::sqlite::schema::DecoratorRow;

/// Upsert a single decorator entry.
pub fn upsert_decorator(conn: &Connection, decorator: &DecoratorRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO decorators (
  symbol_id, name, arguments, target_line, decorator_type, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, unixepoch())
ON CONFLICT(symbol_id, name) DO UPDATE SET
  arguments=excluded.arguments,
  target_line=excluded.target_line,
  decorator_type=excluded.decorator_type,
  updated_at=unixepoch()
"#,
        params![
            decorator.symbol_id,
            decorator.name,
            decorator.arguments,
            decorator.target_line,
            decorator.decorator_type,
        ],
    )
    .context("Failed to upsert decorator")?;
    Ok(())
}

/// Upsert multiple decorator entries in a transaction.
pub fn batch_upsert_decorators(conn: &Connection, decorators: &[DecoratorRow]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for decorator in decorators {
        upsert_decorator(&tx, decorator)?;
    }
    tx.commit()?;
    Ok(())
}

/// Get all decorators for a specific symbol.
pub fn get_decorators_by_symbol(conn: &Connection, symbol_id: &str) -> Result<Vec<DecoratorRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT symbol_id, name, arguments, target_line, decorator_type
FROM decorators
WHERE symbol_id = ?1
ORDER BY target_line ASC
"#,
        )
        .context("Failed to prepare get_decorators_by_symbol")?;

    let mut rows = stmt.query(params![symbol_id])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(DecoratorRow {
            symbol_id: row.get(0)?,
            name: row.get(1)?,
            arguments: row.get(2)?,
            target_line: row.get::<_, i64>(3)? as u32,
            decorator_type: row.get(4)?,
            updated_at: 0, // Not queried back
        });
    }
    Ok(out)
}

/// Search for decorators by name (exact or prefix match).
pub fn search_decorators_by_name(
    conn: &Connection,
    name: &str,
    limit: usize,
) -> Result<Vec<DecoratorRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT d.symbol_id, d.name, d.arguments, d.target_line, d.decorator_type
FROM decorators d
JOIN symbols s ON d.symbol_id = s.id
WHERE d.name = ?1 OR d.name LIKE (?1 || '%')
ORDER BY s.file_path ASC, d.target_line ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare search_decorators_by_name")?;

    let mut rows = stmt.query(params![name, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(DecoratorRow {
            symbol_id: row.get(0)?,
            name: row.get(1)?,
            arguments: row.get(2)?,
            target_line: row.get::<_, i64>(3)? as u32,
            decorator_type: row.get(4)?,
            updated_at: 0,
        });
    }
    Ok(out)
}

/// Delete all decorators for symbols in a specific file.
pub fn delete_decorators_by_file(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM decorators WHERE symbol_id IN (SELECT id FROM symbols WHERE file_path = ?1)",
        params![file_path],
    )
    .context("Failed to delete decorators for file")?;
    Ok(())
}

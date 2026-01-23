//! CRUD operations for query_selections table

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::storage::sqlite::schema::QuerySelectionRow;

pub fn insert_query_selection(
    conn: &Connection,
    query_text: &str,
    query_normalized: &str,
    selected_symbol_id: &str,
    position: u32,
) -> Result<i64> {
    conn.execute(
        r#"
INSERT INTO query_selections (query_text, query_normalized, selected_symbol_id, position, created_at)
VALUES (?1, ?2, ?3, ?4, unixepoch())
"#,
        params![query_text, query_normalized, selected_symbol_id, position as i64],
    )
    .context("Failed to insert query selection")?;
    Ok(conn.last_insert_rowid())
}

pub fn get_selections_for_query(
    conn: &Connection,
    query_normalized: &str,
    limit: usize,
) -> Result<Vec<QuerySelectionRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT id, query_text, query_normalized, selected_symbol_id, position, created_at
FROM query_selections
WHERE query_normalized = ?1
ORDER BY created_at DESC
LIMIT ?2
"#,
        )
        .context("Failed to prepare get_selections_for_query")?;

    let mut rows = stmt.query(params![query_normalized, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(QuerySelectionRow {
            id: row.get(0)?,
            query_text: row.get(1)?,
            query_normalized: row.get(2)?,
            selected_symbol_id: row.get(3)?,
            position: row.get::<_, i64>(4)? as u32,
            created_at: row.get(5)?,
        });
    }
    Ok(out)
}

pub fn get_symbol_selection_count(conn: &Connection, symbol_id: &str) -> Result<u64> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM query_selections WHERE selected_symbol_id = ?1",
            params![symbol_id],
            |row| row.get(0),
        )
        .context("Failed to count symbol selections")?;
    Ok(count.max(0) as u64)
}

pub fn get_recent_selections(conn: &Connection, limit: usize) -> Result<Vec<QuerySelectionRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT id, query_text, query_normalized, selected_symbol_id, position, created_at
FROM query_selections
ORDER BY created_at DESC
LIMIT ?1
"#,
        )
        .context("Failed to prepare get_recent_selections")?;

    let mut rows = stmt.query(params![limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(QuerySelectionRow {
            id: row.get(0)?,
            query_text: row.get(1)?,
            query_normalized: row.get(2)?,
            selected_symbol_id: row.get(3)?,
            position: row.get::<_, i64>(4)? as u32,
            created_at: row.get(5)?,
        });
    }
    Ok(out)
}

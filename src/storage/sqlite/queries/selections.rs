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

/// Batch query for selection boost scores
///
/// Returns a HashMap with keys "query_normalized|symbol_id" mapping to boost scores.
/// Boost score = position_discount * time_decay where:
/// - position_discount = 1.0 / ln(position + 2.0) for position bias correction
/// - time_decay = exp(-0.1 * age_in_days) with lambda=0.1
///
/// Multiple selections per (query, symbol) pair are aggregated by summing boosts.
pub fn batch_get_selection_boosts(
    conn: &Connection,
    pairs: &[(String, String)],
) -> Result<std::collections::HashMap<String, f32>> {
    use std::collections::HashMap;

    if pairs.is_empty() {
        return Ok(HashMap::new());
    }

    let mut result = HashMap::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as f64;

    // Query per pair — simpler and avoids CTE/math function issues with SQLite.
    // Pair count is bounded by search limit (typically 5-40), so this is fine.
    let mut stmt = conn.prepare(
        r#"
        SELECT position, created_at
        FROM query_selections
        WHERE query_normalized = ?1 AND selected_symbol_id = ?2
        "#,
    )?;

    for (query_normalized, symbol_id) in pairs {
        let rows: Vec<(i64, i64)> = stmt
            .query_map(params![query_normalized, symbol_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut boost = 0.0f64;
        for (position, created_at) in &rows {
            let age_days = (now - *created_at as f64) / 86400.0;
            let position_weight = 1.0 / (*position as f64 + 2.0).ln();
            let time_decay = (-0.1 * age_days).exp();
            boost += position_weight * time_decay;
        }

        if boost > 0.0 {
            let key = format!("{}|{}", query_normalized, symbol_id);
            result.insert(key, boost as f32);
        }
    }

    Ok(result)
}

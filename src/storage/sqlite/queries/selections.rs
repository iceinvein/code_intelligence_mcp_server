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

    // Build WHERE clause for batch query
    let placeholders: Vec<String> = (0..pairs.len())
        .map(|i| format!("(?{} AS q{}, ?{} AS s{})", i * 2 + 1, i, i * 2 + 2, i))
        .collect();

    // Use a CTE approach for efficient batch lookup
    let query = format!(
        r#"
        WITH input_pairs(query_normalized, symbol_id) AS (
            VALUES {}
        )
        SELECT
            i.query_normalized,
            i.symbol_id,
            SUM(1.0 / LN(qs.position + 2.0) * EXP(-0.1 * (unixepoch() - qs.created_at) / 86400.0)) as boost_score
        FROM input_pairs i
        LEFT JOIN query_selections qs
            ON qs.query_normalized = i.query_normalized
            AND qs.selected_symbol_id = i.symbol_id
        GROUP BY i.query_normalized, i.symbol_id
        "#,
        placeholders.join(", ")
    );

    let mut stmt = conn.prepare(&query)?;

    // Flatten pairs into params
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    for (query, symbol_id) in pairs {
        params.push(rusqlite::types::Value::Text(query.clone()));
        params.push(rusqlite::types::Value::Text(symbol_id.clone()));
    }

    let mut rows = stmt.query(rusqlite::params_from_iter(params))?;

    while let Some(row) = rows.next()? {
        let query_normalized: String = row.get(0)?;
        let symbol_id: String = row.get(1)?;
        let boost_score: f64 = row.get::<_, f64>(2).unwrap_or(0.0);
        let key = format!("{}|{}", query_normalized, symbol_id);
        result.insert(key, boost_score as f32);
    }

    Ok(result)
}

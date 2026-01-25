//! CRUD operations for symbol_metrics table

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::SymbolMetricsRow;

pub fn upsert_symbol_metrics(conn: &Connection, metrics: &SymbolMetricsRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO symbol_metrics (symbol_id, pagerank, in_degree, out_degree, updated_at)
VALUES (?1, ?2, ?3, ?4, unixepoch())
ON CONFLICT(symbol_id) DO UPDATE SET
  pagerank=excluded.pagerank,
  in_degree=excluded.in_degree,
  out_degree=excluded.out_degree,
  updated_at=unixepoch()
"#,
        params![
            metrics.symbol_id,
            metrics.pagerank,
            metrics.in_degree as i64,
            metrics.out_degree as i64,
        ],
    )
    .context("Failed to upsert symbol metrics")?;
    Ok(())
}

pub fn get_symbol_metrics(conn: &Connection, symbol_id: &str) -> Result<Option<SymbolMetricsRow>> {
    conn.query_row(
        r#"
SELECT symbol_id, pagerank, in_degree, out_degree, updated_at
FROM symbol_metrics
WHERE symbol_id = ?1
"#,
        params![symbol_id],
        |row| {
            Ok(SymbolMetricsRow {
                symbol_id: row.get(0)?,
                pagerank: row.get(1)?,
                in_degree: row.get::<_, i64>(2)? as u32,
                out_degree: row.get::<_, i64>(3)? as u32,
                updated_at: row.get(4)?,
            })
        },
    )
    .optional()
    .context("Failed to query symbol metrics")
}

pub fn get_top_symbols_by_pagerank(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<SymbolMetricsRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT symbol_id, pagerank, in_degree, out_degree, updated_at
FROM symbol_metrics
ORDER BY pagerank DESC
LIMIT ?1
"#,
        )
        .context("Failed to prepare get_top_symbols_by_pagerank")?;

    let mut rows = stmt.query(params![limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SymbolMetricsRow {
            symbol_id: row.get(0)?,
            pagerank: row.get(1)?,
            in_degree: row.get::<_, i64>(2)? as u32,
            out_degree: row.get::<_, i64>(3)? as u32,
            updated_at: row.get(4)?,
        });
    }
    Ok(out)
}

pub fn delete_symbol_metrics(conn: &Connection, symbol_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM symbol_metrics WHERE symbol_id = ?1",
        params![symbol_id],
    )
    .context("Failed to delete symbol metrics")?;
    Ok(())
}

pub fn batch_get_symbol_metrics(
    conn: &Connection,
    symbol_ids: &[String],
) -> Result<std::collections::HashMap<String, f64>> {
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
        "SELECT symbol_id, pagerank FROM symbol_metrics WHERE symbol_id IN ({})",
        placeholders
    );

    let mut stmt = conn
        .prepare(&query)
        .context("Failed to prepare batch_get_symbol_metrics")?;

    let params: Vec<&dyn rusqlite::ToSql> = symbol_ids
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql)
        .collect();

    let mut rows = stmt.query(params.as_slice())?;
    let mut out = std::collections::HashMap::new();
    while let Some(row) = rows.next()? {
        let symbol_id: String = row.get(0)?;
        let pagerank: f64 = row.get(1)?;
        out.insert(symbol_id, pagerank);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_get_symbol_metrics() {
        // Create an in-memory database
        let conn = Connection::open_in_memory().unwrap();

        // Create the symbol_metrics table
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS symbol_metrics (
              symbol_id TEXT PRIMARY KEY NOT NULL,
              pagerank REAL NOT NULL DEFAULT 0.0,
              in_degree INTEGER NOT NULL DEFAULT 0,
              out_degree INTEGER NOT NULL DEFAULT 0,
              updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            )
        "#,
            [],
        )
        .unwrap();

        // Insert test data
        conn.execute(
            r#"
            INSERT INTO symbol_metrics (symbol_id, pagerank, in_degree, out_degree)
            VALUES
                ('symbol1', 0.1, 10, 5),
                ('symbol2', 0.05, 5, 2),
                ('symbol3', 0.01, 1, 0)
        "#,
            [],
        )
        .unwrap();

        // Query for 2 out of 3 symbols
        let symbol_ids = vec!["symbol1".to_string(), "symbol2".to_string()];
        let result = batch_get_symbol_metrics(&conn, &symbol_ids).unwrap();

        // Verify results
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("symbol1"), Some(&0.1));
        assert_eq!(result.get("symbol2"), Some(&0.05));
        assert_eq!(result.get("symbol3"), None); // Not requested
    }

    #[test]
    fn test_batch_get_symbol_metrics_empty() {
        // Create an in-memory database
        let conn = Connection::open_in_memory().unwrap();

        // Create the symbol_metrics table
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS symbol_metrics (
              symbol_id TEXT PRIMARY KEY NOT NULL,
              pagerank REAL NOT NULL DEFAULT 0.0,
              in_degree INTEGER NOT NULL DEFAULT 0,
              out_degree INTEGER NOT NULL DEFAULT 0,
              updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            )
        "#,
            [],
        )
        .unwrap();

        // Query with empty input
        let symbol_ids: Vec<String> = vec![];
        let result = batch_get_symbol_metrics(&conn, &symbol_ids).unwrap();

        // Verify empty result
        assert!(result.is_empty());
    }

    #[test]
    fn test_batch_get_symbol_metrics_not_found() {
        // Create an in-memory database
        let conn = Connection::open_in_memory().unwrap();

        // Create the symbol_metrics table
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS symbol_metrics (
              symbol_id TEXT PRIMARY KEY NOT NULL,
              pagerank REAL NOT NULL DEFAULT 0.0,
              in_degree INTEGER NOT NULL DEFAULT 0,
              out_degree INTEGER NOT NULL DEFAULT 0,
              updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            )
        "#,
            [],
        )
        .unwrap();

        // Query for non-existent symbols
        let symbol_ids = vec!["nonexistent1".to_string(), "nonexistent2".to_string()];
        let result = batch_get_symbol_metrics(&conn, &symbol_ids).unwrap();

        // Verify empty result (symbols don't exist)
        assert!(result.is_empty());
    }
}

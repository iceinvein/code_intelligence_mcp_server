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

pub fn get_top_symbols_by_pagerank(conn: &Connection, limit: usize) -> Result<Vec<SymbolMetricsRow>> {
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

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::{SimilarityClusterRow, UsageExampleRow};

pub fn upsert_similarity_cluster(conn: &Connection, row: &SimilarityClusterRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO similarity_clusters(symbol_id, cluster_key, updated_at)
VALUES (?1, ?2, unixepoch())
ON CONFLICT(symbol_id) DO UPDATE SET
  cluster_key=excluded.cluster_key,
  updated_at=unixepoch()
"#,
        params![row.symbol_id, row.cluster_key],
    )
    .context("Failed to upsert similarity cluster")?;
    Ok(())
}

pub fn get_similarity_cluster_key(conn: &Connection, symbol_id: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT cluster_key FROM similarity_clusters WHERE symbol_id = ?1",
        params![symbol_id],
        |row| row.get(0),
    )
    .optional()
    .context("Failed to query similarity cluster key")
}

pub fn list_symbols_in_cluster(
    conn: &Connection,
    cluster_key: &str,
    limit: usize,
) -> Result<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT s.id, s.name
FROM similarity_clusters c
JOIN symbols s ON s.id = c.symbol_id
WHERE c.cluster_key = ?1
ORDER BY s.name ASC, s.file_path ASC, s.kind ASC, s.id ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_symbols_in_cluster")?;
    let mut rows = stmt.query(params![cluster_key, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push((row.get(0)?, row.get(1)?));
    }
    Ok(out)
}

pub fn delete_usage_examples_by_file(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM usage_examples WHERE file_path = ?1",
        params![file_path],
    )
    .with_context(|| format!("Failed to delete usage examples for file: {file_path}"))?;
    Ok(())
}

pub fn upsert_usage_example(conn: &Connection, example: &UsageExampleRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO usage_examples(
  to_symbol_id, from_symbol_id, example_type, file_path, line, snippet
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(to_symbol_id, example_type, file_path, line, snippet) DO NOTHING
"#,
        params![
            example.to_symbol_id,
            example.from_symbol_id,
            example.example_type,
            example.file_path,
            example.line.map(|v| v as i64),
            example.snippet
        ],
    )
    .context("Failed to upsert usage example")?;
    Ok(())
}

pub fn list_usage_examples_for_symbol(
    conn: &Connection,
    to_symbol_id: &str,
    limit: usize,
) -> Result<Vec<UsageExampleRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  to_symbol_id, from_symbol_id, example_type, file_path, line, snippet
FROM usage_examples
WHERE to_symbol_id = ?1
ORDER BY example_type ASC, file_path ASC, line ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_usage_examples_for_symbol")?;

    let mut rows = stmt.query(params![to_symbol_id, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(UsageExampleRow {
            to_symbol_id: row.get(0)?,
            from_symbol_id: row.get(1)?,
            example_type: row.get(2)?,
            file_path: row.get(3)?,
            line: row
                .get::<_, Option<i64>>(4)?
                .and_then(|v| u32::try_from(v).ok()),
            snippet: row.get(5)?,
        });
    }
    Ok(out)
}

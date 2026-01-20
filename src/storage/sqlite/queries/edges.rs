use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::storage::sqlite::schema::{EdgeEvidenceRow, EdgeRow};

pub fn upsert_edge(conn: &Connection, edge: &EdgeRow) -> Result<()> {
    let resolution_rank = edge_resolution_rank(edge.resolution.as_str());
    conn.execute(
        r#"
INSERT INTO edges(from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count, resolution, resolution_rank)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
ON CONFLICT(from_symbol_id, to_symbol_id, edge_type) DO UPDATE SET
  at_file=COALESCE(edges.at_file, excluded.at_file),
  at_line=COALESCE(edges.at_line, excluded.at_line),
  confidence=MAX(edges.confidence, excluded.confidence),
  evidence_count=MAX(edges.evidence_count, excluded.evidence_count),
  resolution_rank=MAX(edges.resolution_rank, excluded.resolution_rank),
  resolution=CASE
    WHEN excluded.resolution_rank > edges.resolution_rank THEN excluded.resolution
    ELSE edges.resolution
  END
"#,
        params![
            edge.from_symbol_id,
            edge.to_symbol_id,
            edge.edge_type,
            edge.at_file,
            edge.at_line.map(|v| v as i64),
            edge.confidence,
            edge.evidence_count as i64,
            edge.resolution,
            resolution_rank
        ],
    )
    .context("Failed to upsert edge")?;
    Ok(())
}

pub fn upsert_edge_evidence(conn: &Connection, evidence: &EdgeEvidenceRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO edge_evidence(from_symbol_id, to_symbol_id, edge_type, at_file, at_line, count)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(from_symbol_id, to_symbol_id, edge_type, at_file, at_line) DO UPDATE SET
  count=MAX(edge_evidence.count, excluded.count)
"#,
        params![
            evidence.from_symbol_id,
            evidence.to_symbol_id,
            evidence.edge_type,
            evidence.at_file,
            evidence.at_line as i64,
            evidence.count as i64
        ],
    )
    .context("Failed to upsert edge evidence")?;
    Ok(())
}

pub fn list_edge_evidence(
    conn: &Connection,
    from_symbol_id: &str,
    to_symbol_id: &str,
    edge_type: &str,
    limit: usize,
) -> Result<Vec<EdgeEvidenceRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  from_symbol_id, to_symbol_id, edge_type, at_file, at_line, count
FROM edge_evidence
WHERE from_symbol_id = ?1 AND to_symbol_id = ?2 AND edge_type = ?3
ORDER BY count DESC, at_file ASC, at_line ASC, id ASC
LIMIT ?4
"#,
        )
        .context("Failed to prepare list_edge_evidence")?;

    let mut rows = stmt.query(params![
        from_symbol_id,
        to_symbol_id,
        edge_type,
        limit as i64
    ])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(EdgeEvidenceRow {
            from_symbol_id: row.get(0)?,
            to_symbol_id: row.get(1)?,
            edge_type: row.get(2)?,
            at_file: row.get(3)?,
            at_line: u32::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
            count: u32::try_from(row.get::<_, i64>(5)?).unwrap_or(1),
        });
    }
    Ok(out)
}

pub fn list_edges_from(
    conn: &Connection,
    from_symbol_id: &str,
    limit: usize,
) -> Result<Vec<EdgeRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count, resolution
FROM edges
WHERE from_symbol_id = ?1
ORDER BY edge_type ASC, to_symbol_id ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_edges_from")?;

    let mut rows = stmt.query(params![from_symbol_id, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(EdgeRow {
            from_symbol_id: row.get(0)?,
            to_symbol_id: row.get(1)?,
            edge_type: row.get(2)?,
            at_file: row.get(3)?,
            at_line: row
                .get::<_, Option<i64>>(4)?
                .and_then(|v| u32::try_from(v).ok()),
            confidence: row.get::<_, f64>(5)? as f32,
            evidence_count: u32::try_from(row.get::<_, i64>(6)?).unwrap_or(1),
            resolution: row.get(7)?,
        });
    }
    Ok(out)
}

pub fn list_edges_to(conn: &Connection, to_symbol_id: &str, limit: usize) -> Result<Vec<EdgeRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count, resolution
FROM edges
WHERE to_symbol_id = ?1
ORDER BY edge_type ASC, from_symbol_id ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_edges_to")?;

    let mut rows = stmt.query(params![to_symbol_id, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(EdgeRow {
            from_symbol_id: row.get(0)?,
            to_symbol_id: row.get(1)?,
            edge_type: row.get(2)?,
            at_file: row.get(3)?,
            at_line: row
                .get::<_, Option<i64>>(4)?
                .and_then(|v| u32::try_from(v).ok()),
            confidence: row.get::<_, f64>(5)? as f32,
            evidence_count: u32::try_from(row.get::<_, i64>(6)?).unwrap_or(1),
            resolution: row.get(7)?,
        });
    }
    Ok(out)
}

pub fn count_incoming_edges(conn: &Connection, to_symbol_id: &str) -> Result<u64> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE to_symbol_id = ?1",
            params![to_symbol_id],
            |row| row.get(0),
        )
        .context("Failed to count incoming edges")?;
    Ok(count.max(0) as u64)
}

pub fn count_edges(conn: &Connection) -> Result<u64> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))
        .context("Failed to count edges")?;
    Ok(count.max(0) as u64)
}

fn edge_resolution_rank(resolution: &str) -> i64 {
    match resolution {
        "local" => 3,
        "import" => 2,
        "heuristic" => 1,
        _ => 0,
    }
}

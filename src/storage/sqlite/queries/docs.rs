use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::DocMetaRow;

/// Upsert documentation metadata for one file. Called from the indexing
/// write phase for markdown files only.
pub fn upsert_doc_meta(conn: &Connection, row: &DocMetaRow) -> Result<()> {
    let labels_json = serde_json::to_string(&row.labels).context("serialize doc labels")?;
    conn.execute(
        r#"
INSERT INTO doc_metadata(file_path, doc_type, status, date, number, labels_json)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(file_path) DO UPDATE SET
  doc_type=excluded.doc_type,
  status=excluded.status,
  date=excluded.date,
  number=excluded.number,
  labels_json=excluded.labels_json,
  updated_at=unixepoch()
"#,
        params![
            row.file_path,
            row.doc_type,
            row.status,
            row.date,
            row.number,
            labels_json
        ],
    )
    .with_context(|| format!("Failed to upsert doc metadata for {}", row.file_path))?;
    Ok(())
}

pub fn delete_doc_meta_by_file(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM doc_metadata WHERE file_path = ?1",
        params![file_path],
    )
    .with_context(|| format!("Failed to delete doc metadata for {file_path}"))?;
    Ok(())
}

/// Batch lookup of doc metadata by file paths (query-time ranking support).
pub fn get_doc_meta_for_paths(
    conn: &Connection,
    file_paths: &[String],
) -> Result<std::collections::HashMap<String, DocMetaRow>> {
    let mut map = std::collections::HashMap::new();
    for path in file_paths {
        let result = conn
            .query_row(
                "SELECT file_path, doc_type, status, date, number, labels_json \
                 FROM doc_metadata WHERE file_path = ?1",
                params![path],
                |row| {
                    let labels_json: String = row.get(5)?;
                    Ok(DocMetaRow {
                        file_path: row.get(0)?,
                        doc_type: row.get(1)?,
                        status: row.get(2)?,
                        date: row.get(3)?,
                        number: row.get(4)?,
                        labels: serde_json::from_str(&labels_json).unwrap_or_default(),
                    })
                },
            )
            .optional()
            .with_context(|| format!("Failed to query doc metadata for {path}"))?;
        if let Some(row) = result {
            map.insert(path.clone(), row);
        }
    }
    Ok(map)
}

/// Aggregate doc counts by type plus superseded/deprecated status count,
/// used by the repo-map docs summary.
pub fn doc_metadata_summary(
    conn: &Connection,
) -> Result<crate::storage::sqlite::schema::DocSummary> {
    let mut summary = crate::storage::sqlite::schema::DocSummary::default();
    let mut stmt = conn
        .prepare("SELECT doc_type, COALESCE(status, '') FROM doc_metadata")
        .context("Failed to prepare doc metadata summary query")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("Failed to query doc metadata summary")?;
    for row in rows {
        let (doc_type, status) = row.context("Failed to read doc metadata summary row")?;
        summary.total += 1;
        *summary.by_type.entry(doc_type).or_insert(0) += 1;
        if status == "superseded" || status == "deprecated" {
            summary.superseded += 1;
        }
    }
    // Stale links are recomputed live (docs-indexing design, Phase 3) so
    // delta index runs never leave a stale count behind.
    let stale = crate::indexer::pipeline::doc_links::count_stale_doc_links(conn)?;
    summary.stale_links = stale.total;
    Ok(summary)
}

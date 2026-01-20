use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::{IndexRunRow, SearchRunRow};

pub fn insert_index_run(conn: &Connection, run: &IndexRunRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO index_runs(
  started_at, duration_ms, files_scanned, files_indexed, files_skipped, files_unchanged,
  files_deleted, symbols_indexed
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
"#,
        params![
            run.started_at_unix_s,
            run.duration_ms as i64,
            run.files_scanned as i64,
            run.files_indexed as i64,
            run.files_skipped as i64,
            run.files_unchanged as i64,
            run.files_deleted as i64,
            run.symbols_indexed as i64
        ],
    )
    .context("Failed to insert index run")?;
    Ok(())
}

pub fn insert_search_run(conn: &Connection, run: &SearchRunRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO search_runs(
  started_at, duration_ms, keyword_ms, vector_ms, merge_ms, query, query_limit, exported_only, result_count
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
"#,
        params![
            run.started_at_unix_s,
            run.duration_ms as i64,
            run.keyword_ms as i64,
            run.vector_ms as i64,
            run.merge_ms as i64,
            run.query,
            run.query_limit as i64,
            if run.exported_only { 1 } else { 0 },
            run.result_count as i64
        ],
    )
    .context("Failed to insert search run")?;
    Ok(())
}

pub fn latest_index_run(conn: &Connection) -> Result<Option<IndexRunRow>> {
    conn.query_row(
        r#"
SELECT
  started_at, duration_ms, files_scanned, files_indexed, files_skipped, files_unchanged,
  files_deleted, symbols_indexed
FROM index_runs
ORDER BY started_at DESC, id DESC
LIMIT 1
"#,
        [],
        |row| {
            Ok(IndexRunRow {
                started_at_unix_s: row.get(0)?,
                duration_ms: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                files_scanned: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                files_indexed: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                files_skipped: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                files_unchanged: u64::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
                files_deleted: u64::try_from(row.get::<_, i64>(6)?).unwrap_or(0),
                symbols_indexed: u64::try_from(row.get::<_, i64>(7)?).unwrap_or(0),
            })
        },
    )
    .optional()
    .context("Failed to query latest index run")
}

pub fn latest_search_run(conn: &Connection) -> Result<Option<SearchRunRow>> {
    conn.query_row(
        r#"
SELECT
  started_at, duration_ms, keyword_ms, vector_ms, merge_ms, query, query_limit, exported_only, result_count
FROM search_runs
ORDER BY started_at DESC, id DESC
LIMIT 1
"#,
        [],
        |row| {
            let exported_only: i64 = row.get(7)?;
            Ok(SearchRunRow {
                started_at_unix_s: row.get(0)?,
                duration_ms: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                keyword_ms: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                vector_ms: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                merge_ms: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                query: row.get(5)?,
                query_limit: u64::try_from(row.get::<_, i64>(6)?).unwrap_or(0),
                exported_only: exported_only != 0,
                result_count: u64::try_from(row.get::<_, i64>(8)?).unwrap_or(0),
            })
        },
    )
    .optional()
    .context("Failed to query latest search run")
}

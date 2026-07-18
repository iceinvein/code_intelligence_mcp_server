use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::{IndexRunRow, SearchRunRow};

pub fn insert_index_run(conn: &Connection, run: &IndexRunRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO index_runs(
  started_at, duration_ms, files_scanned, files_indexed, files_skipped, files_unchanged,
  files_deleted, symbols_indexed, scan_ms, cleanup_ms, parse_ms, sqlite_write_ms, tantivy_ms,
  binding_ms, edge_ms, embedding_ms, vector_write_ms, pagerank_ms, optimize_ms
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
"#,
        params![
            run.started_at_unix_s,
            run.duration_ms as i64,
            run.files_scanned as i64,
            run.files_indexed as i64,
            run.files_skipped as i64,
            run.files_unchanged as i64,
            run.files_deleted as i64,
            run.symbols_indexed as i64,
            run.scan_ms as i64,
            run.cleanup_ms as i64,
            run.parse_ms as i64,
            run.sqlite_write_ms as i64,
            run.tantivy_ms as i64,
            run.binding_ms as i64,
            run.edge_ms as i64,
            run.embedding_ms as i64,
            run.vector_write_ms as i64,
            run.pagerank_ms as i64,
            run.optimize_ms as i64,
        ],
    )
    .context("Failed to insert index run")?;
    Ok(())
}

pub fn insert_search_run(conn: &Connection, run: &SearchRunRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO search_runs(
  started_at, duration_ms, keyword_ms, vector_ms, merge_ms, query, query_limit, exported_only, result_count,
  embedding_ms, reranker_ms, scoring_ms, assembly_ms, fusion_ms, search_path, cache_status,
  subquery_count, keyword_candidates, vector_candidates, fused_candidates
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
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
            run.result_count as i64,
            run.embedding_ms as i64,
            run.reranker_ms as i64,
            run.scoring_ms as i64,
            run.assembly_ms as i64,
            run.fusion_ms as i64,
            run.search_path,
            run.cache_status,
            run.subquery_count as i64,
            run.keyword_candidates as i64,
            run.vector_candidates as i64,
            run.fused_candidates as i64,
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
  files_deleted, symbols_indexed, scan_ms, cleanup_ms, parse_ms, sqlite_write_ms, tantivy_ms,
  binding_ms, edge_ms, embedding_ms, vector_write_ms, pagerank_ms, optimize_ms
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
                scan_ms: u64::try_from(row.get::<_, i64>(8)?).unwrap_or(0),
                cleanup_ms: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
                parse_ms: u64::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
                sqlite_write_ms: u64::try_from(row.get::<_, i64>(11)?).unwrap_or(0),
                tantivy_ms: u64::try_from(row.get::<_, i64>(12)?).unwrap_or(0),
                binding_ms: u64::try_from(row.get::<_, i64>(13)?).unwrap_or(0),
                edge_ms: u64::try_from(row.get::<_, i64>(14)?).unwrap_or(0),
                embedding_ms: u64::try_from(row.get::<_, i64>(15)?).unwrap_or(0),
                vector_write_ms: u64::try_from(row.get::<_, i64>(16)?).unwrap_or(0),
                pagerank_ms: u64::try_from(row.get::<_, i64>(17)?).unwrap_or(0),
                optimize_ms: u64::try_from(row.get::<_, i64>(18)?).unwrap_or(0),
            })
        },
    )
    .optional()
    .context("Failed to query latest index run")
}

pub fn latest_index_run_version(conn: &Connection) -> Result<Option<String>> {
    conn.query_row(
        r#"
SELECT started_at, id
FROM index_runs
ORDER BY started_at DESC, id DESC
LIMIT 1
"#,
        [],
        |row| {
            let started_at: i64 = row.get(0)?;
            let id: i64 = row.get(1)?;
            Ok(format!("{started_at}:{id}"))
        },
    )
    .optional()
    .context("Failed to query latest index run version")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::schema::SCHEMA_SQL;

    fn run(started_at_unix_s: i64) -> IndexRunRow {
        IndexRunRow {
            started_at_unix_s,
            duration_ms: 1,
            files_scanned: 1,
            files_indexed: 1,
            files_skipped: 0,
            files_unchanged: 0,
            files_deleted: 0,
            symbols_indexed: 1,
            scan_ms: 2,
            cleanup_ms: 3,
            parse_ms: 4,
            sqlite_write_ms: 5,
            tantivy_ms: 6,
            binding_ms: 7,
            edge_ms: 8,
            embedding_ms: 9,
            vector_write_ms: 10,
            pagerank_ms: 11,
            optimize_ms: 12,
        }
    }

    #[test]
    fn latest_index_run_version_distinguishes_same_second_runs() {
        let conn = Connection::open_in_memory().expect("memory sqlite");
        conn.execute_batch(SCHEMA_SQL).expect("schema");
        insert_index_run(&conn, &run(123)).expect("first run");
        let first = latest_index_run_version(&conn)
            .expect("first version")
            .expect("first version present");

        insert_index_run(&conn, &run(123)).expect("second run");
        let second = latest_index_run_version(&conn)
            .expect("second version")
            .expect("second version present");

        assert_ne!(first, second);
        assert!(first.starts_with("123:"));
        assert!(second.starts_with("123:"));
    }

    #[test]
    fn stage_telemetry_round_trips_for_index_and_search_runs() {
        let conn = Connection::open_in_memory().expect("memory sqlite");
        conn.execute_batch(SCHEMA_SQL).expect("schema");

        let index = run(456);
        insert_index_run(&conn, &index).expect("insert index run");
        assert_eq!(latest_index_run(&conn).unwrap(), Some(index));

        let search = SearchRunRow {
            started_at_unix_s: 789,
            duration_ms: 20,
            keyword_ms: 2,
            vector_ms: 3,
            merge_ms: 4,
            query: "sha256:abc:len=1".to_string(),
            query_limit: 10,
            exported_only: false,
            result_count: 6,
            embedding_ms: 1,
            reranker_ms: 0,
            scoring_ms: 5,
            assembly_ms: 6,
            fusion_ms: 7,
            search_path: "multi".to_string(),
            cache_status: "miss".to_string(),
            subquery_count: 3,
            keyword_candidates: 80,
            vector_candidates: 40,
            fused_candidates: 90,
        };
        insert_search_run(&conn, &search).expect("insert search run");
        assert_eq!(latest_search_run(&conn).unwrap(), Some(search));
    }
}

pub fn latest_search_run(conn: &Connection) -> Result<Option<SearchRunRow>> {
    conn.query_row(
        r#"
SELECT
  started_at, duration_ms, keyword_ms, vector_ms, merge_ms, query, query_limit, exported_only, result_count,
  embedding_ms, reranker_ms, scoring_ms, assembly_ms, fusion_ms, search_path, cache_status,
  subquery_count, keyword_candidates, vector_candidates, fused_candidates
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
                embedding_ms: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
                reranker_ms: u64::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
                scoring_ms: u64::try_from(row.get::<_, i64>(11)?).unwrap_or(0),
                assembly_ms: u64::try_from(row.get::<_, i64>(12)?).unwrap_or(0),
                fusion_ms: u64::try_from(row.get::<_, i64>(13)?).unwrap_or(0),
                search_path: row.get(14)?,
                cache_status: row.get(15)?,
                subquery_count: u64::try_from(row.get::<_, i64>(16)?).unwrap_or(0),
                keyword_candidates: u64::try_from(row.get::<_, i64>(17)?).unwrap_or(0),
                vector_candidates: u64::try_from(row.get::<_, i64>(18)?).unwrap_or(0),
                fused_candidates: u64::try_from(row.get::<_, i64>(19)?).unwrap_or(0),
            })
        },
    )
    .optional()
    .context("Failed to query latest search run")
}

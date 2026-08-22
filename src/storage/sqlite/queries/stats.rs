use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::storage::sqlite::schema::{DailyUsageRow, IndexRunRow, SearchRunRow, UsageSummary};

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
  started_at, duration_ms, keyword_ms, vector_ms, merge_ms, query, query_text, query_limit,
  exported_only, result_count,
  embedding_ms, reranker_ms, scoring_ms, assembly_ms, fusion_ms, search_path, cache_status,
  subquery_count, keyword_candidates, vector_candidates, fused_candidates
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
"#,
        params![
            run.started_at_unix_s,
            run.duration_ms as i64,
            run.keyword_ms as i64,
            run.vector_ms as i64,
            run.merge_ms as i64,
            run.query,
            run.query_text,
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

    fn search(started_at_unix_s: i64, duration_ms: u64, cache_status: &str) -> SearchRunRow {
        SearchRunRow {
            started_at_unix_s,
            duration_ms,
            keyword_ms: 1,
            vector_ms: 2,
            merge_ms: 1,
            query: "sha256:abc:len=1".to_string(),
            query_text: None,
            query_limit: 10,
            exported_only: false,
            result_count: 5,
            embedding_ms: 1,
            reranker_ms: 0,
            scoring_ms: 1,
            assembly_ms: 1,
            fusion_ms: 1,
            search_path: "single".to_string(),
            cache_status: cache_status.to_string(),
            subquery_count: 1,
            keyword_candidates: 40,
            vector_candidates: 40,
            fused_candidates: 59,
        }
    }

    #[test]
    fn usage_summary_counts_searches_cache_hits_and_index_runs() {
        let conn = Connection::open_in_memory().expect("memory sqlite");
        conn.execute_batch(SCHEMA_SQL).expect("schema");

        let empty = usage_summary(&conn).expect("empty summary");
        assert_eq!(empty.search_run_count, 0);
        assert_eq!(empty.cache_hit_count, 0);
        assert_eq!(empty.avg_duration_ms, 0);
        assert_eq!(empty.last_search_at_unix_s, None);
        assert_eq!(empty.index_run_count, 0);
        assert_eq!(empty.last_index_at_unix_s, None);

        insert_search_run(&conn, &search(1_000, 300, "hit")).expect("hit run");
        insert_search_run(&conn, &search(2_000, 101, "miss")).expect("miss run");
        insert_index_run(&conn, &run(1_500)).expect("index run");

        let summary = usage_summary(&conn).expect("summary");
        assert_eq!(summary.search_run_count, 2);
        assert_eq!(summary.cache_hit_count, 1);
        assert_eq!(summary.avg_duration_ms, 200);
        assert_eq!(summary.last_search_at_unix_s, Some(2_000));
        assert_eq!(summary.index_run_count, 1);
        assert_eq!(summary.last_index_at_unix_s, Some(1_500));
    }

    #[test]
    fn recent_search_runs_returns_newest_first_and_respects_limit() {
        let conn = Connection::open_in_memory().expect("memory sqlite");
        conn.execute_batch(SCHEMA_SQL).expect("schema");

        insert_search_run(&conn, &search(1_000, 10, "miss")).expect("first");
        insert_search_run(&conn, &search(2_000, 10, "miss")).expect("second");
        insert_search_run(&conn, &search(3_000, 10, "hit")).expect("third");

        let all = recent_search_runs(&conn, 10).expect("all runs");
        let times: Vec<i64> = all.iter().map(|r| r.started_at_unix_s).collect();
        assert_eq!(times, vec![3_000, 2_000, 1_000]);

        let two = recent_search_runs(&conn, 2).expect("limited runs");
        let times: Vec<i64> = two.iter().map(|r| r.started_at_unix_s).collect();
        assert_eq!(times, vec![3_000, 2_000]);
    }

    #[test]
    fn usage_daily_buckets_by_utc_day_inside_window() {
        use chrono::DateTime;

        // Fixed reference instant so bucket strings are deterministic.
        let now: i64 = 1_760_000_000; // 2025-10-09T09:33:20Z
        let day_of = |ts: i64| {
            DateTime::from_timestamp(ts, 0)
                .unwrap()
                .format("%Y-%m-%d")
                .to_string()
        };

        let conn = Connection::open_in_memory().expect("memory sqlite");
        conn.execute_batch(SCHEMA_SQL).expect("schema");

        insert_search_run(&conn, &search(now, 10, "miss")).expect("now a");
        insert_search_run(&conn, &search(now + 60, 10, "hit")).expect("now b");
        insert_search_run(&conn, &search(now - 3 * 86_400, 10, "miss")).expect("three days ago");
        insert_search_run(&conn, &search(now - 40 * 86_400, 10, "miss")).expect("outside window");

        let daily = usage_daily(&conn, 14, now).expect("daily buckets");
        assert_eq!(
            daily,
            vec![
                DailyUsageRow {
                    day: day_of(now - 3 * 86_400),
                    searches: 1,
                },
                DailyUsageRow {
                    day: day_of(now),
                    searches: 2,
                },
            ]
        );
    }

    #[test]
    fn stage_telemetry_round_trips_for_index_and_search_runs() {
        let conn = Connection::open_in_memory().expect("memory sqlite");
        conn.execute_batch(SCHEMA_SQL).expect("schema");

        let index = run(456);
        insert_index_run(&conn, &index).expect("insert index run");
        assert_eq!(latest_index_run(&conn).unwrap(), Some(index));

        let mut search = search(789, 20, "miss");
        search.query_limit = 10;
        search.search_path = "multi".to_string();
        insert_search_run(&conn, &search).expect("insert search run");
        assert_eq!(latest_search_run(&conn).unwrap(), Some(search.clone()));

        // Opt-in plaintext query text round-trips alongside the hash.
        search.started_at_unix_s = 790;
        search.query_text = Some("how does auth work?".to_string());
        insert_search_run(&conn, &search).expect("insert search run with text");
        assert_eq!(recent_search_runs(&conn, 1).unwrap(), vec![search]);
    }
}

const SEARCH_RUN_COLUMNS: &str = r#"
  started_at, duration_ms, keyword_ms, vector_ms, merge_ms, query, query_text, query_limit,
  exported_only, result_count,
  embedding_ms, reranker_ms, scoring_ms, assembly_ms, fusion_ms, search_path, cache_status,
  subquery_count, keyword_candidates, vector_candidates, fused_candidates
"#;

fn search_run_from_row(row: &Row<'_>) -> rusqlite::Result<SearchRunRow> {
    let exported_only: i64 = row.get(8)?;
    Ok(SearchRunRow {
        started_at_unix_s: row.get(0)?,
        duration_ms: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
        keyword_ms: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
        vector_ms: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
        merge_ms: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
        query: row.get(5)?,
        query_text: row.get(6)?,
        query_limit: u64::try_from(row.get::<_, i64>(7)?).unwrap_or(0),
        exported_only: exported_only != 0,
        result_count: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
        embedding_ms: u64::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
        reranker_ms: u64::try_from(row.get::<_, i64>(11)?).unwrap_or(0),
        scoring_ms: u64::try_from(row.get::<_, i64>(12)?).unwrap_or(0),
        assembly_ms: u64::try_from(row.get::<_, i64>(13)?).unwrap_or(0),
        fusion_ms: u64::try_from(row.get::<_, i64>(14)?).unwrap_or(0),
        search_path: row.get(15)?,
        cache_status: row.get(16)?,
        subquery_count: u64::try_from(row.get::<_, i64>(17)?).unwrap_or(0),
        keyword_candidates: u64::try_from(row.get::<_, i64>(18)?).unwrap_or(0),
        vector_candidates: u64::try_from(row.get::<_, i64>(19)?).unwrap_or(0),
        fused_candidates: u64::try_from(row.get::<_, i64>(20)?).unwrap_or(0),
    })
}

/// Whole-history usage counters for the dashboard's usage view.
pub fn usage_summary(conn: &Connection) -> Result<UsageSummary> {
    conn.query_row(
        r#"
SELECT
  (SELECT COUNT(*) FROM search_runs),
  (SELECT COUNT(*) FROM search_runs WHERE cache_status = 'hit'),
  (SELECT COALESCE(CAST(AVG(duration_ms) AS INTEGER), 0) FROM search_runs),
  (SELECT MAX(started_at) FROM search_runs),
  (SELECT COUNT(*) FROM index_runs),
  (SELECT MAX(started_at) FROM index_runs)
"#,
        [],
        |row| {
            Ok(UsageSummary {
                search_run_count: u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                cache_hit_count: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                avg_duration_ms: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                last_search_at_unix_s: row.get(3)?,
                index_run_count: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                last_index_at_unix_s: row.get(5)?,
            })
        },
    )
    .context("Failed to query usage summary")
}

/// Newest-first page of recorded search runs.
pub fn recent_search_runs(conn: &Connection, limit: u32) -> Result<Vec<SearchRunRow>> {
    let sql = format!(
        r#"
SELECT {SEARCH_RUN_COLUMNS}
FROM search_runs
ORDER BY started_at DESC, id DESC
LIMIT ?1
"#
    );
    let mut stmt = conn
        .prepare(&sql)
        .context("Failed to prepare recent search runs query")?;
    let rows = stmt
        .query_map(params![i64::from(limit)], search_run_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("Failed to query recent search runs")?;
    Ok(rows)
}

/// Search counts bucketed by UTC day over the trailing `days` calendar-day
/// window ending today. The cutoff is midnight UTC `days - 1` days before
/// `now_unix_s`, so the oldest bucket is a whole day and matches a client
/// that zero-fills `days` buckets ending at `now_unix_s`. Days with no
/// searches are absent.
pub fn usage_daily(conn: &Connection, days: u32, now_unix_s: i64) -> Result<Vec<DailyUsageRow>> {
    let window_start = now_unix_s.saturating_sub(i64::from(days.saturating_sub(1)) * 86_400);
    let cutoff = window_start - window_start.rem_euclid(86_400);
    let mut stmt = conn
        .prepare(
            r#"
SELECT date(started_at, 'unixepoch') AS day, COUNT(*) AS searches
FROM search_runs
WHERE started_at >= ?1
GROUP BY day
ORDER BY day ASC
"#,
        )
        .context("Failed to prepare usage daily query")?;
    let rows = stmt
        .query_map(params![cutoff], |row| {
            Ok(DailyUsageRow {
                day: row.get(0)?,
                searches: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("Failed to query usage daily buckets")?;
    Ok(rows)
}

pub fn latest_search_run(conn: &Connection) -> Result<Option<SearchRunRow>> {
    let sql = format!(
        r#"
SELECT {SEARCH_RUN_COLUMNS}
FROM search_runs
ORDER BY started_at DESC, id DESC
LIMIT 1
"#
    );
    conn.query_row(&sql, [], search_run_from_row)
        .optional()
        .context("Failed to query latest search run")
}

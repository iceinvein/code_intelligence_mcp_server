//! `GET /api/usage`: cross-repo search/index usage aggregates for the
//! dashboard's usage view.
//!
//! Reads the per-repo `search_runs`/`index_runs` telemetry that every search
//! and index already writes; no new telemetry is recorded here. Query text is
//! included only when a repo was indexed with `telemetry.store_query_text`
//! enabled (`query_text` is null otherwise).

use axum::{extract::State, response::Json};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::ApiError;
use super::ApiState;

/// Trailing window for per-day buckets shown in the dashboard.
const DAILY_WINDOW_DAYS: u32 = 14;
/// Per-repo page size before the cross-repo merge.
const RECENT_RUNS_PER_REPO: u32 = 25;
/// Cap on merged recent runs in the response.
const RECENT_RUNS_TOTAL_CAP: usize = 50;

pub(crate) async fn handle_usage(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Value>, ApiError> {
    let generated_at_unix_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let now_unix_s = i64::try_from(generated_at_unix_s).unwrap_or(0);

    let entries = state
        .session_manager
        .registry
        .list_all()
        .map_err(|e| ApiError::internal(format!("registry list_all failed: {e}")))?;

    let mut repos_out: Vec<Value> = Vec::new();
    let mut recent_runs: Vec<(i64, Value)> = Vec::new();
    let mut totals_searches: u64 = 0;
    let mut totals_cache_hits: u64 = 0;

    for e in entries {
        let id = crate::registry::RepoRegistry::path_hash(&e.path);
        let name = e.name.clone();
        let db_path = e.data_dir.join("code-intelligence.db");

        let (summary, daily, runs) = read_repo_usage(&db_path, now_unix_s);

        if let Some(s) = &summary {
            totals_searches += s.search_run_count;
            totals_cache_hits += s.cache_hit_count;
        }

        for r in runs {
            recent_runs.push((
                r.started_at_unix_s,
                json!({
                    "repo_id": id,
                    "repo_name": name,
                    "started_at_unix_s": r.started_at_unix_s,
                    "duration_ms": r.duration_ms,
                    "query_text": r.query_text,
                    "query_limit": r.query_limit,
                    "exported_only": r.exported_only,
                    "result_count": r.result_count,
                    "search_path": r.search_path,
                    "cache_status": r.cache_status,
                }),
            ));
        }

        repos_out.push(json!({
            "id": id,
            "name": name,
            "path": e.path,
            "search_total": summary.as_ref().map(|s| s.search_run_count).unwrap_or(0),
            "cache_hit_count": summary.as_ref().map(|s| s.cache_hit_count).unwrap_or(0),
            "avg_duration_ms": summary.as_ref().map(|s| s.avg_duration_ms).unwrap_or(0),
            "last_search_at_unix_s": summary.as_ref().and_then(|s| s.last_search_at_unix_s),
            "index_run_count": summary.as_ref().map(|s| s.index_run_count).unwrap_or(0),
            "last_index_at_unix_s": summary.as_ref().and_then(|s| s.last_index_at_unix_s),
            "daily": daily,
        }));
    }

    recent_runs.sort_by_key(|(started_at, _)| std::cmp::Reverse(*started_at));
    recent_runs.truncate(RECENT_RUNS_TOTAL_CAP);

    Ok(Json(json!({
        "generated_at_unix_s": generated_at_unix_s,
        "window_days": DAILY_WINDOW_DAYS,
        "totals": {
            "searches": totals_searches,
            "cache_hits": totals_cache_hits,
        },
        "repos": repos_out,
        "recent_runs": recent_runs.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
    })))
}

/// Open a repo db and collect its usage aggregates. Missing or unreadable
/// databases yield zeroed defaults so the repo still shows up in the view.
fn read_repo_usage(
    db_path: &crate::path::Utf8Path,
    now_unix_s: i64,
) -> (
    Option<crate::storage::sqlite::UsageSummary>,
    Vec<crate::storage::sqlite::DailyUsageRow>,
    Vec<crate::storage::sqlite::SearchRunRow>,
) {
    use crate::storage::sqlite::SqliteStore;

    if !db_path.as_std_path().exists() {
        return (None, Vec::new(), Vec::new());
    }
    let Ok(sqlite) = SqliteStore::open(db_path) else {
        tracing::warn!(path = %db_path, "failed to open repo db for usage");
        return (None, Vec::new(), Vec::new());
    };
    // A repo db is migrated when its owning session binds, but a repo
    // registered under an older daemon may still lack newer telemetry
    // columns; bring the schema current so reads cannot fail on them.
    if let Err(e) = sqlite.init() {
        tracing::warn!(path = %db_path, error = %e, "failed to migrate repo db for usage");
        return (None, Vec::new(), Vec::new());
    };
    (
        sqlite.usage_summary().ok(),
        sqlite
            .usage_daily(DAILY_WINDOW_DAYS, now_unix_s)
            .unwrap_or_default(),
        sqlite
            .recent_search_runs(RECENT_RUNS_PER_REPO)
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Utf8PathBuf;

    /// A repo db left behind by an older daemon lacks newer telemetry columns.
    /// The usage endpoint must migrate it on read instead of returning empty
    /// recent runs.
    #[tokio::test]
    async fn usage_reads_legacy_schema_repo_db_with_recent_runs() {
        let (_data, state) = crate::server::api::test_api_state().await;
        let repo = tempfile::tempdir().unwrap();
        let requested = Utf8PathBuf::from_path_buf(repo.path().to_path_buf()).unwrap();
        let path = crate::path::canonicalize_existing_dir(&requested).unwrap();
        state
            .session_manager
            .registry
            .register(path.as_str())
            .unwrap();

        let mut entries = state.session_manager.registry.list_all().unwrap();
        let entry = entries.remove(0);
        std::fs::create_dir_all(entry.data_dir.as_std_path()).unwrap();
        let db_path = entry.data_dir.join("code-intelligence.db");
        let conn = rusqlite::Connection::open(db_path.as_std_path()).unwrap();
        conn.execute_batch(
            r#"
CREATE TABLE search_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at INTEGER NOT NULL,
  duration_ms INTEGER NOT NULL,
  keyword_ms INTEGER NOT NULL,
  vector_ms INTEGER NOT NULL,
  merge_ms INTEGER NOT NULL,
  query TEXT NOT NULL,
  query_limit INTEGER NOT NULL,
  exported_only INTEGER NOT NULL,
  result_count INTEGER NOT NULL
);
INSERT INTO search_runs(started_at, duration_ms, keyword_ms, vector_ms, merge_ms, query, query_limit, exported_only, result_count)
VALUES (123, 50, 1, 2, 3, 'sha256:abc:len=4', 5, 0, 2);
"#,
        )
        .unwrap();
        drop(conn);

        let response = handle_usage(State(state)).await.unwrap();
        let value = response.0;
        assert_eq!(value["totals"]["searches"], 1);
        assert_eq!(value["repos"].as_array().unwrap()[0]["search_total"], 1);
        let runs = value["recent_runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["result_count"], 2);
        assert_eq!(runs[0]["query_text"], Value::Null);
    }
}

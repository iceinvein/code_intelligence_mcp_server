//! `/api/repos` endpoints: list, detail (+ stats), add, reindex, delete, and
//! the per-repo activity/stats helpers they rely on.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{validate_repo_path, ApiError, ApiState};
use crate::server::jobs::{self, JobRegistry};
use crate::storage::sqlite::schema::{IndexRunRow, SearchRunRow};

pub(crate) async fn handle_repos(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Value>, ApiError> {
    let entries = state
        .session_manager
        .registry
        .list_all()
        .map_err(|e| ApiError(format!("registry list_all failed: {e}")))?;
    let count = entries.len();
    let items: Vec<Value> = entries
        .into_iter()
        .map(|e| {
            let id = crate::registry::RepoRegistry::path_hash(&e.path);
            let persisted_activity = read_repo_persisted_activity(&e);
            let activity = build_repo_activity(&state.job_registry, &id, persisted_activity);
            json!({
                "id": id,
                "name": e.name,
                "path": e.path,
                "data_dir": e.data_dir,
                "created_at": e.created_at,
                "last_accessed": e.last_accessed,
                "activity": activity,
            })
        })
        .collect();
    Ok(Json(json!({ "count": count, "repos": items })))
}

/// Build the per-repo `activity` block surfaced by `/api/repos`.
///
/// Returns the most recent Running job (if any) plus the most recent
/// finished job, plus durable SQLite activity so the dashboard does not
/// report "never" after job TTL eviction or daemon restart.
fn build_repo_activity(
    job_registry: &JobRegistry,
    repo_id: &str,
    persisted: PersistedRepoActivity,
) -> Value {
    let running = jobs::most_recent_running_for_repo(job_registry, repo_id);
    let last_finished = jobs::most_recent_finished_for_repo(job_registry, repo_id);
    json!({
        "running": running.is_some(),
        "current": running,
        "last_finished": last_finished,
        "latest_index_run": persisted.latest_index_run,
        "latest_search_run": persisted.latest_search_run,
        "last_updated_unix_s": persisted.last_updated_unix_s,
    })
}

#[derive(Default)]
struct PersistedRepoActivity {
    latest_index_run: Option<IndexRunRow>,
    latest_search_run: Option<SearchRunRow>,
    last_updated_unix_s: Option<i64>,
}

fn read_repo_persisted_activity(entry: &crate::registry::RepoEntry) -> PersistedRepoActivity {
    let db_path = entry.data_dir.join("code-intelligence.db");
    if !db_path.as_std_path().exists() {
        return PersistedRepoActivity::default();
    }

    let Ok(sqlite) = crate::storage::sqlite::SqliteStore::open(&db_path) else {
        tracing::warn!(path = %db_path, "failed to open repo db for activity");
        return PersistedRepoActivity::default();
    };

    PersistedRepoActivity {
        latest_index_run: sqlite.latest_index_run().ok().flatten(),
        latest_search_run: sqlite.latest_search_run().ok().flatten(),
        last_updated_unix_s: sqlite.most_recent_symbol_update().ok().flatten(),
    }
}

pub(crate) async fn handle_repo_detail(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let entry = match state
        .session_manager
        .registry
        .get_by_hash(&id)
        .map_err(|e| ApiError(format!("registry lookup failed: {e}")))?
    {
        Some(e) => e,
        None => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("repo not found: {id}") })),
            )
                .into_response())
        }
    };

    // Stats: open the per-repo SQLite directly instead of going through
    // SessionManager::get_or_create_repo, which would warm a cold repo
    // just to render a dashboard tile. The db file may not exist yet on a
    // freshly registered repo that has never been indexed; surface that
    // as `stats: null` rather than an error. All SQLite work runs on a
    // blocking thread because rusqlite is synchronous: doing it on the
    // axum runtime thread starves every other dashboard request whenever
    // a multi-million-row repo (e.g. wolfmax) is in play.
    let db_path = entry.data_dir.join("code-intelligence.db");
    let stats = if db_path.as_std_path().exists() {
        tokio::task::spawn_blocking(move || read_repo_stats(&db_path))
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    Ok(Json(json!({
        "id": id,
        "name": entry.name,
        "path": entry.path,
        "data_dir": entry.data_dir,
        "created_at": entry.created_at,
        "last_accessed": entry.last_accessed,
        "stats": stats,
    }))
    .into_response())
}

/// Best-effort stats read. Runs on a blocking thread.
///
/// Heavy counts come from the `repo_stats` cache, refreshed at the end of
/// every index run. On cache miss (repo indexed before the cache existed,
/// or never indexed at all) we fall back to live counts AND backfill the
/// cache so the next dashboard click is fast. The live fallback only runs
/// once per repo, not on every page open.
///
/// Any individual query failure becomes `null` so the dashboard can render
/// partial data instead of a 500.
fn read_repo_stats(db_path: &crate::path::Utf8Path) -> Option<Value> {
    let sqlite = match crate::storage::sqlite::SqliteStore::open(db_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, path = %db_path, "failed to open repo db for stats");
            return None;
        }
    };

    // Ensure the schema (in particular the repo_stats table) exists. Legacy
    // repo dbs created before the cache table shipped won't have it, and
    // the indexing pipeline is the only other code path that runs init();
    // without this, the first dashboard click on a pre-existing repo logs
    // "no such table: repo_stats" instead of triggering the backfill.
    // init() is idempotent and microseconds on an already-initialised db.
    if let Err(e) = sqlite.init() {
        tracing::warn!(error = %e, path = %db_path, "failed to init repo db schema for stats");
        return None;
    }

    let cached = match sqlite.read_repo_stats_cached() {
        Ok(Some(snap)) => Some(snap),
        Ok(None) => {
            // Backfill: this scans the heavy tables once, then future
            // requests hit the cache. The fallback also covers fresh
            // index dbs (table exists but cache row never written) and
            // repos indexed before this cache table shipped.
            match sqlite.recompute_repo_stats() {
                Ok(snap) => Some(snap),
                Err(e) => {
                    tracing::warn!(error = %e, path = %db_path, "failed to backfill repo_stats cache");
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, path = %db_path, "failed to read repo_stats cache");
            None
        }
    };

    let (symbols, edges, descriptions, undescribed, last_updated) = match cached {
        Some(s) => (
            Some(s.symbols),
            Some(s.edges),
            Some(s.descriptions as usize),
            Some(s.undescribed_symbols as usize),
            s.last_updated_unix_s,
        ),
        None => (None, None, None, None, None),
    };

    let latest_index_run = sqlite.latest_index_run().ok().flatten();
    let latest_search_run = sqlite.latest_search_run().ok().flatten();
    let external = sqlite.external_overlay_stats().ok();
    let external_producers = crate::external_index::manifest::producer_availability()
        .unwrap_or_else(|err| {
            tracing::warn!(error = %err, "failed to read external producer availability");
            Vec::new()
        });

    Some(json!({
        "symbols": symbols,
        "edges": edges,
        "descriptions": descriptions,
        "undescribed_symbols": undescribed,
        "last_updated_unix_s": last_updated,
        "latest_index_run": latest_index_run,
        "latest_search_run": latest_search_run,
        "external_indexes": external.map(|external| {
            json!({
                "index_count": external.index_count,
                "symbol_count": external.symbol_count,
                "reference_count": external.reference_count,
                "mapped_symbol_count": external.mapped_symbol_count,
            })
        }),
        "external_producers": external_producers,
    }))
}

pub(crate) async fn handle_repo_reindex(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    // Look up the repo by its 16-char SHA256-prefix hash.
    let entry = match state
        .session_manager
        .registry
        .get_by_hash(&id)
        .map_err(|e| ApiError(format!("registry lookup failed: {e}")))?
    {
        Some(e) => e,
        None => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("repo not found: {id}") })),
            )
                .into_response())
        }
    };

    let path = crate::path::Utf8PathBuf::from(entry.path.clone());
    let sm = state.session_manager.clone();

    // Resolve (or create) the per-repo AppState, then spawn the indexer in
    // the background so the HTTP request returns immediately. A full
    // re-index can take minutes; the caller polls /api/jobs for status.
    let job_id = format!(
        "reindex-{}-{}",
        id,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    // Register the job up-front so /api/jobs shows it immediately.
    jobs::register_running(
        &state.job_registry,
        job_id.clone(),
        jobs::JobKind::ManualReindex,
        id.clone(),
        entry.path.clone(),
    );

    let job_id_log = job_id.clone();
    let registry = state.job_registry.clone();

    // Spawn the worker that runs the indexer and records its result.
    let worker_registry = registry.clone();
    let worker_job_id = job_id_log.clone();
    let worker_path = path.clone();
    let task = tokio::spawn(async move {
        let app_state = match sm.get_or_create_repo(&worker_path).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, path = %worker_path, "reindex: failed to load repo");
                jobs::mark_failed(&worker_registry, &worker_job_id, format!("load repo: {e}"));
                return;
            }
        };
        let tool = crate::tools::RefreshIndexTool { files: None };
        match crate::handlers::handle_refresh_index(&app_state, tool).await {
            Ok(stats) => {
                tracing::info!(job = %worker_job_id, path = %worker_path, stats = %stats, "reindex completed");
                jobs::mark_succeeded(&worker_registry, &worker_job_id, stats);
            }
            Err(e) => {
                tracing::error!(job = %worker_job_id, error = %e, path = %worker_path, "reindex failed");
                jobs::mark_failed(&worker_registry, &worker_job_id, e.to_string());
            }
        }
    });

    // Watchdog: if the worker task panics or is cancelled, the worker's
    // own `mark_failed` never fires and the job would stay at Running
    // forever. Awaiting the JoinHandle gives us the only signal we have
    // about an aborted task, and `mark_failed_if_running` is a no-op when
    // the worker already recorded its own outcome.
    tokio::spawn(async move {
        if let Err(join_err) = task.await {
            let reason = if join_err.is_panic() {
                format!("reindex task panicked: {join_err}")
            } else if join_err.is_cancelled() {
                "reindex task cancelled before completion".to_string()
            } else {
                format!("reindex task aborted: {join_err}")
            };
            tracing::error!(job = %job_id_log, reason = %reason, "reindex watchdog");
            jobs::mark_failed_if_running(&registry, &job_id_log, reason);
        }
    });

    let body = Json(json!({
        "status": "started",
        "job_id": job_id,
        "repo_id": id,
        "repo_path": entry.path,
    }));
    Ok((StatusCode::ACCEPTED, body).into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct AddRepoRequest {
    path: String,
}

/// `POST /api/repos` -> register a repo explicitly (consent = Approved).
pub(crate) async fn handle_repo_add(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<AddRepoRequest>,
) -> Result<Response, ApiError> {
    let repo_path = match validate_repo_path(&req.path) {
        Ok(p) => p,
        Err(msg) => {
            return Ok((StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response())
        }
    };
    let entry = state
        .session_manager
        .registry
        .register(repo_path.as_str())
        .map_err(|e| ApiError(format!("register failed: {e}")))?;
    let id = crate::registry::RepoRegistry::path_hash(&entry.path);
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "name": entry.name,
            "path": entry.path,
            "data_dir": entry.data_dir,
            "created_at": entry.created_at,
            "last_accessed": entry.last_accessed,
        })),
    )
        .into_response())
}

pub(crate) async fn handle_repo_delete(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    match state
        .session_manager
        .delete_repo_by_hash(&id)
        .await
        .map_err(|e| ApiError(format!("delete failed: {e}")))?
    {
        Some(entry) => {
            let body = Json(json!({
                "status": "deleted",
                "repo_id": id,
                "repo_path": entry.path,
                "data_dir": entry.data_dir,
            }));
            Ok((StatusCode::OK, body).into_response())
        }
        None => Ok((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("repo not found: {id}") })),
        )
            .into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Utf8PathBuf;
    use crate::registry::RepoEntry;
    use crate::storage::sqlite::SqliteStore;
    use tempfile::tempdir;

    #[test]
    fn read_repo_persisted_activity_reads_latest_index_run() {
        let tmp = tempdir().expect("tempdir");
        let data_dir = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("utf8 path");
        let db_path = data_dir.join("code-intelligence.db");
        let sqlite = SqliteStore::open(&db_path).expect("open sqlite");
        sqlite.init().expect("init sqlite");
        sqlite
            .insert_index_run(&IndexRunRow {
                started_at_unix_s: 123,
                duration_ms: 45,
                files_scanned: 10,
                files_indexed: 2,
                files_skipped: 0,
                files_unchanged: 8,
                files_deleted: 0,
                symbols_indexed: 9,
                scan_ms: 0,
                cleanup_ms: 0,
                parse_ms: 0,
                sqlite_write_ms: 0,
                tantivy_ms: 0,
                binding_ms: 0,
                edge_ms: 0,
                embedding_ms: 0,
                vector_write_ms: 0,
                pagerank_ms: 0,
                optimize_ms: 0,
            })
            .expect("insert index run");

        let entry = RepoEntry {
            path: "/repo".to_string(),
            name: "repo".to_string(),
            data_dir,
            created_at: "2026-05-25T00:00:00Z".to_string(),
            last_accessed: "2026-05-25T00:00:00Z".to_string(),
            consent: crate::registry::IndexConsent::Approved,
        };

        let activity = read_repo_persisted_activity(&entry);

        assert_eq!(activity.latest_index_run.unwrap().started_at_unix_s, 123);
    }

    #[test]
    fn build_repo_activity_includes_persisted_index_run_when_jobs_are_empty() {
        let registry = jobs::new_job_registry();
        let activity = build_repo_activity(
            &registry,
            "repo-id",
            PersistedRepoActivity {
                latest_index_run: Some(IndexRunRow {
                    started_at_unix_s: 456,
                    duration_ms: 20,
                    files_scanned: 3,
                    files_indexed: 1,
                    files_skipped: 0,
                    files_unchanged: 2,
                    files_deleted: 0,
                    symbols_indexed: 4,
                    scan_ms: 0,
                    cleanup_ms: 0,
                    parse_ms: 0,
                    sqlite_write_ms: 0,
                    tantivy_ms: 0,
                    binding_ms: 0,
                    edge_ms: 0,
                    embedding_ms: 0,
                    vector_write_ms: 0,
                    pagerank_ms: 0,
                    optimize_ms: 0,
                }),
                latest_search_run: None,
                last_updated_unix_s: None,
            },
        );

        assert_eq!(activity["running"], false);
        assert_eq!(activity["latest_index_run"]["started_at_unix_s"], 456);
    }

    #[test]
    fn read_repo_stats_includes_external_overlay_counts() {
        let tmp = tempdir().expect("tempdir");
        let data_dir = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("utf8 path");
        let db_path = data_dir.join("code-intelligence.db");
        let sqlite = SqliteStore::open(&db_path).expect("open sqlite");
        sqlite.init().expect("init sqlite");
        sqlite
            .upsert_external_index(
                &crate::storage::sqlite::queries::external::ExternalIndexInsert {
                    id: "external:test",
                    source_kind: "normalized_json",
                    producer: "test",
                    language: "rust",
                    root_path: "/repo",
                    artifact_path: "/repo/external.json",
                    artifact_hash: "hash",
                    status: "ready",
                    diagnostics_json: "{}",
                },
            )
            .expect("upsert external index");

        let stats = read_repo_stats(&db_path).expect("stats");

        assert_eq!(stats["external_indexes"]["index_count"], 1);
        assert_eq!(stats["external_indexes"]["symbol_count"], 0);
        assert_eq!(stats["external_indexes"]["reference_count"], 0);
        assert_eq!(stats["external_indexes"]["mapped_symbol_count"], 0);
        let rust = stats["external_producers"]
            .as_array()
            .expect("external producers")
            .iter()
            .find(|producer| producer["id"] == "rust")
            .expect("rust producer");
        assert_eq!(rust["availability"], "missing");
        assert_eq!(rust["readiness"], "integrated");
        let java = stats["external_producers"]
            .as_array()
            .expect("external producers")
            .iter()
            .find(|producer| producer["id"] == "java")
            .expect("java producer");
        assert_eq!(java["availability"], "adapter_only");
        assert_eq!(java["readiness"], "adapter_only");
    }
}

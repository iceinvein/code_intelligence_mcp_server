//! JSON API endpoint for the daemon.
//!
//! Runs on a separate port from the MCP endpoint (default: `mcp_port + 2`) and
//! exposes read-only inspection routes that the lifecycle subcommands and the
//! future web UI consume.
//!
//! Routes:
//! - `GET /api/version`                  -> `{ version, started_at_unix_s, uptime_s }`
//! - `GET /api/status`                   -> daemon overview
//! - `GET /api/repos`                    -> registered repos
//! - `GET /api/sessions`                 -> bound MCP sessions
//! - `GET /api/repos/{id}`               -> repo metadata + per-repo stats
//! - `POST /api/repos/{id}/reindex`      -> spawn a background re-index
//! - `DELETE /api/repos/{id}`            -> drop the index, registry entry, and data dir
//! - `GET /api/jobs`                     -> recent background jobs (running + last 15m finished)
//!
//! All routes bind 127.0.0.1 only and reject requests whose `Origin` header is
//! not `http://localhost:<port>` or `http://127.0.0.1:<port>`. The check is a
//! DNS-rebinding defence: a malicious web page that resolves `example.com` to
//! 127.0.0.1 cannot read the daemon's repo list because its `Origin` would
//! still be `https://example.com`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json, Response,
    },
    routing::{get, post},
    Router,
};
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::log_broadcast::LogBroadcaster;
use crate::server::jobs::{self, JobRegistry};
use crate::server::standalone::SessionRepos;
use crate::session::SessionManager;
use crate::storage::sqlite::schema::{IndexRunRow, SearchRunRow};

#[derive(Clone)]
struct ApiState {
    session_manager: Arc<SessionManager>,
    session_repos: SessionRepos,
    log_broadcaster: LogBroadcaster,
    job_registry: JobRegistry,
    started_at_unix_s: u64,
}

/// Spawn the API server on `api_port`, returning once it is bound.
/// Errors are surfaced only if the bind itself fails.
pub async fn spawn_api_server(
    host: &str,
    api_port: u16,
    session_manager: Arc<SessionManager>,
    session_repos: SessionRepos,
    log_broadcaster: LogBroadcaster,
    job_registry: JobRegistry,
) -> anyhow::Result<()> {
    let started_at_unix_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let state = Arc::new(ApiState {
        session_manager,
        session_repos,
        log_broadcaster,
        job_registry,
        started_at_unix_s,
    });

    let app = Router::new()
        .route("/api/version", get(handle_version))
        .route("/api/status", get(handle_status))
        .route("/api/repos", get(handle_repos).post(handle_repo_add))
        .route("/api/repos/{id}/reindex", post(handle_repo_reindex))
        .route(
            "/api/repos/{id}",
            get(handle_repo_detail).delete(handle_repo_delete),
        )
        .route("/api/query/search", post(handle_query_search))
        .route("/api/query/investigate", post(handle_query_investigate))
        .route("/api/query/ask", post(handle_query_ask))
        .route("/api/query/hydrate", post(handle_query_hydrate))
        .route("/api/query/repo-map", post(handle_query_repo_map))
        .route("/api/query/definition", post(handle_query_definition))
        .route("/api/query/references", post(handle_query_references))
        .route(
            "/api/consent",
            get(handle_consent_get).post(handle_consent_post),
        )
        .route("/api/jobs", get(handle_jobs))
        .route("/api/sessions", get(handle_sessions))
        .route("/api/logs/stream", get(handle_logs_stream))
        .fallback(crate::server::assets::serve_spa)
        .layer(middleware::from_fn(crate::server::origin::check_origin))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{api_port}")
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid API address {host}:{api_port}: {e}"))?;
    let listener = crate::server::net::bind_reusable_listener(addr)?;

    tracing::info!(
        api_port,
        "Dashboard at http://{host}:{api_port}/ (API: /api/status, /api/repos, /api/version)"
    );

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "API server exited with error");
        }
    });
    Ok(())
}

async fn handle_version(State(state): State<Arc<ApiState>>) -> Json<Value> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "started_at_unix_s": state.started_at_unix_s,
        "uptime_s": now.saturating_sub(state.started_at_unix_s),
    }))
}

async fn handle_repos(State(state): State<Arc<ApiState>>) -> Result<Json<Value>, ApiError> {
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

async fn handle_status(State(state): State<Arc<ApiState>>) -> Result<Json<Value>, ApiError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let registered = state
        .session_manager
        .registry
        .list_all()
        .map(|v| v.len())
        .unwrap_or(0);
    let connected_sessions = state.session_repos.len();
    let bound_sessions = state
        .session_repos
        .iter()
        .filter(|e| e.value().repo.is_some())
        .count();
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "started_at_unix_s": state.started_at_unix_s,
        "uptime_s": now.saturating_sub(state.started_at_unix_s),
        "registered_repos": registered,
        // `active_sessions` retained for backward compatibility; equal to
        // `bound_sessions`.
        "active_sessions": bound_sessions,
        "connected_sessions": connected_sessions,
        "bound_sessions": bound_sessions,
    })))
}

async fn handle_repo_detail(
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

    Some(json!({
        "symbols": symbols,
        "edges": edges,
        "descriptions": descriptions,
        "undescribed_symbols": undescribed,
        "last_updated_unix_s": last_updated,
        "latest_index_run": latest_index_run,
        "latest_search_run": latest_search_run,
    }))
}

async fn handle_repo_reindex(
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

async fn handle_jobs(State(state): State<Arc<ApiState>>) -> Json<Value> {
    let items = jobs::snapshot(&state.job_registry);
    let running = items
        .iter()
        .filter(|j| matches!(j.status, jobs::JobStatus::Running))
        .count();
    Json(json!({
        "count": items.len(),
        "running": running,
        "jobs": items,
    }))
}

#[derive(Debug, Deserialize)]
struct QuerySearchRequest {
    repo: String,
    query: String,
    limit: Option<u32>,
    context: Option<String>,
    exported_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct QueryInvestigateRequest {
    repo: String,
    question: String,
    target: Option<String>,
    file_path: Option<String>,
    mode: Option<String>,
    max_hops: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct QueryAskRequest {
    repo: String,
    question: String,
    target: Option<String>,
    file_path: Option<String>,
    mode: Option<String>,
    max_evidence: Option<u32>,
    quality: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QueryHydrateRequest {
    repo: String,
    ids: Vec<String>,
    mode: Option<String>,
    verbose: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct QueryRepoMapRequest {
    repo: String,
    budget_tokens: Option<u32>,
    max_files: Option<u32>,
    max_symbols_per_file: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct QueryDefinitionRequest {
    repo: String,
    symbol_name: String,
    file: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct QueryReferencesRequest {
    repo: String,
    symbol_name: String,
    file: Option<String>,
    reference_type: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct AddRepoRequest {
    path: String,
}

/// Validate and canonicalize a user-supplied repo path for explicit
/// registration via `POST /api/repos`. Returns a message suitable for a 400.
fn validate_repo_path(input: &str) -> Result<crate::path::Utf8PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("path is required".to_string());
    }
    let canonical = dunce::canonicalize(std::path::Path::new(trimmed))
        .map_err(|e| format!("path not found or not accessible: {e}"))?;
    if !canonical.is_dir() {
        return Err("path is not a directory".to_string());
    }
    crate::path::Utf8PathBuf::from_path_buf(canonical)
        .map_err(|_| "path is not valid UTF-8".to_string())
}

/// `POST /api/repos` -> register a repo explicitly (consent = Approved).
async fn handle_repo_add(
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

async fn handle_query_search(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QuerySearchRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.query.trim().is_empty() {
        return Err(ApiError("query is required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::SearchCodeTool {
        query: req.query,
        limit: req.limit,
        exported_only: req.exported_only,
        context: req.context,
    };
    let result =
        crate::handlers::handle_search_code(&app_state.retriever, &app_state.config.db_path, tool)
            .await
            .map_err(|e| ApiError(format!("search failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "search",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

async fn handle_query_investigate(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryInvestigateRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.question.trim().is_empty() {
        return Err(ApiError("question is required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::InvestigateTool {
        question: req.question,
        target: req.target,
        file_path: req.file_path,
        mode: req.mode,
        max_hops: req.max_hops,
    };
    let result = crate::handlers::handle_investigate(&app_state, tool)
        .await
        .map_err(|e| ApiError(format!("investigate failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "investigate",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

async fn handle_query_ask(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryAskRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.question.trim().is_empty() {
        return Err(ApiError("question is required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::AskCodeTool {
        question: req.question,
        target: req.target,
        file_path: req.file_path,
        mode: req.mode,
        max_evidence: req.max_evidence,
        quality: req.quality,
    };
    let result = crate::handlers::handle_ask_code(&app_state, tool)
        .await
        .map_err(|e| ApiError(format!("ask failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "ask",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

async fn handle_query_hydrate(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryHydrateRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.ids.is_empty() {
        return Err(ApiError("ids are required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::HydrateSymbolsTool {
        ids: req.ids,
        mode: req.mode,
        verbose: req.verbose,
    };
    let result = crate::handlers::handle_hydrate_symbols(&app_state, tool)
        .map_err(|e| ApiError(format!("hydrate failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "hydrate",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

async fn handle_query_repo_map(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryRepoMapRequest>,
) -> Result<Json<Value>, ApiError> {
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let result = crate::handlers::handle_repo_map(
        &app_state,
        crate::handlers::RepoMapOptions {
            budget_tokens: req.budget_tokens,
            max_files: req.max_files,
            max_symbols_per_file: req.max_symbols_per_file,
        },
    )
    .map_err(|e| ApiError(format!("repo-map failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "repo-map",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

async fn handle_query_definition(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryDefinitionRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.symbol_name.trim().is_empty() {
        return Err(ApiError("symbol_name is required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::GetDefinitionTool {
        symbol_name: req.symbol_name,
        file: req.file,
        limit: req.limit,
    };
    let result = crate::handlers::handle_get_definition(&app_state, tool)
        .await
        .map_err(|e| ApiError(format!("definition failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "definition",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

async fn handle_query_references(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryReferencesRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.symbol_name.trim().is_empty() {
        return Err(ApiError("symbol_name is required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::FindReferencesTool {
        symbol_name: req.symbol_name,
        file: req.file,
        reference_type: req.reference_type,
        limit: req.limit,
    };
    // handle_find_references is synchronous (unlike handle_get_definition above), so no .await here.
    let result = crate::handlers::handle_find_references(&app_state, tool)
        .map_err(|e| ApiError(format!("references failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "references",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

/// Assemble the `/api/consent` body: pending repos from the in-memory tracker,
/// plus registry entries the user previously declined.
fn build_consent_response(
    pending: Vec<crate::session::PendingConsent>,
    repos: Vec<crate::registry::RepoEntry>,
) -> Value {
    let declined: Vec<Value> = repos
        .into_iter()
        .filter(|e| e.consent == crate::registry::IndexConsent::Declined)
        .map(|e| {
            let detected =
                crate::server::project_check::classify_repo(crate::path::Utf8Path::new(&e.path))
                    .kind();
            json!({
                "repo_path": e.path,
                "repo_id": crate::registry::RepoRegistry::path_hash(&e.path),
                "detected": detected,
            })
        })
        .collect();
    json!({
        "pending": serde_json::to_value(&pending).unwrap_or(Value::Null),
        "declined": declined,
    })
}

async fn handle_consent_get(State(state): State<Arc<ApiState>>) -> Result<Json<Value>, ApiError> {
    let pending = state.session_manager.list_pending();
    let repos = state
        .session_manager
        .registry
        .list_all()
        .map_err(|e| ApiError(format!("failed to list repos: {e}")))?;
    Ok(Json(build_consent_response(pending, repos)))
}

#[derive(Debug, Deserialize)]
struct ConsentDecisionRequest {
    repo: String,
    decision: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ConsentDecision {
    Approve,
    Decline,
}

fn parse_consent_decision(decision: &str) -> Result<ConsentDecision, String> {
    match decision {
        "approve" => Ok(ConsentDecision::Approve),
        "decline" => Ok(ConsentDecision::Decline),
        other => Err(format!(
            "decision must be \"approve\" or \"decline\", got: {other}"
        )),
    }
}

async fn handle_consent_post(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<ConsentDecisionRequest>,
) -> Result<Response, ApiError> {
    let decision = match parse_consent_decision(&req.decision) {
        Ok(d) => d,
        Err(msg) => {
            return Ok((StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response());
        }
    };
    let repo_path = match validate_repo_path(&req.repo) {
        Ok(p) => p,
        Err(msg) => {
            return Ok((StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response());
        }
    };
    let repo_id = crate::registry::RepoRegistry::path_hash(repo_path.as_str());

    // Only act on repos the gate already surfaced (pending) or the user already
    // declined. Indexing an arbitrary new path is the Repos -> Add flow, not this.
    let is_declined = matches!(
        state
            .session_manager
            .registry
            .consent_status(repo_path.as_str())
            .ok()
            .flatten(),
        Some(crate::registry::IndexConsent::Declined)
    );
    if !state.session_manager.is_pending(&repo_id) && !is_declined {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "repo is neither pending nor previously declined; use Add Repo to index a new path"
            })),
        )
            .into_response());
    }

    match decision {
        ConsentDecision::Approve => {
            state
                .session_manager
                .get_or_create_repo(&repo_path)
                .await
                .map_err(|e| ApiError(format!("failed to start indexing: {e}")))?;
            state.session_manager.clear_pending(&repo_id);
            Ok((
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "status": "indexing_started",
                    "repo": repo_path.as_str(),
                    "repo_id": repo_id,
                })),
            )
                .into_response())
        }
        ConsentDecision::Decline => {
            state
                .session_manager
                .registry
                .set_consent(repo_path.as_str(), crate::registry::IndexConsent::Declined)
                .map_err(|e| ApiError(format!("failed to record decline: {e}")))?;
            state.session_manager.clear_pending(&repo_id);
            Ok((
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "status": "declined",
                    "repo": repo_path.as_str(),
                    "repo_id": repo_id,
                })),
            )
                .into_response())
        }
    }
}

async fn resolve_query_repo(
    state: &ApiState,
    repo: &str,
) -> Result<
    (
        crate::path::Utf8PathBuf,
        String,
        Arc<crate::handlers::AppState>,
    ),
    ApiError,
> {
    let raw = crate::path::Utf8PathBuf::from(repo);
    let canonical = dunce::canonicalize(raw.as_std_path()).map_err(|e| {
        ApiError(format!(
            "workspace not found or not accessible: {repo}: {e}"
        ))
    })?;
    if !canonical.is_dir() {
        return Err(ApiError(format!("workspace is not a directory: {repo}")));
    }
    let repo_path = crate::path::Utf8PathBuf::from_path_buf(canonical)
        .map_err(|_| ApiError(format!("workspace path is not valid UTF-8: {repo}")))?;
    let repo_id = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
    let app_state = state
        .session_manager
        .get_or_create_repo(&repo_path)
        .await
        .map_err(|e| ApiError(format!("failed to load repo: {e}")))?;
    Ok((repo_path, repo_id, app_state))
}

fn query_envelope(
    command: &str,
    repo_path: &crate::path::Utf8Path,
    repo_id: &str,
    index_version_unix_s: Option<i64>,
    result: Value,
) -> Value {
    json!({
        "ok": true,
        "command": command,
        "repo": {
            "path": repo_path.as_str(),
            "id": repo_id,
        },
        "index": {
            "version_unix_s": index_version_unix_s,
            "fresh": true,
        },
        "warnings": [],
        "result": result,
    })
}

async fn handle_repo_delete(
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

async fn handle_logs_stream(
    State(state): State<Arc<ApiState>>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.log_broadcaster.subscribe();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(line) => Some((Ok(Event::default().data(line)), rx)),
            Err(RecvError::Lagged(n)) => Some((
                Ok(Event::default()
                    .event("lagged")
                    .data(format!("{n} log messages dropped"))),
                rx,
            )),
            Err(RecvError::Closed) => None,
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn handle_sessions(State(state): State<Arc<ApiState>>) -> Json<Value> {
    let now = Instant::now();
    let sessions: Vec<Value> = state
        .session_repos
        .iter()
        .map(|entry| {
            let info = entry.value();
            let initialized_at_unix_s = info
                .initialized_at
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let last_seen_secs_ago = now.saturating_duration_since(info.last_seen).as_secs();
            json!({
                "session_id": entry.key(),
                "repo": info.repo.as_ref().map(|p| p.as_str()),
                "bound": info.repo.is_some(),
                "initialized_at_unix_s": initialized_at_unix_s,
                "last_seen_secs_ago": last_seen_secs_ago,
                "bind_skipped_reason": info.bind_skipped_reason.clone(),
            })
        })
        .collect();
    let bound = sessions
        .iter()
        .filter(|v| v.get("bound").and_then(|b| b.as_bool()).unwrap_or(false))
        .count();
    Json(json!({
        "count": sessions.len(),
        "bound_count": bound,
        "connected_count": sessions.len(),
        "sessions": sessions,
    }))
}

/// Error wrapper that returns a JSON body with HTTP 500.
struct ApiError(String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": self.0 })),
        )
            .into_response()
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
    fn query_envelope_has_stable_agent_contract_fields() {
        for command in [
            "ask",
            "search",
            "investigate",
            "hydrate",
            "repo-map",
            "definition",
            "references",
        ] {
            let envelope = query_envelope(
                command,
                crate::path::Utf8Path::new("/tmp/workspace"),
                "repo123",
                Some(123),
                json!({ "value": true }),
            );

            assert_eq!(envelope["ok"], true);
            assert_eq!(envelope["command"], command);
            assert_eq!(envelope["repo"]["path"], "/tmp/workspace");
            assert_eq!(envelope["repo"]["id"], "repo123");
            assert_eq!(envelope["index"]["version_unix_s"], 123);
            assert_eq!(envelope["index"]["fresh"], true);
            assert_eq!(envelope["warnings"].as_array().unwrap().len(), 0);
            assert_eq!(envelope["result"]["value"], true);
        }
    }

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
    fn validate_repo_path_accepts_existing_dir_and_rejects_bad_input() {
        let tmp = tempdir().expect("tempdir");
        let dir = tmp.path().to_str().expect("utf8 tmp path");

        // Existing directory canonicalizes to an Ok UTF-8 path.
        assert!(validate_repo_path(dir).is_ok());
        // Blank input is rejected.
        assert!(validate_repo_path("   ").is_err());
        // Nonexistent path is rejected.
        assert!(validate_repo_path("/no/such/path/xyzzy-1b").is_err());
        // A file (not a directory) is rejected.
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, b"x").expect("write file");
        assert!(validate_repo_path(file.to_str().unwrap()).is_err());
    }

    #[test]
    fn build_consent_response_shapes_pending_and_filters_declined() {
        let pending = vec![crate::session::PendingConsent {
            repo_path: "/Users/me/wt".to_string(),
            repo_id: "id_pending".to_string(),
            detected: "git_worktree".to_string(),
            recommendation: "ask before indexing".to_string(),
            detail: Some("git worktree of /Users/me/main".to_string()),
            first_seen_unix_s: 10,
            last_seen_unix_s: 20,
            occurrences: 3,
        }];
        let repos = vec![
            RepoEntry {
                path: "/Users/me/declined".to_string(),
                name: "declined".to_string(),
                data_dir: Utf8PathBuf::from("/data/declined"),
                created_at: "x".to_string(),
                last_accessed: "x".to_string(),
                consent: crate::registry::IndexConsent::Declined,
            },
            RepoEntry {
                path: "/Users/me/approved".to_string(),
                name: "approved".to_string(),
                data_dir: Utf8PathBuf::from("/data/approved"),
                created_at: "x".to_string(),
                last_accessed: "x".to_string(),
                consent: crate::registry::IndexConsent::Approved,
            },
        ];

        let v = build_consent_response(pending, repos);

        // Pending item carries every field the frontend type expects.
        assert_eq!(v["pending"][0]["repo_path"], "/Users/me/wt");
        assert_eq!(v["pending"][0]["repo_id"], "id_pending");
        assert_eq!(v["pending"][0]["detected"], "git_worktree");
        assert_eq!(v["pending"][0]["recommendation"], "ask before indexing");
        assert_eq!(v["pending"][0]["detail"], "git worktree of /Users/me/main");
        assert_eq!(v["pending"][0]["occurrences"], 3);

        // Only the declined repo is surfaced; the approved one is filtered out.
        assert_eq!(v["declined"].as_array().unwrap().len(), 1);
        assert_eq!(v["declined"][0]["repo_path"], "/Users/me/declined");
        assert_eq!(v["declined"][0]["detected"], "standard");
        assert!(v["declined"][0]["repo_id"].is_string());
    }

    #[test]
    fn parse_consent_decision_accepts_approve_decline_and_rejects_other() {
        assert_eq!(
            parse_consent_decision("approve"),
            Ok(ConsentDecision::Approve)
        );
        assert_eq!(
            parse_consent_decision("decline"),
            Ok(ConsentDecision::Decline)
        );
        let err = parse_consent_decision("maybe").unwrap_err();
        assert!(err.contains("approve"));
        assert!(err.contains("maybe"));
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
                }),
                latest_search_run: None,
                last_updated_unix_s: None,
            },
        );

        assert_eq!(activity["running"], false);
        assert_eq!(activity["latest_index_run"]["started_at_unix_s"], 456);
    }
}

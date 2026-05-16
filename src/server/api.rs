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
    extract::{Path, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Json, Response,
    },
    routing::{delete, get, post},
    Router,
};
use futures::stream::Stream;
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

const DASHBOARD_HTML: &str = include_str!("../../ui/dashboard.html");
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::log_broadcast::LogBroadcaster;
use crate::server::jobs::{self, JobRegistry};
use crate::server::standalone::SessionRepos;
use crate::session::SessionManager;

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
        .route("/", get(handle_dashboard))
        .route("/api/version", get(handle_version))
        .route("/api/status", get(handle_status))
        .route("/api/repos", get(handle_repos))
        .route("/api/repos/{id}/reindex", post(handle_repo_reindex))
        .route("/api/repos/{id}", delete(handle_repo_delete))
        .route("/api/jobs", get(handle_jobs))
        .route("/api/sessions", get(handle_sessions))
        .route("/api/logs/stream", get(handle_logs_stream))
        .layer(middleware::from_fn(check_origin))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{api_port}")
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid API address {host}:{api_port}: {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

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

/// DNS-rebinding guard. Allow only:
/// - requests with no `Origin` header (server-to-server, curl, CLI clients)
/// - requests whose Origin host is `localhost` or `127.0.0.1`
async fn check_origin(req: Request, next: Next) -> Result<Response, StatusCode> {
    if let Some(origin) = req.headers().get("origin") {
        let Ok(s) = origin.to_str() else {
            return Err(StatusCode::FORBIDDEN);
        };
        if !is_local_origin(s) {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(next.run(req).await)
}

fn is_local_origin(origin: &str) -> bool {
    // Strip scheme.
    let after_scheme = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .unwrap_or(origin);
    // Strip any trailing path (Origin should not have one, defensive).
    let host_port = after_scheme.split('/').next().unwrap_or("");
    // Handle bracketed IPv6 literals like `[::1]:17800`.
    if let Some(rest) = host_port.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return &rest[..end] == "::1";
        }
        return false;
    }
    let host = host_port.split(':').next().unwrap_or("");
    host == "localhost" || host == "127.0.0.1"
}

async fn handle_dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
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
            json!({
                "id": crate::registry::RepoRegistry::path_hash(&e.path),
                "name": e.name,
                "path": e.path,
                "data_dir": e.data_dir,
                "created_at": e.created_at,
                "last_accessed": e.last_accessed,
            })
        })
        .collect();
    Ok(Json(json!({ "count": count, "repos": items })))
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
        loop {
            match rx.recv().await {
                Ok(line) => {
                    return Some((Ok(Event::default().data(line)), rx));
                }
                Err(RecvError::Lagged(n)) => {
                    return Some((
                        Ok(Event::default()
                            .event("lagged")
                            .data(format!("{n} log messages dropped"))),
                        rx,
                    ));
                }
                Err(RecvError::Closed) => return None,
            }
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

    #[test]
    fn is_local_origin_accepts_localhost_variants() {
        assert!(is_local_origin("http://localhost:17800"));
        assert!(is_local_origin("http://localhost"));
        assert!(is_local_origin("http://127.0.0.1:17802"));
        assert!(is_local_origin("https://127.0.0.1"));
        assert!(is_local_origin("http://[::1]:17802"));
    }

    #[test]
    fn is_local_origin_rejects_remote() {
        assert!(!is_local_origin("https://example.com"));
        assert!(!is_local_origin("http://example.com:17800"));
        assert!(!is_local_origin("http://192.168.1.42:17800"));
        assert!(!is_local_origin("http://attacker.localhost.evil:17800"));
    }

    #[test]
    fn is_local_origin_handles_missing_scheme() {
        // `Origin` should always have a scheme, but be defensive.
        assert!(is_local_origin("127.0.0.1"));
        assert!(!is_local_origin("example.com"));
    }
}

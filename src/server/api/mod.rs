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
    extract::State,
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
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::log_broadcast::LogBroadcaster;
use crate::server::jobs::{self, JobRegistry};
use crate::server::standalone::SessionRepos;
use crate::session::SessionManager;

mod consent;
mod query;
mod repos;

use consent::{handle_consent_get, handle_consent_post};
use query::{
    handle_query_ask, handle_query_definition, handle_query_hydrate, handle_query_investigate,
    handle_query_references, handle_query_repo_map, handle_query_search,
};
use repos::{
    handle_repo_add, handle_repo_delete, handle_repo_detail, handle_repo_reindex, handle_repos,
};

#[derive(Clone)]
pub(crate) struct ApiState {
    pub(crate) session_manager: Arc<SessionManager>,
    pub(crate) session_repos: SessionRepos,
    pub(crate) log_broadcaster: LogBroadcaster,
    pub(crate) job_registry: JobRegistry,
    pub(crate) started_at_unix_s: u64,
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

/// Validate and canonicalize a user-supplied repo path for explicit
/// registration via `POST /api/repos`. Returns a message suitable for a 400.
pub(crate) fn validate_repo_path(input: &str) -> Result<crate::path::Utf8PathBuf, String> {
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
pub(crate) struct ApiError(pub(crate) String);

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
    use tempfile::tempdir;

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
}

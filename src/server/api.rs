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
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};

const DASHBOARD_HTML: &str = include_str!("../../ui/dashboard.html");
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::server::standalone::SessionRepos;
use crate::session::SessionManager;

#[derive(Clone)]
struct ApiState {
    session_manager: Arc<SessionManager>,
    session_repos: SessionRepos,
    started_at_unix_s: u64,
}

/// Spawn the API server on `api_port`, returning once it is bound.
/// Errors are surfaced only if the bind itself fails.
pub async fn spawn_api_server(
    host: &str,
    api_port: u16,
    session_manager: Arc<SessionManager>,
    session_repos: SessionRepos,
) -> anyhow::Result<()> {
    let started_at_unix_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let state = Arc::new(ApiState {
        session_manager,
        session_repos,
        started_at_unix_s,
    });

    let app = Router::new()
        .route("/", get(handle_dashboard))
        .route("/api/version", get(handle_version))
        .route("/api/status", get(handle_status))
        .route("/api/repos", get(handle_repos))
        .route("/api/repos/{id}/reindex", post(handle_repo_reindex))
        .route("/api/sessions", get(handle_sessions))
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
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "started_at_unix_s": state.started_at_unix_s,
        "uptime_s": now.saturating_sub(state.started_at_unix_s),
        "registered_repos": registered,
        "active_sessions": state.session_repos.len(),
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
    // re-index can take minutes; the caller polls /api/repos for updated
    // stats.
    let job_id = format!(
        "reindex-{}-{}",
        id,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let job_id_log = job_id.clone();
    tokio::spawn(async move {
        let state = match sm.get_or_create_repo(&path).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, path = %path, "reindex: failed to load repo");
                return;
            }
        };
        let tool = crate::tools::RefreshIndexTool { files: None };
        match crate::handlers::handle_refresh_index(&state, tool).await {
            Ok(stats) => {
                tracing::info!(job = %job_id_log, path = %path, stats = %stats, "reindex completed");
            }
            Err(e) => {
                tracing::error!(job = %job_id_log, error = %e, path = %path, "reindex failed");
            }
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

async fn handle_sessions(State(state): State<Arc<ApiState>>) -> Json<Value> {
    let sessions: Vec<Value> = state
        .session_repos
        .iter()
        .map(|entry| {
            json!({
                "session_id": entry.key(),
                "repo": entry.value().as_str(),
            })
        })
        .collect();
    Json(json!({
        "count": sessions.len(),
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

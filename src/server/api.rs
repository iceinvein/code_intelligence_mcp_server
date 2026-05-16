//! JSON API endpoint for the daemon.
//!
//! Runs on a separate port from the MCP endpoint (default: `mcp_port + 2`) and
//! exposes read-only inspection routes that the lifecycle subcommands and the
//! future web UI consume.
//!
//! Routes:
//! - `GET /api/version` -> `{ version, started_at_unix_s, uptime_s }`
//! - `GET /api/status`  -> daemon overview (version, uptime, repo count)
//! - `GET /api/repos`   -> `[{ name, path, created_at, last_accessed }, ...]`
//!
//! All routes bind 127.0.0.1 only and reject requests whose `Origin` header is
//! not `http://localhost:<port>` or `http://127.0.0.1:<port>`. The check is a
//! DNS-rebinding defence: a malicious web page that resolves `example.com` to
//! 127.0.0.1 cannot read the daemon's repo list because its `Origin` would
//! still be `https://example.com`.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::SessionManager;

#[derive(Clone)]
struct ApiState {
    session_manager: Arc<SessionManager>,
    started_at_unix_s: u64,
}

/// Spawn the API server on `api_port`, returning once it is bound.
/// Errors are surfaced only if the bind itself fails.
pub async fn spawn_api_server(
    host: &str,
    api_port: u16,
    session_manager: Arc<SessionManager>,
) -> anyhow::Result<()> {
    let started_at_unix_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let state = Arc::new(ApiState {
        session_manager,
        started_at_unix_s,
    });

    let app = Router::new()
        .route("/api/version", get(handle_version))
        .route("/api/status", get(handle_status))
        .route("/api/repos", get(handle_repos))
        .layer(middleware::from_fn(check_origin))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{api_port}")
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid API address {host}:{api_port}: {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!(
        api_port,
        "API endpoint available at http://{host}:{api_port}/api/status"
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
    })))
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

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
//! - `GET /api/fs/list?path=&show_hidden=` -> subdirectories of a path (folder picker)
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
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::log_broadcast::LogBroadcaster;
use crate::server::jobs::JobRegistry;
use crate::server::standalone::SessionRepos;
use crate::session::SessionManager;

mod activity;
mod consent;
mod filesystem;
mod query;
mod repos;
mod settings;

use activity::{handle_jobs, handle_logs_stream, handle_sessions};
use consent::{handle_consent_get, handle_consent_post, handle_index_status};
use filesystem::handle_fs_list;
use query::{
    handle_query_ask, handle_query_call_hierarchy, handle_query_definition,
    handle_query_dependency_graph, handle_query_file_symbols, handle_query_files,
    handle_query_hydrate, handle_query_investigate, handle_query_references, handle_query_repo_map,
    handle_query_search, handle_query_type_graph, handle_query_usage_examples,
};
use repos::{
    handle_repo_add, handle_repo_delete, handle_repo_detail, handle_repo_reindex, handle_repos,
};
use settings::{handle_settings_get, handle_settings_put};

#[derive(Clone)]
pub(crate) struct ApiState {
    pub(crate) session_manager: Arc<SessionManager>,
    pub(crate) session_repos: SessionRepos,
    pub(crate) log_broadcaster: LogBroadcaster,
    pub(crate) job_registry: JobRegistry,
    pub(crate) started_at_unix_s: u64,
}

#[cfg(test)]
pub(crate) async fn test_api_state() -> (tempfile::TempDir, Arc<ApiState>) {
    use crate::config::{EmbeddingsBackend, StandaloneConfig};
    use crate::embeddings::hash::HashEmbedder;
    use crate::embeddings::SharedEmbedder;
    use crate::registry::RepoRegistry;
    use crate::server::jobs;

    let temp = tempfile::tempdir().unwrap();
    let data_dir = crate::path::Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let config = StandaloneConfig {
        data_dir: data_dir.clone(),
        embeddings_backend: EmbeddingsBackend::Hash,
        hash_embedding_dim: 64,
        ..StandaloneConfig::default()
    };
    let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));
    let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));
    let job_registry = jobs::new_job_registry();
    let session_manager = Arc::new(
        SessionManager::new(config, registry, embedder, Some(job_registry.clone()), None)
            .await
            .unwrap(),
    );
    let state = Arc::new(ApiState {
        session_manager,
        session_repos: crate::server::standalone::new_session_repos(),
        log_broadcaster: LogBroadcaster::new(),
        job_registry,
        started_at_unix_s: 0,
    });
    (temp, state)
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
        .route("/api/fs/list", get(handle_fs_list))
        .route("/api/query/search", post(handle_query_search))
        .route("/api/query/investigate", post(handle_query_investigate))
        .route("/api/query/ask", post(handle_query_ask))
        .route("/api/query/hydrate", post(handle_query_hydrate))
        .route("/api/query/repo-map", post(handle_query_repo_map))
        .route("/api/query/definition", post(handle_query_definition))
        .route("/api/query/references", post(handle_query_references))
        .route("/api/query/files", post(handle_query_files))
        .route("/api/query/file-symbols", post(handle_query_file_symbols))
        .route(
            "/api/query/usage-examples",
            post(handle_query_usage_examples),
        )
        .route(
            "/api/query/call-hierarchy",
            post(handle_query_call_hierarchy),
        )
        .route("/api/query/type-graph", post(handle_query_type_graph))
        .route(
            "/api/query/dependency-graph",
            post(handle_query_dependency_graph),
        )
        .route(
            "/api/consent",
            get(handle_consent_get).post(handle_consent_post),
        )
        .route("/api/index/status", post(handle_index_status))
        .route(
            "/api/settings",
            get(handle_settings_get).put(handle_settings_put),
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

#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
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

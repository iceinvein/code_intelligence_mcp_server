//! Standalone mode MCP handler — routes sessions to per-repo AppState

use crate::handlers::{
    handle_explore_cross_repo_dependencies, handle_search_across_repos, parse_tool_args,
    tool_internal_error, AppState,
};
use crate::path::Utf8PathBuf;
use crate::server::mcp_proxy::PendingRepos;
use crate::server::{all_tools, dispatch_tool_call, tool_json_content};
use crate::session::SessionManager;
use crate::tools::{BindWorkspaceTool, ExploreCrossRepoDependenciesTool, SearchAcrossReposTool};
use async_trait::async_trait;
use dashmap::DashMap;
use rust_mcp_sdk::{
    mcp_server::ServerHandler,
    schema::{
        CallToolError, CallToolRequestParams, CallToolResult, CreateTaskResult, InitializeResult,
        ListToolsResult, PaginatedRequestParams, RpcError,
    },
    task_store::ServerTaskCreator,
    McpServer,
};
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

/// SessionId is String (from rust_mcp_transport)
type SessionId = String;

/// TTL after which a session with no recent activity is evicted from the
/// dashboard's `Sessions` map. We need this because rust_mcp_sdk 0.8 does
/// not expose a session-close hook, so we can't observe disconnects.
/// Five minutes is long enough that a session staying open without calls
/// is still visible, short enough that abandoned connections fade out.
pub const SESSION_INACTIVITY_TTL: Duration = Duration::from_secs(300);

/// Per-session bookkeeping for the dashboard. Tracks both initialized-only
/// and bound sessions so the UI can distinguish them.
#[derive(Clone, Debug)]
pub struct SessionInfo {
    /// Bound workspace root, if any. `None` between `on_initialized` and
    /// the moment `roots/list` resolves, the `bind_workspace` tool fires,
    /// or the single-repo fallback triggers.
    pub repo: Option<Utf8PathBuf>,
    /// Wall-clock time of `on_initialized` (for the UI).
    pub initialized_at: SystemTime,
    /// Monotonic timestamp refreshed on every tool call; entries past
    /// `SESSION_INACTIVITY_TTL` are evicted by [`spawn_session_eviction_loop`].
    pub last_seen: Instant,
}

/// Shared map of MCP session-id to bookkeeping info. Cloned (cheaply, it is
/// an `Arc`) into both `StandaloneHandler` (which writes) and the JSON API
/// server (which reads for `/api/sessions` and the dashboard counters).
pub type Sessions = Arc<DashMap<SessionId, SessionInfo>>;

/// Back-compat alias for existing call sites. Prefer `Sessions` in new code.
pub type SessionRepos = Sessions;

pub fn new_session_repos() -> Sessions {
    Arc::new(DashMap::new())
}

/// Spawn a background task that evicts session entries whose `last_seen` is
/// older than [`SESSION_INACTIVITY_TTL`]. Runs once per minute for the
/// lifetime of the process.
pub fn spawn_session_eviction_loop(sessions: Sessions) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let stale: Vec<SessionId> = sessions
                .iter()
                .filter(|e| e.value().last_seen.elapsed() > SESSION_INACTIVITY_TTL)
                .map(|e| e.key().clone())
                .collect();
            for id in stale {
                if sessions.remove(&id).is_some() {
                    tracing::info!(session = %id, "Evicted stale session (inactivity TTL)");
                }
            }
        }
    });
}

pub struct StandaloneHandler {
    pub session_manager: Arc<SessionManager>,
    pub server_details: InitializeResult,
    /// Maps session_id → bookkeeping. An entry is inserted with `repo: None`
    /// when `on_initialized` fires, then upgraded to `repo: Some(_)` when
    /// the workspace root is resolved (via `?repo=` URL binding,
    /// `roots/list`, `bind_workspace`, or the single-repo fallback).
    session_repos: Sessions,
    /// URL-query bindings captured by the proxy in front of the SDK. The
    /// proxy reads `?repo=` from each request and pairs it with the
    /// `mcp-session-id` header in the upstream response. This map is the
    /// highest-priority binding source per the v4 plan: it works on every
    /// HTTP MCP client (not just Claude Code) and is set before the client
    /// has a chance to issue any tool call.
    pending_repos: PendingRepos,
}

impl StandaloneHandler {
    pub fn new(
        session_manager: Arc<SessionManager>,
        server_details: InitializeResult,
        session_repos: Sessions,
        pending_repos: PendingRepos,
    ) -> Self {
        Self {
            session_manager,
            server_details,
            session_repos,
            pending_repos,
        }
    }

    /// Consume the proxy's `?repo=` binding for this session, if any, and
    /// promote it to a fully bound session. Returns the bound repo path,
    /// or `None` if no URL binding was captured.
    fn try_url_query_binding(&self, session_id: &SessionId) -> Option<Utf8PathBuf> {
        let (_id, repo) = self.pending_repos.remove(session_id)?;

        let was_already_bound = self
            .session_repos
            .get(session_id)
            .map(|i| i.repo.is_some())
            .unwrap_or(false);
        self.upsert_session(session_id, Some(repo.clone()));
        if !was_already_bound {
            tracing::info!(
                session = %session_id,
                repo = %repo,
                "Session bound to repo via ?repo= URL query"
            );

            // Pre-warm in the background so the first tool call is fast.
            let sm = self.session_manager.clone();
            let rp = repo.clone();
            tokio::spawn(async move {
                match sm.get_or_create_repo(&rp).await {
                    Ok(_) => tracing::info!(repo = %rp, "Repo initialized successfully"),
                    Err(e) => tracing::error!(repo = %rp, error = %e, "Failed to pre-warm repo"),
                }
            });
        }
        Some(repo)
    }

    /// Upsert a session: if it already exists, update `last_seen` (and
    /// optionally promote `repo`); otherwise insert a fresh entry. Returns
    /// the bound repo path if known, `None` otherwise.
    fn upsert_session(&self, session_id: &SessionId, repo: Option<Utf8PathBuf>) {
        let now_instant = Instant::now();
        let now_system = SystemTime::now();
        self.session_repos
            .entry(session_id.clone())
            .and_modify(|info| {
                info.last_seen = now_instant;
                if let Some(p) = &repo {
                    info.repo = Some(p.clone());
                }
            })
            .or_insert(SessionInfo {
                repo,
                initialized_at: now_system,
                last_seen: now_instant,
            });
    }

    /// Refresh `last_seen` for a session without changing its repo. Used by
    /// every tool call so an active session never gets evicted by the
    /// inactivity TTL.
    fn touch_session(&self, session_id: &SessionId) {
        if let Some(mut info) = self.session_repos.get_mut(session_id) {
            info.last_seen = Instant::now();
        }
    }

    /// Return the currently-bound repo for a session, if any.
    fn bound_repo(&self, session_id: &SessionId) -> Option<Utf8PathBuf> {
        self.session_repos
            .get(session_id)
            .and_then(|info| info.repo.clone())
    }

    /// Parse a workspace-root URI into a UTF-8 path.
    ///
    /// Handles `file://` URIs (including Windows `file:///C:/...`) and falls
    /// back to treating the URI as a plain path when it does not parse.
    fn parse_root_uri(uri: &str) -> Utf8PathBuf {
        match url::Url::parse(uri) {
            Ok(parsed) => match parsed.to_file_path() {
                Ok(std_path) => Utf8PathBuf::from_path_buf(std_path).unwrap_or_else(|_| {
                    Utf8PathBuf::from(uri.strip_prefix("file://").unwrap_or(uri))
                }),
                Err(_) => Utf8PathBuf::from(uri.strip_prefix("file://").unwrap_or(uri)),
            },
            Err(_) => Utf8PathBuf::from(uri),
        }
    }

    /// Bind a session to its workspace root via the MCP `roots/list` request.
    ///
    /// Idempotent and safe to call concurrently. Returns the bound path on
    /// success, `None` if the client did not (yet) supply a root. The deferred
    /// retry exists because `on_initialized` fires before the client opens its
    /// server-to-client SSE stream in Streamable HTTP transport; in that
    /// window any server-initiated request fails with "transport stream does
    /// not exists or is closed". By the time the first tool call arrives the
    /// stream is up.
    async fn try_bind_session(
        &self,
        runtime: &Arc<dyn McpServer>,
        session_id: &SessionId,
    ) -> Option<Utf8PathBuf> {
        if let Some(bound) = self.bound_repo(session_id) {
            return Some(bound);
        }

        #[allow(deprecated)] // list_roots deprecated in 0.8.0 in favor of request_root_list
        let roots_result = match runtime.request_root_list(None).await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(
                    session = %session_id,
                    error = %e,
                    "roots/list request failed; will retry on next tool call"
                );
                return None;
            }
        };

        let root = roots_result.roots.first()?;
        let repo_path = Self::parse_root_uri(&root.uri);

        // Detect "we just promoted from unbound to bound" so we only log /
        // pre-warm once per session.
        let was_already_bound = self
            .session_repos
            .get(session_id)
            .map(|i| i.repo.is_some())
            .unwrap_or(false);

        self.upsert_session(session_id, Some(repo_path.clone()));

        if !was_already_bound {
            tracing::info!(
                session = %session_id,
                repo = %repo_path,
                "Session bound to repo"
            );

            // Pre-warm: trigger repo initialization in background so the
            // first tool call does not pay the indexer-startup cost.
            let sm = self.session_manager.clone();
            let rp = repo_path.clone();
            tokio::spawn(async move {
                match sm.get_or_create_repo(&rp).await {
                    Ok(_) => tracing::info!(repo = %rp, "Repo initialized successfully"),
                    Err(e) => {
                        tracing::error!(repo = %rp, error = %e, "Failed to pre-warm repo")
                    }
                }
            });
        }

        Some(repo_path)
    }

    /// Bind a session to an explicit repo path supplied by the client via the
    /// `bind_workspace` MCP tool. Overwrites any prior binding.
    async fn handle_bind_workspace(
        &self,
        runtime: &Arc<dyn McpServer>,
        tool: BindWorkspaceTool,
    ) -> Result<serde_json::Value, CallToolError> {
        let session_id = runtime.session_id().ok_or_else(|| {
            CallToolError::from_message(
                "No session ID; standalone mode requires Streamable HTTP transport".to_string(),
            )
        })?;

        let repo_path = Utf8PathBuf::from(tool.repo.as_str());
        if !repo_path.is_absolute() {
            return Err(CallToolError::from_message(format!(
                "bind_workspace.repo must be an absolute path, got: {}",
                tool.repo
            )));
        }
        if !repo_path.is_dir() {
            return Err(CallToolError::from_message(format!(
                "bind_workspace.repo does not exist or is not a directory: {}",
                repo_path
            )));
        }

        self.upsert_session(&session_id, Some(repo_path.clone()));

        tracing::info!(
            session = %session_id,
            repo = %repo_path,
            "Session bound to repo via bind_workspace"
        );

        // Pre-warm so the next tool call is fast.
        let sm = self.session_manager.clone();
        let rp = repo_path.clone();
        tokio::spawn(async move {
            match sm.get_or_create_repo(&rp).await {
                Ok(_) => tracing::info!(repo = %rp, "Repo initialized successfully"),
                Err(e) => tracing::error!(repo = %rp, error = %e, "Failed to pre-warm repo"),
            }
        });

        Ok(serde_json::json!({
            "ok": true,
            "session_id": session_id,
            "repo": repo_path.as_str(),
        }))
    }

    /// Fall back to the single repo registered with the SessionManager when
    /// the registry has exactly one entry. Returns `None` if zero or more
    /// than one repos are registered. This rule disables itself the moment
    /// a second repo is added, so users with multiple workspaces always
    /// hit the explicit `bind_workspace` path instead of a silent wrong-bind.
    async fn try_single_repo_fallback(&self, session_id: &SessionId) -> Option<Utf8PathBuf> {
        let repos = self.session_manager.registry.list_all().ok()?;
        if repos.len() != 1 {
            return None;
        }
        let path = Utf8PathBuf::from(repos.into_iter().next()?.path);
        let was_already_bound = self
            .session_repos
            .get(session_id)
            .map(|i| i.repo.is_some())
            .unwrap_or(false);
        self.upsert_session(session_id, Some(path.clone()));
        if !was_already_bound {
            tracing::info!(
                session = %session_id,
                repo = %path,
                "Session bound to sole registered repo (single-repo fallback)"
            );
        }
        Some(path)
    }

    /// Resolve the AppState for the current session's repo, binding lazily
    /// if `on_initialized` did not manage to during the race window.
    async fn resolve_state(
        &self,
        runtime: &Arc<dyn McpServer>,
    ) -> Result<Arc<AppState>, CallToolError> {
        let session_id = runtime.session_id().ok_or_else(|| {
            CallToolError::from_message(
                "No session ID — standalone mode requires Streamable HTTP transport".to_string(),
            )
        })?;

        // Every tool call refreshes the inactivity TTL so an active session
        // never gets evicted by `spawn_session_eviction_loop`.
        self.touch_session(&session_id);

        // Binding hierarchy (first match wins):
        //   1. `?repo=` URL query — captured by the proxy, universal client support
        //   2. MCP `roots/list` — opportunistic, Claude Code only in practice
        //   3. Single-repo fallback — only when exactly one repo is registered
        //   4. Hard error with actionable guidance
        let repo_path = if let Some(path) = self.try_url_query_binding(&session_id) {
            path
        } else if let Some(path) = self.try_bind_session(runtime, &session_id).await {
            path
        } else if let Some(path) = self.try_single_repo_fallback(&session_id).await {
            path
        } else {
            return Err(CallToolError::from_message(
                "Session not bound to a repo. Configure your MCP client URL as \
                 `http://127.0.0.1:17800/mcp?repo=/abs/path/to/your/workspace`, \
                 or call `bind_workspace` with \
                 `{ \"repo\": \"/abs/path\" }`, \
                 or use an MCP client that implements the `roots` capability."
                    .to_string(),
            ));
        };

        self.session_manager
            .get_or_create_repo(&repo_path)
            .await
            .map_err(|e| CallToolError::from_message(format!("Failed to load repo: {}", e)))
    }
}

#[async_trait]
impl ServerHandler for StandaloneHandler {
    async fn on_initialized(&self, runtime: Arc<dyn McpServer>) {
        let Some(session_id) = runtime.session_id() else {
            tracing::warn!("on_initialized called without session_id");
            return;
        };

        // Register the session as initialized-but-not-yet-bound so the
        // dashboard shows it immediately. `try_bind_session` upgrades to
        // bound if `roots/list` resolves; otherwise the entry stays in the
        // `repo: None` state until the first tool call or `bind_workspace`.
        self.upsert_session(&session_id, None);

        tracing::info!(
            session = %session_id,
            "Session initialized, attempting workspace root bind"
        );

        // Best-effort eager bind. In Streamable HTTP transport `on_initialized`
        // can fire before the client opens its server-to-client SSE stream, so
        // this request may fail with a closed-stream error. `resolve_state`
        // retries on the first tool call, by which time the stream is up.
        if self.try_bind_session(&runtime, &session_id).await.is_none() {
            tracing::info!(
                session = %session_id,
                "Workspace root not yet available; will retry on first tool call"
            );
        }
    }

    async fn handle_list_tools_request(
        &self,
        _request: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListToolsResult, RpcError> {
        Ok(ListToolsResult {
            tools: all_tools(),
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_task_augmented_tool_call(
        &self,
        params: CallToolRequestParams,
        task_creator: ServerTaskCreator,
        runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CreateTaskResult, CallToolError> {
        // Resolve per-session state before creating the task so that
        // failures (e.g., no repo bound) return immediately without
        // creating an orphaned task.
        let state = self.resolve_state(&runtime).await?;
        crate::server::dispatch_task_augmented_call(state, params, task_creator, runtime).await
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        // Workspace binding bypasses resolve_state because it is the call
        // that establishes the binding in the first place.
        if params.name == "bind_workspace" {
            let tool: BindWorkspaceTool = parse_tool_args(&params)?;
            let result = self.handle_bind_workspace(&runtime, tool).await?;
            return Ok(tool_json_content(&result));
        }

        // Cross-repo search bypasses single-repo resolution — handle before resolve_state()
        if params.name == "search_across_repos" {
            let tool: SearchAcrossReposTool = parse_tool_args(&params)?;
            let result = handle_search_across_repos(&self.session_manager, tool)
                .await
                .map_err(tool_internal_error)?;
            return Ok(tool_json_content(&result));
        }

        // Cross-repo dependency exploration needs both per-repo state AND SessionManager
        if params.name == "explore_cross_repo_dependencies" {
            let state = self.resolve_state(&runtime).await?;
            let tool: ExploreCrossRepoDependenciesTool = parse_tool_args(&params)?;
            let result =
                handle_explore_cross_repo_dependencies(&state, self.session_manager.as_ref(), tool)
                    .map_err(tool_internal_error)?;
            return Ok(tool_json_content(&result));
        }

        let state = self.resolve_state(&runtime).await?;
        // Store the MCP runtime on first tool call so the description worker
        // can use it for sampling-based description generation.
        // NOTE: Currently forward-looking plumbing — standalone mode does not
        // yet spawn per-repo description workers. Once it does, this will
        // enable MCP sampling for standalone repos.
        state.mcp_runtime.get_or_init(|| runtime.clone());
        dispatch_tool_call(&state, params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_mcp_sdk::schema::{Implementation, ProtocolVersion, ServerCapabilities};

    #[tokio::test]
    async fn standalone_handler_creates_successfully() {
        let temp_dir = tempfile::tempdir().unwrap();
        let data_dir = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap();

        let session_manager = SessionManager::new_for_test(data_dir).await;

        let server_details = InitializeResult {
            server_info: Implementation {
                name: "test-server".into(),
                version: "1.0.0".into(),
                title: None,
                description: None,
                icons: vec![],
                website_url: None,
            },
            capabilities: ServerCapabilities::default(),
            protocol_version: ProtocolVersion::V2025_11_25.into(),
            instructions: None,
            meta: None,
        };

        let handler = StandaloneHandler::new(
            Arc::new(session_manager),
            server_details,
            new_session_repos(),
            crate::server::mcp_proxy::new_pending_repos(),
        );
        assert_eq!(handler.session_repos.len(), 0);
    }

    #[test]
    fn parse_root_uri_handles_unix_file_url() {
        let parsed = StandaloneHandler::parse_root_uri("file:///Users/me/projects/foo");
        assert_eq!(parsed.as_str(), "/Users/me/projects/foo");
    }

    #[test]
    fn parse_root_uri_handles_plain_path() {
        let parsed = StandaloneHandler::parse_root_uri("/Users/me/projects/foo");
        assert_eq!(parsed.as_str(), "/Users/me/projects/foo");
    }

    #[test]
    fn parse_root_uri_handles_file_url_with_spaces() {
        let parsed = StandaloneHandler::parse_root_uri("file:///Users/me/My%20Projects/foo");
        assert_eq!(parsed.as_str(), "/Users/me/My Projects/foo");
    }

    fn test_handler() -> StandaloneHandler {
        let temp_dir = tempfile::tempdir().unwrap();
        let data_dir = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap();
        let server_details = InitializeResult {
            server_info: Implementation {
                name: "test-server".into(),
                version: "1.0.0".into(),
                title: None,
                description: None,
                icons: vec![],
                website_url: None,
            },
            capabilities: ServerCapabilities::default(),
            protocol_version: ProtocolVersion::V2025_11_25.into(),
            instructions: None,
            meta: None,
        };
        // Tests don't actually use the session_manager for these helpers; we
        // just need a value to construct the handler.
        let sm = futures::executor::block_on(SessionManager::new_for_test(data_dir));
        StandaloneHandler::new(
            Arc::new(sm),
            server_details,
            new_session_repos(),
            crate::server::mcp_proxy::new_pending_repos(),
        )
    }

    #[test]
    fn upsert_inserts_unbound_session_then_promotes_to_bound() {
        let h = test_handler();
        let sid = "session-1".to_string();

        h.upsert_session(&sid, None);
        assert_eq!(h.session_repos.len(), 1);
        let info = h.session_repos.get(&sid).unwrap().clone();
        assert!(info.repo.is_none());

        let repo = Utf8PathBuf::from("/tmp/some-repo");
        h.upsert_session(&sid, Some(repo.clone()));
        let info2 = h.session_repos.get(&sid).unwrap().clone();
        assert_eq!(info2.repo.as_deref(), Some(repo.as_path()));
        // initialized_at must NOT change on promote-to-bound
        assert_eq!(info2.initialized_at, info.initialized_at);
    }

    #[test]
    fn touch_session_refreshes_last_seen() {
        let h = test_handler();
        let sid = "session-2".to_string();
        h.upsert_session(&sid, None);

        let before = h.session_repos.get(&sid).unwrap().last_seen;
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.touch_session(&sid);
        let after = h.session_repos.get(&sid).unwrap().last_seen;
        assert!(after > before, "last_seen should advance on touch");
    }

    #[test]
    fn touch_session_no_op_for_unknown_session() {
        let h = test_handler();
        // Must not panic / not insert anything.
        h.touch_session(&"never-registered".to_string());
        assert_eq!(h.session_repos.len(), 0);
    }

    // The bind path spawns a background pre-warm task with `tokio::spawn`,
    // so these tests need a runtime.
    #[tokio::test]
    async fn try_url_query_binding_consumes_pending_and_promotes_session() {
        let h = test_handler();
        let sid = "session-url-1".to_string();
        let repo = Utf8PathBuf::from("/tmp/url-bound-repo");

        h.upsert_session(&sid, None);
        h.pending_repos.insert(sid.clone(), repo.clone());

        let bound = h.try_url_query_binding(&sid);
        assert_eq!(bound.as_deref(), Some(repo.as_path()));

        // pending entry was consumed (one-shot)
        assert!(!h.pending_repos.contains_key(&sid));

        // session is now bound
        let info = h.session_repos.get(&sid).unwrap().clone();
        assert_eq!(info.repo.as_deref(), Some(repo.as_path()));
    }

    #[tokio::test]
    async fn try_url_query_binding_returns_none_without_pending_entry() {
        let h = test_handler();
        let sid = "session-url-2".to_string();
        h.upsert_session(&sid, None);
        assert!(h.try_url_query_binding(&sid).is_none());
    }

    #[tokio::test]
    async fn session_eviction_loop_drops_stale_entries() {
        let sessions = new_session_repos();
        // Insert a fresh session and a session that's already past the TTL.
        sessions.insert(
            "fresh".to_string(),
            SessionInfo {
                repo: None,
                initialized_at: SystemTime::now(),
                last_seen: Instant::now(),
            },
        );
        sessions.insert(
            "stale".to_string(),
            SessionInfo {
                repo: None,
                initialized_at: SystemTime::now(),
                last_seen: Instant::now()
                    .checked_sub(SESSION_INACTIVITY_TTL + Duration::from_secs(60))
                    .unwrap(),
            },
        );

        // Run one iteration of the loop's eviction body directly.
        let stale: Vec<_> = sessions
            .iter()
            .filter(|e| e.value().last_seen.elapsed() > SESSION_INACTIVITY_TTL)
            .map(|e| e.key().clone())
            .collect();
        for id in stale {
            sessions.remove(&id);
        }

        assert!(sessions.contains_key("fresh"));
        assert!(!sessions.contains_key("stale"));
    }
}

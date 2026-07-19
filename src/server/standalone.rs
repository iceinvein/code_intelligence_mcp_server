//! Standalone mode MCP handler — routes sessions to per-repo AppState

use crate::handlers::{parse_tool_args, AppState};
use crate::path::Utf8PathBuf;
use crate::registry::RepoRegistry;
use crate::server::mcp_proxy::{BoundRepos, PendingRepos};
use crate::server::project_check::{ProjectGate, SkipReason};
use crate::server::{all_tools, dispatch_tool_call, tool_json_content};
use crate::session::{RepoAccess, SessionManager};
use crate::tools::{ApproveIndexingTool, BindWorkspaceTool};
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
    /// Set when an implicit binding source (roots/list or the single-repo
    /// fallback) offered a path that failed the project-marker heuristic in
    /// [`crate::server::project_check`]. Surfaces in the dashboard and short-
    /// circuits further auto-bind attempts so we don't loop on `roots/list`
    /// every tool call. Explicit binds (`bind_workspace`, `?repo=` URL)
    /// clear this field.
    pub bind_skipped_reason: Option<String>,
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
/// lifetime of the process. Also drops the matching `bound_repos` entry so
/// the recovery-cache does not outlive its session.
pub fn spawn_session_eviction_loop(sessions: Sessions, bound_repos: BoundRepos) {
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
                    bound_repos.remove(&id);
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
    /// Durable session-id → repo bindings used by `recover_session` (in the
    /// proxy) to re-apply `bind_workspace`- and `roots/list`-style bindings
    /// after an SDK eviction. Written by every successful binding path here;
    /// dropped in lockstep with `session_repos` by the eviction loop.
    bound_repos: BoundRepos,
}

/// Outcome of resolving a session to its repo.
pub(crate) enum Resolved {
    /// Repo is loaded; proceed with tool dispatch.
    Ready(Arc<AppState>),
    /// Return this structured lifecycle payload instead of dispatching.
    Blocked(serde_json::Value),
}

impl StandaloneHandler {
    pub fn new(
        session_manager: Arc<SessionManager>,
        server_details: InitializeResult,
        session_repos: Sessions,
        pending_repos: PendingRepos,
        bound_repos: BoundRepos,
    ) -> Self {
        Self {
            session_manager,
            server_details,
            session_repos,
            pending_repos,
            bound_repos,
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
        // Explicit URL binding overrides any prior auto-bind skip.
        self.clear_bind_skip(session_id);
        // Cache for recovery so a future SDK eviction can re-apply the
        // same repo even if the client request no longer carries `?repo=`.
        self.bound_repos.insert(session_id.clone(), repo.clone());
        if !was_already_bound {
            tracing::info!(
                session = %session_id,
                repo = %repo,
                "Session bound to repo via ?repo= URL query"
            );
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
                bind_skipped_reason: None,
            });
    }

    /// Record that an automatic bind was skipped because the candidate path
    /// failed the project-marker heuristic. Idempotent and safe to call from
    /// any binding path. Only updates the reason if no explicit repo is set
    /// so a later `bind_workspace` cleanly overrides the skip.
    fn mark_bind_skipped(&self, session_id: &SessionId, reason: String) {
        if let Some(mut info) = self.session_repos.get_mut(session_id) {
            if info.repo.is_none() {
                info.bind_skipped_reason = Some(reason);
            }
        }
    }

    /// Return the previously-recorded skip reason for this session, if the
    /// session is unbound. A bound session is never considered skipped.
    fn bind_skip_reason(&self, session_id: &SessionId) -> Option<String> {
        let info = self.session_repos.get(session_id)?;
        if info.repo.is_some() {
            return None;
        }
        info.bind_skipped_reason.clone()
    }

    /// Clear any prior skip reason for this session. Called whenever an
    /// explicit binding succeeds so the user can recover from an earlier
    /// auto-bind rejection without restarting the session.
    fn clear_bind_skip(&self, session_id: &SessionId) {
        if let Some(mut info) = self.session_repos.get_mut(session_id) {
            info.bind_skipped_reason = None;
        }
    }

    /// Log and remember that an automatic bind was rejected. Ensures the
    /// session row is present (so the dashboard can show the skip), updates
    /// the reason, and emits a warning to the log stream.
    fn record_auto_bind_skip(
        &self,
        session_id: &SessionId,
        repo_path: &crate::path::Utf8Path,
        reason: &SkipReason,
        source: &'static str,
    ) {
        let reason_str = reason.to_string();
        // Make sure the session row exists before we try to mark it.
        self.upsert_session(session_id, None);
        self.mark_bind_skipped(session_id, reason_str.clone());
        tracing::warn!(
            session = %session_id,
            repo = %repo_path,
            source,
            reason = %reason_str,
            "Skipped auto-bind: candidate path does not look like a project. \
             Call bind_workspace explicitly or set ?repo=... if this is intentional.",
        );
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

        // A prior roots/list returned a path that failed the project-marker
        // check. Don't re-issue the request on every tool call; the user can
        // still bind explicitly via `bind_workspace` or `?repo=`, both of
        // which clear the skip when they fire.
        if self.bind_skip_reason(session_id).is_some() {
            return None;
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

        if let Err(reason) = ProjectGate::from_env().check(&repo_path) {
            self.record_auto_bind_skip(session_id, &repo_path, &reason, "roots/list");
            return None;
        }

        // Detect when the session is first promoted so we only log once.
        let was_already_bound = self
            .session_repos
            .get(session_id)
            .map(|i| i.repo.is_some())
            .unwrap_or(false);

        self.upsert_session(session_id, Some(repo_path.clone()));
        self.bound_repos
            .insert(session_id.clone(), repo_path.clone());

        if !was_already_bound {
            tracing::info!(
                session = %session_id,
                repo = %repo_path,
                "Session bound to repo"
            );
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

        let requested_repo_path = Utf8PathBuf::from(tool.repo.as_str());
        if !requested_repo_path.is_absolute() {
            return Err(CallToolError::from_message(format!(
                "bind_workspace.repo must be an absolute path, got: {}",
                tool.repo
            )));
        }
        let repo_path =
            crate::path::canonicalize_existing_dir(&requested_repo_path).map_err(|e| {
                CallToolError::from_message(format!(
                    "bind_workspace.repo does not exist or is not an accessible directory: {e}"
                ))
            })?;

        self.bind_workspace_path(&session_id, repo_path).await
    }

    async fn bind_workspace_path(
        &self,
        session_id: &SessionId,
        repo_path: Utf8PathBuf,
    ) -> Result<serde_json::Value, CallToolError> {
        self.upsert_session(session_id, Some(repo_path.clone()));
        self.bound_repos
            .insert(session_id.clone(), repo_path.clone());
        // Explicit bind overrides any prior auto-bind skip.
        self.clear_bind_skip(session_id);

        tracing::info!(
            session = %session_id,
            repo = %repo_path,
            "Session bound to repo via bind_workspace"
        );

        match self.resolve_repo_path(&repo_path).await? {
            Resolved::Ready(_) => Ok(serde_json::json!({
                "ok": true,
                "status": "ready",
                "session_id": session_id,
                "repo": repo_path.as_str(),
            })),
            Resolved::Blocked(mut payload) => {
                payload["session_id"] = serde_json::json!(session_id);
                Ok(payload)
            }
        }
    }

    /// Apply an approve/decline decision for a concrete repository path.
    async fn approve_indexing_decision(
        &self,
        repo: &str,
        decision: &str,
    ) -> Result<serde_json::Value, CallToolError> {
        if !matches!(decision, "approve" | "decline") {
            return Err(CallToolError::from_message(format!(
                "approve_indexing.decision must be \"approve\" or \"decline\", got: {decision}"
            )));
        }

        let requested = Utf8PathBuf::from(repo);
        let repo_path = crate::path::canonicalize_existing_dir(&requested).map_err(|error| {
            CallToolError::from_message(format!(
                "approve_indexing.repo does not exist or is not an accessible directory: {error}"
            ))
        })?;
        let repo_id = RepoRegistry::path_hash(repo_path.as_str());
        match decision {
            "approve" => {
                let access = self
                    .session_manager
                    .approve_and_start_initial_index(repo_path.as_path())
                    .await
                    .map_err(|error| {
                        CallToolError::from_message(format!("Failed to start indexing: {error}"))
                    })?;
                match access {
                    RepoAccess::Ready(_) => Ok(serde_json::json!({
                        "ok": true,
                        "status": "ready",
                        "repo": repo_path.as_str(),
                        "repo_id": repo_id,
                    })),
                    RepoAccess::Indexing { job, started } => {
                        Ok(crate::server::consent::indexing_payload(&job, started))
                    }
                    RepoAccess::NeedsApproval => {
                        Ok(crate::server::consent::consent_required_payload(
                            repo_path.as_str(),
                            &repo_id,
                        ))
                    }
                    RepoAccess::Declined => Ok(crate::server::consent::declined_payload(
                        repo_path.as_str(),
                        &repo_id,
                    )),
                }
            }
            "decline" => {
                self.session_manager
                    .decline_initial_index(repo_path.as_path())
                    .map_err(|error| {
                        CallToolError::from_message(format!("Failed to record decline: {error}"))
                    })?;
                Ok(serde_json::json!({
                    "ok": true,
                    "status": "declined",
                    "repo": repo_path.as_str(),
                    "repo_id": repo_id,
                }))
            }
            _ => unreachable!(),
        }
    }

    /// Resolve the target repo (explicit arg or session binding), update session
    /// bookkeeping, and apply the decision.
    async fn handle_approve_indexing(
        &self,
        runtime: &Arc<dyn McpServer>,
        tool: ApproveIndexingTool,
    ) -> Result<serde_json::Value, CallToolError> {
        let session_id = runtime.session_id().ok_or_else(|| {
            CallToolError::from_message(
                "No session ID; standalone mode requires Streamable HTTP transport".to_string(),
            )
        })?;

        let repo_path = match tool.repo.as_deref() {
            Some(p) => {
                let path = Utf8PathBuf::from(p);
                if !path.is_absolute() {
                    return Err(CallToolError::from_message(format!(
                        "approve_indexing.repo must be an absolute path, got: {p}"
                    )));
                }
                crate::path::canonicalize_existing_dir(&path).map_err(|error| {
                    CallToolError::from_message(format!(
                        "approve_indexing.repo does not exist or is not an accessible directory: {error}"
                    ))
                })?
            }
            None => self.bound_repo(&session_id).ok_or_else(|| {
                CallToolError::from_message(
                    "No repo bound to this session and no `repo` provided. Pass an absolute `repo` path."
                        .to_string(),
                )
            })?,
        };

        // On approve, bind the session to this repo so subsequent calls resolve
        // to it and clear any prior auto-bind skip.
        if tool.decision == "approve" {
            self.upsert_session(&session_id, Some(repo_path.clone()));
            self.bound_repos
                .insert(session_id.clone(), repo_path.clone());
            self.clear_bind_skip(&session_id);
        }

        let result = self
            .approve_indexing_decision(repo_path.as_str(), &tool.decision)
            .await?;

        tracing::info!(
            session = %session_id,
            repo = %repo_path,
            decision = %tool.decision,
            "approve_indexing handled"
        );
        Ok(result)
    }

    /// Fall back to the single repo registered with the SessionManager when
    /// the registry has exactly one entry. Returns `None` if zero or more
    /// than one repos are registered. This rule disables itself the moment
    /// a second repo is added, so users with multiple workspaces always
    /// hit the explicit `bind_workspace` path instead of a silent wrong-bind.
    async fn try_single_repo_fallback(&self, session_id: &SessionId) -> Option<Utf8PathBuf> {
        if self.bind_skip_reason(session_id).is_some() {
            return None;
        }
        let repos = self.session_manager.registry.list_all().ok()?;
        if repos.len() != 1 {
            return None;
        }
        let path = Utf8PathBuf::from(repos.into_iter().next()?.path);
        if let Err(reason) = ProjectGate::from_env().check(&path) {
            self.record_auto_bind_skip(session_id, &path, &reason, "single_repo_fallback");
            return None;
        }
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

    async fn resolve_repo_path(&self, repo_path: &Utf8PathBuf) -> Result<Resolved, CallToolError> {
        let repo_id = RepoRegistry::path_hash(repo_path.as_str());
        let access = self
            .session_manager
            .resolve_repo(repo_path.as_path())
            .await
            .map_err(|error| {
                CallToolError::from_message(format!("Failed to resolve repository: {error}"))
            })?;

        Ok(match access {
            RepoAccess::Ready(state) => Resolved::Ready(state),
            RepoAccess::NeedsApproval => Resolved::Blocked(
                crate::server::consent::consent_required_payload(repo_path.as_str(), &repo_id),
            ),
            RepoAccess::Declined => Resolved::Blocked(crate::server::consent::declined_payload(
                repo_path.as_str(),
                &repo_id,
            )),
            RepoAccess::Indexing { job, started } => {
                Resolved::Blocked(crate::server::consent::indexing_payload(&job, started))
            }
        })
    }

    /// Resolve the AppState for the current session's repository, binding
    /// lazily if `on_initialized` did not manage to during the race window.
    async fn resolve_state(&self, runtime: &Arc<dyn McpServer>) -> Result<Resolved, CallToolError> {
        let session_id = runtime.session_id().ok_or_else(|| {
            CallToolError::from_message(
                "No session ID; standalone mode requires Streamable HTTP transport".to_string(),
            )
        })?;

        // Every tool call refreshes the inactivity TTL so an active session
        // never gets evicted by `spawn_session_eviction_loop`.
        self.touch_session(&session_id);

        // Binding hierarchy (first match wins). Every source uses the same
        // first-index lifecycle after a path is selected.
        //   1. `?repo=` URL query, captured by the proxy
        //   2. MCP `roots/list`
        //   3. Single-repo fallback when exactly one repo is registered
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

        self.resolve_repo_path(&repo_path).await
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
        let state = match self.resolve_state(&runtime).await? {
            Resolved::Ready(s) => s,
            Resolved::Blocked(payload) => {
                return Err(CallToolError::from_message(payload.to_string()))
            }
        };
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

        // Consent resolution runs before resolve_state because the target repo
        // may not be initialized yet (and it needs the SessionManager).
        if params.name == "approve_indexing" {
            let tool: ApproveIndexingTool = parse_tool_args(&params)?;
            let result = self.handle_approve_indexing(&runtime, tool).await?;
            return Ok(tool_json_content(&result));
        }

        let state = match self.resolve_state(&runtime).await? {
            Resolved::Ready(s) => s,
            Resolved::Blocked(payload) => return Ok(tool_json_content(&payload)),
        };
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
    use crate::path::Utf8Path;
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
            crate::server::mcp_proxy::new_bound_repos(),
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

    fn test_handler_in(data_dir: Utf8PathBuf) -> StandaloneHandler {
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
            crate::server::mcp_proxy::new_bound_repos(),
        )
    }

    fn test_handler() -> StandaloneHandler {
        let temp_dir = tempfile::tempdir().unwrap();
        let data_dir = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap();
        test_handler_in(data_dir)
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

    #[tokio::test]
    async fn url_binding_records_path_without_initializing_repo() {
        let data = tempfile::tempdir().unwrap();
        let data_dir = Utf8PathBuf::from_path_buf(data.path().to_path_buf()).unwrap();
        let h = test_handler_in(data_dir);
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(repo_dir.path().to_path_buf()).unwrap();
        let sid = "explicit-url".to_string();

        h.upsert_session(&sid, None);
        h.pending_repos.insert(sid.clone(), repo.clone());

        let bound = h.try_url_query_binding(&sid);
        assert_eq!(bound.as_deref(), Some(repo.as_path()));

        // pending entry was consumed (one-shot)
        assert!(!h.pending_repos.contains_key(&sid));

        // session is now bound
        let info = h.session_repos.get(&sid).unwrap().clone();
        assert_eq!(info.repo.as_deref(), Some(repo.as_path()));

        let prewarmed = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if h.session_manager.loaded_repo_count() > 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(prewarmed.is_err(), "URL binding must not prewarm the repo");
    }

    #[tokio::test]
    async fn bind_workspace_returns_consent_required_for_new_repo() {
        let data = tempfile::tempdir().unwrap();
        let data_dir = Utf8PathBuf::from_path_buf(data.path().to_path_buf()).unwrap();
        let handler = test_handler_in(data_dir);
        let session_id = "bind-new".to_string();
        let repo = tempfile::tempdir().unwrap();
        let repo_path = Utf8PathBuf::from_path_buf(repo.path().to_path_buf()).unwrap();
        handler.upsert_session(&session_id, None);

        let value = handler
            .bind_workspace_path(&session_id, repo_path)
            .await
            .unwrap();

        assert_eq!(value["status"], "consent_required");
        assert_eq!(handler.session_manager.loaded_repo_count(), 0);
    }

    #[tokio::test]
    async fn approve_indexing_starts_real_job_and_returns_id() {
        let data = tempfile::tempdir().unwrap();
        let data_dir = Utf8PathBuf::from_path_buf(data.path().to_path_buf()).unwrap();
        let handler = test_handler_in(data_dir);
        let repo = tempfile::tempdir().unwrap();
        let repo_path = repo.path().to_str().unwrap();

        let value = handler
            .approve_indexing_decision(repo_path, "approve")
            .await
            .unwrap();

        assert_eq!(value["status"], "indexing_started");
        assert!(value["job_id"].as_str().unwrap().starts_with("initial-"));
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
                bind_skipped_reason: None,
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
                bind_skipped_reason: None,
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

    #[test]
    fn record_auto_bind_skip_persists_reason_on_session() {
        let h = test_handler();
        let sid = "session-skip-1".to_string();
        h.upsert_session(&sid, None);

        let reason = SkipReason::Blocklisted { matched: "/tmp" };
        h.record_auto_bind_skip(&sid, Utf8Path::new("/private/tmp"), &reason, "roots/list");

        let stored = h.bind_skip_reason(&sid).expect("skip recorded");
        assert!(stored.contains("/tmp"));
    }

    #[test]
    fn bind_skip_reason_clears_when_session_becomes_bound() {
        let h = test_handler();
        let sid = "session-skip-2".to_string();
        h.upsert_session(&sid, None);
        let reason = SkipReason::NoProjectMarkers;
        h.record_auto_bind_skip(
            &sid,
            Utf8Path::new("/Users/me/scratch"),
            &reason,
            "roots/list",
        );
        assert!(h.bind_skip_reason(&sid).is_some());

        // Promote to bound — even though the field still holds a value
        // internally, the accessor must return None so the dashboard does
        // not display a stale skip alongside a bound repo.
        h.upsert_session(&sid, Some(Utf8PathBuf::from("/Users/me/scratch/real")));
        assert!(h.bind_skip_reason(&sid).is_none());
    }

    #[test]
    fn clear_bind_skip_drops_stored_reason() {
        let h = test_handler();
        let sid = "session-skip-3".to_string();
        h.upsert_session(&sid, None);
        h.mark_bind_skipped(&sid, "test reason".to_string());
        assert!(h.bind_skip_reason(&sid).is_some());
        h.clear_bind_skip(&sid);
        assert!(h.bind_skip_reason(&sid).is_none());
    }

    #[tokio::test]
    async fn approve_indexing_decline_records_declined_without_indexing() {
        let h = test_handler();
        let dir = tempfile::tempdir().unwrap();
        let requested = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let repo = crate::path::canonicalize_existing_dir(&requested)
            .unwrap()
            .to_string();

        let res = h
            .approve_indexing_decision(&repo, "decline")
            .await
            .expect("decline ok");
        assert_eq!(res["status"], "declined");
        assert_eq!(
            h.session_manager.registry.consent_status(&repo).unwrap(),
            Some(crate::registry::IndexConsent::Declined)
        );
        // No data dir created for a decline.
        let hash = crate::registry::RepoRegistry::path_hash(&repo);
        let entry = h.session_manager.registry.get(&repo).unwrap().unwrap();
        assert_eq!(entry.data_dir.file_name(), Some(hash.as_str()));
        assert!(!entry.data_dir.as_std_path().exists());
    }

    #[tokio::test]
    async fn approve_indexing_rejects_unknown_decision() {
        let h = test_handler();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_str().unwrap().to_string();
        let err = h
            .approve_indexing_decision(&repo, "maybe")
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("approve") || format!("{err}").contains("decline"));
    }
}

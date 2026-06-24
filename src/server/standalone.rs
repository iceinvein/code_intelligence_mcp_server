//! Standalone mode MCP handler — routes sessions to per-repo AppState

use crate::handlers::{parse_tool_args, AppState};
use crate::path::Utf8PathBuf;
use crate::registry::{IndexConsent, RepoRegistry};
use crate::server::mcp_proxy::{BoundRepos, PendingRepos};
use crate::server::project_check::{ProjectGate, SkipReason};
use crate::server::{all_tools, dispatch_tool_call, tool_json_content};
use crate::session::SessionManager;
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
    /// Return this structured JSON payload to the agent instead of dispatching
    /// (status `consent_required` or `declined`).
    Consent(serde_json::Value),
}

/// What the consent gate decides for a resolved (path, source) pair.
#[derive(Debug, PartialEq, Eq)]
enum GateDecision {
    Proceed,
    NeedConsent,
    Declined,
}

/// Pure consent-gate decision. Explicit binds and a disabled gate always
/// proceed; otherwise an implicit bind proceeds only if the repo is already
/// approved, is declined (-> Declined), or has never been seen (-> NeedConsent).
fn consent_decision(
    consent_required: bool,
    explicit: bool,
    status: Option<IndexConsent>,
) -> GateDecision {
    if explicit || !consent_required {
        return GateDecision::Proceed;
    }
    match status {
        Some(IndexConsent::Approved) => GateDecision::Proceed,
        Some(IndexConsent::Declined) => GateDecision::Declined,
        None => GateDecision::NeedConsent,
    }
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

    /// Whether an implicitly-bound repo may be auto-indexed (pre-warmed) without
    /// asking. True when the consent gate is disabled or the repo is already
    /// recorded as approved; false for never-seen or declined repos. A registry
    /// read error is treated as non-approved (returns false), matching the safe
    /// default of deferring indexing until the user consents.
    fn may_auto_index(&self, repo_path: &crate::path::Utf8Path) -> bool {
        if !self
            .session_manager
            .standalone_config
            .index_consent_required
        {
            return true;
        }
        matches!(
            self.session_manager
                .registry
                .consent_status(repo_path.as_str()),
            Ok(Some(IndexConsent::Approved))
        )
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

        // Detect "we just promoted from unbound to bound" so we only log /
        // pre-warm once per session.
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

            if self.may_auto_index(&repo_path) {
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
            } else {
                tracing::info!(
                    session = %session_id,
                    repo = %repo_path,
                    "New repo bound via roots/list; deferring index until user consents"
                );
            }
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

        self.upsert_session(&session_id, Some(repo_path.clone()));
        self.bound_repos
            .insert(session_id.clone(), repo_path.clone());
        // Explicit bind overrides any prior auto-bind skip.
        self.clear_bind_skip(&session_id);

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

    /// Apply an approve/decline decision for a concrete repo path. Returns the
    /// JSON result. `approve` registers + initializes the repo (which starts
    /// indexing); `decline` records the decision without initializing.
    async fn approve_indexing_decision(
        &self,
        repo: &str,
        decision: &str,
    ) -> Result<serde_json::Value, CallToolError> {
        let repo_id = RepoRegistry::path_hash(repo);
        match decision {
            "approve" => {
                let repo_path = Utf8PathBuf::from(repo);
                self.session_manager
                    .get_or_create_repo(&repo_path)
                    .await
                    .map_err(|e| {
                        CallToolError::from_message(format!("Failed to start indexing: {e}"))
                    })?;
                Ok(serde_json::json!({
                    "ok": true,
                    "status": "indexing_started",
                    "repo": repo,
                    "repo_id": repo_id,
                }))
            }
            "decline" => {
                self.session_manager
                    .registry
                    .set_consent(repo, IndexConsent::Declined)
                    .map_err(|e| {
                        CallToolError::from_message(format!("Failed to record decline: {e}"))
                    })?;
                Ok(serde_json::json!({
                    "ok": true,
                    "status": "declined",
                    "repo": repo,
                    "repo_id": repo_id,
                }))
            }
            other => Err(CallToolError::from_message(format!(
                "approve_indexing.decision must be \"approve\" or \"decline\", got: {other}"
            ))),
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
                if !path.is_dir() {
                    return Err(CallToolError::from_message(format!(
                        "approve_indexing.repo does not exist or is not a directory: {path}"
                    )));
                }
                path
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

    /// Resolve the AppState for the current session's repo, binding lazily
    /// if `on_initialized` did not manage to during the race window.
    async fn resolve_state(&self, runtime: &Arc<dyn McpServer>) -> Result<Resolved, CallToolError> {
        let session_id = runtime.session_id().ok_or_else(|| {
            CallToolError::from_message(
                "No session ID — standalone mode requires Streamable HTTP transport".to_string(),
            )
        })?;

        // Every tool call refreshes the inactivity TTL so an active session
        // never gets evicted by `spawn_session_eviction_loop`.
        self.touch_session(&session_id);

        // Binding hierarchy (first match wins). The URL query is the only
        // *explicit* source among these; roots/list and the single-repo
        // fallback are *implicit* and subject to the consent gate.
        //   1. `?repo=` URL query — captured by the proxy, universal client support
        //   2. MCP `roots/list` — opportunistic, Claude Code only in practice
        //   3. Single-repo fallback — only when exactly one repo is registered
        //   4. Hard error with actionable guidance
        let (repo_path, explicit) = if let Some(path) = self.try_url_query_binding(&session_id) {
            (path, true)
        } else if let Some(path) = self.try_bind_session(runtime, &session_id).await {
            (path, false)
        } else if let Some(path) = self.try_single_repo_fallback(&session_id).await {
            (path, false)
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

        let consent_required = self
            .session_manager
            .standalone_config
            .index_consent_required;
        let status = self
            .session_manager
            .registry
            .consent_status(repo_path.as_str())
            .unwrap_or_else(|e| {
                tracing::warn!(
                    repo = %repo_path,
                    error = %e,
                    "Failed to read indexing-consent status; treating repo as pending"
                );
                None
            });

        match consent_decision(consent_required, explicit, status) {
            GateDecision::Proceed => {
                let state = self
                    .session_manager
                    .get_or_create_repo(&repo_path)
                    .await
                    .map_err(|e| {
                        CallToolError::from_message(format!("Failed to load repo: {}", e))
                    })?;
                Ok(Resolved::Ready(state))
            }
            GateDecision::NeedConsent => {
                self.session_manager.record_pending(&repo_path);
                let repo_id = RepoRegistry::path_hash(repo_path.as_str());
                tracing::info!(
                    session = %session_id,
                    repo = %repo_path,
                    "Implicit bind of a new repo; awaiting user consent before indexing"
                );
                Ok(Resolved::Consent(
                    crate::server::consent::consent_required_payload(repo_path.as_str(), &repo_id),
                ))
            }
            GateDecision::Declined => {
                let repo_id = RepoRegistry::path_hash(repo_path.as_str());
                Ok(Resolved::Consent(crate::server::consent::declined_payload(
                    repo_path.as_str(),
                    &repo_id,
                )))
            }
        }
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
            Resolved::Consent(payload) => {
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
            Resolved::Consent(payload) => return Ok(tool_json_content(&payload)),
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
            crate::server::mcp_proxy::new_bound_repos(),
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
        let repo = dir.path().to_str().unwrap().to_string();

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

    #[test]
    fn consent_decision_matrix() {
        use crate::registry::IndexConsent::*;

        // Explicit binds always proceed, regardless of status.
        assert_eq!(consent_decision(true, true, None), GateDecision::Proceed);
        assert_eq!(
            consent_decision(true, true, Some(Declined)),
            GateDecision::Proceed
        );

        // Gate disabled => always proceed.
        assert_eq!(consent_decision(false, false, None), GateDecision::Proceed);

        // Implicit + gate on:
        assert_eq!(
            consent_decision(true, false, Some(Approved)),
            GateDecision::Proceed
        );
        assert_eq!(
            consent_decision(true, false, Some(Declined)),
            GateDecision::Declined
        );
        assert_eq!(
            consent_decision(true, false, None),
            GateDecision::NeedConsent
        );
    }

    #[test]
    fn may_auto_index_only_for_approved_or_disabled_gate() {
        let h = test_handler();
        let repo = Utf8PathBuf::from("/Users/me/new-implicit-repo");

        // Gate on (default), repo never seen -> must NOT auto-index.
        assert!(!h.may_auto_index(&repo));

        // Once approved in the registry -> may auto-index.
        h.session_manager
            .registry
            .set_consent(repo.as_str(), crate::registry::IndexConsent::Approved)
            .unwrap();
        assert!(h.may_auto_index(&repo));

        // Declined -> must NOT auto-index.
        let declined = Utf8PathBuf::from("/Users/me/declined-repo");
        h.session_manager
            .registry
            .set_consent(declined.as_str(), crate::registry::IndexConsent::Declined)
            .unwrap();
        assert!(!h.may_auto_index(&declined));
    }
}

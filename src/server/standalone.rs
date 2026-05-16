//! Standalone mode MCP handler — routes sessions to per-repo AppState

use crate::handlers::{
    handle_explore_cross_repo_dependencies, handle_search_across_repos, parse_tool_args,
    tool_internal_error, AppState,
};
use crate::path::Utf8PathBuf;
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
use std::sync::Arc;

/// SessionId is String (from rust_mcp_transport)
type SessionId = String;

/// Shared map of MCP session-id to bound workspace root. Cloned (cheaply,
/// it is an `Arc`) into both `StandaloneHandler` (which writes) and the
/// JSON API server (which reads for `/api/sessions`).
pub type SessionRepos = Arc<DashMap<SessionId, Utf8PathBuf>>;

pub fn new_session_repos() -> SessionRepos {
    Arc::new(DashMap::new())
}

pub struct StandaloneHandler {
    pub session_manager: Arc<SessionManager>,
    pub server_details: InitializeResult,
    /// Maps session_id → repo path (set during on_initialized via list_roots
    /// or via the bind_workspace tool).
    session_repos: SessionRepos,
}

impl StandaloneHandler {
    pub fn new(
        session_manager: Arc<SessionManager>,
        server_details: InitializeResult,
        session_repos: SessionRepos,
    ) -> Self {
        Self {
            session_manager,
            server_details,
            session_repos,
        }
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
        if let Some(existing) = self.session_repos.get(session_id) {
            return Some(existing.value().clone());
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

        // Atomic insert-if-absent so concurrent tool calls cannot bind twice.
        let entry = self
            .session_repos
            .entry(session_id.clone())
            .or_insert_with(|| repo_path.clone());
        let bound_path = entry.value().clone();
        let we_inserted = bound_path == repo_path;
        drop(entry);

        if we_inserted {
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

        Some(bound_path)
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

        self.session_repos
            .insert(session_id.clone(), repo_path.clone());

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
        self.session_repos
            .entry(session_id.clone())
            .or_insert_with(|| path.clone());
        tracing::info!(
            session = %session_id,
            repo = %path,
            "Session bound to sole registered repo (single-repo fallback)"
        );
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

        let repo_path = if let Some(path) = self.try_bind_session(runtime, &session_id).await {
            path
        } else if let Some(path) = self.try_single_repo_fallback(&session_id).await {
            path
        } else {
            return Err(CallToolError::from_message(
                "Session not bound to a repo. Call \
                 `bind_workspace` with `{ \"repo\": \"/abs/path/to/your/workspace\" }` \
                 first, or use an MCP client that implements the `roots` capability \
                 (currently only Claude Code)."
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
}

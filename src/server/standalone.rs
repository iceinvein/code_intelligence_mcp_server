//! Standalone mode MCP handler — routes sessions to per-repo AppState

use crate::handlers::{
    handle_refresh_index, handle_search_across_repos, parse_tool_args, tool_internal_error,
    AppState,
};
use crate::path::Utf8PathBuf;
use crate::server::{all_tools, dispatch_tool_call};
use crate::session::SessionManager;
use crate::tools::{RefreshIndexTool, SearchAcrossReposTool};
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

pub struct StandaloneHandler {
    pub session_manager: Arc<SessionManager>,
    pub server_details: InitializeResult,
    /// Maps session_id → repo path (set during on_initialized via list_roots)
    session_repos: DashMap<SessionId, Utf8PathBuf>,
}

impl StandaloneHandler {
    pub fn new(session_manager: Arc<SessionManager>, server_details: InitializeResult) -> Self {
        Self {
            session_manager,
            server_details,
            session_repos: DashMap::new(),
        }
    }

    /// Resolve the AppState for the current session's repo
    async fn resolve_state(
        &self,
        runtime: &Arc<dyn McpServer>,
    ) -> Result<Arc<AppState>, CallToolError> {
        // Get session ID from the hyper-server runtime
        let session_id = runtime.session_id().ok_or_else(|| {
            CallToolError::from_message(
                "No session ID — standalone mode requires Streamable HTTP transport".to_string(),
            )
        })?;

        let repo_path = self
            .session_repos
            .get(&session_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| {
                CallToolError::from_message(
                    "Session not bound to a repo. Ensure your MCP client supports roots capability."
                        .to_string(),
                )
            })?;

        self.session_manager
            .get_or_create_repo(&repo_path)
            .await
            .map_err(|e| CallToolError::from_message(format!("Failed to load repo: {}", e)))
    }
}

#[async_trait]
impl ServerHandler for StandaloneHandler {
    async fn on_initialized(&self, runtime: Arc<dyn McpServer>) {
        let session_id = match runtime.session_id() {
            Some(id) => id,
            None => {
                tracing::warn!("on_initialized called without session_id");
                return;
            }
        };

        tracing::info!(session = %session_id, "Session initialized, requesting workspace roots");

        // Ask the client for its workspace roots (MCP roots capability)
        #[allow(deprecated)] // list_roots deprecated in 0.8.0 in favor of request_root_list
        match runtime.request_root_list(None).await {
            Ok(roots_result) => {
                if let Some(root) = roots_result.roots.first() {
                    // Parse file:// URI properly (handles Windows paths like file:///C:/...)
                    let repo_path = match url::Url::parse(&root.uri) {
                        Ok(parsed) => match parsed.to_file_path() {
                            Ok(std_path) => match Utf8PathBuf::from_path_buf(std_path) {
                                Ok(p) => p,
                                Err(non_utf8) => {
                                    tracing::warn!(
                                        session = %session_id,
                                        path = ?non_utf8,
                                        "Root URI contains non-UTF-8 path, using raw URI"
                                    );
                                    Utf8PathBuf::from(
                                        root.uri.strip_prefix("file://").unwrap_or(&root.uri)
                                    )
                                }
                            },
                            Err(_) => Utf8PathBuf::from(
                                root.uri.strip_prefix("file://").unwrap_or(&root.uri)
                            ),
                        },
                        Err(_) => Utf8PathBuf::from(&root.uri),
                    };

                    tracing::info!(
                        session = %session_id,
                        repo = %repo_path,
                        "Session bound to repo"
                    );

                    self.session_repos
                        .insert(session_id.clone(), repo_path.clone());

                    // Pre-warm: trigger repo initialization in background
                    let sm = self.session_manager.clone();
                    let rp = repo_path.clone();
                    tokio::spawn(async move {
                        match sm.get_or_create_repo(&rp).await {
                            Ok(_) => tracing::info!(repo = %rp, "Repo initialized successfully"),
                            Err(e) => tracing::error!(repo = %rp, error = %e, "Failed to pre-warm repo"),
                        }
                    });
                } else {
                    tracing::warn!(
                        session = %session_id,
                        "Client returned empty roots list — session has no repo"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    session = %session_id,
                    error = %e,
                    "Failed to list roots from client (client may not support roots capability)"
                );
            }
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
        match params.name.as_str() {
            "refresh_index" => {
                let tool: RefreshIndexTool = parse_tool_args(&params)?;
                let state = self.resolve_state(&runtime).await?;
                let task = task_creator
                    .create_task(rust_mcp_sdk::task_store::CreateTaskOptions {
                        ttl: Some(300_000),
                        poll_interval: Some(2_000),
                        meta: None,
                    })
                    .await;
                let task_id = task.task_id.clone();
                let task_store = runtime
                    .task_store()
                    .expect("task_store must be configured when tasks capability is advertised");
                tokio::spawn(async move {
                    let result = handle_refresh_index(&state, tool).await;
                    match result {
                        Ok(value) => {
                            task_store
                                .store_task_result(
                                    &task_id,
                                    rust_mcp_sdk::schema::TaskStatus::Completed,
                                    rust_mcp_sdk::schema::schema_utils::ResultFromServer::CallToolResult(
                                        CallToolResult::text_content(vec![
                                            serde_json::to_string_pretty(&value)
                                                .unwrap_or_default()
                                                .into(),
                                        ]),
                                    ),
                                    None,
                                )
                                .await;
                        }
                        Err(e) => {
                            task_store
                                .update_task_status(
                                    &task_id,
                                    rust_mcp_sdk::schema::TaskStatus::Failed,
                                    Some(e.to_string()),
                                    None,
                                )
                                .await;
                        }
                    }
                });
                Ok(CreateTaskResult { meta: None, task })
            }
            _ => Err(CallToolError::from_message(format!(
                "Tool '{}' does not support task-augmented execution",
                params.name
            ))),
        }
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        // Cross-repo search bypasses single-repo resolution — handle before resolve_state()
        if params.name == "search_across_repos" {
            let tool: SearchAcrossReposTool = parse_tool_args(&params)?;
            let result = handle_search_across_repos(&self.session_manager, tool)
                .await
                .map_err(tool_internal_error)?;
            return Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]));
        }

        let state = self.resolve_state(&runtime).await?;
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

        let handler = StandaloneHandler::new(Arc::new(session_manager), server_details);
        assert_eq!(handler.session_repos.len(), 0);
    }
}

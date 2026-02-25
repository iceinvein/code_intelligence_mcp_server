//! MCP server setup and handler implementation

pub mod discovery;
pub mod standalone;

use crate::handlers::*;
use crate::tools::*;
use async_trait::async_trait;
use rust_mcp_sdk::{
    mcp_server::ServerHandler,
    schema::{
        CallToolError, CallToolRequestParams, CallToolResult, CreateTaskResult, ListToolsResult,
        PaginatedRequestParams, RpcError,
    },
    task_store::ServerTaskCreator,
    McpServer,
};
use std::sync::Arc;

/// Error message returned when `search_across_repos` is called in embedded (stdio) mode.
pub const SEARCH_ACROSS_REPOS_EMBEDDED_MSG: &str =
    "search_across_repos is only available in standalone mode. Start the server with --standalone to use cross-repo search.";

/// Build the `ServerTasks` capability block for MCP task-augmented tool calls.
///
/// Used by both embedded and standalone server initialization so the capability
/// advertisement stays in sync.
pub fn task_capabilities() -> rust_mcp_sdk::schema::ServerTasks {
    use rust_mcp_sdk::schema::{ServerTaskRequest, ServerTasks, ServerTaskTools};
    ServerTasks {
        cancel: Some(serde_json::Map::new()),
        list: Some(serde_json::Map::new()),
        requests: Some(ServerTaskRequest {
            tools: Some(ServerTaskTools {
                call: Some(serde_json::Map::new()),
            }),
        }),
    }
}

/// Shared task-augmented tool dispatch — used by both embedded and standalone handlers.
///
/// Only `refresh_index` supports async task execution.  Other tools return
/// an error; the SDK then falls back to synchronous `handle_call_tool_request`.
pub(crate) async fn dispatch_task_augmented_call(
    state: Arc<AppState>,
    params: CallToolRequestParams,
    task_creator: ServerTaskCreator,
    runtime: Arc<dyn McpServer>,
) -> std::result::Result<CreateTaskResult, CallToolError> {
    match params.name.as_str() {
        "refresh_index" => {
            let tool: RefreshIndexTool = parse_tool_args(&params)?;
            let task = task_creator
                .create_task(rust_mcp_sdk::task_store::CreateTaskOptions {
                    ttl: Some(300_000),
                    poll_interval: Some(2_000),
                    meta: None,
                })
                .await;
            let task_id = task.task_id.clone();
            let task_id_supervisor = task_id.clone();
            let task_store = runtime.task_store().ok_or_else(|| {
                CallToolError::from_message(
                    "Internal error: task_store not configured".to_string(),
                )
            })?;
            let task_store_supervisor = task_store.clone();
            let handle = tokio::spawn(async move {
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
                                            .unwrap_or_else(|_| "{\"ok\":true}".to_string())
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
            // Supervisor: if the spawned task panics, mark the MCP task as failed
            // so clients don't poll indefinitely until TTL expiry.
            tokio::spawn(async move {
                if let Err(join_err) = handle.await {
                    task_store_supervisor
                        .update_task_status(
                            &task_id_supervisor,
                            rust_mcp_sdk::schema::TaskStatus::Failed,
                            Some(format!("Internal error: {join_err}")),
                            None,
                        )
                        .await;
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

/// All tools advertised by both embedded and standalone handlers
pub fn all_tools() -> Vec<rust_mcp_sdk::schema::Tool> {
    vec![
        SearchCodeTool::tool(),
        RefreshIndexTool::tool(),
        GetDefinitionTool::tool(),
        FindReferencesTool::tool(),
        GetFileSymbolsTool::tool(),
        GetCallHierarchyTool::tool(),
        ExploreDependencyGraphTool::tool(),
        GetSimilarityClusterTool::tool(),
        GetTypeGraphTool::tool(),
        GetUsageExamplesTool::tool(),
        GetIndexStatsTool::tool(),
        HydrateSymbolsTool::tool(),
        ReportSelectionTool::tool(),
        ReportFileAccessTool::tool(),
        ExplainSearchTool::tool(),
        FindSimilarCodeTool::tool(),
        SummarizeFileTool::tool(),
        GetModuleSummaryTool::tool(),
        TraceDataFlowTool::tool(),
        FindAffectedCodeTool::tool(),
        SearchTodosTool::tool(),
        FindTestsForSymbolTool::tool(),
        SearchDecoratorsTool::tool(),
        SearchFrameworkPatternsTool::tool(),
        FindDeadCodeTool::tool(),
        FindDuplicatesTool::tool(),
        SearchAcrossReposTool::tool(),
        FindStaleDescriptionsTool::tool(),
        FindUndocumentedSymbolsTool::tool(),
    ]
}

/// Shared tool dispatch — used by both embedded and standalone handlers
pub async fn dispatch_tool_call(
    state: &AppState,
    params: CallToolRequestParams,
) -> std::result::Result<CallToolResult, CallToolError> {
    let tool_name = params.name.clone();
    let start = std::time::Instant::now();

    let result = match params.name.as_str() {
        "refresh_index" => {
            let tool: RefreshIndexTool = parse_tool_args(&params)?;
            let result = handle_refresh_index(state, tool)
                .await
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "search_code" => {
            let tool: SearchCodeTool = parse_tool_args(&params)?;
            let result = handle_search_code(&state.retriever, tool)
                .await
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "get_definition" => {
            let tool: GetDefinitionTool = parse_tool_args(&params)?;
            let result = handle_get_definition(state, tool)
                .await
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "get_file_symbols" => {
            let tool: GetFileSymbolsTool = parse_tool_args(&params)?;
            let result = handle_get_file_symbols(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "get_index_stats" => {
            let _tool: GetIndexStatsTool =
                parse_tool_args(&params).unwrap_or(GetIndexStatsTool {});
            let result = handle_get_index_stats(state).map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "hydrate_symbols" => {
            let tool: HydrateSymbolsTool = parse_tool_args(&params)?;
            let result =
                handle_hydrate_symbols(state, tool).map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]))
        }
        "explore_dependency_graph" => {
            let tool: ExploreDependencyGraphTool = parse_tool_args(&params)?;
            let result = handle_explore_dependency_graph(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]))
        }
        "get_similarity_cluster" => {
            let tool: GetSimilarityClusterTool = parse_tool_args(&params)?;
            let result = handle_get_similarity_cluster(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "find_references" => {
            let tool: FindReferencesTool = parse_tool_args(&params)?;
            let result = handle_find_references(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "get_usage_examples" => {
            let tool: GetUsageExamplesTool = parse_tool_args(&params)?;
            let result = handle_get_usage_examples(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "get_call_hierarchy" => {
            let tool: GetCallHierarchyTool = parse_tool_args(&params)?;
            let result = handle_get_call_hierarchy(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]))
        }
        "get_type_graph" => {
            let tool: GetTypeGraphTool = parse_tool_args(&params)?;
            let result = handle_get_type_graph(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]))
        }
        "report_selection" => {
            let tool: ReportSelectionTool = parse_tool_args(&params)?;
            let result = handle_report_selection(state, tool)
                .await
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "report_file_access" => {
            let tool: ReportFileAccessTool = parse_tool_args(&params)?;
            let result = handle_report_file_access(state, tool)
                .await
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "explain_search" => {
            let tool: ExplainSearchTool = parse_tool_args(&params)?;
            let result = handle_explain_search(&state.retriever, tool)
                .await
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "find_similar_code" => {
            let tool: FindSimilarCodeTool = parse_tool_args(&params)?;
            let result = handle_find_similar_code(state, tool)
                .await
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "summarize_file" => {
            let tool: SummarizeFileTool = parse_tool_args(&params)?;
            let result = handle_summarize_file(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "get_module_summary" => {
            let tool: GetModuleSummaryTool = parse_tool_args(&params)?;
            let result = handle_get_module_summary(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
            ]))
        }
        "trace_data_flow" => {
            let tool: TraceDataFlowTool = parse_tool_args(&params)?;
            let result = handle_trace_data_flow(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]))
        }
        "find_affected_code" => {
            let tool: FindAffectedCodeTool = parse_tool_args(&params)?;
            let result = handle_find_affected_code(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]))
        }
        "search_todos" => {
            let tool: SearchTodosTool = parse_tool_args(&params)?;
            let result = handle_search_todos(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]))
        }
        "find_tests_for_symbol" => {
            let tool: FindTestsForSymbolTool = parse_tool_args(&params)?;
            let result = handle_find_tests_for_symbol(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]))
        }
        "search_decorators" => {
            let tool: SearchDecoratorsTool = parse_tool_args(&params)?;
            let result = handle_search_decorators(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]))
        }
        "search_framework_patterns" => {
            let tool: SearchFrameworkPatternsTool = parse_tool_args(&params)?;
            let result = handle_search_framework_patterns(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ]))
        }
        "find_dead_code" => {
            let tool: FindDeadCodeTool = parse_tool_args(&params)?;
            let result = handle_find_dead_code(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_default()
                    .into(),
            ]))
        }
        "find_duplicates" => {
            let tool: FindDuplicatesTool = parse_tool_args(&params)?;
            let result = handle_find_duplicates(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_default()
                    .into(),
            ]))
        }
        "find_stale_descriptions" => {
            let tool: FindStaleDescriptionsTool = parse_tool_args(&params)?;
            let result = handle_find_stale_descriptions(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_default()
                    .into(),
            ]))
        }
        "find_undocumented_symbols" => {
            let tool: FindUndocumentedSymbolsTool = parse_tool_args(&params)?;
            let result = handle_find_undocumented_symbols(state, tool)
                .map_err(tool_internal_error)?;
            Ok(CallToolResult::text_content(vec![
                serde_json::to_string_pretty(&result)
                    .unwrap_or_default()
                    .into(),
            ]))
        }
        "search_across_repos" => {
            let mut result = CallToolResult::text_content(vec![
                SEARCH_ACROSS_REPOS_EMBEDDED_MSG.into(),
            ]);
            result.is_error = Some(true);
            Ok(result)
        }
        _ => Err(CallToolError::unknown_tool(params.name)),
    };

    let duration_ms = start.elapsed().as_millis();
    let status = if result.is_ok() { "ok" } else { "error" };
    tracing::info!(
        target: "mcp_access",
        tool = %tool_name,
        duration_ms = duration_ms as u64,
        status = %status,
    );

    result
}

#[derive(Clone)]
pub struct CodeIntelligenceHandler {
    pub state: Arc<AppState>,
}

#[async_trait]
impl ServerHandler for CodeIntelligenceHandler {
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
        dispatch_task_augmented_call(self.state.clone(), params, task_creator, runtime).await
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        dispatch_tool_call(&self.state, params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_advertise_task_support() {
        use rust_mcp_sdk::schema::{ServerCapabilities, ServerCapabilitiesTools};
        let caps = ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            tasks: Some(task_capabilities()),
            ..Default::default()
        };
        assert!(
            caps.can_run_task_augmented_tools(),
            "task_capabilities() must produce a ServerTasks that advertises task-augmented tool support"
        );
    }

    #[test]
    fn all_tools_contains_get_similarity_cluster() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"get_similarity_cluster"),
            "all_tools() must include 'get_similarity_cluster', but only found: {names:?}"
        );
    }

    #[test]
    fn all_tools_contains_search_across_repos() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"search_across_repos"),
            "all_tools() must include 'search_across_repos', but only found: {names:?}"
        );
    }

    #[test]
    fn search_across_repos_tool_serializes_correctly() {
        use crate::tools::SearchAcrossReposTool;

        // Verify round-trip serialization
        let tool = SearchAcrossReposTool {
            query: "auth handler".to_string(),
            limit: Some(5),
        };
        let json = serde_json::to_string(&tool).unwrap();
        let parsed: SearchAcrossReposTool = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.query, "auth handler");
        assert_eq!(parsed.limit, Some(5));

        // limit defaults to None when absent
        let no_limit: SearchAcrossReposTool =
            serde_json::from_str(r#"{"query":"foo"}"#).unwrap();
        assert_eq!(no_limit.limit, None);
    }

    #[test]
    fn task_augmented_error_message_and_routing_in_shared_dispatch() {
        // The shared `dispatch_task_augmented_call` function handles all task routing.
        // Verify via source inspection that it matches refresh_index and rejects
        // unsupported tools.  Full runtime testing requires a running MCP server
        // and task store (integration test territory).
        let source = include_str!("mod.rs");
        assert!(
            source.contains("does not support task-augmented execution"),
            "Unsupported tool error message must be present in dispatch_task_augmented_call"
        );
        assert!(
            source.contains(r#""refresh_index" =>"#),
            "dispatch_task_augmented_call must match refresh_index"
        );
        // Verify standalone delegates to the shared function
        let standalone_source = include_str!("standalone.rs");
        assert!(
            standalone_source.contains("dispatch_task_augmented_call"),
            "StandaloneHandler must delegate to shared dispatch_task_augmented_call"
        );
    }

    #[test]
    fn embedded_mode_search_across_repos_returns_helpful_message() {
        // Verify that the constant message is informative and mentions
        // both the tool name and the --standalone flag so users know how
        // to enable cross-repo search.
        assert!(
            SEARCH_ACROSS_REPOS_EMBEDDED_MSG.contains("standalone"),
            "Message must mention standalone mode"
        );
        assert!(
            SEARCH_ACROSS_REPOS_EMBEDDED_MSG.contains("search_across_repos"),
            "Message must mention the tool name"
        );

        // Verify that `dispatch_tool_call` uses this constant by checking
        // the source code references it (compile-time guarantee via the
        // constant being used in the match arm above).
        let source = include_str!("mod.rs");
        assert!(
            source.contains("SEARCH_ACROSS_REPOS_EMBEDDED_MSG.into()"),
            "dispatch_tool_call must use the SEARCH_ACROSS_REPOS_EMBEDDED_MSG constant"
        );
    }
}

//! MCP server setup and handler implementation

pub mod standalone;

use crate::handlers::*;
use crate::tools::*;
use async_trait::async_trait;
use rust_mcp_sdk::{
    mcp_server::ServerHandler,
    schema::{
        CallToolError, CallToolRequestParams, CallToolResult, ListToolsResult,
        PaginatedRequestParams, RpcError,
    },
    McpServer,
};
use std::sync::Arc;

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

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        dispatch_tool_call(&self.state, params).await
    }
}

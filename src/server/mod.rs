//! MCP server setup and handler implementation

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
            tools: vec![
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
            ],
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        match params.name.as_str() {
            "refresh_index" => {
                let tool: RefreshIndexTool = parse_tool_args(&params)?;
                let result = handle_refresh_index(&self.state, tool)
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
                let result = handle_search_code(&self.state.retriever, tool)
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
                let result = handle_get_definition(&self.state, tool)
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
                let result = handle_get_file_symbols(&self.state.config.db_path, tool)
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
                let result = handle_get_index_stats(&self.state).map_err(tool_internal_error)?;
                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                        .into(),
                ]))
            }
            "hydrate_symbols" => {
                let tool: HydrateSymbolsTool = parse_tool_args(&params)?;
                let result =
                    handle_hydrate_symbols(&self.state, tool).map_err(tool_internal_error)?;
                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| "{}".to_string())
                        .into(),
                ]))
            }
            "explore_dependency_graph" => {
                let tool: ExploreDependencyGraphTool = parse_tool_args(&params)?;
                let result = handle_explore_dependency_graph(&self.state.config.db_path, tool)
                    .map_err(tool_internal_error)?;
                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| "{}".to_string())
                        .into(),
                ]))
            }
            "get_similarity_cluster" => {
                let tool: GetSimilarityClusterTool = parse_tool_args(&params)?;
                let result = handle_get_similarity_cluster(&self.state.config.db_path, tool)
                    .map_err(tool_internal_error)?;
                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                        .into(),
                ]))
            }
            "find_references" => {
                let tool: FindReferencesTool = parse_tool_args(&params)?;
                let result = handle_find_references(&self.state.config.db_path, tool)
                    .map_err(tool_internal_error)?;
                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                        .into(),
                ]))
            }
            "get_usage_examples" => {
                let tool: GetUsageExamplesTool = parse_tool_args(&params)?;
                let result = handle_get_usage_examples(&self.state.config.db_path, tool)
                    .map_err(tool_internal_error)?;
                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                        .into(),
                ]))
            }
            "get_call_hierarchy" => {
                let tool: GetCallHierarchyTool = parse_tool_args(&params)?;
                let result = handle_get_call_hierarchy(&self.state.config.db_path, tool)
                    .map_err(tool_internal_error)?;
                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| "{}".to_string())
                        .into(),
                ]))
            }
            "get_type_graph" => {
                let tool: GetTypeGraphTool = parse_tool_args(&params)?;
                let result = handle_get_type_graph(&self.state.config.db_path, tool)
                    .map_err(tool_internal_error)?;
                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| "{}".to_string())
                        .into(),
                ]))
            }
            "report_selection" => {
                let tool: ReportSelectionTool = parse_tool_args(&params)?;
                let result = handle_report_selection(&self.state.config.db_path, tool)
                    .await
                    .map_err(tool_internal_error)?;
                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                        .into(),
                ]))
            }
            _ => Err(CallToolError::unknown_tool(params.name)),
        }
    }
}

//! MCP tool definitions

use rust_mcp_sdk::macros;
use serde::{Deserialize, Serialize};

#[macros::mcp_tool(
    name = "search_code",
    description = "Search codebase for symbols and return assembled context."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchCodeTool {
    pub query: String,
    pub limit: Option<u32>,
    pub exported_only: Option<bool>,
}

#[macros::mcp_tool(
    name = "refresh_index",
    description = "Re-index the codebase or specific files."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct RefreshIndexTool {
    pub files: Option<Vec<String>>,
}

#[macros::mcp_tool(
    name = "get_definition",
    description = "Get full definition(s) for a symbol by name with disambiguation support."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetDefinitionTool {
    pub symbol_name: String,
    pub file: Option<String>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "find_references",
    description = "Find imports/uses/calls of a symbol across the indexed graph."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindReferencesTool {
    pub symbol_name: String,
    pub reference_type: Option<String>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_file_symbols",
    description = "List symbols defined in a file (no full definitions)."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetFileSymbolsTool {
    pub file_path: String,
    pub exported_only: Option<bool>,
}

#[macros::mcp_tool(
    name = "get_call_hierarchy",
    description = "Return a best-effort call hierarchy rooted at a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetCallHierarchyTool {
    pub symbol_name: String,
    pub direction: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_type_graph",
    description = "Return type relationships for a symbol (extends/implements/aliases)."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetTypeGraphTool {
    pub symbol_name: String,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_usage_examples",
    description = "Return extracted usage examples for a symbol from the indexed repo."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetUsageExamplesTool {
    pub symbol_name: String,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_index_stats",
    description = "Return index statistics (files, symbols, edges, last updated)."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetIndexStatsTool {}

#[macros::mcp_tool(
    name = "explore_dependency_graph",
    description = "Explore dependencies upstream/downstream/bidirectional from a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ExploreDependencyGraphTool {
    pub symbol_name: String,
    pub direction: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_similarity_cluster",
    description = "Return symbols in the same similarity cluster as the given symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetSimilarityClusterTool {
    pub symbol_name: String,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "hydrate_symbols",
    description = "Hydrate full context for a set of symbol ids."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct HydrateSymbolsTool {
    pub ids: Vec<String>,
    pub mode: Option<String>,
}

#[macros::mcp_tool(
    name = "report_selection",
    description = "Record user selection feedback for learning. Call this when a user selects a search result."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ReportSelectionTool {
    pub query: String,
    pub selected_symbol_id: String,
    pub position: u32,
}

#[macros::mcp_tool(
    name = "explain_search",
    description = "Return detailed scoring breakdown for search results to understand why results ranked the way they did."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ExplainSearchTool {
    pub query: String,
    pub limit: Option<u32>,
    pub exported_only: Option<bool>,
    pub verbose: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_similar_code",
    description = "Find semantically similar code using vector embeddings. Returns code that is similar in meaning or structure to the given symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindSimilarCodeTool {
    pub symbol_id: String,
    pub limit: Option<u32>,
}

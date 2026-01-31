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
    description = "Get full definition(s) for a symbol by name. When multiple symbols share the same name, use the 'file' parameter to disambiguate (e.g., file: \"src/auth.ts\")."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetDefinitionTool {
    /// The symbol name to look up
    pub symbol_name: String,
    /// Optional file path to disambiguate when multiple symbols share the same name
    pub file: Option<String>,
    /// Maximum number of definitions to return (default: 10)
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "find_references",
    description = "Find imports/uses/calls of a symbol across the indexed graph. When multiple symbols share the same name, use 'file' parameter to disambiguate."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindReferencesTool {
    /// The symbol name to find references for
    pub symbol_name: String,
    /// Optional file path to disambiguate when multiple symbols share the same name
    pub file: Option<String>,
    /// Filter by reference type: "call", "import", "reference", "extends", "implements", or "all" (default)
    pub reference_type: Option<String>,
    /// Maximum number of references to return (default: 200)
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
    description = "Find code semantically similar to a given symbol or code snippet."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindSimilarCodeTool {
    pub symbol_name: Option<String>,
    pub code_snippet: Option<String>,
    pub file_path: Option<String>,
    pub limit: Option<u32>,
    pub threshold: Option<f32>,
}

#[macros::mcp_tool(
    name = "trace_data_flow",
    description = "Trace variable reads and writes through the codebase to understand data flow."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct TraceDataFlowTool {
    pub symbol_name: String,
    pub file_path: Option<String>,
    pub direction: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "summarize_file",
    description = "Generate a summary of file contents including symbol counts, structure overview, and key exports."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SummarizeFileTool {
    pub file_path: String,
    pub include_signatures: Option<bool>,
    pub verbose: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_affected_code",
    description = "Find code that would be affected if the given symbol changes (reverse dependencies)."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindAffectedCodeTool {
    pub symbol_name: String,
    pub file_path: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
    pub include_tests: Option<bool>,
}

#[macros::mcp_tool(
    name = "get_module_summary",
    description = "List all exported symbols from a module/file with their signatures."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetModuleSummaryTool {
    pub file_path: String,
    pub group_by_kind: Option<bool>,
}

#[macros::mcp_tool(
    name = "search_todos",
    description = "Search for TODO and FIXME comments in the codebase to track technical debt."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchTodosTool {
    /// Keyword to search for in TODO text (e.g., 'auth', 'parser', 'refactor')
    pub query: Option<String>,
    /// Filter to specific file path
    pub file_path: Option<String>,
    /// Filter to specific TODO kind: 'todo', 'fixme', or None for both
    pub kind: Option<String>,
    /// Maximum number of results to return
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "find_tests_for_symbol",
    description = "Find test files that test a given symbol or source file. Returns test file paths and associated symbols."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindTestsForSymbolTool {
    /// Symbol name to find tests for
    pub symbol_name: String,
    /// Optional file path to disambiguate symbols
    pub file_path: Option<String>,
    /// Maximum number of test files to return
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "search_decorators",
    description = "Search for TypeScript/JavaScript decorators in the codebase (e.g., @Component, @Controller, @Injectable, @Get, @Post). Returns decorator metadata including symbol ID, decorator name, arguments, and location."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchDecoratorsTool {
    /// Decorator name to search for (e.g., 'Component', 'Controller', 'Get'). Exact or prefix match.
    pub name: Option<String>,
    /// Filter by decorator type (e.g., 'component', 'injectable', 'controller', 'get')
    pub decorator_type: Option<String>,
    /// Maximum number of results to return (default: 50)
    pub limit: Option<u32>,
}

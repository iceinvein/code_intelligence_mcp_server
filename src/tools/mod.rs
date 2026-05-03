//! MCP tool definitions

use rust_mcp_sdk::macros;
use serde::{Deserialize, Serialize};

#[macros::mcp_tool(
    name = "search_code",
    description = "Search indexed code. Returns ranked hits; pass context=\"snippets\" or \"full\" to also receive source code."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchCodeTool {
    pub query: String,
    pub limit: Option<u32>,
    pub exported_only: Option<bool>,
    /// none (default), snippets (compact per-hit), or full (legacy markdown bundle).
    pub context: Option<String>,
}

#[macros::mcp_tool(
    name = "refresh_index",
    description = "Re-index the codebase or selected files."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct RefreshIndexTool {
    pub files: Option<Vec<String>>,
}

#[macros::mcp_tool(
    name = "get_definition",
    description = "Get definition context for a symbol. Use file to disambiguate duplicate names."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetDefinitionTool {
    pub symbol_name: String,
    /// Disambiguating file path.
    pub file: Option<String>,
    /// Default 10.
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "find_references",
    description = "Find imports, uses, or calls of a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindReferencesTool {
    pub symbol_name: String,
    /// Disambiguating file path.
    pub file: Option<String>,
    /// call, import, reference, extends, implements, or all.
    pub reference_type: Option<String>,
    /// Default 200.
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
    description = "Return callers, callees, or both for a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetCallHierarchyTool {
    pub symbol_name: String,
    /// callees, callers, or both.
    pub direction: Option<String>,
    /// Default 3, max 10.
    pub depth: Option<u32>,
    /// Default 50.
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_type_graph",
    description = "Return type relationships for a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetTypeGraphTool {
    pub symbol_name: String,
    /// downstream, upstream, or both.
    pub direction: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_usage_examples",
    description = "Return indexed usage examples for a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetUsageExamplesTool {
    pub symbol_name: String,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_index_stats",
    description = "Return index statistics and description coverage."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetIndexStatsTool {}

#[macros::mcp_tool(
    name = "explore_dependency_graph",
    description = "Explore upstream or downstream dependencies from a symbol."
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
    description = "Return symbols in the same semantic cluster."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetSimilarityClusterTool {
    pub symbol_name: String,
    /// Default 20.
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "hydrate_symbols",
    description = "Return source context for symbol IDs from other tools."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct HydrateSymbolsTool {
    /// Symbol IDs.
    pub ids: Vec<String>,
    /// full for surrounding context.
    pub mode: Option<String>,
    /// Include per-symbol metadata (id, role, tokens, reasons).
    pub verbose: Option<bool>,
}

#[macros::mcp_tool(
    name = "report_selection",
    description = "Record selected search result feedback."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ReportSelectionTool {
    pub query: String,
    pub selected_symbol_id: String,
    pub position: u32,
}

#[macros::mcp_tool(
    name = "report_file_access",
    description = "Record viewed or edited files for ranking feedback."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ReportFileAccessTool {
    /// Repo-relative path.
    pub file_path: String,
    /// view or edit.
    pub action: Option<String>,
}

#[macros::mcp_tool(
    name = "explain_search",
    description = "Explain ranking scores for a search query."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ExplainSearchTool {
    pub query: String,
    pub limit: Option<u32>,
    pub exported_only: Option<bool>,
    /// Include per-signal details.
    pub verbose: Option<bool>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_similar_code",
    description = "Find code similar to a symbol or snippet."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindSimilarCodeTool {
    pub symbol_name: Option<String>,
    pub code_snippet: Option<String>,
    pub file_path: Option<String>,
    pub limit: Option<u32>,
    pub threshold: Option<f32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "trace_data_flow",
    description = "Trace reads and writes related to a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct TraceDataFlowTool {
    pub symbol_name: String,
    pub file_path: Option<String>,
    /// forward, backward, or both.
    pub direction: Option<String>,
    /// Default 3, max 10.
    pub depth: Option<u32>,
    pub limit: Option<u32>,
    /// Expand into called functions.
    pub inter_procedural: Option<bool>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "summarize_file",
    description = "Summarize an indexed file at symbol level."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SummarizeFileTool {
    /// Repo-relative file path.
    pub file_path: String,
    /// Include signatures.
    pub include_signatures: Option<bool>,
    /// Include extra details.
    pub verbose: Option<bool>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_affected_code",
    description = "Find reverse dependencies affected by a symbol change."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindAffectedCodeTool {
    pub symbol_name: String,
    pub file_path: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
    pub include_tests: Option<bool>,
    /// Edge type filter.
    pub edge_types: Option<Vec<String>>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "get_module_summary",
    description = "List exported symbols from a module or file."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetModuleSummaryTool {
    pub file_path: String,
    pub group_by_kind: Option<bool>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "search_todos",
    description = "Search indexed TODO and FIXME comments."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchTodosTool {
    /// TODO text keyword.
    pub query: Option<String>,
    /// File path filter.
    pub file_path: Option<String>,
    /// todo or fixme.
    pub kind: Option<String>,
    pub limit: Option<u32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_tests_for_symbol",
    description = "Find tests associated with a symbol or source file."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindTestsForSymbolTool {
    pub symbol_name: String,
    /// Disambiguating file path.
    pub file_path: Option<String>,
    pub limit: Option<u32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "search_decorators",
    description = "Search TypeScript or JavaScript decorators."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchDecoratorsTool {
    /// Decorator name.
    pub name: Option<String>,
    /// Decorator type.
    pub decorator_type: Option<String>,
    /// Default 50.
    pub limit: Option<u32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "search_framework_patterns",
    description = "Search indexed framework patterns such as routes."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchFrameworkPatternsTool {
    /// Framework filter.
    pub framework: Option<String>,
    /// Pattern kind.
    pub kind: Option<String>,
    /// HTTP method.
    pub http_method: Option<String>,
    /// Route path.
    pub path: Option<String>,
    /// Default 50.
    pub limit: Option<u32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_dead_code",
    description = "Find symbols with no incoming references."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindDeadCodeTool {
    /// File path filter.
    pub file_path: Option<String>,
    /// Language filter.
    pub language: Option<String>,
    /// Symbol kind filter.
    pub kind: Option<String>,
    /// Include tests.
    pub include_tests: Option<bool>,
    /// Default 50.
    pub limit: Option<u32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_duplicates",
    description = "Find likely duplicate symbols by semantic cluster."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindDuplicatesTool {
    /// File path filter.
    pub file_path: Option<String>,
    /// Symbol kind filter.
    pub kind: Option<String>,
    /// Default 50.
    pub limit: Option<u32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "search_across_repos",
    description = "Search all indexed repositories. Standalone only."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchAcrossReposTool {
    pub query: String,
    /// Default 10.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "explore_cross_repo_dependencies",
    description = "Explore cross-repo dependencies for a symbol. Standalone only."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ExploreCrossRepoDependenciesTool {
    pub symbol_name: String,
    /// Disambiguating file path.
    pub file_path: Option<String>,
    /// downstream, upstream, or both.
    pub direction: Option<String>,
    /// Default 20.
    pub limit: Option<u32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_stale_descriptions",
    description = "Find cached symbol descriptions whose source changed."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindStaleDescriptionsTool {
    /// File path filter.
    pub file_path: Option<String>,
    /// Default 100.
    pub limit: Option<u32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_undocumented_symbols",
    description = "Find symbols without generated descriptions."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindUndocumentedSymbolsTool {
    /// File path filter.
    pub file_path: Option<String>,
    /// Default 3.
    pub min_lines: Option<u32>,
    /// Exported only.
    pub exported_only: Option<bool>,
    /// Default 100.
    pub limit: Option<u32>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "predict_impact",
    description = "Predict change impact using dependencies and git co-change."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct PredictImpactTool {
    pub symbol_name: String,
    /// Disambiguating file path.
    pub file_path: Option<String>,
    /// Default 20.
    pub limit: Option<u32>,
    /// Include tests.
    pub include_tests: Option<bool>,
    /// Include markdown summary.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "get_context_bundle",
    description = "Build one compact context bundle for a task."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetContextBundleTool {
    /// Task description.
    pub task: String,
    /// Context token budget.
    pub max_tokens: Option<u32>,
    /// definitions, call_chain, tests, similar, affected.
    pub sections: Option<Vec<String>>,
    /// Default 3.
    pub seed_limit: Option<u32>,
    /// Include raw section outputs.
    pub include_raw_sections: Option<bool>,
}

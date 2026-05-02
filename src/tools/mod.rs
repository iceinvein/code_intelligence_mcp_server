//! MCP tool definitions

use rust_mcp_sdk::macros;
use serde::{Deserialize, Serialize};

#[macros::mcp_tool(
    name = "search_code",
    description = "Search indexed code and return ranked hits with assembled context."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchCodeTool {
    pub query: String,
    pub limit: Option<u32>,
    pub exported_only: Option<bool>,
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
    /// The symbol name to look up
    pub symbol_name: String,
    /// Optional file path to disambiguate when multiple symbols share the same name
    pub file: Option<String>,
    /// Maximum number of definitions to return (default: 10)
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "find_references",
    description = "Find imports, uses, or calls of a symbol."
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
    description = "Return callers, callees, or both for a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetCallHierarchyTool {
    pub symbol_name: String,
    /// Direction: \"callees\" (what this calls), \"callers\" (what calls this), or \"both\" (default)
    pub direction: Option<String>,
    /// Traversal depth (default: 3, max: 10)
    pub depth: Option<u32>,
    /// Maximum number of nodes to return (default: 50)
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_type_graph",
    description = "Return type relationships for a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetTypeGraphTool {
    pub symbol_name: String,
    /// Direction of traversal: "downstream" (what does this extend/implement), "upstream" (who extends/implements this), or "both" (default)
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
    /// Maximum number of cluster members to return (default: 20)
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "hydrate_symbols",
    description = "Return source context for symbol IDs from other tools."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct HydrateSymbolsTool {
    /// Symbol IDs to hydrate (from search_code, find_references, etc.)
    pub ids: Vec<String>,
    /// Output mode: \"full\" includes surrounding context (callers, types), omit for code-only (default)
    pub mode: Option<String>,
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
    /// The file path being accessed (relative to repo root)
    pub file_path: String,
    /// Access type: "view" (default) or "edit"
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
    /// When true, includes per-signal breakdown (term_coverage, popularity_boost, definition_bias, symbol_importance, test_penalty) for each result
    pub verbose: Option<bool>,
    /// Include markdown display summary. Default false.
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
    /// Include markdown display summary. Default false.
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
    /// Direction: \"forward\" (where data flows to), \"backward\" (where data comes from), or \"both\" (default)
    pub direction: Option<String>,
    /// Graph traversal depth (default: 3, max: 10)
    pub depth: Option<u32>,
    pub limit: Option<u32>,
    /// Expand 1 level into called functions to trace cross-function data flow (default: false)
    pub inter_procedural: Option<bool>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "summarize_file",
    description = "Summarize an indexed file at symbol level."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SummarizeFileTool {
    /// File path relative to repo root (e.g., \"src/auth.ts\")
    pub file_path: String,
    /// Include function/method signatures in the summary (default: false)
    pub include_signatures: Option<bool>,
    /// Include additional detail: import/export counts, edge statistics (default: false)
    pub verbose: Option<bool>,
    /// Include markdown display summary. Default false.
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
    /// Filter by edge types (default: ["call", "reference"]). Options: call, reference, type, extends, implements, alias
    pub edge_types: Option<Vec<String>>,
    /// Include markdown display summary. Default false.
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
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "search_todos",
    description = "Search indexed TODO and FIXME comments."
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
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_tests_for_symbol",
    description = "Find tests associated with a symbol or source file."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindTestsForSymbolTool {
    /// Symbol name to find tests for
    pub symbol_name: String,
    /// Optional file path to disambiguate symbols
    pub file_path: Option<String>,
    /// Maximum number of test files to return
    pub limit: Option<u32>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "search_decorators",
    description = "Search TypeScript or JavaScript decorators."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchDecoratorsTool {
    /// Decorator name to search for (e.g., 'Component', 'Controller', 'Get'). Exact or prefix match.
    pub name: Option<String>,
    /// Filter by decorator type (e.g., 'component', 'injectable', 'controller', 'get')
    pub decorator_type: Option<String>,
    /// Maximum number of results to return (default: 50)
    pub limit: Option<u32>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "search_framework_patterns",
    description = "Search indexed framework patterns such as routes."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchFrameworkPatternsTool {
    /// Framework to filter by (e.g., 'elysia'). If not specified, returns patterns from all frameworks.
    pub framework: Option<String>,
    /// Pattern kind to filter by (e.g., 'route', 'websocket', 'plugin', 'middleware')
    pub kind: Option<String>,
    /// HTTP method to filter by (e.g., 'GET', 'POST', 'PUT', 'DELETE')
    pub http_method: Option<String>,
    /// Route path pattern to search for (e.g., '/api/users')
    pub path: Option<String>,
    /// Maximum number of results to return (default: 50)
    pub limit: Option<u32>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_dead_code",
    description = "Find symbols with no incoming references."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindDeadCodeTool {
    /// Scope to specific file path
    pub file_path: Option<String>,
    /// Filter by language (e.g., "rust", "typescript")
    pub language: Option<String>,
    /// Filter by kind (e.g., "function", "class", "struct")
    pub kind: Option<String>,
    /// Include test symbols (default false)
    pub include_tests: Option<bool>,
    /// Maximum number of results (default 50)
    pub limit: Option<u32>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_duplicates",
    description = "Find likely duplicate symbols by semantic cluster."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindDuplicatesTool {
    /// Filter to symbols in a specific file path
    pub file_path: Option<String>,
    /// Filter by symbol kind (e.g., "function", "class", "struct")
    pub kind: Option<String>,
    /// Maximum number of duplicate groups to return (default: 50)
    pub limit: Option<u32>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "search_across_repos",
    description = "Search all indexed repositories. Standalone only."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchAcrossReposTool {
    /// Search query (natural language or code pattern)
    pub query: String,
    /// Maximum total results to return (default: 10)
    #[serde(default)]
    pub limit: Option<u32>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "explore_cross_repo_dependencies",
    description = "Explore cross-repo dependencies for a symbol. Standalone only."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ExploreCrossRepoDependenciesTool {
    /// The symbol name to explore cross-repo dependencies for
    pub symbol_name: String,
    /// Optional file path to disambiguate when multiple symbols share the same name
    pub file_path: Option<String>,
    /// Direction: "downstream" (what this symbol references in other repos). "upstream" and "both" are accepted but upstream is not yet implemented.
    pub direction: Option<String>,
    /// Maximum number of cross-repo edges to return (default: 20)
    pub limit: Option<u32>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_stale_descriptions",
    description = "Find cached symbol descriptions whose source changed."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindStaleDescriptionsTool {
    /// Filter to symbols in a specific file path
    pub file_path: Option<String>,
    /// Maximum number of results to return (default: 100)
    pub limit: Option<u32>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_undocumented_symbols",
    description = "Find symbols without generated descriptions."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindUndocumentedSymbolsTool {
    /// Filter to symbols in a specific file path
    pub file_path: Option<String>,
    /// Minimum line count to include (default: 3)
    pub min_lines: Option<u32>,
    /// Only include exported/public symbols
    pub exported_only: Option<bool>,
    /// Maximum number of results to return (default: 100)
    pub limit: Option<u32>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "predict_impact",
    description = "Predict change impact using dependencies and git co-change."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct PredictImpactTool {
    /// The symbol name to predict impact for
    pub symbol_name: String,
    /// Optional file path to disambiguate
    pub file_path: Option<String>,
    /// Maximum number of predictions to return (default: 20)
    pub limit: Option<u32>,
    /// Include test files in predictions (default: false)
    pub include_tests: Option<bool>,
    /// Include markdown display summary. Default false.
    pub include_display: Option<bool>,
}

#[macros::mcp_tool(
    name = "get_context_bundle",
    description = "Build one compact context bundle for a task."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetContextBundleTool {
    /// Description of the task to gather context for
    pub task: String,
    /// Maximum tokens for the context string (estimated as chars/4)
    pub max_tokens: Option<u32>,
    /// Which sections to include: definitions, call_chain, tests, similar, affected. Default: all.
    pub sections: Option<Vec<String>>,
    /// Number of seed symbols from initial search (default: 3)
    pub seed_limit: Option<u32>,
    /// Include raw per-section tool outputs. Default false; use only for debugging.
    pub include_raw_sections: Option<bool>,
}

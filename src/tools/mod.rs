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
    description = "Return a best-effort call hierarchy rooted at a symbol. Shows what a function calls (callees) or what calls it (callers)."
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
    description = "Return type relationships for a symbol (extends/implements/aliases)."
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
    description = "Return extracted usage examples for a symbol from the indexed repo."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetUsageExamplesTool {
    pub symbol_name: String,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_index_stats",
    description = "Return index statistics (files, symbols, edges, last updated). Includes description generation progress (descriptions, undescribed_symbols)."
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
    description = "Return symbols that share the same embedding cluster as the given symbol — i.e., semantically similar code. Small symbols (< 3 lines) or test helpers are skipped during clustering and will return empty results. For arbitrary code snippet similarity, use find_similar_code instead."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetSimilarityClusterTool {
    pub symbol_name: String,
    /// Maximum number of cluster members to return (default: 20)
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "hydrate_symbols",
    description = "Hydrate full source code and context for a set of symbol IDs (from search_code hits or find_references results). Returns the actual code text for each symbol. Use mode \"full\" to include surrounding context (callers, type hierarchy), or omit for code-only output."
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
    description = "Record user selection feedback for learning. Call this when a user selects a search result."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ReportSelectionTool {
    pub query: String,
    pub selected_symbol_id: String,
    pub position: u32,
}

#[macros::mcp_tool(
    name = "report_file_access",
    description = "Record file access for learning. Call this when a user views or edits a file to improve future search relevance."
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
    description = "Return detailed scoring breakdown for search results — shows keyword/vector/RRF scores, intent multipliers, and structural adjustments for each hit. Use verbose=true to include per-signal details (term_coverage, popularity_boost, etc.). Useful for debugging why a symbol ranks higher or lower than expected."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ExplainSearchTool {
    pub query: String,
    pub limit: Option<u32>,
    pub exported_only: Option<bool>,
    /// When true, includes per-signal breakdown (term_coverage, popularity_boost, definition_bias, symbol_importance, test_penalty) for each result
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
    description = "Trace where a variable/field is read and written across the codebase. Shows data flow edges (reads/writes) from the symbol's function scope. Use inter_procedural=true to expand 1 level into called functions, revealing cross-function data flow."
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
}

#[macros::mcp_tool(
    name = "summarize_file",
    description = "Generate a structural summary of an indexed file: symbol counts by kind (functions, structs, etc.), key exports, and dependency overview. This is a symbol-level summary from the index, not a line-by-line file read. Use include_signatures=true to show function/method signatures."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SummarizeFileTool {
    /// File path relative to repo root (e.g., \"src/auth.ts\")
    pub file_path: String,
    /// Include function/method signatures in the summary (default: false)
    pub include_signatures: Option<bool>,
    /// Include additional detail: import/export counts, edge statistics (default: false)
    pub verbose: Option<bool>,
}

#[macros::mcp_tool(
    name = "find_affected_code",
    description = "Find code that would be affected if the given symbol changes, using the structural dependency graph (callers, importers, type implementors). Returns reverse dependencies with severity ratings. For impact analysis that also considers git co-change history, use predict_impact instead."
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

#[macros::mcp_tool(
    name = "search_framework_patterns",
    description = "Search for framework-specific patterns in the codebase (e.g., Elysia routes, WebSocket handlers, middleware). Returns pattern metadata including file path, line, framework, kind, HTTP method, and route path."
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
}

#[macros::mcp_tool(
    name = "find_dead_code",
    description = "Find unused symbols (functions, classes, types) with zero incoming references. Identifies dead code that can be safely removed."
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
}

#[macros::mcp_tool(
    name = "find_duplicates",
    description = "Find groups of semantically similar symbols (potential duplicates) based on embedding clusters. Returns symbol groups that share the same embedding cluster, suggesting high semantic similarity."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindDuplicatesTool {
    /// Filter to symbols in a specific file path
    pub file_path: Option<String>,
    /// Filter by symbol kind (e.g., "function", "class", "struct")
    pub kind: Option<String>,
    /// Maximum number of duplicate groups to return (default: 50)
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "search_across_repos",
    description = "Search across all indexed repositories. Returns results merged by score. Standalone mode only."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchAcrossReposTool {
    /// Search query (natural language or code pattern)
    pub query: String,
    /// Maximum total results to return (default: 10)
    #[serde(default)]
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "explore_cross_repo_dependencies",
    description = "Explore cross-repo dependency edges for a symbol. Shows references from this repo's symbols to symbols in other indexed repos. Currently only downstream direction is implemented; upstream is planned. Standalone mode only."
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
}

#[macros::mcp_tool(
    name = "find_stale_descriptions",
    description = "Find symbols whose LLM-generated descriptions are stale (content hash mismatch). Returns symbols where the code has changed since the description was generated."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindStaleDescriptionsTool {
    /// Filter to symbols in a specific file path
    pub file_path: Option<String>,
    /// Maximum number of results to return (default: 100)
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "find_undocumented_symbols",
    description = "Find symbols that don't have LLM-generated descriptions yet. Returns symbols ordered by importance (exported first, then by size)."
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
}

#[macros::mcp_tool(
    name = "predict_impact",
    description = "Predict what code would be affected by changing a symbol. Combines structural dependencies (call graph, type hierarchy) with git co-change history (files that historically change together). Returns a ranked list with confidence scores. Unlike find_affected_code (which only uses the dependency graph), this also considers statistical co-change patterns from git log."
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
}

#[macros::mcp_tool(
    name = "get_context_bundle",
    description = "Accept a task description and return a pre-assembled context bundle with relevant definitions, call chains, test coverage, and similar code in one call."
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
}

//! MCP tool definitions

use rust_mcp_sdk::macros;
use serde::{Deserialize, Serialize};

#[macros::mcp_tool(
    name = "search_code",
    description = "Hybrid keyword + semantic code search. Returns ranked hits with symbol IDs (no bodies by default). To fetch source for the located symbols, call hydrate_symbols with the returned IDs; do NOT fall back to grep/read for symbols search_code already located. Pass context=\"snippets\" for an inline 8-line preview per hit, or context=\"full\" for the legacy markdown bundle."
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
    description = "Walk module-level imports and exports up or down from a symbol. Use this for 'what does this module depend on' or 'who imports this module' questions, especially when the answer spans multiple files. Do NOT grep for `use` / `import` statements when you need a transitive view. Specify direction='upstream' (who depends on this) or 'downstream' (what this depends on)."
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
    description = "Fetch source bodies for symbol IDs returned by search_code, find_references, get_call_hierarchy, or any other code-intelligence tool. Use this to read the body of an already-located symbol instead of reaching for read/grep. Accepts a verbose flag to control how much surrounding context is returned per symbol."
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
    description = "Trace where a variable, field, or symbol is read and written across the codebase. Use this when answering 'how does data flow from X to Y' or 'where does this value come from'; it follows reads/writes edges that plain grep cannot infer. Do NOT fall back to grep + manual reading when you need to understand dataflow. Pair with hydrate_symbols to fetch the bodies of the readers and writers it returns."
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
    description = "Get a one-pass symbol-level summary of a single file: which symbols it defines, their kinds, and a brief description of each. Use this instead of Read when you only need to know what's in a file, not its full contents. Prefer over get_file_symbols when you also want descriptions of each symbol; prefer get_module_summary when the unit is a module/directory rather than a single file."
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
    description = "Find every symbol that depends on a target (reverse dependency graph). Use this when answering 'if I rename or change X, what breaks?'; it walks the indexed dependency graph and returns affected sites with file:line. Do NOT fall back to grep + manual reading for impact analysis on symbols this tool can already locate. Use predict_impact if you also want git co-change signal alongside the static graph."
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
    description = "Get a structured overview of a module's exported public API surface: types, functions, and traits with their roles. Use this for 'what's in this module' or 'walk me through the public API of X' questions. Prefer this over get_file_symbols when the question is about a directory or module rather than a single file, and over Read+Grep when you need just the API surface, not the bodies."
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
    description = "Predict the blast radius of changing a symbol by combining the static dependency graph with git co-change history. Use this for 'what will break if I refactor X' and 'which files historically change with X'; it surfaces both compile-time deps and behavioral coupling that grep alone cannot see. Do NOT manually grep for callers and then guess at impact; this tool already does both passes."
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_code_description_mentions_hydrate_symbols_and_discourages_grep() {
        let desc = SearchCodeTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.contains("hydrate_symbols"),
            "search_code description must mention hydrate_symbols, got: {desc}"
        );
        assert!(
            desc.contains("do NOT fall back to grep"),
            "search_code description must explicitly discourage grep fallback, got: {desc}"
        );
        assert!(
            desc.contains("snippets"),
            "search_code description must still document the snippets mode"
        );
        assert!(
            desc.contains("full"),
            "search_code description must still document the full mode"
        );
    }

    #[test]
    fn hydrate_symbols_description_names_search_code_as_upstream() {
        let desc = HydrateSymbolsTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.contains("search_code"),
            "hydrate_symbols description must name search_code as a primary upstream, got: {desc}"
        );
        assert!(
            desc.to_lowercase().contains("instead of"),
            "hydrate_symbols description must position itself against read/grep, got: {desc}"
        );
        assert!(
            desc.contains("verbose"),
            "hydrate_symbols description must mention the verbose flag"
        );
    }

    #[test]
    fn trace_data_flow_description_advertises_dataflow_and_discourages_grep() {
        let desc = TraceDataFlowTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.to_lowercase().contains("how does data flow"),
            "trace_data_flow description must include the dataflow value hook, got: {desc}"
        );
        assert!(
            desc.contains("Do NOT fall back to grep"),
            "trace_data_flow description must explicitly discourage grep fallback, got: {desc}"
        );
        assert!(
            desc.contains("hydrate_symbols"),
            "trace_data_flow description must name hydrate_symbols as the chain target, got: {desc}"
        );
    }

    #[test]
    fn find_affected_code_description_advertises_impact_and_chains_predict_impact() {
        let desc = FindAffectedCodeTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.to_lowercase().contains("if i rename or change"),
            "find_affected_code description must include the rename-impact value hook, got: {desc}"
        );
        assert!(
            desc.contains("Do NOT fall back to grep"),
            "find_affected_code description must explicitly discourage grep fallback, got: {desc}"
        );
        assert!(
            desc.contains("predict_impact"),
            "find_affected_code description must name predict_impact as a richer alternative, got: {desc}"
        );
    }

    #[test]
    fn predict_impact_description_advertises_blast_radius_and_discourages_manual_grep() {
        let desc = PredictImpactTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.to_lowercase().contains("blast radius"),
            "predict_impact description must include the blast-radius value hook, got: {desc}"
        );
        assert!(
            desc.to_lowercase().contains("co-change"),
            "predict_impact description must mention git co-change as the differentiator, got: {desc}"
        );
        assert!(
            desc.contains("Do NOT manually grep"),
            "predict_impact description must explicitly discourage manual grep + guess, got: {desc}"
        );
    }

    #[test]
    fn explore_dependency_graph_description_advertises_module_walk_and_discourages_use_grep() {
        let desc = ExploreDependencyGraphTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.to_lowercase().contains("who imports this module"),
            "explore_dependency_graph description must include the module-walk value hook, got: {desc}"
        );
        assert!(
            desc.contains("Do NOT grep for `use`"),
            "explore_dependency_graph description must explicitly discourage grepping for use/import, got: {desc}"
        );
        assert!(
            desc.contains("upstream") && desc.contains("downstream"),
            "explore_dependency_graph description must document the direction parameter values, got: {desc}"
        );
    }

    #[test]
    fn get_module_summary_description_advertises_api_surface_and_prefers_over_get_file_symbols() {
        let desc = GetModuleSummaryTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.to_lowercase().contains("public api"),
            "get_module_summary description must include the API-surface value hook, got: {desc}"
        );
        assert!(
            desc.contains("Prefer this over get_file_symbols"),
            "get_module_summary description must position itself against get_file_symbols, got: {desc}"
        );
        assert!(
            desc.contains("module"),
            "get_module_summary description must mention 'module' as the unit, got: {desc}"
        );
    }

    #[test]
    fn summarize_file_description_advertises_file_summary_and_prefers_over_read() {
        let desc = SummarizeFileTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.to_lowercase().contains("symbol-level summary"),
            "summarize_file description must include the symbol-level summary value hook, got: {desc}"
        );
        assert!(
            desc.to_lowercase().contains("instead of read"),
            "summarize_file description must explicitly position against Read, got: {desc}"
        );
        assert!(
            desc.contains("get_module_summary"),
            "summarize_file description must name get_module_summary as the directory-scoped sibling, got: {desc}"
        );
    }
}

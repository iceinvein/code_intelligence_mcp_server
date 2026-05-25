//! MCP tool definitions

use rust_mcp_sdk::macros;
use serde::{Deserialize, Serialize};

#[macros::mcp_tool(
    name = "search_code",
    description = "Hybrid keyword + semantic code search. Default returns ranked hits with symbol IDs only (no bodies). For nontrivial questions, prefer `investigate` - it runs the full chain (search_code -> shape-driven specialist hop) server-side and returns one bundled response with verified locations, so you do not need to chain tools or fall back to Grep/Read. To fetch source for the located symbols without running a full investigation, call hydrate_symbols with the returned IDs. Pass context=\"snippets\" for an inline per-hit code preview, or context=\"full\" for the legacy markdown bundle with graph expansion."
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
    name = "plan_code_investigation",
    description = "Recommend a code-intelligence workflow for a natural-language codebase question. Use this before Grep/Read when deciding whether the task needs search_code, find_references, find_affected_code, predict_impact, trace_data_flow, explore_dependency_graph, get_module_summary, summarize_file, or hydrate_symbols. This tool only recommends next tool calls; it does not execute them. For most non-trivial questions, prefer `investigate` (the executing variant) over running this plan and the recommended steps yourself."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct PlanCodeInvestigationTool {
    pub question: String,
    pub target: Option<String>,
    pub file_path: Option<String>,
    /// Default 4, clamped to 1..=6.
    pub max_steps: Option<u32>,
}

#[macros::mcp_tool(
    name = "investigate",
    description = "Run a complete multi-step code investigation in one call. Pass a natural-language question; the server picks the right specialist chain (search_code -> get_call_hierarchy / trace_data_flow / find_affected_code / explore_dependency_graph based on question shape), executes it, and returns `pack.rows` plus `verified_locations`. Use `pack.rows` as the synthesis outline for callsite enumeration, pipeline traces, data flow, impact radius, dependency maps, and symbol lookup. Rows with role=\"candidate\" or a `pack.coverage.status` of partial/no_hits must be presented as candidates or followed up with `verified_locations`/specialist tools before making definitive line-level claims. Avoid Grep/Read unless `pack.coverage.status` is partial/no_hits or rows are candidates. Pass mode=\"auto\" (default) to let the server classify, or override with discover/trace/data/impact/dependency/module."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct InvestigateTool {
    /// Natural-language question.
    pub question: String,
    /// Optional symbol or file the investigation should pivot on.
    pub target: Option<String>,
    /// Optional file path for disambiguation or module-survey shape.
    pub file_path: Option<String>,
    /// auto (default), discover, trace, data, impact, dependency, or module.
    pub mode: Option<String>,
    /// Default 3, clamped 1..=5.
    pub max_hops: Option<u32>,
}

#[macros::mcp_tool(
    name = "ask_code",
    description = "Ask a question about the codebase and retrieve grounded evidence. The server runs the full investigate chain and returns structured `pack.rows`, `evidence[]`, mode metadata, and `pack.coverage.status`. The `answer` field is empty by default because local prose caused hallucinations; synthesize the user-facing answer yourself. Prefer `pack.rows` when present as the synthesis outline, but treat rows with role=\"candidate\" or `pack.coverage.status` partial/no_hits as candidates until confirmed with `evidence[]`, verified locations, or specialist tools. The evidence[] array contains source bodies and line ranges for verification and citation."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct AskCodeTool {
    /// Natural-language question about the codebase.
    pub question: String,
    /// Optional symbol or file the investigation should pivot on.
    pub target: Option<String>,
    /// Optional file path for disambiguation or module-survey shape.
    pub file_path: Option<String>,
    /// auto (default), discover, trace, data, impact, dependency, or module.
    pub mode: Option<String>,
    /// Number of evidence entries to include in the prompt. Default 8, clamp 1..=15.
    pub max_evidence: Option<u32>,
    /// fast | balanced (default). 'deep' (Qwen 7B) reserved for a later version.
    pub quality: Option<String>,
}

#[macros::mcp_tool(
    name = "bind_workspace",
    description = "Bind this MCP session to a workspace root by absolute path. Required as the first tool call when the client does not implement the MCP roots capability (every client except Claude Code, as of v4.0). The path must be an absolute path to an existing directory. Subsequent tool calls in the same session operate against the bound repository. Calling again with a different path rebinds the session."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct BindWorkspaceTool {
    /// Absolute path to the workspace root.
    pub repo: String,
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
    fn search_code_description_routes_multi_hop_to_investigate() {
        let desc = SearchCodeTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.contains("hydrate_symbols"),
            "search_code description must mention hydrate_symbols, got: {desc}"
        );
        assert!(
            desc.contains("investigate"),
            "search_code description must route multi-hop questions to investigate, got: {desc}"
        );
        assert!(
            desc.contains("snippets"),
            "search_code description must still document the snippets mode, got: {desc}"
        );
        assert!(
            desc.contains("full"),
            "search_code description must still document the full mode, got: {desc}"
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
    fn plan_code_investigation_description_advertises_routing_and_specialists() {
        let desc = PlanCodeInvestigationTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.contains("Grep/Read"),
            "plan_code_investigation description must position itself before Grep/Read, got: {desc}"
        );
        assert!(
            desc.contains("find_affected_code"),
            "plan_code_investigation description must mention find_affected_code, got: {desc}"
        );
        assert!(
            desc.contains("trace_data_flow"),
            "plan_code_investigation description must mention trace_data_flow, got: {desc}"
        );
        assert!(
            desc.contains("explore_dependency_graph"),
            "plan_code_investigation description must mention explore_dependency_graph, got: {desc}"
        );
        assert!(
            desc.contains("does not execute"),
            "plan_code_investigation description must say it only recommends, got: {desc}"
        );
    }

    #[test]
    fn investigate_description_mentions_evidence_packs() {
        let desc = InvestigateTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        assert!(
            desc.contains("pack.rows"),
            "investigate description must tell agents to use pack.rows, got: {desc}"
        );
        assert!(
            desc.contains("callsite") || desc.contains("pipeline"),
            "investigate description must advertise graph-shaped pack kinds, got: {desc}"
        );
        for required in ["pack.coverage.status", "partial", "no_hits", "candidate"] {
            assert!(
                desc.contains(required),
                "investigate description must mention {required}, got: {desc}"
            );
        }
    }

    #[test]
    fn ask_code_description_mentions_evidence_packs() {
        let desc = AskCodeTool::tool().description.clone().unwrap_or_default();
        assert!(
            desc.contains("pack"),
            "ask_code description must mention structured evidence packs, got: {desc}"
        );
        assert!(
            desc.contains("evidence[]"),
            "ask_code description must preserve evidence[] contract, got: {desc}"
        );
        for required in ["pack.coverage.status", "partial", "no_hits", "candidate"] {
            assert!(
                desc.contains(required),
                "ask_code description must mention {required}, got: {desc}"
            );
        }
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

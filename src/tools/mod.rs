//! MCP tool definitions

use rust_mcp_sdk::macros;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

fn invalid_option(name: &str, value: &str, expected: &str) -> String {
    format!("invalid {name} '{value}'; expected one of: {expected}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, macros::JsonSchema)]
pub enum SearchContext {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "snippets")]
    Snippets,
    #[serde(rename = "full")]
    Full,
}

impl SearchContext {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Snippets => "snippets",
            Self::Full => "full",
        }
    }
}

impl fmt::Display for SearchContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SearchContext {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "snippets" => Ok(Self::Snippets),
            "full" => Ok(Self::Full),
            _ => Err(invalid_option("context", value, "none, snippets, full")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, macros::JsonSchema)]
pub enum InvestigationMode {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "discover")]
    Discover,
    #[serde(rename = "trace", alias = "call_trace")]
    Trace,
    #[serde(rename = "data", alias = "data_trace")]
    Data,
    #[serde(rename = "impact", alias = "impact_radius")]
    Impact,
    #[serde(rename = "dependency", alias = "dependency_walk")]
    Dependency,
    #[serde(rename = "module", alias = "module_survey")]
    Module,
}

impl InvestigationMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Discover => "discover",
            Self::Trace => "trace",
            Self::Data => "data",
            Self::Impact => "impact",
            Self::Dependency => "dependency",
            Self::Module => "module",
        }
    }
}

impl fmt::Display for InvestigationMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for InvestigationMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "discover" => Ok(Self::Discover),
            "trace" | "call_trace" => Ok(Self::Trace),
            "data" | "data_trace" => Ok(Self::Data),
            "impact" | "impact_radius" => Ok(Self::Impact),
            "dependency" | "dependency_walk" => Ok(Self::Dependency),
            "module" | "module_survey" => Ok(Self::Module),
            _ => Err(invalid_option(
                "mode",
                value,
                "auto, discover, trace, data, impact, dependency, module",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, macros::JsonSchema)]
pub enum AnswerQuality {
    #[serde(rename = "fast")]
    Fast,
    #[serde(rename = "balanced")]
    Balanced,
}

impl AnswerQuality {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
        }
    }
}

impl fmt::Display for AnswerQuality {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for AnswerQuality {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "fast" => Ok(Self::Fast),
            "balanced" => Ok(Self::Balanced),
            _ => Err(invalid_option("quality", value, "fast, balanced")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, macros::JsonSchema)]
pub enum HydrateMode {
    #[serde(rename = "full")]
    Full,
}

impl HydrateMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
        }
    }
}

impl fmt::Display for HydrateMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for HydrateMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "full" => Ok(Self::Full),
            _ => Err(invalid_option("hydrate mode", value, "full")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, macros::JsonSchema)]
pub enum CallHierarchyDirection {
    #[serde(rename = "callees")]
    Callees,
    #[serde(rename = "callers")]
    Callers,
    #[serde(rename = "both")]
    Both,
}

impl CallHierarchyDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Callees => "callees",
            Self::Callers => "callers",
            Self::Both => "both",
        }
    }
}

impl fmt::Display for CallHierarchyDirection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CallHierarchyDirection {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "callees" => Ok(Self::Callees),
            "callers" => Ok(Self::Callers),
            "both" => Ok(Self::Both),
            _ => Err(invalid_option(
                "call hierarchy direction",
                value,
                "callees, callers, both",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, macros::JsonSchema)]
pub enum TraversalDirection {
    #[serde(rename = "downstream")]
    Downstream,
    #[serde(rename = "upstream")]
    Upstream,
    #[serde(rename = "both")]
    Both,
}

impl TraversalDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Downstream => "downstream",
            Self::Upstream => "upstream",
            Self::Both => "both",
        }
    }
}

impl fmt::Display for TraversalDirection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TraversalDirection {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "downstream" => Ok(Self::Downstream),
            "upstream" => Ok(Self::Upstream),
            "both" => Ok(Self::Both),
            _ => Err(invalid_option(
                "traversal direction",
                value,
                "downstream, upstream, both",
            )),
        }
    }
}

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
    pub context: Option<SearchContext>,
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
    name = "import_external_index",
    description = "Import a normalized external code index artifact and merge its precise symbols/references into the provenance overlay. The artifact path must be local and within the bound repository."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ImportExternalIndexTool {
    pub artifact_path: String,
}

#[macros::mcp_tool(
    name = "generate_external_index",
    description = "Run an explicitly configured external index producer for the bound repository, then import the generated artifact. This is opt-in and never runs automatically unless external index auto generation is enabled."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GenerateExternalIndexTool {
    pub producer: Option<String>,
    pub language: Option<String>,
}

#[macros::mcp_tool(
    name = "get_definition",
    description = "Get definition context for a specific symbol name. This low-level lookup does not return source bodies for a full natural-language investigation. For natural-language questions, flows, callsite enumeration, or anything that needs grounded synthesis, prefer ask_code or investigate because they return evidence bodies and pack.rows in one response. Use this only when you already know the exact symbol and need its definition metadata; use hydrate_symbols if bodies are needed."
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
    description = "Find imports, uses, or calls of a specific symbol name. This low-level lookup does not return source bodies for a full natural-language investigation. For natural-language callsite enumeration or questions that ask what each caller is doing, prefer ask_code or investigate because they return evidence bodies and pack.rows in one response. Use this only when you already know the exact symbol and need raw reference edges; use hydrate_symbols if bodies are needed."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindReferencesTool {
    pub symbol_name: String,
    /// Disambiguating file path.
    pub file: Option<String>,
    /// Optional edge-type filter. Common values are call, import, reference,
    /// extends, implements, and all. External producers may add other types.
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
    pub direction: Option<CallHierarchyDirection>,
    /// Default 3, max 10.
    pub depth: Option<u32>,
    /// Default 50.
    pub limit: Option<u32>,
    /// Optional file to disambiguate the root among same-named symbols.
    pub file: Option<String>,
}

#[macros::mcp_tool(
    name = "get_type_graph",
    description = "Return type relationships for a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetTypeGraphTool {
    pub symbol_name: String,
    /// downstream, upstream, or both.
    pub direction: Option<TraversalDirection>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
    /// Optional file to disambiguate the root among same-named symbols.
    pub file: Option<String>,
}

#[macros::mcp_tool(
    name = "get_usage_examples",
    description = "Return indexed usage examples for a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetUsageExamplesTool {
    pub symbol_name: String,
    pub limit: Option<u32>,
    /// Optional file to disambiguate same-named symbols. When set, only the
    /// symbol defined in this file contributes usage examples.
    pub file: Option<String>,
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
    pub direction: Option<TraversalDirection>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
    /// Optional file to disambiguate the root among same-named symbols.
    pub file: Option<String>,
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
    pub mode: Option<HydrateMode>,
    /// Include per-symbol metadata (id, role, tokens, reasons).
    pub verbose: Option<bool>,
}

#[macros::mcp_tool(
    name = "investigate",
    description = "Run a complete multi-step code investigation in one call. Pass a natural-language question; the server picks the right specialist chain (search_code -> get_call_hierarchy / trace_data_flow / find_affected_code / explore_dependency_graph based on question shape), executes it, and returns `pack.rows` plus `verified_locations`. Use `pack.rows` as the synthesis outline for callsite enumeration, pipeline traces, data flow, impact radius, dependency maps, and symbol lookup. `pack.coverage` names required, optional, resolved, missing, ambiguous, and candidate semantic roles. Rows also expose `coverage_role`, `verification`, and `source_backed`; make exact path:line claims only from source-backed rows. Rows with role=\"candidate\", verification=\"ambiguous\", or a `pack.coverage.status` of partial/no_hits must be presented as candidates or followed up with `verified_locations`/specialist tools before making definitive claims. Don't Read or Grep files the rows already cover; cite directly. If the question names a file, symbol, or path that no row contains (the coverage classifier can mark complete and still miss test files, configs, or files outside the question's main noun-phrase), fall back to Grep/Glob/Read once -- don't re-query investigate with rephrased prompts. Pass mode=\"auto\" (default) to let the server classify, or override with discover/trace/data/impact/dependency/module."
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
    pub mode: Option<InvestigationMode>,
    /// Default 3, clamped 1..=5.
    pub max_hops: Option<u32>,
}

#[macros::mcp_tool(
    name = "ask_code",
    description = "Ask a question about the codebase and retrieve grounded evidence. The server runs the full investigate chain and returns structured `pack.rows`, `evidence[]`, mode metadata, and an explicit `pack.coverage` role contract. The contract names required, optional, resolved, missing, ambiguous, and candidate roles; each row exposes `coverage_role`, `verification`, and `source_backed`. The `answer` field is empty by default because local prose caused hallucinations; synthesize the user-facing answer yourself. Prefer `pack.rows` when present as the synthesis outline, but treat rows with role=\"candidate\", verification=\"ambiguous\", or `pack.coverage.status` partial/no_hits as candidates until confirmed with `evidence[]`, verified locations, or specialist tools. Make exact path:line claims only from source-backed rows. The evidence[] array contains source bodies and line ranges for verification and citation. Don't Read or Grep files the rows already cover; cite directly. If the question names a file, symbol, or path that no row contains (the coverage classifier can mark complete and still miss test files, configs, or files outside the question's main noun-phrase), fall back to Grep/Glob/Read once -- don't re-query ask_code or investigate with rephrased prompts."
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
    pub mode: Option<InvestigationMode>,
    /// Number of evidence entries to include in the prompt. Default 8, clamp 1..=15.
    pub max_evidence: Option<u32>,
    /// fast | balanced (default). 'deep' (Qwen 7B) reserved for a later version.
    pub quality: Option<AnswerQuality>,
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
    name = "approve_indexing",
    description = "Approve or decline the first full index for the repo bound to this session (or an explicit `repo` path). Every never-indexed repository returns `consent_required`, including explicit `?repo=` and `bind_workspace` selections. Tell the user in chat that indexing uses local compute, memory, and disk, then wait for explicit user approval before calling this tool with `approve`. Use `decline` when the user declines. Approval starts a background index job immediately and is remembered; later watcher and manual reindexes do not ask again."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ApproveIndexingTool {
    /// Optional absolute path to the repo. Defaults to the session's bound repo.
    pub repo: Option<String>,
    /// "approve" to index and remember the choice; "decline" to skip and remember.
    pub decision: String,
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
    description = "Find every symbol, transparent wrapper, and public API path that depends on a target (reverse dependency graph). Use this when answering 'if I rename or change X, what breaks?'; it walks calls, delegation, references, imports, exports, and re-exports and returns affected sites with file:line. Do NOT fall back to grep + manual reading for impact analysis on symbols this tool can already locate."
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
    name = "find_tests_for_symbol",
    description = "Find tests associated with a symbol or source file. Returns `test_files` (verified test-source links from path-pattern inference at index time) and `tests_for_symbol` (specific test functions calling the target via call-graph edges). When `test_files` is non-empty, the paths are guaranteed to be indexed: cite them directly without Read/Grep verification. Use this BEFORE falling back to ask_code/investigate for test-coverage questions; it gives a direct test-file answer that ask_code's BM25 ranking often misses because production symbols outrank test wrappers."
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_options_are_strict_and_advertised_in_tool_schemas() {
        let invalid = serde_json::from_value::<SearchCodeTool>(serde_json::json!({
            "query": "auth",
            "context": "verbose"
        }));
        assert!(invalid.is_err());

        let schema = serde_json::to_string(&SearchCodeTool::tool().input_schema).unwrap();
        for expected in ["none", "snippets", "full"] {
            assert!(
                schema.contains(&format!("\"{expected}\"")),
                "search_code schema must advertise {expected}: {schema}"
            );
        }
    }

    #[test]
    fn investigation_mode_deserialization_keeps_legacy_aliases() {
        let tool = serde_json::from_value::<InvestigateTool>(serde_json::json!({
            "question": "trace the call path",
            "mode": "call_trace"
        }))
        .unwrap();
        assert_eq!(tool.mode, Some(InvestigationMode::Trace));
    }

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
        for required in [
            "pack.coverage.status",
            "partial",
            "no_hits",
            "candidate",
            "required",
            "ambiguous",
            "source_backed",
            "exact path:line",
            "Don't Read or Grep files the rows already cover",
            "fall back to Grep/Glob/Read once",
            "don't re-query",
        ] {
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
        for required in [
            "pack.coverage.status",
            "partial",
            "no_hits",
            "candidate",
            "required",
            "ambiguous",
            "source_backed",
            "exact path:line",
            "Don't Read or Grep files the rows already cover",
            "fall back to Grep/Glob/Read once",
            "don't re-query",
        ] {
            assert!(
                desc.contains(required),
                "ask_code description must mention {required}, got: {desc}"
            );
        }
    }

    #[test]
    fn low_level_navigation_descriptions_route_natural_language_to_composites() {
        let find_refs = FindReferencesTool::tool()
            .description
            .clone()
            .unwrap_or_default();
        let get_def = GetDefinitionTool::tool()
            .description
            .clone()
            .unwrap_or_default();

        for (name, desc) in [
            ("find_references", find_refs.as_str()),
            ("get_definition", get_def.as_str()),
        ] {
            for required in [
                "natural-language",
                "ask_code",
                "investigate",
                "does not return source bodies",
            ] {
                assert!(
                    desc.contains(required),
                    "{name} description must mention {required}, got: {desc}"
                );
            }
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
    fn find_affected_code_description_advertises_impact_and_discourages_grep() {
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

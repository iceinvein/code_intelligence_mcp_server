//! `investigate` composite handler.
//!
//! Runs a multi-step code investigation server-side and returns one structured
//! response. Replaces the agent's plan→search→specialist→hydrate dance with a
//! single tool call. The shape classifier inspects the question text and picks
//! the second-hop specialist (call-graph, data-flow, impact, or dependency)
//! whose result the agent would otherwise have to fetch by hand.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::handlers::framework_routes::route_exposures_for_symbol;
use crate::handlers::planning::plan_code_investigation;
use crate::tools::InvestigateTool;

use super::evidence_pack::{build_evidence_pack, pack_to_value, EvidencePackInput, PackLocation};
use super::AppState;

/// Hard cap for the response JSON. Beyond this we degrade by dropping
/// secondary `body` fields, then by trimming `context_chain`, then by
/// truncating `verified_locations`.
const RESPONSE_BUDGET_BYTES: usize = 64 * 1024;

/// Per-symbol body cap (lines) for verified_locations entries.
const PER_BODY_LINES_CAP: usize = 200;

/// Hops past the initial search that we will execute. v3.1.0 ships with at
/// most one extra hop (the shape-driven specialist).
const DEFAULT_MAX_HOPS: u32 = 3;
const MAX_HOPS_HARD_CAP: u32 = 5;

/// Coarse classification of what kind of follow-up the question demands. Maps
/// directly to the second-hop specialist tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationShape {
    /// Single-symbol lookup or "what is X". No second hop; the search_code
    /// response is structurally complete.
    Discover,
    /// "How does X flow / pipeline / dispatch / merge / end-to-end" — needs a
    /// call-graph traversal off the top hit.
    CallTrace,
    /// "Where is X read / written" — needs trace_data_flow off the top hit.
    DataTrace,
    /// "What breaks if I change X" — needs find_affected_code off the top hit.
    ImpactRadius,
    /// "What does this module depend on / import / export" — needs
    /// explore_dependency_graph.
    DependencyWalk,
    /// "Walk me through this module / what's in this file" — gets the
    /// module-summary path. Only fires when a file_path is provided.
    ModuleSurvey,
}

impl InvestigationShape {
    fn as_str(self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::CallTrace => "call_trace",
            Self::DataTrace => "data_trace",
            Self::ImpactRadius => "impact_radius",
            Self::DependencyWalk => "dependency_walk",
            Self::ModuleSurvey => "module_survey",
        }
    }

    fn from_mode_override(mode: &str) -> Option<Self> {
        match mode {
            "auto" => None,
            "discover" => Some(Self::Discover),
            "trace" | "call_trace" => Some(Self::CallTrace),
            "data" | "data_trace" => Some(Self::DataTrace),
            "impact" | "impact_radius" => Some(Self::ImpactRadius),
            "dependency" | "dependency_walk" => Some(Self::DependencyWalk),
            "module" | "module_survey" => Some(Self::ModuleSurvey),
            _ => None,
        }
    }
}

/// Pick the second-hop specialist for a question. The classifier is
/// deterministic and order-sensitive: more specific shapes are checked
/// first. `mode_override` short-circuits the heuristic.
pub fn classify_shape(
    question: &str,
    file_path: Option<&str>,
    mode_override: Option<&str>,
) -> InvestigationShape {
    if let Some(mode) = mode_override {
        if let Some(forced) = InvestigationShape::from_mode_override(mode) {
            return forced;
        }
    }

    let q = question.to_lowercase();

    // Impact comes first because phrases like "what breaks if I change X"
    // outrank generic "trace through" mentions. Same for explicit refactor
    // language. Keyword set covers first/second/third person and past/present
    // tense after R006's q10 missed via "would break" / "downstream" /
    // "if X changed".
    if contains_any(
        &q,
        &[
            "what breaks",
            "would break",
            "will break",
            "what depends",
            "downstream code",
            "downstream",
            "if i change",
            "if we change",
            "if it changed",
            "if it changes",
            "if changed",
            "blast radius",
            "affected by",
            "what's affected",
            "whats affected",
            "rename",
            "refactor",
            "removing",
            "deleting",
            "impact of",
        ],
    ) {
        return InvestigationShape::ImpactRadius;
    }

    if contains_any(
        &q,
        &[
            "data flow",
            "reads and writes",
            "where does this value come from",
            "where is this value used",
            "lifecycle of",
            "set and read",
        ],
    ) {
        return InvestigationShape::DataTrace;
    }

    // Call-trace fires for the broad "how does X flow through the pipeline"
    // family. These questions are the largest agent regression source under
    // the v3.0.0 None default.
    if contains_any(
        &q,
        &[
            "pipeline",
            "end-to-end",
            "end to end",
            "dispatch",
            "merge",
            "merged",
            "merger",
            "flows through",
            "flow through",
            "trace how",
            "trace the",
            "how does the",
            "step by step",
            "before reaching",
            "after reaching",
            "call chain",
            "call hierarchy",
        ],
    ) {
        return InvestigationShape::CallTrace;
    }

    if contains_any(
        &q,
        &[
            "depends on",
            "depend on",
            "who imports",
            "imports this",
            "consumes",
            "dependency graph",
            "upstream",
            "downstream",
            "which modules",
        ],
    ) {
        return InvestigationShape::DependencyWalk;
    }

    if file_path.is_some()
        && contains_any(
            &q,
            &[
                "what's in this module",
                "whats in this module",
                "what's in this file",
                "whats in this file",
                "walk me through this module",
                "summarize this file",
                "public api",
            ],
        )
    {
        return InvestigationShape::ModuleSurvey;
    }

    InvestigationShape::Discover
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| text.contains(n))
}

/// Public entry point wired from the dispatcher.
pub async fn handle_investigate(state: &AppState, tool: InvestigateTool) -> Result<Value> {
    let question = tool.question.trim().to_string();
    if question.is_empty() {
        anyhow::bail!("question is required");
    }
    let target = tool
        .target
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let file_path = tool
        .file_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let max_hops = tool
        .max_hops
        .unwrap_or(DEFAULT_MAX_HOPS)
        .clamp(1, MAX_HOPS_HARD_CAP);

    let shape = classify_shape(&question, file_path, tool.mode.as_deref());

    // Step 1: planner provides the recommended chain (always included so the
    // agent can audit our routing).
    let plan_value = serde_json::to_value(plan_code_investigation(
        &question,
        target,
        file_path,
        max_hops as usize,
    )?)?;

    // Step 2: run the first specialist hop (search_code + bodies).
    let primary = run_primary_hop(state, &question, target, file_path).await?;

    // Step 3: run the shape-driven second hop, if any.
    let secondary = if max_hops >= 2 {
        run_secondary_hop(state, shape, &primary, target, file_path).await?
    } else {
        None
    };

    let bundle = build_response(&question, shape, plan_value, primary, secondary, max_hops);
    Ok(bundle)
}

/// Result of the first hop: the search_code response plus a list of
/// verified-location entries we extracted from it.
struct PrimaryHop {
    /// Full search_code response (with `context: "full"` markdown bundle).
    raw: Value,
    /// Verified-location entries pulled from `hits[]`. Bodies are filled
    /// from SQLite. Empty if search returned no hits.
    locations: Vec<VerifiedLocation>,
}

#[derive(Debug, Clone, Serialize)]
struct VerifiedLocation {
    symbol_id: String,
    symbol_name: String,
    file_path: String,
    kind: String,
    start_line: u32,
    end_line: u32,
    via: &'static str,
    body: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    route_exposure: Vec<Value>,
}

async fn run_primary_hop(
    state: &AppState,
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
) -> Result<PrimaryHop> {
    use crate::tools::SearchCodeTool;

    let query = target.unwrap_or(question).to_string();
    let tool = SearchCodeTool {
        query,
        limit: Some(5),
        exported_only: None,
        context: Some("full".to_string()),
    };
    let raw =
        super::search::handle_search_code(&state.retriever, &state.config.db_path, tool).await?;

    let locations = extract_locations_from_search(state, &raw, "search_code", file_path)?;
    Ok(PrimaryHop { raw, locations })
}

fn extract_locations_from_search(
    state: &AppState,
    raw: &Value,
    via: &'static str,
    file_filter: Option<&str>,
) -> Result<Vec<VerifiedLocation>> {
    let Some(hits) = raw.get("hits").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let sqlite = &state.sqlite;
    let mut out = Vec::with_capacity(hits.len());
    for hit in hits {
        let Some(id) = hit.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(row) = sqlite.get_symbol_by_id(id)? else {
            continue;
        };
        if let Some(filter) = file_filter {
            if !row.file_path.contains(filter) {
                // file_path is a "filter" hint, not a strict gate. Keep the
                // row even if it doesn't match - hits that don't match get
                // deprioritised by retrieval already.
            }
        }
        let body = body_with_cap(&row.text, PER_BODY_LINES_CAP);
        let route_exposure = route_exposures_for_symbol(sqlite, &row, 20)?;
        out.push(VerifiedLocation {
            symbol_id: row.id,
            symbol_name: row.name,
            file_path: row.file_path,
            kind: row.kind,
            start_line: row.start_line,
            end_line: row.end_line,
            via,
            body,
            route_exposure,
        });
    }
    Ok(out)
}

/// Secondary hop result: the raw response from the specialist tool, plus any
/// verified-location entries we extracted from it.
struct SecondaryHop {
    via: &'static str,
    raw: Value,
    locations: Vec<VerifiedLocation>,
}

async fn run_secondary_hop(
    state: &AppState,
    shape: InvestigationShape,
    primary: &PrimaryHop,
    target: Option<&str>,
    file_path: Option<&str>,
) -> Result<Option<SecondaryHop>> {
    // Pick the symbol the secondary specialist will pivot off. Prefer the
    // explicit `target`, fall back to the top hit's name.
    let pivot_name = match target {
        Some(t) => t.to_string(),
        None => primary
            .locations
            .first()
            .map(|l| l.symbol_name.clone())
            .unwrap_or_default(),
    };
    if pivot_name.is_empty() {
        return Ok(None);
    }

    match shape {
        InvestigationShape::Discover => Ok(None),
        InvestigationShape::CallTrace => run_call_hierarchy_hop(state, &pivot_name, file_path),
        InvestigationShape::DataTrace => {
            run_trace_data_flow_hop(state, &pivot_name, file_path).await
        }
        InvestigationShape::ImpactRadius => {
            run_find_affected_hop(state, &pivot_name, file_path).await
        }
        InvestigationShape::DependencyWalk => run_dependency_graph_hop(state, &pivot_name),
        InvestigationShape::ModuleSurvey => run_module_summary_hop(state, file_path).await,
    }
}

fn run_call_hierarchy_hop(
    state: &AppState,
    pivot: &str,
    _file_path: Option<&str>,
) -> Result<Option<SecondaryHop>> {
    use crate::tools::GetCallHierarchyTool;

    let tool = GetCallHierarchyTool {
        symbol_name: pivot.to_string(),
        direction: Some("both".to_string()),
        // Depth 3 reaches typical pipeline chains like
        // handler -> dispatcher -> retriever -> specialist that the previous
        // depth=2 stopped short of. R006 q15's missing-merger rubric failure
        // was the trigger.
        depth: Some(3),
        limit: Some(50),
    };
    let raw = super::graph::handle_get_call_hierarchy(state, tool)?;
    let locations = extract_locations_from_graph_nodes(state, &raw, "get_call_hierarchy")?;
    Ok(Some(SecondaryHop {
        via: "get_call_hierarchy",
        raw,
        locations,
    }))
}

async fn run_trace_data_flow_hop(
    state: &AppState,
    pivot: &str,
    file_path: Option<&str>,
) -> Result<Option<SecondaryHop>> {
    use crate::tools::TraceDataFlowTool;

    let tool = TraceDataFlowTool {
        symbol_name: pivot.to_string(),
        file_path: file_path.map(ToOwned::to_owned),
        direction: Some("both".to_string()),
        depth: Some(3),
        limit: Some(50),
        inter_procedural: Some(false),
        include_display: Some(false),
    };
    let raw = super::graph::handle_trace_data_flow(state, tool)?;
    let locations = extract_locations_from_flows(state, &raw, "trace_data_flow")?;
    Ok(Some(SecondaryHop {
        via: "trace_data_flow",
        raw,
        locations,
    }))
}

async fn run_find_affected_hop(
    state: &AppState,
    pivot: &str,
    file_path: Option<&str>,
) -> Result<Option<SecondaryHop>> {
    use crate::tools::FindAffectedCodeTool;

    let tool = FindAffectedCodeTool {
        symbol_name: pivot.to_string(),
        file_path: file_path.map(ToOwned::to_owned),
        depth: Some(2),
        limit: Some(50),
        include_tests: Some(false),
        edge_types: None,
        include_display: Some(false),
    };
    let raw = super::analysis::handle_find_affected_code(state, tool)?;
    let locations = extract_locations_from_affected(state, &raw, "find_affected_code")?;
    Ok(Some(SecondaryHop {
        via: "find_affected_code",
        raw,
        locations,
    }))
}

fn run_dependency_graph_hop(state: &AppState, pivot: &str) -> Result<Option<SecondaryHop>> {
    use crate::tools::ExploreDependencyGraphTool;

    let tool = ExploreDependencyGraphTool {
        symbol_name: pivot.to_string(),
        direction: Some("both".to_string()),
        depth: Some(2),
        limit: Some(50),
    };
    let raw = super::graph::handle_explore_dependency_graph(state, tool)?;
    let locations = extract_locations_from_graph_nodes(state, &raw, "explore_dependency_graph")?;
    Ok(Some(SecondaryHop {
        via: "explore_dependency_graph",
        raw,
        locations,
    }))
}

async fn run_module_summary_hop(
    state: &AppState,
    file_path: Option<&str>,
) -> Result<Option<SecondaryHop>> {
    let Some(path) = file_path else {
        return Ok(None);
    };
    use crate::tools::GetModuleSummaryTool;

    let tool = GetModuleSummaryTool {
        file_path: path.to_string(),
        group_by_kind: Some(true),
        include_display: Some(false),
    };
    let raw = super::navigation::handle_get_module_summary(state, tool)?;
    Ok(Some(SecondaryHop {
        via: "get_module_summary",
        raw,
        locations: Vec::new(), // module summary already includes structured info
    }))
}

fn extract_locations_from_graph_nodes(
    state: &AppState,
    raw: &Value,
    via: &'static str,
) -> Result<Vec<VerifiedLocation>> {
    let Some(nodes) = raw.get("nodes").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let sqlite = &state.sqlite;
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        let id = node.get("id").or_else(|| node.get("symbol_id"));
        let Some(id) = id.and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(row) = sqlite.get_symbol_by_id(id)? else {
            continue;
        };
        let body = body_with_cap(&row.text, PER_BODY_LINES_CAP);
        let route_exposure = route_exposures_for_symbol(sqlite, &row, 20)?;
        out.push(VerifiedLocation {
            symbol_id: row.id,
            symbol_name: row.name,
            file_path: row.file_path,
            kind: row.kind,
            start_line: row.start_line,
            end_line: row.end_line,
            via,
            body,
            route_exposure,
        });
    }
    Ok(out)
}

fn extract_locations_from_flows(
    state: &AppState,
    raw: &Value,
    via: &'static str,
) -> Result<Vec<VerifiedLocation>> {
    let Some(flows) = raw.get("flows").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let sqlite = &state.sqlite;
    let mut out = Vec::with_capacity(flows.len());
    for flow in flows {
        let Some(id) = flow.get("symbol_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(row) = sqlite.get_symbol_by_id(id)? else {
            continue;
        };
        let body = body_with_cap(&row.text, PER_BODY_LINES_CAP);
        let route_exposure = route_exposures_for_symbol(sqlite, &row, 20)?;
        out.push(VerifiedLocation {
            symbol_id: row.id,
            symbol_name: row.name,
            file_path: row.file_path,
            kind: row.kind,
            start_line: row.start_line,
            end_line: row.end_line,
            via,
            body,
            route_exposure,
        });
    }
    Ok(out)
}

fn extract_locations_from_affected(
    state: &AppState,
    raw: &Value,
    via: &'static str,
) -> Result<Vec<VerifiedLocation>> {
    let Some(affected) = raw.get("affected").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let sqlite = &state.sqlite;
    let mut out = Vec::with_capacity(affected.len());
    for entry in affected {
        let Some(id) = entry.get("symbol_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(row) = sqlite.get_symbol_by_id(id)? else {
            continue;
        };
        let body = body_with_cap(&row.text, PER_BODY_LINES_CAP);
        let route_exposure = route_exposures_for_symbol(sqlite, &row, 20)?;
        out.push(VerifiedLocation {
            symbol_id: row.id,
            symbol_name: row.name,
            file_path: row.file_path,
            kind: row.kind,
            start_line: row.start_line,
            end_line: row.end_line,
            via,
            body,
            route_exposure,
        });
    }
    Ok(out)
}

fn body_with_cap(text: &str, max_lines: usize) -> String {
    let total = text.lines().count();
    let kept: Vec<&str> = text.lines().take(max_lines).map(|l| l.trim_end()).collect();
    let mut out = kept.join("\n");
    if total > max_lines {
        out.push_str(&format!("\n// ... {} more lines", total - max_lines));
    }
    out
}

fn dedup_locations(mut locations: Vec<VerifiedLocation>) -> Vec<VerifiedLocation> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut out: Vec<VerifiedLocation> = Vec::with_capacity(locations.len());
    for loc in locations.drain(..) {
        if let Some(&_existing_idx) = seen.get(&loc.symbol_id) {
            continue;
        }
        seen.insert(loc.symbol_id.clone(), out.len());
        out.push(loc);
    }
    out
}

fn pack_locations_from_verified(locations: &[VerifiedLocation]) -> Vec<PackLocation> {
    locations
        .iter()
        .map(|loc| PackLocation {
            symbol_id: Some(loc.symbol_id.clone()),
            symbol_name: Some(loc.symbol_name.clone()),
            file_path: Some(loc.file_path.clone()),
            kind: Some(loc.kind.clone()),
            start_line: Some(loc.start_line),
            end_line: Some(loc.end_line),
            via: Some(loc.via.to_string()),
            body: Some(loc.body.clone()),
        })
        .collect()
}

fn build_response(
    question: &str,
    shape: InvestigationShape,
    plan: Value,
    primary: PrimaryHop,
    secondary: Option<SecondaryHop>,
    max_hops: u32,
) -> Value {
    let mut all_locations = primary.locations.clone();
    if let Some(s) = secondary.as_ref() {
        all_locations.extend(s.locations.clone());
    }
    let dedup = dedup_locations(all_locations);

    let primary_symbol = primary.locations.first().map(|l| {
        json!({
            "symbol_id": l.symbol_id,
            "symbol_name": l.symbol_name,
            "file_path": l.file_path,
            "start_line": l.start_line,
            "end_line": l.end_line,
            "kind": l.kind,
        })
    });

    let stop_reason = if dedup.is_empty() {
        "no_hits"
    } else if matches!(shape, InvestigationShape::Discover) {
        "shape_complete_discover"
    } else if secondary.is_none() {
        "max_hops_reached"
    } else {
        "shape_complete"
    };

    let context_chain = primary
        .raw
        .get("context")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let secondary_summary = secondary.as_ref().map(|s| {
        json!({
            "via": s.via,
            "summary": summarize_secondary(&s.raw, s.via),
        })
    });
    let pack_target = primary
        .locations
        .first()
        .map(|loc| loc.symbol_name.clone())
        .unwrap_or_else(|| question.to_string());
    let pack = build_evidence_pack(EvidencePackInput {
        question: question.to_string(),
        target: pack_target,
        shape,
        primary: pack_locations_from_verified(&primary.locations),
        secondary: secondary
            .as_ref()
            .map(|s| pack_locations_from_verified(&s.locations))
            .unwrap_or_default(),
        secondary_via: secondary.as_ref().map(|s| s.via.to_string()),
    });

    let mut response = json!({
        "question": question,
        "mode_used": shape.as_str(),
        "max_hops": max_hops,
        "stop_reason": stop_reason,
        "plan": plan,
        "primary_symbol": primary_symbol,
        "verified_locations": dedup,
        "secondary": secondary_summary,
        "pack": pack_to_value(&pack),
        "context_chain": context_chain,
        "answer_hint": "Cite only entries from `verified_locations`. Identifiers \
            mentioned inside `body` text or in `context_chain` but NOT listed in \
            verified_locations are NOT verified locations - do not state their \
            file paths or line numbers without a separate get_definition or \
            find_references call. The server has already executed the appropriate \
            multi-hop chain for this question's shape; do NOT call Grep, Read, or \
            search_code to verify or expand."
    });

    apply_response_budget(&mut response);
    response
}

fn summarize_secondary(raw: &Value, via: &str) -> Value {
    match via {
        "get_call_hierarchy" | "explore_dependency_graph" => json!({
            "node_count": raw.get("nodes").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
            "edge_count": raw.get("edges").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
        }),
        "trace_data_flow" => json!({
            "flow_count": raw.get("flows").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
        }),
        "find_affected_code" => json!({
            "affected_count": raw
                .get("affected")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0),
        }),
        "get_module_summary" => json!({
            "summary_present": raw.is_object(),
        }),
        _ => json!({}),
    }
}

/// Trim the response to fit `RESPONSE_BUDGET_BYTES`. Degrades in stages:
///   1. Drop bodies from secondary verified_locations (anything past the
///      first ~3 entries).
///   2. Truncate `context_chain` to a head slice.
///   3. Truncate verified_locations to top-K by index.
fn apply_response_budget(response: &mut Value) {
    fn estimate(v: &Value) -> usize {
        serde_json::to_string(v).map(|s| s.len()).unwrap_or(0)
    }
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    // Stage 1: drop bodies past index 2.
    if let Some(arr) = response
        .get_mut("verified_locations")
        .and_then(|v| v.as_array_mut())
    {
        for (i, entry) in arr.iter_mut().enumerate() {
            if i >= 3 {
                if let Some(obj) = entry.as_object_mut() {
                    obj.insert("body".to_string(), json!(""));
                    obj.insert("body_dropped".to_string(), json!(true));
                }
            }
        }
    }
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    // Stage 2: truncate context_chain.
    if let Some(s) = response.get("context_chain").and_then(|v| v.as_str()) {
        let max = (RESPONSE_BUDGET_BYTES / 4).min(s.len());
        let mut head = s[..max].to_string();
        head.push_str("\n... [context_chain truncated]");
        response["context_chain"] = json!(head);
    }
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    // Stage 3: truncate verified_locations to first 8.
    if let Some(arr) = response
        .get_mut("verified_locations")
        .and_then(|v| v.as_array_mut())
    {
        if arr.len() > 8 {
            arr.truncate(8);
            response["verified_locations_truncated"] = json!(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_default_is_discover_for_simple_lookup() {
        assert_eq!(
            classify_shape("what is PathNormalizer", None, None),
            InvestigationShape::Discover
        );
    }

    #[test]
    fn classify_call_trace_for_pipeline_questions() {
        assert_eq!(
            classify_shape(
                "trace how the query string flows through the search pipeline",
                None,
                None
            ),
            InvestigationShape::CallTrace
        );
        assert_eq!(
            classify_shape(
                "where is BM25 keyword search merged with vector search via RRF",
                None,
                None
            ),
            InvestigationShape::CallTrace
        );
        assert_eq!(
            classify_shape(
                "end-to-end flow of a search call from MCP dispatch to ranking",
                None,
                None
            ),
            InvestigationShape::CallTrace
        );
    }

    #[test]
    fn classify_callsite_enumeration_phrases_as_discover_for_pack_adapter() {
        assert_eq!(
            classify_shape("who calls SessionManager.createSession", None, None),
            InvestigationShape::Discover
        );
        assert_eq!(
            classify_shape("list callsites for getProvider", None, None),
            InvestigationShape::Discover
        );
    }

    #[test]
    fn pipeline_trace_outranks_callsite_words() {
        assert_eq!(
            classify_shape(
                "trace how the provider calls flow through IPC to the renderer subscriber",
                None,
                None,
            ),
            InvestigationShape::CallTrace
        );
    }

    #[test]
    fn classify_data_trace_for_data_flow_questions() {
        assert_eq!(
            classify_shape("trace data flow for HYBRID_ALPHA", None, None),
            InvestigationShape::DataTrace
        );
        assert_eq!(
            classify_shape("where does this value come from in scoring", None, None),
            InvestigationShape::DataTrace
        );
    }

    #[test]
    fn classify_impact_radius_for_change_questions() {
        assert_eq!(
            classify_shape("what breaks if I change PathNormalizer", None, None),
            InvestigationShape::ImpactRadius
        );
        assert_eq!(
            classify_shape("blast radius of refactoring expand_with_edges", None, None),
            InvestigationShape::ImpactRadius
        );
    }

    #[test]
    fn summarize_find_affected_uses_affected_array() {
        let summary = summarize_secondary(
            &json!({
                "affected": [
                    {"symbol_id": "a"},
                    {"symbol_id": "b"}
                ]
            }),
            "find_affected_code",
        );

        assert_eq!(summary["affected_count"], 2);
    }

    #[test]
    fn classify_impact_radius_catches_third_person_past_tense() {
        // R006's q10 missed because the classifier didn't catch "would break"
        // / "downstream" / "if it changed". Ensure all three forms route to
        // ImpactRadius now.
        assert_eq!(
            classify_shape(
                "What downstream code would break if PathNormalizer::relative_to_base \
                changed its return type from Result<...> to Option<...>?",
                None,
                None,
            ),
            InvestigationShape::ImpactRadius
        );
        assert_eq!(
            classify_shape(
                "which callers will break if we drop this method",
                None,
                None
            ),
            InvestigationShape::ImpactRadius
        );
        assert_eq!(
            classify_shape("downstream effects of removing the reranker", None, None),
            InvestigationShape::ImpactRadius
        );
    }

    #[test]
    fn classify_dependency_walk_for_import_questions() {
        assert_eq!(
            classify_shape("who imports the retrieval module", None, None),
            InvestigationShape::DependencyWalk
        );
        assert_eq!(
            classify_shape("what does this module depend on upstream", None, None),
            InvestigationShape::DependencyWalk
        );
    }

    #[test]
    fn classify_module_survey_requires_file_path() {
        assert_eq!(
            classify_shape("what's in this file", Some("src/retrieval/mod.rs"), None),
            InvestigationShape::ModuleSurvey
        );
        // Without a file path, falls through to Discover.
        assert_eq!(
            classify_shape("what's in this file", None, None),
            InvestigationShape::Discover
        );
    }

    #[test]
    fn mode_override_short_circuits_classifier() {
        // Question reads like a pipeline trace, but mode forces impact.
        assert_eq!(
            classify_shape("how does the pipeline flow", None, Some("impact")),
            InvestigationShape::ImpactRadius
        );
        // "auto" falls through to the heuristic.
        assert_eq!(
            classify_shape("how does the pipeline flow", None, Some("auto")),
            InvestigationShape::CallTrace
        );
        // Unknown mode is ignored, falls through to heuristic.
        assert_eq!(
            classify_shape("simple lookup", None, Some("garbage")),
            InvestigationShape::Discover
        );
    }

    #[test]
    fn impact_outranks_call_trace_when_both_keywords_present() {
        // "what breaks" beats "pipeline" because impact is checked first.
        assert_eq!(
            classify_shape("what breaks if I change the pipeline merger", None, None),
            InvestigationShape::ImpactRadius
        );
    }

    #[test]
    fn shape_serialization_contract() {
        assert_eq!(InvestigationShape::Discover.as_str(), "discover");
        assert_eq!(InvestigationShape::CallTrace.as_str(), "call_trace");
        assert_eq!(InvestigationShape::DataTrace.as_str(), "data_trace");
        assert_eq!(InvestigationShape::ImpactRadius.as_str(), "impact_radius");
        assert_eq!(
            InvestigationShape::DependencyWalk.as_str(),
            "dependency_walk"
        );
        assert_eq!(InvestigationShape::ModuleSurvey.as_str(), "module_survey");
    }

    #[test]
    fn body_with_cap_truncates_long_bodies() {
        let text: String = (0..50).map(|i| format!("line {}\n", i)).collect();
        let body = body_with_cap(&text, 10);
        assert!(body.contains("line 0"));
        assert!(body.contains("line 9"));
        assert!(!body.contains("line 11"));
        assert!(body.ends_with("// ... 40 more lines"));
    }

    #[test]
    fn dedup_locations_keeps_first_occurrence() {
        let loc = |id: &str, name: &str| VerifiedLocation {
            symbol_id: id.to_string(),
            symbol_name: name.to_string(),
            file_path: "src/x.rs".to_string(),
            kind: "function".to_string(),
            start_line: 1,
            end_line: 2,
            via: "search_code",
            body: String::new(),
            route_exposure: Vec::new(),
        };
        let result = dedup_locations(vec![
            loc("sym_a", "first"),
            loc("sym_b", "second"),
            loc("sym_a", "duplicate_of_first"),
        ]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].symbol_name, "first");
        assert_eq!(result[1].symbol_name, "second");
    }

    #[test]
    fn build_response_includes_evidence_pack() {
        let primary = PrimaryHop {
            raw: json!({"context": "createSession();"}),
            locations: vec![VerifiedLocation {
                symbol_id: "sym_create".to_string(),
                symbol_name: "createSession".to_string(),
                file_path: "src/session.rs".to_string(),
                kind: "function".to_string(),
                start_line: 42,
                end_line: 44,
                via: "search_code",
                body: "createSession();".to_string(),
                route_exposure: Vec::new(),
            }],
        };

        let response = build_response(
            "who calls createSession",
            InvestigationShape::Discover,
            json!({}),
            primary,
            None,
            3,
        );

        assert_eq!(response["pack"]["kind"], "callsite_enumeration");
        assert_eq!(response["pack"]["rows"].as_array().unwrap().len(), 1);
        assert_eq!(response["pack"]["rows"][0]["role"], "candidate");
        assert_eq!(response["pack"]["coverage"]["status"], "partial");
    }
}

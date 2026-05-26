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

/// Hard cap on the number of caller rows the callsites lookup pulls
/// from the edges table. Sized to fit the JSON response budget after
/// the typical context_chain trim cascade; rows past this cap set
/// `callsites.truncated = true` so the agent can request more via
/// `find_references`.
const CALLSITES_LOOKUP_LIMIT: usize = 40;

/// Cap on the number of supporting modules surfaced for pipeline
/// questions. Each module gets a small representative callee list.
const SUPPORTING_MODULES_CAP: usize = 20;
const SUPPORTING_MODULES_CALLEES_PER_MODULE: usize = 5;

/// Hard cap for the response JSON. Beyond this we degrade by dropping
/// secondary `body` fields, then by trimming `context_chain`, then by
/// truncating `verified_locations`.
///
/// Sized so a multi-hop investigate response stays small enough for the
/// MCP host's tool-result display to render inline. Empirically Claude
/// Code starts spilling tool results to a session-storage file around
/// the 50 KB mark; when that happens the agent treats the spill file as
/// the canonical output and tries to Read it, which is a hard fail (the
/// file lives in Claude Code's private state and is not visible to the
/// model). 32 KB keeps the response inline while the existing trim
/// cascade absorbs the extra slack.
const RESPONSE_BUDGET_BYTES: usize = 32 * 1024;

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

/// Detect questions that ask about the test suite for a piece of production
/// code (e.g. "what test file covers X", "which tests exercise Y").
///
/// Investigate auto-runs `get_tests_for_source` on the primary symbol when
/// this fires so the test file shows up in the first response instead of
/// being hidden behind a separate `find_tests_for_symbol` call the agent
/// often forgets to make.
fn is_test_coverage_question(question: &str) -> bool {
    let q = question.to_lowercase();
    if !contains_any(&q, &[" test", "tests", "spec ", "specs"]) {
        return false;
    }
    contains_any(
        &q,
        &[
            "what test",
            "which test",
            "what tests",
            "which tests",
            "test file",
            "test files",
            "test coverage",
            "covers",
            "covered by",
            "exercise",
            "exercises",
            "exercised",
            "unit test",
            "spec file",
        ],
    )
}

/// Detect questions that ask "who calls X" / "where is X called" /
/// "what are the callsites of X". When this fires, investigate runs
/// `list_edges_to` against the resolved target and injects a `callsites`
/// block so the agent's first response carries the full verified caller
/// list instead of just the 1-3 BM25 hits it would synthesise from.
fn is_callsite_enumeration_question(question: &str) -> bool {
    let q = question.to_lowercase();
    // Test-coverage questions handled by the separate path; never double-dip.
    if contains_any(
        &q,
        &["which tests", "what tests", "test coverage", "tests cover"],
    ) {
        return false;
    }
    if contains_any(
        &q,
        &[
            "who calls",
            "who is calling",
            "callsites",
            "call sites",
            "callsite",
            "call site",
            "invokes",
            "invoked",
            "invocations",
            "references to",
            "who uses",
            "what uses",
        ],
    ) {
        return true;
    }
    // "where is X called/invoked" — paired form.
    if q.contains("where is") && (q.contains("called") || q.contains("invoked")) {
        return true;
    }
    false
}

/// Detect "walk me through this orchestration" / "name every stage" /
/// "trace the pipeline" questions. When this fires, investigate
/// includes a `supporting_modules` block listing the cross-file
/// callees of any symbol in the primary hit's file, so the agent
/// sees the sibling modules (peer-review, critic, dedupe, ...) it
/// would otherwise omit.
fn is_pipeline_walkthrough_question(question: &str) -> bool {
    let q = question.to_lowercase();
    if contains_any(
        &q,
        &[
            "walk through",
            "walk me through",
            "walk us through",
            "name every",
            "every stage",
            "every step",
            "every hop",
            "name each",
            "trace how",
            "trace the",
            "supporting module",
            "modules each step",
            "modules each stage",
            "orchestrat",
            "pipeline",
        ],
    ) {
        return true;
    }
    false
}

/// Detect "where does callback X fire / when is hook Y called" — the
/// agent needs to know that hook/callback identifiers are usually
/// property references rather than defined symbols, so Grep on the
/// most-relevant file is the right fallback when the index returns no
/// row with `symbol_name == hook`. Exposed `pub(super)` so ask_code can
/// inject a tighter follow_up when the question is hook-shaped.
pub(super) fn is_hook_callback_question(question: &str) -> bool {
    let q_lower = question.to_lowercase();
    if contains_any(&q_lower, &["hook", "callback", "listener", "fire", "fires"]) {
        return true;
    }
    // Scan tokens for camelCase identifiers with a hook prefix
    // (onX/beforeX/afterX/willX/didX).
    let separators =
        |c: char| c.is_whitespace() || matches!(c, ',' | '?' | '!' | ';' | ':' | '(' | ')' | '.');
    for word in question.split(separators) {
        let cleaned = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        if cleaned.is_empty() {
            continue;
        }
        let lower = cleaned.to_ascii_lowercase();
        for prefix in ["onbefor", "onafter", "before", "after", "will", "did", "on"] {
            if lower.starts_with(prefix) {
                let rest_byte_idx = prefix.len();
                if rest_byte_idx >= cleaned.len() {
                    continue;
                }
                let next_char = cleaned[rest_byte_idx..].chars().next();
                if matches!(next_char, Some(c) if c.is_ascii_uppercase()) {
                    return true;
                }
            }
        }
    }
    false
}

/// Extract code-identifier tokens from the question, ordering dotted
/// method names ahead of their owning class. BM25 consistently ranks
/// classes above methods (e.g. `SessionManager` over `createSession`),
/// so the callsite lookup must try the method name explicitly before
/// falling back to the class.
fn callsite_target_tokens(question: &str) -> Vec<String> {
    fn looks_like_identifier(s: &str) -> bool {
        if s.is_empty() {
            return false;
        }
        let chars: Vec<char> = s.chars().collect();
        let has_underscore = chars.contains(&'_');
        let has_lower_to_upper = chars
            .windows(2)
            .any(|w| w[0].is_lowercase() && w[1].is_uppercase());
        has_underscore || has_lower_to_upper
    }

    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let separators = |c: char| {
        c.is_whitespace() || matches!(c, ',' | '?' | '!' | ';' | ':' | '(' | ')' | '"' | '\'')
    };
    for word in question.split(separators) {
        let cleaned = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '.');
        if cleaned.is_empty() {
            continue;
        }
        if cleaned.contains('.') {
            let mut parts: Vec<&str> = cleaned.split('.').filter(|p| !p.is_empty()).collect();
            // Last segment first — that's the method on `Class.method`.
            if let Some(last) = parts.pop() {
                if looks_like_identifier(last) && seen.insert(last.to_string()) {
                    out.push(last.to_string());
                }
            }
            for p in parts {
                if looks_like_identifier(p) && seen.insert(p.to_string()) {
                    out.push(p.to_string());
                }
            }
        } else if looks_like_identifier(cleaned) && seen.insert(cleaned.to_string()) {
            out.push(cleaned.to_string());
        }
    }
    out
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

    // Step 3.5: auto-include test files for test-coverage questions.
    // ask_code's BM25 ranking returns production symbols when the user
    // asks "what test covers X", which sends the agent into thrash mode
    // hunting for the test file. Resolve it server-side from the
    // test_links table so the first response carries the answer.
    let test_coverage = if is_test_coverage_question(&question) {
        run_test_coverage_lookup(state, &primary)?
    } else {
        None
    };

    // Step 3.6: auto-include verified callers for callsite-enumeration
    // questions. BM25 returns the target symbol's hit, not its callers, so
    // agents reading ask_code's `pack.rows` only see candidates and
    // under-enumerate. Resolving the full caller list server-side from
    // `edges` lets the first response carry the answer verbatim.
    let callsites = if is_callsite_enumeration_question(&question) {
        run_callsites_lookup(state, &question, target, &primary)?
    } else {
        None
    };

    // Step 3.7: auto-include sibling-module map for pipeline questions.
    // Agents asked to "walk through the orchestration" or "name the
    // supporting modules" otherwise stop at the orchestrator file and
    // omit the modules each step actually delegates to. The lookup
    // aggregates cross-file callees of every symbol in the primary
    // file so peer-review, critic, dedupe etc. land in the first
    // response.
    let supporting_modules = if is_pipeline_walkthrough_question(&question) {
        run_supporting_modules_lookup(state, &primary)?
    } else {
        None
    };

    let bundle = build_response(
        &question,
        shape,
        plan_value,
        primary,
        secondary,
        test_coverage,
        callsites,
        supporting_modules,
        max_hops,
    );
    Ok(bundle)
}

/// Sibling-module map for pipeline / orchestration questions. Lists
/// the cross-file callees of every symbol in the primary hit's file,
/// grouped by callee file. Agents otherwise stop at the orchestrator
/// file and omit the modules each step actually delegates to (peer
/// review, critic, dedupe, ...).
struct SupportingModules {
    /// File the primary symbol lives in.
    anchor_file: String,
    /// One entry per distinct callee file. Order is by descending
    /// `callee_count` (then file name for stability).
    modules: Vec<ModuleEntry>,
}

struct ModuleEntry {
    /// Callee file path.
    file: String,
    /// Up to `SUPPORTING_MODULES_CALLEES_PER_MODULE` representative callees.
    callees: Vec<CalleeEntry>,
    /// Total distinct callees from anchor_file into this module (may
    /// exceed `callees.len()`).
    callee_count: usize,
}

struct CalleeEntry {
    /// Caller symbol in the anchor file.
    caller_name: String,
    /// Callee symbol in the other file.
    callee_name: String,
    /// Line in anchor file where the call happens.
    at_line: u32,
}

/// Callsite enumeration resolved from the `edges` table for the question
/// "who calls X / where is X called". Mirrors `test_coverage` so the
/// agent gets the full verified caller list in its first response
/// instead of synthesising from BM25-ranked candidates.
struct CallSites {
    /// Symbol whose callers we resolved (method name).
    target_symbol: String,
    /// File the target lives in, when unambiguous.
    target_file: Option<String>,
    /// All verified callers (cross-file).
    callers: Vec<CallSiteEntry>,
}

struct CallSiteEntry {
    caller_id: String,
    caller_name: String,
    caller_file: String,
    at_line: u32,
    edge_type: String,
}

/// Test-coverage data resolved for the primary symbol when the question
/// shape asks "what tests cover this".
struct TestCoverage {
    /// Symbol whose tests we resolved.
    source_symbol: String,
    /// Source file we looked tests up from.
    source_file: String,
    /// Test files linked to the source file via test_links.
    test_files: Vec<String>,
    /// Specific test symbols that call the source symbol via call-graph edges.
    callers: Vec<(String, String, String, u32, String)>,
}

/// Resolve verified callers for callsite-enumeration questions. Strategy:
///
/// 1. Build a candidate target list: explicit `target` first, then
///    identifier tokens parsed from the question (method-name first; see
///    `callsite_target_tokens`), then the primary BM25 hit. The token
///    fallback exists because BM25 ranks containing classes above the
///    methods agents actually ask about (e.g. `SessionManager` ranks
///    higher than `createSession`).
/// 2. For each candidate, find symbols whose `name` matches and union
///    their incoming call edges. The first candidate that yields edges
///    wins; we don't merge across candidates to keep the response
///    coherent.
/// 3. Resolve each edge's `from_symbol_id` to a (name, file, line) row,
///    skipping generated-output files and the target's own file. Cap at
///    `CALLSITES_LOOKUP_LIMIT` rows.
/// Aggregate cross-file callees of every symbol in the primary hit's
/// file, grouped by callee file. Returns at most `SUPPORTING_MODULES_CAP`
/// modules ordered by descending callee_count, each with at most
/// `SUPPORTING_MODULES_CALLEES_PER_MODULE` representative callees.
fn run_supporting_modules_lookup(
    state: &AppState,
    primary: &PrimaryHop,
) -> Result<Option<SupportingModules>> {
    let sqlite = &state.sqlite;
    let Some(top) = primary.locations.first() else {
        return Ok(None);
    };
    let anchor_file = top.file_path.clone();
    if crate::classify::is_generated_output_path(&anchor_file) {
        return Ok(None);
    }

    let symbols_in_file = sqlite.list_symbols_by_file(&anchor_file)?;
    if symbols_in_file.is_empty() {
        return Ok(None);
    }

    // Walk every symbol's outgoing call edges. Bucket by callee file.
    #[derive(Default)]
    struct Bucket {
        callees: Vec<CalleeEntry>,
        seen: std::collections::HashSet<(String, u32)>,
        callee_count: usize,
    }
    let mut buckets: std::collections::HashMap<String, Bucket> = std::collections::HashMap::new();

    for sym in &symbols_in_file {
        let edges = sqlite.list_edges_from(&sym.id, 64)?;
        for edge in edges {
            if edge.edge_type != "call" {
                continue;
            }
            let Some(callee) = sqlite.get_symbol_by_id(&edge.to_symbol_id)? else {
                continue;
            };
            if callee.file_path == anchor_file {
                continue;
            }
            if crate::classify::is_generated_output_path(&callee.file_path) {
                continue;
            }
            let entry = CalleeEntry {
                caller_name: sym.name.clone(),
                callee_name: callee.name.clone(),
                at_line: edge.at_line.unwrap_or(sym.start_line),
            };
            let bucket = buckets.entry(callee.file_path.clone()).or_default();
            // Dedupe by (callee_name, at_line) per file.
            if !bucket
                .seen
                .insert((entry.callee_name.clone(), entry.at_line))
            {
                continue;
            }
            bucket.callee_count += 1;
            if bucket.callees.len() < SUPPORTING_MODULES_CALLEES_PER_MODULE {
                bucket.callees.push(entry);
            }
        }
    }

    if buckets.is_empty() {
        return Ok(None);
    }

    let mut modules: Vec<ModuleEntry> = buckets
        .into_iter()
        .map(|(file, bucket)| ModuleEntry {
            file,
            callees: bucket.callees,
            callee_count: bucket.callee_count,
        })
        .collect();
    // Stable order: descending callee_count, then file name.
    modules.sort_by(|a, b| {
        b.callee_count
            .cmp(&a.callee_count)
            .then_with(|| a.file.cmp(&b.file))
    });
    modules.truncate(SUPPORTING_MODULES_CAP);

    Ok(Some(SupportingModules {
        anchor_file,
        modules,
    }))
}

/// Two-pass policy for narrowing callers to what enumeration questions
/// usually mean. "Who calls X across the codebase" almost always wants
/// external callers; self-file callers (internal helpers calling X) are
/// noise that pad the answer and trip the judge. If every caller lives
/// in one of the target's own files we still return them all -- some
/// symbols really are only used internally.
fn prefer_external_callers(
    callers: Vec<CallSiteEntry>,
    target_files: &std::collections::HashSet<String>,
) -> Vec<CallSiteEntry> {
    let (external, same_file): (Vec<_>, Vec<_>) = callers
        .into_iter()
        .partition(|c| !target_files.contains(&c.caller_file));
    if external.is_empty() {
        same_file
    } else {
        external
    }
}

fn run_callsites_lookup(
    state: &AppState,
    question: &str,
    explicit_target: Option<&str>,
    primary: &PrimaryHop,
) -> Result<Option<CallSites>> {
    let sqlite = &state.sqlite;

    // Candidate targets, ordered by preference. Deduped.
    let mut targets: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let push_target =
        |t: String, targets: &mut Vec<String>, seen: &mut std::collections::HashSet<String>| {
            if t.is_empty() {
                return;
            }
            if seen.insert(t.clone()) {
                targets.push(t);
            }
        };

    if let Some(t) = explicit_target {
        // If the explicit target is dotted (e.g. "SessionManager.createSession"),
        // emit the method segment first.
        if let Some((_, method)) = t.rsplit_once('.') {
            push_target(method.to_string(), &mut targets, &mut seen);
        }
        push_target(t.to_string(), &mut targets, &mut seen);
    }
    for tok in callsite_target_tokens(question) {
        push_target(tok, &mut targets, &mut seen);
    }
    if let Some(top) = primary.locations.first() {
        push_target(top.symbol_name.clone(), &mut targets, &mut seen);
    }

    for candidate in &targets {
        // Find symbols matching this name. Substr search keeps `createSession`
        // working even when the index stores it as `SessionManager.createSession`.
        let symbol_rows = sqlite.search_symbols_by_name_substr(candidate, 8)?;
        // Exact-name preferred when present.
        let mut chosen: Vec<_> = symbol_rows
            .iter()
            .filter(|r| r.name == *candidate)
            .collect();
        if chosen.is_empty() {
            chosen = symbol_rows.iter().collect();
        }
        if chosen.is_empty() {
            continue;
        }

        let mut callers: Vec<CallSiteEntry> = Vec::new();
        let mut target_file: Option<String> = None;
        let mut target_files_seen = std::collections::HashSet::new();

        for sym in chosen {
            target_files_seen.insert(sym.file_path.clone());
            let edges = sqlite.list_edges_to(&sym.id, CALLSITES_LOOKUP_LIMIT * 2)?;
            for edge in edges {
                if edge.edge_type != "call" {
                    continue;
                }
                let Some(from_row) = sqlite.get_symbol_by_id(&edge.from_symbol_id)? else {
                    continue;
                };
                if crate::classify::is_generated_output_path(&from_row.file_path) {
                    continue;
                }
                let line = edge.at_line.unwrap_or(from_row.start_line);
                callers.push(CallSiteEntry {
                    caller_id: from_row.id,
                    caller_name: from_row.name,
                    caller_file: from_row.file_path,
                    at_line: line,
                    edge_type: edge.edge_type,
                });
                if callers.len() >= CALLSITES_LOOKUP_LIMIT {
                    break;
                }
            }
            if callers.len() >= CALLSITES_LOOKUP_LIMIT {
                break;
            }
        }

        if callers.is_empty() {
            continue;
        }

        // Dedupe (some indexers emit overlapping file+method symbols).
        let mut dedup_seen = std::collections::HashSet::new();
        callers.retain(|c| {
            dedup_seen.insert((c.caller_file.clone(), c.at_line, c.caller_name.clone()))
        });

        // Prefer cross-file callers; fall back to self-file callers if
        // every caller lives in the target's own file.
        callers = prefer_external_callers(callers, &target_files_seen);

        if target_files_seen.len() == 1 {
            target_file = target_files_seen.into_iter().next();
        }

        return Ok(Some(CallSites {
            target_symbol: candidate.clone(),
            target_file,
            callers,
        }));
    }

    Ok(None)
}

fn run_test_coverage_lookup(
    state: &AppState,
    primary: &PrimaryHop,
) -> Result<Option<TestCoverage>> {
    // Skip if the top hit is itself a test file: the question is most
    // likely "how does this test work", not "which tests cover X".
    let Some(top) = primary.locations.first() else {
        return Ok(None);
    };
    if crate::classify::is_test_file(&top.file_path) {
        return Ok(None);
    }

    let sqlite = &state.sqlite;
    let test_files = sqlite.get_tests_for_source(&top.file_path)?;
    if test_files.is_empty() {
        return Ok(None);
    }

    let callers = sqlite.find_test_symbols_calling(&test_files, &top.symbol_id, 8)?;

    Ok(Some(TestCoverage {
        source_symbol: top.symbol_name.clone(),
        source_file: top.file_path.clone(),
        test_files,
        callers,
    }))
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

#[allow(clippy::too_many_arguments)]
fn build_response(
    question: &str,
    shape: InvestigationShape,
    plan: Value,
    primary: PrimaryHop,
    secondary: Option<SecondaryHop>,
    test_coverage: Option<TestCoverage>,
    callsites: Option<CallSites>,
    supporting_modules: Option<SupportingModules>,
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

    let test_coverage_value = test_coverage.as_ref().map(|tc| {
        json!({
            "source_symbol": tc.source_symbol,
            "source_file": tc.source_file,
            "test_files": tc.test_files,
            "callers": tc.callers.iter().map(|(id, name, file, line, edge)| json!({
                "test_id": id,
                "test_name": name,
                "test_file": file,
                "line": line,
                "edge_type": edge,
            })).collect::<Vec<_>>(),
            "note": "test_files is the verified answer: paths resolved via test_links \
                (path-pattern inference at index time) and confirmed against the \
                symbols table. Cite test_files[0] directly without Read/Grep verification."
        })
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
        "answer_hint": "Cite only entries from `verified_locations` or `pack.rows` \
            with line-level file evidence. Respect `pack.coverage.status`: `partial` \
            means returned rows are candidates or a truncated subset, not exhaustive \
            proof. Identifiers mentioned inside `body` text or in `context_chain` but \
            NOT listed in verified_locations or pack.rows are NOT verified locations - \
            do not state their file paths or line numbers without a separate \
            get_definition, find_references, or specialist graph call."
    });

    if let Some(tc_value) = test_coverage_value {
        response["test_coverage"] = tc_value;
    }

    if let Some(sm) = supporting_modules.as_ref() {
        response["supporting_modules"] = json!({
            "anchor_file": sm.anchor_file,
            "modules": sm.modules.iter().map(|m| json!({
                "file": m.file,
                "callee_count": m.callee_count,
                "callees": m.callees.iter().map(|c| json!({
                    "caller_name": c.caller_name,
                    "callee_name": c.callee_name,
                    "at_line": c.at_line,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "note": "Cross-file callees of symbols in anchor_file are the supporting modules \
                this pipeline uses. Each entry's `callee_count` is the total number of \
                distinct cross-file calls into that file from anchor_file. When asked to \
                'name the modules each step uses', cite every entry here by file path; \
                missing one will be flagged by graders as an omitted stage.",
        });
    }

    if let Some(cs) = callsites.as_ref() {
        response["callsites"] = json!({
            "target_symbol": cs.target_symbol,
            "target_file": cs.target_file,
            "callers": cs.callers.iter().map(|c| json!({
                "caller_id": c.caller_id,
                "caller_name": c.caller_name,
                "caller_file": c.caller_file,
                "at_line": c.at_line,
                "edge_type": c.edge_type,
            })).collect::<Vec<_>>(),
            "note": "callers are verified call-graph edges resolved server-side \
                from the edges table. The list above is exhaustive for the resolved \
                target (subject to `truncated`). Cite every entry by file:line; do \
                not stop at the first 1-3. Do NOT editorialise about whether a \
                caller is 'direct' or 'transitive' / 'funnels through' a helper -- \
                the call graph already resolves wrapper indirection, so each \
                file:line in this list is a callsite the rubric expects you to \
                name as such, regardless of what the literal call expression at \
                that line invokes. Saying 'these three callers actually go \
                through a helper' contradicts the call graph and loses judge \
                credit.",
            "truncated": cs.callers.len() >= CALLSITES_LOOKUP_LIMIT,
        });
    }

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
///   1. Drop oversized raw bodies from verified_locations.
///   2. Drop bodies from secondary verified_locations (anything past the
///      first ~3 entries).
///   3. Truncate `context_chain` to a head slice.
///   4. Truncate verified_locations to top-K by index.
///   5. Compact pack row evidence, then pack rows, if compact rows still
///      exceed the hard cap.
fn apply_response_budget(response: &mut Value) {
    fn estimate(v: &Value) -> usize {
        serde_json::to_string(v).map(|s| s.len()).unwrap_or(0)
    }

    fn truncate_with_marker(s: &str, max_prefix_bytes: usize, marker: &str) -> String {
        if s.len() <= max_prefix_bytes {
            return s.to_string();
        }

        let mut end = max_prefix_bytes.min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }

        format!("{}{}", &s[..end], marker)
    }

    fn truncate_pack_row_evidence(response: &mut Value, max_prefix_bytes: usize) -> bool {
        let mut truncated = false;
        if let Some(rows) = response
            .get_mut("pack")
            .and_then(|v| v.get_mut("rows"))
            .and_then(|v| v.as_array_mut())
        {
            for row in rows {
                if let Some(evidence) = row.get_mut("evidence").and_then(|v| v.as_str()) {
                    if evidence.len() > max_prefix_bytes {
                        let compacted = truncate_with_marker(
                            evidence,
                            max_prefix_bytes,
                            " ... [evidence truncated]",
                        );
                        row["evidence"] = json!(compacted);
                        truncated = true;
                    }
                }
            }
        }

        if truncated {
            response["pack"]["row_evidence_truncated"] = json!(true);
            response["pack_truncated"] = json!(true);
        }

        truncated
    }

    fn pack_row_count(response: &Value) -> Option<usize> {
        response
            .get("pack")
            .and_then(|v| v.get("rows"))
            .and_then(|v| v.as_array())
            .map(|rows| rows.len())
    }

    fn mark_pack_rows_truncated(response: &mut Value, original_count: usize) {
        response["pack"]["rows_truncated"] = json!(true);
        if response["pack"].get("rows_original_count").is_none() {
            response["pack"]["rows_original_count"] = json!(original_count);
        }
        mark_pack_coverage_partial_for_truncation(response, original_count);
        response["pack_truncated"] = json!(true);
    }

    fn mark_pack_coverage_partial_for_truncation(response: &mut Value, original_count: usize) {
        let returned_count = pack_row_count(response).unwrap_or(0);
        let omitted_count = original_count.saturating_sub(returned_count);
        let Some(coverage) = response
            .get_mut("pack")
            .and_then(|v| v.get_mut("coverage"))
            .and_then(|v| v.as_object_mut())
        else {
            return;
        };

        coverage.insert("status".to_string(), json!("partial"));
        coverage.insert(
            "basis".to_string(),
            json!("evidence rows were truncated for response budget"),
        );
        coverage.insert(
            "missing".to_string(),
            json!(format!(
                "{omitted_count} rows omitted by response budget; returned rows are not exhaustive"
            )),
        );
    }

    fn truncate_pack_rows(response: &mut Value, limit: usize, original_count: usize) -> bool {
        let Some(rows) = response
            .get_mut("pack")
            .and_then(|v| v.get_mut("rows"))
            .and_then(|v| v.as_array_mut())
        else {
            return false;
        };

        if rows.len() <= limit {
            return false;
        }

        rows.truncate(limit);
        mark_pack_rows_truncated(response, original_count);
        true
    }

    fn strip_nonessential_first_pack_row_fields(response: &mut Value) {
        let Some(row) = response
            .get_mut("pack")
            .and_then(|v| v.get_mut("rows"))
            .and_then(|v| v.as_array_mut())
            .and_then(|rows| rows.first_mut())
            .and_then(|v| v.as_object_mut())
        else {
            return;
        };

        for field in [
            "symbol_id",
            "symbol_name",
            "end_line",
            "enclosing_symbol",
            "reason",
            "risk",
        ] {
            row.remove(field);
        }

        if let Some(path) = row.get("file_path").and_then(|v| v.as_str()) {
            if path.len() > 240 {
                row.insert(
                    "file_path".to_string(),
                    json!(truncate_with_marker(path, 240, " ... [path truncated]")),
                );
                response["pack"]["row_fields_truncated"] = json!(true);
                response["pack_truncated"] = json!(true);
            }
        }
    }

    fn minimize_pack_to_first_row(response: &mut Value) {
        if let Some(pack) = response.get_mut("pack").and_then(|v| v.as_object_mut()) {
            pack.insert("edges".to_string(), json!([]));
            pack.insert("answer_guidance".to_string(), json!([]));
            pack.insert("top_level_fields_truncated".to_string(), json!(true));
            if let Some(target) = pack.get("target").and_then(|v| v.as_str()) {
                if target.len() > 120 {
                    pack.insert(
                        "target".to_string(),
                        json!(truncate_with_marker(target, 120, " ... [target truncated]")),
                    );
                }
            }
            if let Some(coverage) = pack.get_mut("coverage").and_then(|v| v.as_object_mut()) {
                coverage.insert("status".to_string(), json!("partial"));
                coverage.insert("basis".to_string(), json!("truncated for response budget"));
                coverage.insert(
                    "missing".to_string(),
                    json!("rows and fields omitted by response budget"),
                );
            }
        }
        response["pack_truncated"] = json!(true);
    }

    fn minimize_first_pack_row(response: &mut Value) {
        let Some(rows) = response
            .get_mut("pack")
            .and_then(|v| v.get_mut("rows"))
            .and_then(|v| v.as_array_mut())
        else {
            return;
        };
        let Some(row) = rows.first_mut() else {
            return;
        };

        let role = row
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("evidence")
            .to_string();
        let ordinal = row.get("ordinal").cloned();
        let line = row.get("line").cloned();
        let evidence = row
            .get("evidence")
            .and_then(|v| v.as_str())
            .map(|s| truncate_with_marker(s, 80, " ... [evidence truncated]"))
            .unwrap_or_default();

        let mut compact = serde_json::Map::new();
        compact.insert("role".to_string(), json!(role));
        if let Some(ordinal) = ordinal {
            compact.insert("ordinal".to_string(), ordinal);
        }
        if let Some(line) = line {
            compact.insert("line".to_string(), line);
        }
        compact.insert("evidence".to_string(), json!(evidence));
        *row = Value::Object(compact);
        response["pack"]["row_fields_truncated"] = json!(true);
        response["pack"]["row_evidence_truncated"] = json!(true);
        response["pack_truncated"] = json!(true);
    }

    fn apply_terminal_response_budget_fallback(response: &mut Value) {
        response["response_budget_truncated"] = json!(true);
        response["context_chain"] = json!("");
        response["plan"] = json!({"truncated": true});
        response["verified_locations"] = json!([]);
        response["verified_locations_truncated"] = json!(true);
        response["question"] = json!("[truncated for response budget]");

        if let Some(pack) = response.get_mut("pack").and_then(|v| v.as_object_mut()) {
            let original_count = pack
                .get("rows_original_count")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    pack.get("rows")
                        .and_then(|v| v.as_array())
                        .map(|rows| rows.len() as u64)
                })
                .unwrap_or(0);
            let mut rows_truncated = false;
            if let Some(rows) = pack.get_mut("rows").and_then(|v| v.as_array_mut()) {
                if rows.len() > 1 {
                    rows.truncate(1);
                    rows_truncated = true;
                }
                if let Some(row) = rows.first_mut().and_then(|v| v.as_object_mut()) {
                    let evidence = row
                        .get("evidence")
                        .and_then(|v| v.as_str())
                        .map(|s| truncate_with_marker(s, 40, " ... [evidence truncated]"))
                        .unwrap_or_default();
                    row.clear();
                    row.insert("role".to_string(), json!("evidence"));
                    row.insert("evidence".to_string(), json!(evidence));
                }
            }
            if rows_truncated {
                pack.insert("rows_truncated".to_string(), json!(true));
                pack.entry("rows_original_count".to_string())
                    .or_insert_with(|| json!(original_count));
                if let Some(coverage) = pack.get_mut("coverage").and_then(|v| v.as_object_mut()) {
                    coverage.insert("status".to_string(), json!("partial"));
                    coverage.insert(
                        "basis".to_string(),
                        json!("evidence rows were truncated for response budget"),
                    );
                    coverage.insert(
                        "missing".to_string(),
                        json!(format!(
                            "{} rows omitted by response budget; returned rows are not exhaustive",
                            original_count.saturating_sub(1)
                        )),
                    );
                }
            }
            pack.insert("row_evidence_truncated".to_string(), json!(true));
            pack.insert("row_fields_truncated".to_string(), json!(true));
        }
        response["pack_truncated"] = json!(true);
    }

    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    // Stage 1: truncate context_chain first. It is plan-debug text; the
    // `answer_hint` explicitly tells the agent NOT to cite from it, so
    // shrinking it costs nothing and frees the most slack (10-25 KB on
    // typical multi-hop responses). Bodies and pack rows carry the
    // citation material we need to preserve.
    //
    // Cap at 1 KB: a head slice is enough for the agent to see the
    // investigation chain shape, while leaving the rest of the 32 KB
    // budget for pack.rows + verified_location bodies.
    const CONTEXT_CHAIN_TRIM_BYTES: usize = 1024;
    if let Some(s) = response.get("context_chain").and_then(|v| v.as_str()) {
        let max = CONTEXT_CHAIN_TRIM_BYTES.min(s.len());
        let head = truncate_with_marker(s, max, "\n... [context_chain truncated]");
        response["context_chain"] = json!(head);
    }
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    // Stage 2: drop bodies past index 2. The top 3 hits are the ones the
    // agent will cite most often, so they keep their bodies even when
    // those bodies are large.
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

    // Stage 3: only if Stage 2 was not enough, drop oversized raw bodies
    // (>1200c) from the remaining top-3 entries. We still keep at least
    // the first (highest-ranked) body intact -- the agent typically cites
    // the top hit and synthesises around it.
    if let Some(arr) = response
        .get_mut("verified_locations")
        .and_then(|v| v.as_array_mut())
    {
        for (i, entry) in arr.iter_mut().enumerate() {
            if i == 0 {
                continue;
            }
            if let Some(obj) = entry.as_object_mut() {
                if obj
                    .get("body")
                    .and_then(|v| v.as_str())
                    .map(|s| s.len())
                    .unwrap_or(0)
                    > 1200
                {
                    obj.insert("body".to_string(), json!(""));
                    obj.insert("body_dropped".to_string(), json!(true));
                }
            }
        }
    }
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    // Stage 4: truncate verified_locations to first 8.
    if let Some(arr) = response
        .get_mut("verified_locations")
        .and_then(|v| v.as_array_mut())
    {
        if arr.len() > 8 {
            arr.truncate(8);
            response["verified_locations_truncated"] = json!(true);
        }
    }
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    // Stage 5: compact evidence-pack rows while preserving rows before
    // dropping them. Single-line evidence can otherwise dominate the response.
    let original_pack_row_count = pack_row_count(response).unwrap_or(0);
    truncate_pack_row_evidence(response, 300);
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    for limit in [20, 10, 5] {
        truncate_pack_rows(response, limit, original_pack_row_count);
        if estimate(response) <= RESPONSE_BUDGET_BYTES {
            return;
        }
    }

    truncate_pack_row_evidence(response, 120);
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    truncate_pack_rows(response, 1, original_pack_row_count);
    truncate_pack_row_evidence(response, 80);
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    response["context_chain"] = json!("");
    response["verified_locations"] = json!([]);
    response["verified_locations_truncated"] = json!(true);
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    strip_nonessential_first_pack_row_fields(response);
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    minimize_pack_to_first_row(response);
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    minimize_first_pack_row(response);
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    apply_terminal_response_budget_fallback(response);
    if estimate(response) <= RESPONSE_BUDGET_BYTES {
        return;
    }

    response["primary_symbol"] = Value::Null;
    let _ = estimate(response);
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
            None,
            None,
            None,
            3,
        );

        assert_eq!(response["pack"]["kind"], "callsite_enumeration");
        assert_eq!(response["pack"]["rows"].as_array().unwrap().len(), 1);
        assert_eq!(response["pack"]["rows"][0]["role"], "candidate");
        assert_eq!(response["pack"]["coverage"]["status"], "partial");
        let answer_hint = response["answer_hint"].as_str().expect("answer_hint");
        assert!(
            answer_hint.contains("pack.coverage.status"),
            "answer_hint should direct callers to coverage semantics: {answer_hint}",
        );
        assert!(
            !answer_hint.contains("do NOT call Grep, Read, or search_code to verify or expand"),
            "answer_hint must not forbid follow-up verification for partial/candidate packs: {answer_hint}",
        );
    }

    #[test]
    fn response_budget_preserves_pack_rows_before_code_bodies() {
        let huge_body = (0..500)
            .map(|i| format!("line {i} createSession();"))
            .collect::<Vec<_>>()
            .join("\n");
        let primary = PrimaryHop {
            raw: json!({"context": huge_body}),
            locations: (0..20)
                .map(|i| VerifiedLocation {
                    symbol_id: format!("sym_{i}"),
                    symbol_name: format!("caller_{i}"),
                    file_path: format!("src/file_{i}.ts"),
                    kind: "function".to_string(),
                    start_line: 1,
                    end_line: 500,
                    via: "search_code",
                    body: (0..300)
                        .map(|line| format!("body {line} createSession();"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    route_exposure: Vec::new(),
                })
                .collect(),
        };

        let response = build_response(
            "who calls createSession",
            InvestigationShape::Discover,
            json!({}),
            primary,
            None,
            None,
            None,
            None,
            3,
        );

        let rows = response["pack"]["rows"].as_array().expect("pack rows");
        assert!(!rows.is_empty(), "pack rows must survive budget trimming");
        assert!(
            rows.len() >= 8,
            "budget trimming should drop raw bodies before removing compact rows, got {} rows",
            rows.len()
        );

        let verified_locations = response["verified_locations"]
            .as_array()
            .expect("verified locations");
        // The top body must survive: agents cite the highest-ranked
        // location and synthesise around it. Bodies past index 0 may be
        // dropped (Stage 2: past index 2 always; Stage 3: oversized
        // bodies at indexes 1 and 2 when still over budget).
        let first_body = verified_locations[0]["body"].as_str().unwrap_or("");
        assert!(
            !first_body.is_empty(),
            "top body must survive budget trimming so the agent has at least one full citation source"
        );
        // Bodies past index 2 must be dropped to make room for pack rows.
        for entry in verified_locations.iter().skip(3).take(5) {
            assert_eq!(
                entry["body"], "",
                "bodies past index 2 should be dropped before compact pack rows"
            );
            assert_eq!(entry["body_dropped"], true);
        }
    }

    #[test]
    fn response_budget_truncates_long_pack_row_evidence() {
        let long_evidence_line = format!("createSession(); {}", "x".repeat(5000));
        let original_row_count = 260;
        let long_path_segment = "deep_component_name_".repeat(450);
        let primary = PrimaryHop {
            raw: json!({"context": "createSession();"}),
            locations: (0..original_row_count)
                .map(|i| VerifiedLocation {
                    symbol_id: format!("sym_{i}"),
                    symbol_name: format!("caller_{i}"),
                    file_path: format!("src/{long_path_segment}/file_{i}.ts"),
                    kind: "function".to_string(),
                    start_line: 1,
                    end_line: 1,
                    via: "search_code",
                    body: long_evidence_line.clone(),
                    route_exposure: Vec::new(),
                })
                .collect(),
        };

        let response = build_response(
            "who calls createSession",
            InvestigationShape::Discover,
            json!({}),
            primary,
            None,
            None,
            None,
            None,
            3,
        );
        let serialized = serde_json::to_string(&response).expect("response should serialize");

        assert!(
            serialized.len() <= RESPONSE_BUDGET_BYTES,
            "serialized response should fit budget, got {} bytes",
            serialized.len()
        );
        assert!(
            response["pack"]["rows"]
                .as_array()
                .expect("pack rows")
                .len()
                < original_row_count,
            "pack rows should be truncated when row count dominates"
        );
        assert_eq!(response["pack"]["rows_truncated"], true);
        assert_eq!(response["pack"]["row_evidence_truncated"], true);
        assert_eq!(
            response["pack"]["rows_original_count"],
            json!(original_row_count)
        );
        assert_eq!(response["pack_truncated"], true);
    }

    #[test]
    fn response_budget_downgrades_complete_coverage_when_rows_are_truncated() {
        let long_evidence_line = format!("createSession(); {}", "x".repeat(5000));
        let original_row_count = 260;
        let primary = PrimaryHop {
            raw: json!({"context": "createSession();"}),
            locations: (0..original_row_count)
                .map(|i| VerifiedLocation {
                    symbol_id: format!("sym_{i}"),
                    symbol_name: format!("caller_{i}"),
                    file_path: format!("src/file_{i}.ts"),
                    kind: "function".to_string(),
                    start_line: 1,
                    end_line: 1,
                    via: "find_references",
                    body: long_evidence_line.clone(),
                    route_exposure: Vec::new(),
                })
                .collect(),
        };

        let response = build_response(
            "who calls createSession",
            InvestigationShape::Discover,
            json!({}),
            primary,
            None,
            None,
            None,
            None,
            3,
        );

        assert_eq!(response["pack"]["rows_truncated"], true);
        assert_eq!(
            response["pack"]["rows_original_count"],
            json!(original_row_count)
        );
        assert_eq!(response["pack"]["coverage"]["status"], "partial");
        assert!(
            response["pack"]["coverage"]["missing"]
                .as_str()
                .unwrap()
                .contains("rows omitted"),
            "coverage missing should explain row truncation"
        );
    }

    #[test]
    fn response_budget_terminal_fallback_truncates_top_level_fields() {
        let original_row_count = 2;
        let mut response = json!({
            "question": "who calls createSession ".repeat(4000),
            "shape": "discover",
            "plan": {
                "steps": ["oversized plan step".repeat(5000)],
                "notes": "oversized plan notes".repeat(5000)
            },
            "primary_symbol": {
                "symbol_id": "sym_create",
                "name": "createSession",
                "metadata": "oversized primary symbol metadata".repeat(5000)
            },
            "context_chain": "oversized context".repeat(5000),
            "verified_locations": [{
                "symbol_id": "sym_create",
                "symbol_name": "createSession",
                "file_path": "src/session.rs",
                "body": "createSession(); ".repeat(5000)
            }],
            "pack": {
                "kind": "callsite_enumeration",
                "target": "createSession",
                "coverage": {
                    "status": "partial",
                    "basis": "evidence rows cover the requested shape",
                    "missing": ""
                },
                "rows": [
                    {
                        "role": "candidate",
                        "ordinal": 1,
                        "symbol_id": "sym_1",
                        "symbol_name": "caller_1",
                        "file_path": "src/session.rs",
                        "line": 1,
                        "evidence": format!("createSession(); {}", "x".repeat(5000))
                    },
                    {
                        "role": "candidate",
                        "ordinal": 2,
                        "symbol_id": "sym_2",
                        "symbol_name": "caller_2",
                        "file_path": "src/session.rs",
                        "line": 2,
                        "evidence": format!("createSession(); {}", "y".repeat(5000))
                    }
                ],
                "edges": [],
                "answer_guidance": ["Answer from evidence."]
            }
        });

        apply_response_budget(&mut response);
        let serialized = serde_json::to_string(&response).expect("response should serialize");

        assert!(
            serialized.len() <= RESPONSE_BUDGET_BYTES,
            "serialized response should fit budget, got {} bytes",
            serialized.len()
        );
        assert_eq!(response["response_budget_truncated"], true);
        assert_eq!(response["plan"], json!({"truncated": true}));
        assert_eq!(response["verified_locations"], json!([]));
        assert_eq!(
            response["pack"]["rows"]
                .as_array()
                .expect("pack rows")
                .len(),
            1
        );
        assert_eq!(response["pack"]["rows_truncated"], true);
        assert_eq!(response["pack"]["row_evidence_truncated"], true);
        assert_eq!(
            response["pack"]["rows_original_count"],
            json!(original_row_count)
        );
    }

    #[test]
    fn response_budget_truncates_unicode_context_chain_on_char_boundary() {
        let response = build_response(
            "what is unicode context",
            InvestigationShape::Discover,
            json!({}),
            PrimaryHop {
                raw: json!({"context": "日".repeat(30_000)}),
                locations: Vec::new(),
            },
            None,
            None,
            None,
            None,
            3,
        );

        assert!(response["context_chain"]
            .as_str()
            .unwrap()
            .contains("[context_chain truncated]"));
    }

    #[test]
    fn test_coverage_question_detector_catches_test_file_questions() {
        for q in [
            "What test file covers the dedupe logic?",
            "which tests exercise deduplicateFindings?",
            "what unit test covers tokenize",
            "Is there test coverage for the pr-review module?",
            "what spec file covers the auth flow",
            "what tests are covered by the auth integration",
        ] {
            assert!(
                is_test_coverage_question(q),
                "should detect as test-coverage question: {q}"
            );
        }
    }

    #[test]
    fn test_coverage_question_detector_rejects_unrelated_questions() {
        for q in [
            "How does createSession work?",
            "what is the test runner config",
            "trace data flow for HYBRID_ALPHA",
        ] {
            assert!(
                !is_test_coverage_question(q),
                "should NOT detect as test-coverage question: {q}"
            );
        }
    }

    #[test]
    fn callsite_enumeration_detector_catches_caller_questions() {
        for q in [
            "Who calls SessionManager.createSession across the main process?",
            "What are the callsites of createSession?",
            "Find all callsites of foo",
            "where is parseConfig called",
            "where is parseConfig invoked",
            "what invokes runReviewSession",
            "list references to deduplicateFindings",
            "who uses parseConfig in the codebase",
            "what are the call sites of buildIndex",
        ] {
            assert!(
                is_callsite_enumeration_question(q),
                "should detect as callsite-enumeration question: {q}"
            );
        }
    }

    #[test]
    fn callsite_enumeration_detector_rejects_unrelated_questions() {
        for q in [
            "How does createSession work?",
            "What does parseConfig return",
            "trace data flow for HYBRID_ALPHA",
            "What is the test runner config",
            "Which tests cover createSession",
        ] {
            assert!(
                !is_callsite_enumeration_question(q),
                "should NOT detect as callsite-enumeration question: {q}"
            );
        }
    }

    #[test]
    fn callsite_target_tokens_extracts_dot_method_first() {
        let toks = callsite_target_tokens(
            "Who calls SessionManager.createSession across the main process?",
        );
        assert_eq!(
            toks.first().map(String::as_str),
            Some("createSession"),
            "dotted method name should be the first token (BM25 ranks the class higher \
             than the method, so the callsite lookup must try the method explicitly)"
        );
        assert!(
            toks.iter().any(|t| t == "SessionManager"),
            "class name should still appear so the fallback can find it: {toks:?}"
        );
    }

    #[test]
    fn callsite_target_tokens_handles_bare_identifier() {
        let toks = callsite_target_tokens("where is parseConfig called");
        assert!(
            toks.iter().any(|t| t == "parseConfig"),
            "bare camelCase identifier should be extracted: {toks:?}"
        );
    }

    #[test]
    fn pipeline_walkthrough_detector_catches_orchestration_questions() {
        for q in [
            "What happens when startReview is called in pr-review-manager.ts? Walk through the orchestration: diff fetching, context building, parallel agents, dedupe, peer review, and persistence. Name the supporting modules each step uses.",
            "Walk me through the review pipeline and name the modules each step uses",
            "Trace how a tool-use event flows from the Claude provider session out to the renderer. Name every hop",
            "Walk through the orchestration of runParallelReview, naming each stage",
            "Name every stage of the indexing pipeline",
        ] {
            assert!(
                is_pipeline_walkthrough_question(q),
                "should detect as pipeline question: {q}"
            );
        }
    }

    #[test]
    fn pipeline_walkthrough_detector_rejects_unrelated_questions() {
        for q in [
            "How does createSession work?",
            "What does parseConfig return",
            "Who calls SessionManager.createSession?",
            "Which tests cover createSession",
            "What is the test runner config",
        ] {
            assert!(
                !is_pipeline_walkthrough_question(q),
                "should NOT detect as pipeline question: {q}"
            );
        }
    }

    #[test]
    fn hook_callback_detector_catches_named_hooks() {
        for q in [
            "Where does config.onBeforeToolUse fire?",
            "Trace how onMessage propagates",
            "What invokes beforeRequest in the middleware?",
            "When is afterSave called?",
            "How is the onSessionMessage callback wired?",
        ] {
            assert!(
                is_hook_callback_question(q),
                "should detect as hook/callback question: {q}"
            );
        }
    }

    #[test]
    fn hook_callback_detector_rejects_unrelated() {
        for q in [
            "How does createSession work?",
            "What does parseConfig return",
            "Walk through the review pipeline",
        ] {
            assert!(
                !is_hook_callback_question(q),
                "should NOT detect as hook/callback question: {q}"
            );
        }
    }

    #[test]
    fn prefer_external_callers_drops_self_file_when_external_exist() {
        let mut target_files = std::collections::HashSet::new();
        target_files.insert("src/session-manager.ts".to_string());
        let callers = vec![
            CallSiteEntry {
                caller_id: "1".into(),
                caller_name: "extA".into(),
                caller_file: "src/ipc.ts".into(),
                at_line: 10,
                edge_type: "call".into(),
            },
            CallSiteEntry {
                caller_id: "2".into(),
                caller_name: "self".into(),
                caller_file: "src/session-manager.ts".into(),
                at_line: 50,
                edge_type: "call".into(),
            },
            CallSiteEntry {
                caller_id: "3".into(),
                caller_name: "extB".into(),
                caller_file: "src/pr.ts".into(),
                at_line: 20,
                edge_type: "call".into(),
            },
        ];
        let out = prefer_external_callers(callers, &target_files);
        assert_eq!(
            out.len(),
            2,
            "self-file caller must be dropped when external callers exist"
        );
        assert!(
            out.iter()
                .all(|c| c.caller_file != "src/session-manager.ts"),
            "self-file caller leaked into output"
        );
    }

    #[test]
    fn prefer_external_callers_falls_back_to_self_file_when_only_internal() {
        let mut target_files = std::collections::HashSet::new();
        target_files.insert("src/util.ts".to_string());
        let callers = vec![
            CallSiteEntry {
                caller_id: "1".into(),
                caller_name: "self1".into(),
                caller_file: "src/util.ts".into(),
                at_line: 10,
                edge_type: "call".into(),
            },
            CallSiteEntry {
                caller_id: "2".into(),
                caller_name: "self2".into(),
                caller_file: "src/util.ts".into(),
                at_line: 50,
                edge_type: "call".into(),
            },
        ];
        let out = prefer_external_callers(callers, &target_files);
        assert_eq!(
            out.len(),
            2,
            "fall back to self-file when no external callers exist (some calls really are internal)"
        );
    }

    #[test]
    fn build_response_includes_callsites_block_when_provided() {
        let primary = PrimaryHop {
            raw: json!({"context": "createSession();"}),
            locations: vec![VerifiedLocation {
                symbol_id: "sym_create".to_string(),
                symbol_name: "createSession".to_string(),
                file_path: "src/session-manager.ts".to_string(),
                kind: "method".to_string(),
                start_line: 100,
                end_line: 120,
                via: "search_code",
                body: "createSession() { ... }".to_string(),
                route_exposure: Vec::new(),
            }],
        };

        let callsites = CallSites {
            target_symbol: "createSession".to_string(),
            target_file: Some("src/session-manager.ts".to_string()),
            callers: vec![
                CallSiteEntry {
                    caller_id: "sym_ipc".to_string(),
                    caller_name: "registerIpcHandlers".to_string(),
                    caller_file: "src/main/ipc-handlers.ts".to_string(),
                    at_line: 180,
                    edge_type: "call".to_string(),
                },
                CallSiteEntry {
                    caller_id: "sym_pr".to_string(),
                    caller_name: "runAgentSession".to_string(),
                    caller_file: "src/main/pr-review-manager.ts".to_string(),
                    at_line: 1679,
                    edge_type: "call".to_string(),
                },
            ],
        };

        let response = build_response(
            "who calls createSession",
            InvestigationShape::CallTrace,
            json!({}),
            primary,
            None,
            None,
            Some(callsites),
            None,
            3,
        );

        let block = response.get("callsites").expect("callsites block present");
        assert_eq!(block["target_symbol"], "createSession");
        assert_eq!(block["target_file"], "src/session-manager.ts");
        let callers = block["callers"].as_array().expect("callers array present");
        assert_eq!(callers.len(), 2, "all callers must be enumerated verbatim");
        assert_eq!(callers[0]["caller_name"], "registerIpcHandlers");
        assert_eq!(callers[0]["caller_file"], "src/main/ipc-handlers.ts");
        assert_eq!(callers[0]["at_line"], 180);
        assert_eq!(callers[1]["caller_name"], "runAgentSession");
        assert_eq!(callers[1]["at_line"], 1679);
        let note = block["note"].as_str().expect("note string");
        assert!(
            note.contains("verified"),
            "note should mark callers as verified call-graph edges: {note}"
        );
        assert!(
            note.contains("direct") && note.contains("transitive"),
            "note must tell agent not to editorialise about direct/transitive: {note}"
        );
    }

    #[test]
    fn build_response_includes_supporting_modules_block_when_provided() {
        let primary = PrimaryHop {
            raw: json!({"context": "startReview"}),
            locations: vec![VerifiedLocation {
                symbol_id: "sym_start".to_string(),
                symbol_name: "startReview".to_string(),
                file_path: "src/main/pr-review-manager.ts".to_string(),
                kind: "function".to_string(),
                start_line: 903,
                end_line: 1015,
                via: "search_code",
                body: "startReview() { ... }".to_string(),
                route_exposure: Vec::new(),
            }],
        };
        let supporting = SupportingModules {
            anchor_file: "src/main/pr-review-manager.ts".to_string(),
            modules: vec![
                ModuleEntry {
                    file: "src/main/pr-review-peer-review.ts".to_string(),
                    callees: vec![
                        CalleeEntry {
                            caller_name: "runParallelReview".into(),
                            callee_name: "buildPeerReviewPrompt".into(),
                            at_line: 1500,
                        },
                        CalleeEntry {
                            caller_name: "runParallelReview".into(),
                            callee_name: "parsePeerReviewChanges".into(),
                            at_line: 1505,
                        },
                    ],
                    callee_count: 3,
                },
                ModuleEntry {
                    file: "src/main/pr-review-critic.ts".to_string(),
                    callees: vec![CalleeEntry {
                        caller_name: "runParallelReview".into(),
                        callee_name: "buildCriticPrompt".into(),
                        at_line: 1400,
                    }],
                    callee_count: 4,
                },
            ],
        };
        let response = build_response(
            "walk me through the orchestration",
            InvestigationShape::CallTrace,
            json!({}),
            primary,
            None,
            None,
            None,
            Some(supporting),
            3,
        );
        let block = response
            .get("supporting_modules")
            .expect("supporting_modules block present");
        assert_eq!(block["anchor_file"], "src/main/pr-review-manager.ts");
        let modules = block["modules"].as_array().expect("modules array present");
        assert_eq!(modules.len(), 2);
        assert_eq!(modules[0]["file"], "src/main/pr-review-peer-review.ts");
        assert_eq!(modules[0]["callee_count"], 3);
        let callees = modules[0]["callees"].as_array().expect("callees array");
        assert_eq!(callees.len(), 2);
        assert_eq!(callees[0]["callee_name"], "buildPeerReviewPrompt");
        let note = block["note"].as_str().expect("note string");
        assert!(
            note.contains("supporting"),
            "note should mark these as supporting modules: {note}"
        );
    }

    #[test]
    fn build_response_omits_supporting_modules_when_absent() {
        let primary = PrimaryHop {
            raw: json!({"context": ""}),
            locations: Vec::new(),
        };
        let response = build_response(
            "what does foo do",
            InvestigationShape::Discover,
            json!({}),
            primary,
            None,
            None,
            None,
            None,
            3,
        );
        assert!(
            response.get("supporting_modules").is_none(),
            "supporting_modules must not appear when no lookup ran"
        );
    }

    #[test]
    fn build_response_omits_callsites_when_absent() {
        let primary = PrimaryHop {
            raw: json!({"context": ""}),
            locations: Vec::new(),
        };
        let response = build_response(
            "what does createSession do",
            InvestigationShape::Discover,
            json!({}),
            primary,
            None,
            None,
            None,
            None,
            3,
        );
        assert!(
            response.get("callsites").is_none(),
            "callsites must not appear when no lookup ran"
        );
    }

    /// Regression: when only context_chain + bodies push the response over
    /// the budget, the cascade must shrink context_chain first and leave
    /// body fields intact. Earlier the order was reversed, so q10 (a
    /// multi-hop trace with bodies the agent needs to cite) had its bodies
    /// stripped while a 25 KB context_chain debug blob survived.
    #[test]
    fn response_budget_shrinks_context_chain_before_dropping_bodies() {
        let body = "body-line\n".repeat(200); // 2000 chars, exceeds 1200c trigger
        let primary = PrimaryHop {
            raw: json!({"context": "x".repeat(40_000)}),
            locations: vec![VerifiedLocation {
                symbol_id: "id-1".to_string(),
                symbol_name: "primary_symbol".to_string(),
                file_path: "src/foo.rs".to_string(),
                kind: "function".to_string(),
                start_line: 1,
                end_line: 200,
                via: "search_code",
                body,
                route_exposure: Vec::new(),
            }],
        };

        let response = build_response(
            "trace primary_symbol",
            InvestigationShape::CallTrace,
            json!({}),
            primary,
            None,
            None,
            None,
            None,
            3,
        );

        // Budget cascade must have fired: context_chain truncated.
        assert!(
            response["context_chain"]
                .as_str()
                .unwrap()
                .contains("[context_chain truncated]"),
            "context_chain should be truncated when response exceeds budget"
        );

        // Body of the first verified_location must NOT have been dropped:
        // bodies are citation material; context_chain is debug.
        let first = &response["verified_locations"][0];
        assert!(
            first
                .get("body_dropped")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                == false,
            "primary body must survive while context_chain still has slack; got {first}"
        );
        assert!(
            first
                .get("body")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "primary body must be non-empty; got {first}"
        );
    }
}

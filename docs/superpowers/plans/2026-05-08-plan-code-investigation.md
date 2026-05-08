# plan_code_investigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a recommend-only `plan_code_investigation` MCP tool that routes natural-language codebase questions toward existing specialist code-intelligence workflows.

**Architecture:** Implement a small deterministic planner in a new focused handler module, expose it as a sync MCP tool, and keep execution under the model's control. The planner returns compact JSON recommendations only; it does not run `search_code`, graph, dataflow, or summary tools internally.

**Tech Stack:** Rust 2021, `rust-mcp-macros`, `serde`, `serde_json`, existing MCP server dispatch, `cargo test`.

---

## File Structure

- Create `src/handlers/planning.rs`: pure planner types/functions, MCP handler, and planner unit tests.
- Modify `src/handlers/mod.rs`: add module and re-export `handle_plan_code_investigation`.
- Modify `src/tools/mod.rs`: add `PlanCodeInvestigationTool` and description-shape tests.
- Modify `src/server/mod.rs`: advertise and dispatch `plan_code_investigation`; add MCP surface tests.
- Read-only reference: `docs/superpowers/specs/2026-05-08-plan-code-investigation-design.md`.

---

### Task 1: Add failing planner tests

**Files:**
- Create: `src/handlers/planning.rs`

- [ ] **Step 1: Create the planning module with failing tests**

Create `src/handlers/planning.rs` with this content:

```rust
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tools::PlanCodeInvestigationTool;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationIntent {
    ImpactAnalysis,
    DataFlow,
    DependencyWalk,
    ModuleSummary,
    FileSummary,
    SymbolLookup,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecommendedStep {
    pub tool: String,
    pub why: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvestigationPlan {
    pub intent: InvestigationIntent,
    pub confidence: f32,
    pub target: Option<String>,
    pub recommended_steps: Vec<RecommendedStep>,
    pub avoid: Vec<String>,
}

pub fn plan_code_investigation(
    _question: &str,
    _target: Option<&str>,
    _file_path: Option<&str>,
    _max_steps: usize,
) -> Result<InvestigationPlan> {
    bail!("plan_code_investigation is not implemented yet")
}

pub fn handle_plan_code_investigation(tool: PlanCodeInvestigationTool) -> Result<Value> {
    let plan = plan_code_investigation(
        &tool.question,
        tool.target.as_deref(),
        tool.file_path.as_deref(),
        tool.max_steps.unwrap_or(4) as usize,
    )?;
    Ok(json!(plan))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_tools(plan: &InvestigationPlan) -> Vec<&str> {
        plan.recommended_steps
            .iter()
            .map(|step| step.tool.as_str())
            .collect()
    }

    #[test]
    fn impact_phrase_routes_to_impact_tools() {
        let plan = plan_code_investigation(
            "what breaks if I refactor PathNormalizer?",
            Some("PathNormalizer"),
            None,
            4,
        )
        .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::ImpactAnalysis);
        let tools = step_tools(&plan);
        assert!(tools.contains(&"search_code"), "tools were {tools:?}");
        assert!(tools.contains(&"find_affected_code"), "tools were {tools:?}");
        assert!(tools.contains(&"predict_impact"), "tools were {tools:?}");
    }

    #[test]
    fn dataflow_phrase_routes_to_trace_data_flow() {
        let plan = plan_code_investigation(
            "where does this value come from and where is it written?",
            Some("repo_id"),
            None,
            4,
        )
        .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::DataFlow);
        let tools = step_tools(&plan);
        assert!(tools.contains(&"trace_data_flow"), "tools were {tools:?}");
    }

    #[test]
    fn dependency_phrase_routes_to_dependency_graph() {
        let plan = plan_code_investigation(
            "who imports this module?",
            Some("src/storage"),
            None,
            4,
        )
        .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::DependencyWalk);
        let tools = step_tools(&plan);
        assert!(
            tools.contains(&"explore_dependency_graph"),
            "tools were {tools:?}"
        );
    }

    #[test]
    fn public_api_phrase_routes_to_module_summary() {
        let plan = plan_code_investigation(
            "walk me through the public API of the storage module",
            Some("src/storage"),
            Some("src/storage"),
            4,
        )
        .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::ModuleSummary);
        let tools = step_tools(&plan);
        assert!(tools.contains(&"get_module_summary"), "tools were {tools:?}");
    }

    #[test]
    fn file_summary_with_path_routes_to_summarize_file() {
        let plan = plan_code_investigation(
            "summarize this file",
            None,
            Some("src/server/mod.rs"),
            4,
        )
        .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::FileSummary);
        let tools = step_tools(&plan);
        assert!(tools.contains(&"summarize_file"), "tools were {tools:?}");
    }

    #[test]
    fn generic_lookup_routes_to_search_code() {
        let plan = plan_code_investigation(
            "where is PathNormalizer defined?",
            Some("PathNormalizer"),
            None,
            4,
        )
        .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::SymbolLookup);
        let tools = step_tools(&plan);
        assert_eq!(tools.first(), Some(&"search_code"));
    }

    #[test]
    fn max_steps_truncates_recommendations() {
        let plan = plan_code_investigation(
            "what breaks if I rename PathNormalizer?",
            Some("PathNormalizer"),
            None,
            2,
        )
        .unwrap();

        assert_eq!(plan.recommended_steps.len(), 2);
        let tools = step_tools(&plan);
        assert_eq!(tools, vec!["search_code", "find_affected_code"]);
    }

    #[test]
    fn empty_question_returns_error() {
        let err = plan_code_investigation("   ", None, None, 4).unwrap_err();
        assert!(
            err.to_string().contains("question is required"),
            "unexpected error: {err}"
        );
    }
}
```

- [ ] **Step 2: Wire the module just enough for compilation**

In `src/handlers/mod.rs`, add the module declaration near the other handler modules:

```rust
mod planning;
```

Do not re-export the handler yet; `PlanCodeInvestigationTool` does not exist and this task should fail for that reason.

- [ ] **Step 3: Run the planner tests and verify the expected failure**

Run:

```bash
cargo test --lib handlers::planning 2>&1 | tail -25
```

Expected: FAIL at compile time with errors that `PlanCodeInvestigationTool` cannot be found in `crate::tools`. This proves the test module is present before the tool struct is added.

- [ ] **Step 4: Leave the failing test state uncommitted**

Do not commit this compile-failing state. Leave the files modified so Task 2 can add the missing tool struct and implementation in the same buildable commit.

Run:

```bash
git status --short
```

Expected: `src/handlers/mod.rs` and `src/handlers/planning.rs` are modified or untracked; nothing is staged.

---

### Task 2: Add tool struct and planner implementation

**Files:**
- Modify: `src/tools/mod.rs`
- Modify: `src/handlers/planning.rs`
- Modify: `src/handlers/mod.rs`

- [ ] **Step 1: Add the MCP tool struct**

In `src/tools/mod.rs`, insert this block after `HydrateSymbolsTool` and before `ReportSelectionTool`:

```rust
#[macros::mcp_tool(
    name = "plan_code_investigation",
    description = "Recommend a code-intelligence workflow for a natural-language codebase question. Use this before Grep/Read when deciding whether the task needs search_code, find_references, find_affected_code, predict_impact, trace_data_flow, explore_dependency_graph, get_module_summary, summarize_file, or hydrate_symbols. This tool only recommends next tool calls; it does not execute them."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct PlanCodeInvestigationTool {
    pub question: String,
    pub target: Option<String>,
    pub file_path: Option<String>,
    /// Default 4, clamped to 1..=6.
    pub max_steps: Option<u32>,
}
```

- [ ] **Step 2: Replace the planner stub with deterministic implementation**

In `src/handlers/planning.rs`, replace the `plan_code_investigation` stub with this implementation and helper functions:

```rust
pub fn plan_code_investigation(
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
    max_steps: usize,
) -> Result<InvestigationPlan> {
    let question = question.trim();
    if question.is_empty() {
        bail!("question is required");
    }

    let max_steps = max_steps.clamp(1, 6);
    let normalized = question.to_lowercase();
    let intent = classify_intent(&normalized, file_path);
    let target = target
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| derive_target(question, file_path));

    let confidence = confidence_for(&intent, target.as_deref(), file_path);
    let mut recommended_steps = steps_for(&intent, question, target.as_deref(), file_path);
    recommended_steps.truncate(max_steps);

    Ok(InvestigationPlan {
        intent,
        confidence,
        target,
        recommended_steps,
        avoid: vec![
            "Do not start with grep unless code-intelligence cannot locate the target.".to_string(),
        ],
    })
}

fn classify_intent(question: &str, file_path: Option<&str>) -> InvestigationIntent {
    if contains_any(
        question,
        &[
            "data flow",
            "where does this value come from",
            "where is this value used",
            "read and written",
            "reads and writes",
            "written",
            "writes",
        ],
    ) {
        return InvestigationIntent::DataFlow;
    }

    if contains_any(
        question,
        &[
            "what breaks",
            "impact",
            "rename",
            "change",
            "refactor",
            "blast radius",
            "affected",
            "callers",
        ],
    ) {
        return InvestigationIntent::ImpactAnalysis;
    }

    if contains_any(
        question,
        &[
            "who imports",
            "depends on",
            "dependency graph",
            "upstream",
            "downstream",
            "imports this module",
        ],
    ) {
        return InvestigationIntent::DependencyWalk;
    }

    if contains_any(
        question,
        &[
            "public api",
            "what's in this module",
            "whats in this module",
            "walk me through this module",
            "exports",
        ],
    ) {
        return InvestigationIntent::ModuleSummary;
    }

    if file_path.is_some()
        && contains_any(
            question,
            &[
                "summarize this file",
                "what's in this file",
                "whats in this file",
                "symbol-level summary",
            ],
        )
    {
        return InvestigationIntent::FileSummary;
    }

    InvestigationIntent::SymbolLookup
}

fn steps_for(
    intent: &InvestigationIntent,
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
) -> Vec<RecommendedStep> {
    match intent {
        InvestigationIntent::ImpactAnalysis => impact_steps(question, target, file_path),
        InvestigationIntent::DataFlow => dataflow_steps(question, target, file_path),
        InvestigationIntent::DependencyWalk => dependency_steps(question, target, file_path),
        InvestigationIntent::ModuleSummary => module_summary_steps(question, target, file_path),
        InvestigationIntent::FileSummary => file_summary_steps(question, target, file_path),
        InvestigationIntent::SymbolLookup => symbol_lookup_steps(question, target, file_path),
    }
}

fn impact_steps(question: &str, target: Option<&str>, file_path: Option<&str>) -> Vec<RecommendedStep> {
    let query = target.unwrap_or(question);
    let mut steps = vec![search_step(query)];
    let symbol_name = target.unwrap_or(query);
    steps.push(RecommendedStep {
        tool: "find_affected_code".to_string(),
        why: "Find reverse dependencies affected by changes to the target.".to_string(),
        arguments: optional_file_args(
            json!({
                "symbol_name": symbol_name,
                "depth": 3,
                "limit": 20
            }),
            file_path,
        ),
    });
    steps.push(RecommendedStep {
        tool: "predict_impact".to_string(),
        why: "Add git co-change signal alongside the static graph.".to_string(),
        arguments: optional_file_args(
            json!({
                "symbol_name": symbol_name,
                "limit": 20
            }),
            file_path,
        ),
    });
    steps.push(hydrate_step());
    steps
}

fn dataflow_steps(question: &str, target: Option<&str>, file_path: Option<&str>) -> Vec<RecommendedStep> {
    let query = target.unwrap_or(question);
    let symbol_name = target.unwrap_or(query);
    vec![
        search_step(query),
        RecommendedStep {
            tool: "trace_data_flow".to_string(),
            why: "Trace where the target is read and written across the codebase.".to_string(),
            arguments: optional_file_args(
                json!({
                    "symbol_name": symbol_name,
                    "direction": "both",
                    "depth": 3,
                    "limit": 20
                }),
                file_path,
            ),
        },
        hydrate_step(),
    ]
}

fn dependency_steps(
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
) -> Vec<RecommendedStep> {
    let query = target.unwrap_or(question);
    let symbol_name = target.unwrap_or(query);
    let direction = infer_dependency_direction(question);
    vec![
        search_step(query),
        RecommendedStep {
            tool: "explore_dependency_graph".to_string(),
            why: "Walk module-level dependency edges instead of grepping import statements.".to_string(),
            arguments: optional_file_args(
                json!({
                    "symbol_name": symbol_name,
                    "direction": direction,
                    "depth": 3,
                    "limit": 20
                }),
                file_path,
            ),
        },
    ]
}

fn module_summary_steps(question: &str, target: Option<&str>, file_path: Option<&str>) -> Vec<RecommendedStep> {
    if let Some(path) = file_path.or(target) {
        vec![RecommendedStep {
            tool: "get_module_summary".to_string(),
            why: "Summarize the exported public API surface for the module.".to_string(),
            arguments: json!({
                "file_path": path,
                "group_by_kind": true
            }),
        }]
    } else {
        vec![search_step(question)]
    }
}

fn file_summary_steps(question: &str, _target: Option<&str>, file_path: Option<&str>) -> Vec<RecommendedStep> {
    if let Some(path) = file_path {
        vec![RecommendedStep {
            tool: "summarize_file".to_string(),
            why: "Get a symbol-level summary without reading the whole file.".to_string(),
            arguments: json!({
                "file_path": path,
                "include_signatures": true,
                "verbose": false
            }),
        }]
    } else {
        vec![search_step(question)]
    }
}

fn symbol_lookup_steps(question: &str, target: Option<&str>, file_path: Option<&str>) -> Vec<RecommendedStep> {
    let query = target.unwrap_or(question);
    let mut steps = vec![search_step(query), hydrate_step()];
    let lower = question.to_lowercase();
    if lower.contains("definition") || lower.contains("defined") {
        steps.push(RecommendedStep {
            tool: "get_definition".to_string(),
            why: "Fetch definition context once the symbol name is known.".to_string(),
            arguments: optional_file_args(
                json!({
                    "symbol_name": query,
                    "limit": 10
                }),
                file_path,
            ),
        });
    } else if lower.contains("reference") || lower.contains("references") || lower.contains("uses") {
        steps.push(RecommendedStep {
            tool: "find_references".to_string(),
            why: "Find direct references when the question asks for uses.".to_string(),
            arguments: optional_file_args(
                json!({
                    "symbol_name": query,
                    "reference_type": "all",
                    "limit": 200
                }),
                file_path,
            ),
        });
    }
    steps
}

fn search_step(query: &str) -> RecommendedStep {
    RecommendedStep {
        tool: "search_code".to_string(),
        why: "Locate the target symbol or module before choosing a specialist follow-up.".to_string(),
        arguments: json!({
            "query": query,
            "limit": 5
        }),
    }
}

fn hydrate_step() -> RecommendedStep {
    RecommendedStep {
        tool: "hydrate_symbols".to_string(),
        why: "Fetch source bodies for symbol IDs returned by earlier code-intelligence tools.".to_string(),
        arguments: json!({
            "ids": ["<symbol IDs from previous step>"],
            "mode": "full",
            "verbose": true
        }),
    }
}

fn optional_file_args(mut value: Value, file_path: Option<&str>) -> Value {
    if let (Value::Object(map), Some(path)) = (&mut value, file_path) {
        map.insert("file_path".to_string(), Value::String(path.to_string()));
        map.insert("file".to_string(), Value::String(path.to_string()));
    }
    value
}

fn infer_dependency_direction(question: &str) -> &'static str {
    let lower = question.to_lowercase();
    if contains_any(&lower, &["who imports", "depends on this", "upstream", "imports this module"]) {
        "upstream"
    } else if contains_any(&lower, &["what does this depend on", "downstream"]) {
        "downstream"
    } else {
        "upstream"
    }
}

fn confidence_for(intent: &InvestigationIntent, target: Option<&str>, file_path: Option<&str>) -> f32 {
    match intent {
        InvestigationIntent::SymbolLookup => {
            if target.is_some() || file_path.is_some() {
                0.72
            } else {
                0.58
            }
        }
        InvestigationIntent::FileSummary | InvestigationIntent::ModuleSummary if file_path.is_some() => 0.9,
        _ if target.is_some() || file_path.is_some() => 0.86,
        _ => 0.74,
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn derive_target(question: &str, file_path: Option<&str>) -> Option<String> {
    if let Some(path) = file_path {
        return Some(path.to_string());
    }

    for quote in ['`', '"', '\''] {
        let mut parts = question.split(quote);
        while let Some(_before) = parts.next() {
            if let Some(candidate) = parts.next() {
                let candidate = candidate.trim();
                if looks_like_target(candidate) {
                    return Some(candidate.to_string());
                }
            }
        }
    }

    question
        .split_whitespace()
        .map(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != ':'))
        .find(|word| looks_like_target(word))
        .map(ToOwned::to_owned)
}

fn looks_like_target(value: &str) -> bool {
    if value.len() < 3 || value.contains(' ') {
        return false;
    }
    value.contains("::")
        || value.contains('_')
        || value.contains('/')
        || value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
}
```

- [ ] **Step 3: Run planner tests**

Run:

```bash
cargo test --lib handlers::planning
```

Expected: PASS, 8 tests in `handlers::planning`.

- [ ] **Step 4: Commit planner implementation**

Run:

```bash
git status --short
git add src/tools/mod.rs src/handlers/planning.rs src/handlers/mod.rs
git commit -m "feat(planning): implement code investigation planner"
```

Expected: commit succeeds with only `src/tools/mod.rs`, `src/handlers/planning.rs`, and `src/handlers/mod.rs` staged.

---

### Task 3: Expose the planner through MCP server dispatch

**Files:**
- Modify: `src/handlers/mod.rs`
- Modify: `src/server/mod.rs`
- Modify: `src/tools/mod.rs`

- [ ] **Step 1: Re-export the handler**

In `src/handlers/mod.rs`, add this re-export after the navigation/search exports:

```rust
pub use planning::handle_plan_code_investigation;
```

- [ ] **Step 2: Advertise the tool**

In `src/server/mod.rs`, add `PlanCodeInvestigationTool::tool()` immediately after `HydrateSymbolsTool::tool()` in `all_tools()`:

```rust
        HydrateSymbolsTool::tool(),
        PlanCodeInvestigationTool::tool(),
        ReportSelectionTool::tool(),
```

- [ ] **Step 3: Dispatch the tool**

In `src/server/mod.rs`, add a sync dispatch arm immediately after the `hydrate_symbols` arm:

```rust
        "plan_code_investigation" => {
            dispatch_sync!(params, PlanCodeInvestigationTool, |tool| {
                handle_plan_code_investigation(tool)
            })
        }
```

- [ ] **Step 4: Add MCP surface tests**

In `src/server/mod.rs`, inside `#[cfg(test)] mod tests`, add these tests near the existing `all_tools_contains_*` tests:

```rust
    #[test]
    fn all_tools_contains_plan_code_investigation() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"plan_code_investigation"),
            "all_tools() must include 'plan_code_investigation', but only found: {names:?}"
        );
    }

    #[test]
    fn dispatch_routes_plan_code_investigation() {
        let source = include_str!("mod.rs");
        assert!(
            source.contains(r#""plan_code_investigation" =>"#),
            "dispatch_tool_call must route plan_code_investigation"
        );
    }
```

- [ ] **Step 5: Add tool description-shape test**

In `src/tools/mod.rs`, inside the existing `#[cfg(test)] mod tests`, add this test after `hydrate_symbols_description_names_search_code_as_upstream`:

```rust
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
```

- [ ] **Step 6: Run MCP surface tests**

Run:

```bash
cargo test --lib server::tests::all_tools_contains_plan_code_investigation server::tests::dispatch_routes_plan_code_investigation tools::tests::plan_code_investigation_description 2>&1 | tail -30
```

If Cargo rejects multiple test filters, run the equivalent broader filters:

```bash
cargo test --lib server::tests::all_tools_contains_plan_code_investigation
cargo test --lib server::tests::dispatch_routes_plan_code_investigation
cargo test --lib tools::tests::plan_code_investigation_description
```

Expected: all three tests pass.

- [ ] **Step 7: Commit MCP surface**

Run:

```bash
git status --short
git add src/handlers/mod.rs src/server/mod.rs src/tools/mod.rs
git commit -m "feat(server): expose plan_code_investigation tool"
```

Expected: commit succeeds with only the listed files staged.

---

### Task 4: Verify, benchmark smoke, and document R004 command

**Files:**
- Read-only during verification.

- [ ] **Step 1: Run focused planner and surface tests**

Run:

```bash
cargo test --lib handlers::planning
cargo test --lib tools::tests::plan_code_investigation_description
cargo test --lib server::tests::all_tools_contains_plan_code_investigation
cargo test --lib server::tests::dispatch_routes_plan_code_investigation
```

Expected: all commands pass.

- [ ] **Step 2: Run full library tests**

Run:

```bash
cargo test --lib 2>&1 | tail -3
```

Expected: `test result: ok` with no failures.

- [ ] **Step 3: Run clippy for touched modules**

Run:

```bash
cargo clippy --lib 2>&1 | grep -E "src/(handlers/planning.rs|handlers/mod.rs|server/mod.rs|tools/mod.rs)" | head -20
```

Expected: no output. If clippy prints warnings in touched files, fix them and rerun the focused tests plus clippy before continuing.

- [ ] **Step 4: Confirm the tool appears in generated tool list via tests**

Run:

```bash
cargo test --lib server::tests::all_tools_contains_plan_code_investigation -- --nocapture
```

Expected: PASS. This is the compile-time surface check that the MCP server advertises the new tool.

- [ ] **Step 5: Run R004 benchmark after implementation**

Run:

```bash
python3 scripts/bench_agent_qa.py --round 4 --repo self
```

Expected: writes `docs/benchmark_rounds/agent/R004.json` and `docs/benchmark_rounds/agent/R004.md`.

If this run takes too long or the user asks to defer it, run the q13-q18 subset instead:

```bash
python3 scripts/bench_agent_qa.py --round 4 --repo self --question-ids self-q13,self-q14,self-q15,self-q16,self-q17,self-q18
```

- [ ] **Step 6: Analyze R004 adoption results**

Run:

```bash
python3 - <<'PY'
import json
from collections import Counter, defaultdict
from pathlib import Path

runs = json.loads(Path("docs/benchmark_rounds/agent/R004.json").read_text())["runs"]

def qnum(qid):
    return int(qid.split("q")[1])

segment = [
    run for run in runs
    if run["toolset"] == "code_intel" and 13 <= qnum(run["question_id"]) <= 18
]
tools = Counter(
    call["name"].replace("mcp__code-intelligence__", "")
    for run in segment
    for call in run["tool_calls"]
)
specialists = {
    "find_affected_code",
    "predict_impact",
    "trace_data_flow",
    "explore_dependency_graph",
    "get_module_summary",
    "summarize_file",
}
used_specialists = sorted(tool for tool in specialists if tools[tool] > 0)

by_toolset = defaultdict(list)
for run in runs:
    if 13 <= qnum(run["question_id"]) <= 18:
        by_toolset[run["toolset"]].append(run)

print("q13-q18 code_intel tool reach:")
for name, count in tools.most_common():
    print(f"  {name}: {count}")

print(f"plan_code_investigation calls: {tools['plan_code_investigation']}")
print(f"specialist tools used: {used_specialists}")

for toolset, rows in sorted(by_toolset.items()):
    avg_judge = sum(row["judge_score"] for row in rows) / len(rows)
    avg_tokens = sum(row["total_input_tokens"] for row in rows) / len(rows)
    print(f"{toolset}: avg_judge={avg_judge:.2f} avg_tokens={avg_tokens:.0f}")
PY
```

Expected success criteria:

- `plan_code_investigation calls` is at least 4.
- `specialist tools used` contains at least 3 tools.
- q13-q18 code-intel average judge is above R003's 6.17.
- q13-q18 code-intel average judge is at least tied with default.

- [ ] **Step 7: Commit benchmark artifacts only if user wants R004 recorded**

Ask before committing benchmark results. If approved, run:

```bash
git add docs/benchmark_rounds/agent/R004.json docs/benchmark_rounds/agent/R004.md
git commit -m "bench: agent Q&A round 004"
```

Expected: benchmark artifacts are committed separately from implementation commits.

---

## Self-Review Notes

**Spec coverage:**
- Recommend-only tool contract -> Tasks 2 and 3.
- Deterministic classifier, no LLM -> Task 2 helper implementation.
- No internal workflow execution -> Task 2 returns only JSON plan steps.
- Error handling -> Task 2 validates empty questions and clamps `max_steps`.
- MCP integration -> Task 3.
- Tests -> Tasks 1, 3, and 4.
- R004 benchmark criteria -> Task 4.

**Out of scope by design:**
- Tool surface pruning.
- ToolSearch/schema-loading optimization.
- Executing planner workflows inside the server.
- Changing agent benchmark scoring.

**Type consistency:**
- Tool struct is `PlanCodeInvestigationTool`.
- Handler is `handle_plan_code_investigation`.
- Pure planner is `plan_code_investigation`.
- MCP tool name is `plan_code_investigation`.
- JSON response uses `intent`, `confidence`, `target`, `recommended_steps`, and `avoid`.

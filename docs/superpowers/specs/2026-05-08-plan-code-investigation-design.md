# plan_code_investigation Design

## Goal

Improve adoption of specialist code-intelligence tools before optimizing token cost. R003 showed that code-intel still mostly competes as a quality booster for `search_code`, while the model falls back to `Grep` and `Read` and does not select the deeper tools added or rewritten for impact, dataflow, module, and summary workflows.

Add a recommend-only MCP tool named `plan_code_investigation`. Given a natural-language question, it returns a compact, structured investigation plan using existing code-intelligence tools. It does not execute the plan and does not answer the user question. The model remains in control and can accept, adjust, or ignore the recommended steps.

## Non-Goals

- Do not reduce the MCP tool surface in this iteration.
- Do not hide or remove any existing tools.
- Do not build an LLM classifier inside the server.
- Do not execute chained tool calls internally.
- Do not optimize ToolSearch/schema-loading overhead yet.
- Do not alter the agent benchmark scoring model.

## R003 Baseline

R003 ran 18 self-repo questions. Overall, code-intel improved judge score over default but remained more expensive:

| toolset | n | avg mech | avg judge | avg input tokens | avg tool calls |
|---|---:|---:|---:|---:|---:|
| default | 18 | 0.93 | 7.28 | 173,356 | 3.8 |
| code_intel | 18 | 0.90 | 7.61 | 282,108 | 6.4 |

On the new q13-q18 impact/dependency segment, code-intel did not outperform default:

| toolset | n | avg mech | avg judge | avg input tokens | avg tool calls |
|---|---:|---:|---:|---:|---:|
| default | 6 | 0.93 | 6.67 | 243,594 | 5.5 |
| code_intel | 6 | 0.93 | 6.17 | 348,496 | 9.2 |

Specialist adoption was the main failure. R003 code-intel runs used `search_code`, `get_file_symbols`, and `find_references`, but did not use `trace_data_flow`, `find_affected_code`, `predict_impact`, `explore_dependency_graph`, `get_module_summary`, or `summarize_file`.

## Tool Contract

Define a new MCP tool in `src/tools/mod.rs`:

```rust
pub struct PlanCodeInvestigationTool {
    pub question: String,
    pub target: Option<String>,
    pub file_path: Option<String>,
    pub max_steps: Option<u32>,
}
```

Suggested MCP name and description:

- Name: `plan_code_investigation`
- Description: "Recommend a code-intelligence workflow for a natural-language codebase question. Use this before Grep/Read when deciding whether the task needs search_code, find_references, find_affected_code, predict_impact, trace_data_flow, explore_dependency_graph, get_module_summary, summarize_file, or hydrate_symbols. This tool only recommends next tool calls; it does not execute them."

The handler returns JSON content only. It should be concise enough for the model to parse quickly, with a stable shape:

```json
{
  "intent": "impact_analysis",
  "confidence": 0.86,
  "target": "PathNormalizer",
  "recommended_steps": [
    {
      "tool": "search_code",
      "why": "Locate the target symbol before impact analysis.",
      "arguments": {"query": "PathNormalizer", "limit": 5}
    },
    {
      "tool": "find_affected_code",
      "why": "Find reverse dependencies affected by changes to the target.",
      "arguments": {"symbol_name": "PathNormalizer", "depth": 3, "limit": 20}
    },
    {
      "tool": "predict_impact",
      "why": "Add git co-change signal alongside the static graph.",
      "arguments": {"symbol_name": "PathNormalizer", "limit": 20}
    }
  ],
  "avoid": [
    "Do not start with grep unless code-intelligence cannot locate the target."
  ]
}
```

## Intent Model

Use deterministic query-pattern rules first. This keeps the experiment cheap, reproducible, and easy to reason about.

Intents:

- `impact_analysis`: queries containing phrases like "what breaks", "impact", "rename", "change", "refactor", "blast radius", "affected", "callers".
- `data_flow`: "data flow", "read", "written", "writes", "where does this value come from", "where is this value used".
- `dependency_walk`: "who imports", "depends on", "dependency graph", "upstream", "downstream", "imports this module".
- `module_summary`: "public API", "what's in this module", "walk me through this module", "exports".
- `file_summary`: "summarize this file", "what's in this file", "symbol-level summary".
- `symbol_lookup`: fallback for definition, implementation, location, and generic code search questions.

When several intents match, prefer the most specialist intent in this order:

1. `data_flow`
2. `impact_analysis`
3. `dependency_walk`
4. `module_summary`
5. `file_summary`
6. `symbol_lookup`

This priority prevents generic words like "where" or "find" from pulling an impact/dataflow task back to plain search.

## Recommended Workflows

Each workflow should include 1-4 steps. Respect `max_steps` by truncating after intent selection. Use `target` when provided; otherwise derive a simple target phrase from quoted text, code-looking tokens, or the question itself.

### impact_analysis

1. `search_code` to locate the target symbol.
2. `find_affected_code` for reverse dependency sites.
3. `predict_impact` for co-change and static dependency signal.
4. Optional `hydrate_symbols` for returned IDs if the model needs bodies.

### data_flow

1. `search_code` to locate the variable, field, or symbol.
2. `trace_data_flow` with `direction: "both"`.
3. Optional `hydrate_symbols` for readers/writers returned by dataflow.

### dependency_walk

1. `search_code` to locate the module or symbol.
2. `explore_dependency_graph` with a direction inferred from wording:
   - "who imports", "depends on this", "upstream" -> `upstream`
   - "what does this depend on", "downstream" -> `downstream`
   - ambiguous -> omit direction or use `both` only if the underlying tool supports it.

### module_summary

1. `get_module_summary` when `file_path` or a module-like target is present.
2. `search_code` as a fallback if the module path is unknown.

### file_summary

1. `summarize_file` when `file_path` is present.
2. `search_code` as a fallback if the file path is unknown.

### symbol_lookup

1. `search_code`.
2. `hydrate_symbols` for the returned IDs when source bodies are needed.
3. `get_definition` or `find_references` if the question asks specifically for definition or references.

## Integration Points

Add the tool following existing MCP patterns:

- `src/tools/mod.rs`: tool struct and description-shape tests.
- `src/handlers/analysis.rs` or a new small handler module: deterministic planner and response rendering.
- `src/handlers/mod.rs`: re-export handler.
- `src/server/mod.rs`: add to `all_tools()` and `dispatch_tool_call()`.

Prefer a small pure planning function that can be unit tested without MCP state:

```rust
fn plan_code_investigation(question: &str, target: Option<&str>, file_path: Option<&str>, max_steps: usize) -> InvestigationPlan
```

The MCP handler should mostly parse tool input, call the pure planner, and serialize the result.

## Error Handling

- Empty or whitespace-only `question`: return a structured MCP error explaining that `question` is required.
- Invalid `max_steps`: clamp to 1-6. Default to 4.
- Unknown intent: return `symbol_lookup` with lower confidence, not an error.
- Missing `target` or `file_path`: return a workflow that starts with `search_code` rather than inventing precise paths.

## Tests

Add unit tests for deterministic routing:

- Impact phrase routes to `impact_analysis` and includes `find_affected_code` plus `predict_impact`.
- Dataflow phrase routes to `data_flow` and includes `trace_data_flow`.
- Import/dependency phrase routes to `dependency_walk` and includes `explore_dependency_graph`.
- Public API/module phrase routes to `module_summary` and includes `get_module_summary`.
- File summary phrase with `file_path` routes to `file_summary` and includes `summarize_file`.
- Generic lookup routes to `symbol_lookup` and includes `search_code`.
- `max_steps` truncates recommendations.
- Empty question returns an error.

Add MCP surface tests:

- `all_tools()` contains `plan_code_investigation`.
- `dispatch_tool_call()` routes `plan_code_investigation`.
- The tool description mentions `Grep/Read` and at least three specialist tool names.

## Benchmark Plan

Run R004 after implementation:

```bash
python3 scripts/bench_agent_qa.py --round 4 --repo self
```

Primary success criteria for q13-q18:

- `plan_code_investigation` is called in at least 4 of 6 code-intel runs.
- At least 3 specialist tools among `find_affected_code`, `predict_impact`, `trace_data_flow`, `explore_dependency_graph`, `get_module_summary`, and `summarize_file` are used across q13-q18.
- q13-q18 code-intel judge average improves over R003's 6.17.
- q13-q18 code-intel judge average is at least tied with default.

Secondary guardrails:

- Overall code-intel judge average does not drop below R003's 7.61.
- q13-q18 average token usage does not exceed R003 code-intel by more than 25%. Token cost is not the optimization target yet, but runaway exploration would invalidate the adoption signal.

## Expected Outcome

If R004 shows specialist adoption and better q13-q18 quality, the next step is to tune planner recommendations and then revisit token cost. If `plan_code_investigation` is ignored, the issue is likely tool discovery or model policy around MCP tool choice rather than specialist tool descriptions. If it is called but recommendations are ignored, the next experiment should consider a higher-level executing tool or a smaller exposed tool surface.

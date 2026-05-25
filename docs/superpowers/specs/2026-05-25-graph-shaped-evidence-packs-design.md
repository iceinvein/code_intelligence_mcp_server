# Graph-Shaped Evidence Packs Design

## Goal

Make code-intelligence competitive with code graph on broad codebase questions by returning evidence in the shape the agent needs to answer. The server should provide compact, verified fact tables for callsites, traces, data flow, impact radius, dependencies, and symbol lookup. The agent still writes the final user-facing answer, but it should compose from row-level facts instead of raw code blobs.

This targets the Pylon three-way benchmark failure modes from `docs/benchmark_rounds/agent_pylon_three_way/R002.md`: code graph narrowly beats code-intel overall, and code-intel loses points when the agent merges distinct callsites, misses callback producers, or spends too much context reading around plausible but incomplete evidence.

## Non-Goals

- Do not copy code graph's API or output format.
- Do not re-enable local-LLM prose synthesis in `ask_code` by default.
- Do not remove `verified_locations`; keep it as the backward-compatible evidence list.
- Do not solve ToolSearch/schema-loading overhead in this iteration.
- Do not broaden the benchmark harness beyond what is needed to validate the new evidence shapes.
- Do not require perfect graph completeness before shipping; every pack must state its coverage.

## Current Baseline

`agent_pylon_three_way/R002` shows the current standing:

| toolset | n | avg mech | avg judge | avg input tokens | avg tool calls |
|---|---:|---:|---:|---:|---:|
| code_graph | 12 | 0.84 | 8.17 | 189,908 | 5.3 |
| code_intel | 12 | 0.82 | 8.00 | 201,582 | 5.8 |
| default | 12 | 0.80 | 7.92 | 136,225 | 3.6 |

Head-to-head, code graph is ahead but not dominant: graph wins judge score on 5 questions, code-intel wins 4, and 3 tie. The important gap is evidence shape:

- `pylon-q1`: code-intel found the relevant callsites but grouped two distinct `createSession` callsites into one answer item.
- `pylon-q7`: code-intel won by explaining function composition; this is evidence that structured composition facts help.
- `pylon-q9`: both graph and code-intel missed the rubric's callback producer, `config.onBeforeToolUse`, and traced the raw passthrough path instead.
- `pylon-q10+`: broader pipeline and impact questions need ordered hops or edges, not just a ranked list of code bodies.

## Response Contract

Add an optional `pack` object to `investigate` and `ask_code` evidence-only responses:

```json
{
  "pack": {
    "kind": "callsite_enumeration",
    "target": "SessionManager.createSession",
    "coverage": {
      "status": "complete",
      "basis": ["references", "call_hierarchy"],
      "missing": []
    },
    "rows": [],
    "edges": [],
    "answer_guidance": "Use one bullet per row. Do not merge rows with different file:line values."
  }
}
```

`pack` is the primary artifact for agent synthesis. `verified_locations` remains present for compatibility and as a source of code bodies.

Shared fields:

- `kind`: one of the pack types listed below.
- `target`: normalized target symbol, file, or query phrase.
- `coverage.status`: `complete`, `partial`, or `no_hits`.
- `coverage.basis`: tools or index sources used to construct the pack.
- `coverage.missing`: concrete missing evidence, such as "no callback producer found" or "no references indexed".
- `rows`: ordered fact rows.
- `edges`: graph relationships when the shape needs them.
- `answer_guidance`: short, shape-specific instruction phrased around the returned rows.

Rows must carry enough source identity to cite without another read:

```json
{
  "role": "caller",
  "symbol_id": "ts:function:...",
  "symbol_name": "runPeerReviewPass",
  "file_path": "src/main/pr-review-manager.ts",
  "line": 1577,
  "end_line": 1588,
  "enclosing_symbol": "runPeerReviewPass",
  "evidence": "const session = await this.sessionManager.createSession(...)",
  "reason": "Creates a pr-review session for the peer/second-opinion pass."
}
```

## Pack Types

### callsite_enumeration

For questions such as "who calls X", "where is X used", "list callsites", and "what invokes X".

Rows:

- One row per distinct reference or callsite.
- Distinct `file_path:line` values must never be merged.
- Include `role` values such as `caller`, `reference`, `import`, `implementation`, or `test`.
- Include `enclosing_symbol` when available.

Primary data sources:

- `find_references` with `reference_type: "call"` when the question is call-shaped.
- `get_call_hierarchy` with `direction: "callers"` as a secondary source.
- `search_code` only as fallback discovery when the target is ambiguous.

### pipeline_trace

For "trace how X flows", "end-to-end", "pipeline", "from provider to renderer", and similar questions.

Rows:

- Ordered hops with `ordinal`, `role`, `file_path`, `line`, `symbol_name`, and `evidence`.
- Roles should be explicit where possible: `producer`, `normalizer`, `dispatcher`, `bridge`, `channel`, `subscriber`, `consumer`.
- A missing required role should be represented in `coverage.missing`, not silently skipped.

Edges:

- `from_ordinal`, `to_ordinal`, `relationship`, and optional `evidence`.

Primary data sources:

- `get_call_hierarchy` for call edges.
- `trace_data_flow` for value propagation.
- String/search fallback for IPC channel constants, event names, and callback hook names.

This pack is the direct fix for `pylon-q9`: callback producers like `config.onBeforeToolUse` must be represented as `producer` candidates, not hidden inside a raw passthrough body.

### data_flow

For "where is this value read/written", "lifecycle", "set and read", "where does this come from", and similar questions.

Rows:

- `role`: `write`, `read`, `assignment`, `parameter`, `return`, or `unknown`.
- Include symbol/file/line/evidence.
- Group by symbol when multiple fields or variables match.

Primary data sources:

- `trace_data_flow`.
- `get_definition` for the pivot.
- `search_code` fallback for literals or fields not in the graph.

### impact_radius

For "what breaks if I change X", "affected by", "downstream", "rename", "refactor", and "blast radius".

Rows:

- `role`: `affected_production`, `affected_test`, `caller`, `dependency`, or `cochange`.
- Include a `risk` field: `high`, `medium`, or `low`.
- Include `reason` explaining why the row is affected.

Edges:

- Dependency or call edges from the target to affected nodes.

Primary data sources:

- `find_affected_code`.
- `get_call_hierarchy`.
- `explore_dependency_graph`.
- Existing test-discovery helpers when tests are requested or obvious.

### dependency_map

For "who imports this", "what depends on this module", "what does this depend on", and module-level dependency questions.

Rows:

- `role`: `imports_target`, `imported_by_target`, `exports`, or `reexports`.
- Include module/file identity and evidence.

Edges:

- Module dependency relationships with direction.

Primary data sources:

- `explore_dependency_graph`.
- `get_module_summary`.
- Import/export search fallback for unsupported language cases.

### symbol_lookup

For definition, interface, implementation, and "what is X" questions.

Rows:

- `role`: `definition`, `implementation`, `interface`, `type`, `test_double`, or `usage_example`.
- Include source location and a compact body snippet.

Primary data sources:

- `get_definition`.
- `search_code`.
- `get_file_symbols`.
- `find_references` for implementation/interface relationships where indexed.

## Shape Classification

Extend the existing deterministic classifier in `src/handlers/investigation.rs`. Keep classification cheap and reproducible.

Priority order:

1. `pipeline_trace`: phrases like "trace how", "flows from", "end-to-end", "pipeline", "from X to Y", "hop", "bridge", "subscriber".
2. `data_flow`: "data flow", "read/write", "where is this value used", "lifecycle", "set and read".
3. `impact_radius`: "what breaks", "affected", "blast radius", "rename", "refactor", "downstream".
4. `dependency_map`: "depends on", "who imports", "imports this", "dependency graph", "upstream".
5. `callsite_enumeration`: "who calls", "callsites", "where is X called", "references", "invokes".
6. `symbol_lookup`: fallback.

`pipeline_trace` stays above `callsite_enumeration` because trace questions often contain verbs like "calls" but require ordered roles rather than a flat caller list.

## Integration Points

Add a small evidence-pack layer rather than bloating `handle_investigate`:

- `src/handlers/evidence_pack.rs`: pack structs, classifier adapter, pack builders, coverage helpers.
- `src/handlers/investigation.rs`: after primary/secondary hops, call the pack builder and attach `pack` to the response.
- `src/handlers/ask_code.rs`: pass through `pack` from the `investigate` response in evidence-only mode.
- `src/tools/mod.rs`: update `investigate` and `ask_code` descriptions to mention structured evidence packs.
- `src/server/mod.rs`: update server instructions so models prefer `pack.rows` over raw bodies when present.

Implementation should keep builder functions pure where possible:

```rust
fn build_evidence_pack(
    question: &str,
    shape: InvestigationShape,
    primary: &PrimaryHop,
    secondary: Option<&SecondaryHop>,
) -> EvidencePack
```

If private structs make this awkward, introduce small public adapter structs rather than exposing handler internals wholesale.

## Error Handling and Coverage

Every pack must return a coverage object:

- `complete`: the server found at least one row for every required role in the selected shape.
- `partial`: some evidence exists, but a required role or expected graph source is missing.
- `no_hits`: no rows could be constructed.

Examples:

- A `callsite_enumeration` pack with zero references is `no_hits`.
- A `pipeline_trace` pack with producer/bridge/subscriber but no channel constant is `partial`.
- A `data_flow` pack with only writes and no reads is `partial`.

The response should avoid pretending graph coverage is exhaustive when it used fallback text search or when the graph source returned no rows. Put that limitation in `coverage.missing`.

## Testing

Unit tests:

- Classifier routes "who calls X" to `callsite_enumeration`.
- Classifier routes "trace how X flows from provider to renderer" to `pipeline_trace`.
- Classifier routes data-flow, impact, and dependency phrases to their pack types.
- Callsite builder keeps two rows with different lines separate, even if they share an enclosing file.
- Pipeline builder preserves required role ordering and marks missing roles as `partial`.
- `ask_code` evidence-only response includes `pack` when `investigate` returned one.
- Response budget handling does not remove `pack.rows` before raw code bodies.

Golden/fixture tests:

- Create small TypeScript fixtures for callback producer, IPC bridge, channel constant, and renderer subscriber.
- Verify `pipeline_trace` identifies producer, bridge, channel, and subscriber rows.
- Create a fixture where one function calls the target twice on different lines; verify two callsite rows.

Benchmark validation:

Run the pylon regression subset first:

```bash
BENCH_TOOLSETS=default,code_intel,code_graph \
python3 scripts/bench_agent_qa.py \
  --round 3 \
  --repo custom \
  --base-dir /Users/dikrana/Documents/workspace/pylon \
  --queries scripts/queries_qa_pylon.json \
  --output-dir docs/benchmark_rounds/agent_pylon_evidence_packs_subset \
  --question-ids pylon-q1,pylon-q7,pylon-q9,pylon-q10,pylon-q12
```

Success criteria on this subset:

- `code_intel` judge average is at least tied with `code_graph`.
- `pylon-q1` answer lists distinct callsite rows and does not merge `pr-review-manager.ts` callsites.
- `pylon-q9` answer names a provider producer candidate, preferably `config.onBeforeToolUse`, or the pack marks producer coverage as partial instead of presenting raw passthrough as complete.
- Average code-intel total input tokens do not exceed the previous pylon R002 code-intel average by more than 10%.

Then run the full pylon three-way:

```bash
BENCH_TOOLSETS=default,code_intel,code_graph \
python3 scripts/bench_agent_qa.py \
  --round 3 \
  --repo custom \
  --base-dir /Users/dikrana/Documents/workspace/pylon \
  --queries scripts/queries_qa_pylon.json \
  --output-dir docs/benchmark_rounds/agent_pylon_evidence_packs
```

Full-round success criteria:

- `code_intel` average judge score is at least tied with `code_graph`.
- `code_intel` average mechanical score is at least tied with `code_graph`.
- `code_intel` average total input tokens are not higher than `code_graph` by more than 10%.
- Tool reach shows fewer fallback `Grep` + `Read` calls than pylon R002 for code-intel.

## Expected Outcome

Evidence packs should make broad answers more reliable by making the server own retrieval shape and making the agent own concise synthesis. If this works, code-intel should stop losing points from merged callsites and missing trace roles. If it fails, the next likely issue is missing index edges or insufficient callback/event extraction, not answer synthesis.

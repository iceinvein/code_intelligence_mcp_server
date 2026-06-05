# Evidence Pack Quality Layer Design

## Goal

Improve answer quality for broad codebase questions by making `investigate` and `ask_code` return more complete, more honest evidence packs. The next quality gain should come from better structure and coverage checks, not from another retrieval model, reranker, or description backfill.

This targets the failure mode where code-intelligence finds useful source locations but the agent still loses points because it merges distinct callsites, misses callback/event producers, treats candidate rows as verified facts, or marks a trace as complete when a required role is absent.

## Non-Goals

- Do not re-enable local LLM prose synthesis in `ask_code` by default.
- Do not turn the description LLM or cross-encoder reranker back on by default.
- Do not rewrite the hybrid retrieval or ranking pipeline.
- Do not change the public `verified_locations` compatibility contract.
- Do not optimize token cost at the expense of line-level evidence quality.
- Do not require perfect graph completeness before returning a pack; incomplete packs should say exactly what is missing.

## Current Context

The project already has the main high-level agent surfaces:

- `plan_code_investigation` recommends specialist workflows.
- `investigate` executes a shape-driven multi-hop investigation.
- `ask_code` wraps `investigate` and returns evidence-only responses by default.
- `pack.rows` gives agents a structured synthesis outline.

Earlier benchmark notes show that descriptions and reranking did not produce reliable quality gains. The remaining useful direction is to improve the facts and coverage state returned by evidence packs, especially for questions involving pipelines, callbacks, callsites, impact radius, and dependency edges.

## Recommended Approach

Add a small failure-driven quality layer around evidence-pack construction.

### 1. Non-Callgraph Edge Extraction

Add deterministic helpers that discover producer and registration relationships that normal call graphs often miss:

- Callback fields such as `onBefore*`, `onAfter*`, `before*`, `after*`, `handler`, `callback`, and `listener`.
- Event patterns such as `.emit(...)`, `.on(...)`, `.once(...)`, `.addEventListener(...)`, and framework-specific equivalents.
- Route and middleware registrations, including existing framework extractors where available.
- Config object hooks where a function is passed as a property and later invoked indirectly.

The first pass should focus on TypeScript/JavaScript because the documented failures and framework extractors are strongest there. Other languages can keep the current behavior unless the helper can apply generically without false confidence.

### 2. Pack-Specific Builders

Move beyond mostly adapting primary and secondary search rows. Add pack-specific builder logic for the highest-value shapes:

- `callsite_enumeration`: one row per distinct verified reference/callsite. BM25-only hits stay `candidate` and force partial coverage unless confirmed by reference or call-hierarchy data.
- `pipeline_trace`: rows should carry explicit roles such as `producer`, `normalizer`, `dispatcher`, `channel`, `subscriber`, and `consumer`. Missing required roles should be listed in coverage, not hidden.
- `impact_radius`: rows should distinguish `affected_production`, `affected_test`, `dependency`, `config`, and `cochange`, with a simple risk label and reason.

The shared pack struct can remain stable. The change is in how rows, roles, and coverage are produced.

### 3. Pack Verifier

Before returning a pack, run deterministic verification and coverage downgrade rules:

- Distinct callsites must have distinct `file_path:line` identities.
- A row claiming a direct call or reference must have evidence text that supports the relationship, or it must be marked `candidate`.
- A complete `pipeline_trace` must include the required roles for the selected shape.
- Candidate-only packs are never `complete`.
- If response-budget trimming removes rows or important evidence fields, coverage becomes `partial`.
- Coverage `missing` should name concrete gaps, such as `producer`, `subscriber`, `verified callsites`, or `test coverage`.

This verifier should be pure and unit tested so it can run for every pack without depending on MCP state.

### 4. Golden Failure Corpus

Add a small regression corpus for known hard quality cases. Each case should specify:

- Question.
- Expected pack kind.
- Required roles or row identities.
- Required coverage status.
- Forbidden overclaims, such as marking candidate-only evidence complete.

The corpus should include at least:

- Distinct callsites for the same target should not be merged.
- Callback/config producers should appear as producer candidates when direct call edges are absent.
- Pipeline traces should downgrade coverage when producer or subscriber roles are missing.
- Impact questions should separate production and test/config/co-change rows.
- Test-coverage questions should not be answered solely from production-ranked search hits.

This corpus is a quality gate for future changes. It is intentionally smaller and more targeted than the full benchmark.

## Data Flow

1. `investigate` classifies the question and gathers primary and secondary evidence as it does today.
2. The relevant pack builder receives verified locations plus any specialist output.
3. Non-callgraph edge helpers add callback/event/config-hook candidates where the selected shape benefits from them.
4. The pack builder assigns roles, evidence lines, row identity, and initial coverage.
5. The verifier downgrades unsupported or incomplete claims.
6. `investigate` returns the verified pack and `verified_locations`; `ask_code` passes the pack through unchanged.

## Error Handling

- If helper extraction finds no additional edges, keep current pack behavior and explain missing roles through coverage.
- If helper extraction produces ambiguous matches, include them as `candidate` rows and keep coverage `partial`.
- If a required role cannot be inferred reliably, do not invent it from search rank.
- If a language or framework is unsupported, mark the relevant basis as missing instead of claiming complete graph coverage.

## Testing

Add unit tests for:

- Callback/config-hook pattern detection.
- Event registration and event emission row classification.
- Callsite deduplication by `file_path:line`.
- Candidate-only packs becoming `partial`.
- Pipeline required-role coverage.
- Impact row role assignment.
- Response-budget truncation preserving existing downgrade behavior.

Add golden regression tests for the failure corpus. These can live close to `src/handlers/evidence_pack.rs` at first, then move into a benchmark fixture if they grow.

## Success Criteria

- Known hard pack-shape tests pass.
- Existing Rust tests still pass.
- On the relevant benchmark subset, judge quality improves or ties the current result.
- Token and tool-call count do not increase materially. A small token increase is acceptable only if judge quality improves.
- No response marks a pack `complete` when all rows are candidates or required roles are missing.

## Implementation Notes

Prefer small modules and pure functions:

- Keep the public pack JSON shape stable.
- Add helper structs only where they reduce coupling to `handle_investigate`.
- Reuse existing framework extraction data before adding duplicate parser logic.
- Keep TypeScript/JavaScript extraction conservative in the first pass.
- Treat token reduction as a secondary cleanup after quality behavior is measured.

# Token Reduction Without Quality Loss Design

## Context

The Pylon benchmark subset now shows code-intelligence improving mechanical score while using substantially more input tokens than the default toolset. Round 4 measured `code_intel` at 0.84 average mechanical score and 294,807 average input tokens, versus default at 0.77 mechanical score and 146,694 average input tokens.

The largest visible token driver is not only MCP response size. The benchmark transcripts show agents calling `Read` and `Grep` after `ask_code` already returned structured rows or evidence. Across the five-question Pylon subset, `code_intel` made 28 fallback `Read`/`Grep` calls and 7 code-intelligence MCP calls.

## Goal

Reduce benchmark token usage without reducing answer quality or weakening evidence grounding.

Success means:

- Preserve or improve Pylon subset mechanical score.
- Reduce average input tokens for `code_intel`.
- Reduce fallback `Read`/`Grep` calls after complete code-intelligence evidence.
- Keep line-level citations and source-backed synthesis available to the agent.

## Non-Goals

- Do not reintroduce local prose generation in `ask_code`.
- Do not remove `pack.rows`, `evidence[]`, paths, line ranges, or source snippets needed for grounded answers.
- Do not tune against one question by weakening general tool behavior.
- Do not change the benchmark questions during this phase.

## Considered Approaches

### Approach A: Prompt and Tool Contract Tightening

Strengthen the benchmark prompt and MCP tool descriptions so agents synthesize directly from `pack.rows` and `evidence[]` when coverage is complete. `Read`, `Grep`, and `Glob` remain allowed only when coverage is partial, no-hit, candidate-only, or missing the body needed for citation.

This is the fastest and lowest-risk lever. It targets the observed fallback pattern directly, but depends on agent compliance.

### Approach B: Compact Self-Sufficient Responses

Trim redundant response fields from `ask_code` and possibly `investigate` while preserving enough structured evidence to answer without fallback file reads. The compact shape should keep `pack.coverage`, `pack.rows`, `evidence[]` path/line/body fields, mode metadata, and clear follow-up guidance. It should avoid duplicating the same source bodies through multiple fields.

This reduces MCP payload tokens and reinforces that the first response is authoritative enough to use. It has more implementation risk than prompt-only changes because it touches response contracts.

### Approach C: Retrieval Ranking and Evidence Count Tuning

Tune defaults such as `max_evidence`, row ranking, or specialist routing so the first MCP call returns fewer but better evidence items.

This can help after the first two approaches, but it is riskier as an initial step because reducing evidence too early can lower recall and answer quality.

## Recommended Design

Use a two-pass implementation.

First, tighten the benchmark and tool contracts. The agent instructions should say that complete coverage with rows or evidence is enough to synthesize an answer, and that fallback file reads are only for explicit gaps. Tool descriptions should use the same language so the rule applies outside the benchmark harness too.

Second, add a compact response path for code-intelligence evidence. The default `ask_code` evidence-only response should remain self-sufficient but avoid redundant large fields. If `investigate` continues to expose richer diagnostics, `ask_code` should choose the smaller synthesis-oriented shape when wrapping it.

Ranking and evidence-count tuning should wait until the benchmark proves whether prompt contract plus compact responses are enough.

## Data Flow

1. Benchmark asks a Pylon question with the `code_intel` toolset.
2. Agent calls `ask_code` or a specialist code-intelligence tool first.
3. Server returns structured evidence with explicit coverage state.
4. If coverage is complete and evidence bodies are present, agent synthesizes directly.
5. Agent falls back to `Read`/`Grep` only when the response indicates partial coverage, candidate rows, no hits, or missing source bodies.

## Testing

Add focused tests before implementation:

- A benchmark prompt or harness test that asserts the code-intelligence instructions include the no-fallback rule for complete evidence.
- Response-shape tests for `ask_code` that verify compact evidence responses still include coverage, rows, evidence paths, line ranges, and bodies.
- Regression tests that ensure partial, candidate, and no-hit responses still instruct agents to use follow-up tools when needed.

Verify with:

- Existing Rust tests for tool descriptions and `ask_code`.
- Existing Python benchmark harness tests.
- A Pylon subset benchmark run, starting with high-token questions `pylon-q1` and `pylon-q9`, then the five-question subset.

## Risks

- Overly strict no-fallback language could discourage legitimate verification when evidence is incomplete. Mitigation: scope the rule to complete coverage with hydrated evidence.
- Compacting the response could remove a field an agent implicitly used. Mitigation: keep the stable synthesis contract and test for required fields.
- Token reductions may come mostly from benchmark prompt behavior and not general agent behavior. Mitigation: update both benchmark instructions and MCP tool descriptions.

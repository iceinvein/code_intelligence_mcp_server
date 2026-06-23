# Prune the MCP tool surface (39 → 18 advertised)

Date: 2026-06-23
Status: approved (pending spec review)

## Problem

The server advertises 39 MCP tools. A large, flat tool list dilutes a model's
tool-selection accuracy: the more near-synonym tools an agent sees, the more
often it picks a worse-fitting one or wastes a turn deciding. The goal is to be
specific and surface a small, coherent set so agents reliably reach for the
right tool.

This is a tool-count (surface-area) reduction. Prior work in this repo addressed
response payload size and the number of running servers, never the advertised
tool surface itself.

## Goal

Cut the model-facing tool list from 39 to 18, deleting genuinely niche/redundant
tools outright and hiding (but keeping callable) the operational tools that have
non-agent consumers. No agent-facing capability regresses for the common code
question-and-answer paths.

## Approach

Three tiers, replacing the single flat `all_tools()` list.

### Tier A — advertised (model sees these), 18

Core retrieval: `ask_code`, `investigate`, `search_code`, `hydrate_symbols`

Navigation: `get_definition`, `find_references`, `get_call_hierarchy`,
`get_type_graph`, `explore_dependency_graph`, `trace_data_flow`,
`find_affected_code`

Overview: `summarize_file`, `get_module_summary`

Tests: `find_tests_for_symbol`

Lifecycle/admin: `refresh_index`, `get_index_stats`, `bind_workspace`,
`approve_indexing`

### Tier B — kept callable but UNADVERTISED, 7

These are removed from `all_tools()` only. They keep their `dispatch_tool_call`
arm and their handler, so they remain callable by name over JSON-RPC. They are
simply absent from the advertised list, so they no longer clutter the model's
tool surface.

- `get_file_symbols`, `get_usage_examples`: required. `src/server/api/query.rs`
  (lines 293, 318) builds `GetFileSymbolsTool` / `GetUsageExamplesTool` to back
  the web API `/api/query/*` endpoints.
- `explain_search`: `scripts/test_scoring.py:222` and the bench harness call it
  by name over JSON-RPC.
- `import_external_index`, `generate_external_index`: the external-producer
  feature; reachable via API/auto-mode, never called by agents during Q&A.
- `report_selection`, `report_file_access`: feed the learning system
  (`src/retrieval/mod.rs`). Agents rarely call them spontaneously, so learning
  was already effectively sparse; hiding them lets it go dormant. Handlers stay
  callable for any explicit client that wants to feed the signal.

### Tier C — deleted outright (struct + handler + dispatch arm + tests), 14

No non-agent consumer; either redundant with a composite/canonical tool or too
niche to justify a slot:

`plan_code_investigation` (recommendation-only, redundant with `investigate`),
`get_context_bundle`, `get_similarity_cluster`, `find_similar_code`,
`search_todos`, `search_decorators`, `search_framework_patterns`,
`find_dead_code`, `find_duplicates`, `find_stale_descriptions`,
`find_undocumented_symbols`, `predict_impact` (overlaps `find_affected_code`),
`search_across_repos`, `explore_cross_repo_dependencies`.

Net model-facing surface: 39 → 18 (-54%).

## Mechanics

### `src/server/mod.rs`
- `all_tools()` (line ~154): reduce the vec to the 18 Tier-A tools.
- `dispatch_tool_call` match: delete the 14 Tier-C arms. Keep the 7 Tier-B arms
  (callable by name, not listed). Keep the 18 Tier-A arms.
- Instructions string: remove the `` `plan_code_investigation` is
  recommendation-only.`` sentence (line ~43). Keep the `investigate` /
  `search_code` guidance.
- Tests in this file: remove the assertion `instructions.contains(
  "plan_code_investigation")` (line ~454). Keep the `investigate` assertions.

### `src/tools/mod.rs`
- Delete the 14 Tier-C `#[mcp_tool]` structs and their definitions.
- Delete the unit tests tied to deleted tools: the `plan_code_investigation`
  routing test and the `predict_impact` description test. Keep tests for all
  retained tools.
- Retain all 7 Tier-B structs.

### `src/handlers/`
- Delete the 14 Tier-C handler fns:
  - `analysis.rs`: `handle_predict_impact`, `handle_get_context_bundle`,
    `handle_search_todos`, `handle_search_decorators`,
    `handle_search_framework_patterns`, `handle_find_dead_code`,
    `handle_find_duplicates`, `handle_find_stale_descriptions`,
    `handle_find_undocumented_symbols` (and any private helpers used only by
    these).
  - `graph.rs`: `handle_get_similarity_cluster`.
  - `search.rs`: `handle_find_similar_code` + `format_similar_results`. Note:
    `search.rs` also holds `handle_search_code` (retained), so this is a partial
    edit, not a file delete.
  - `cross_repo.rs`: whole-file delete (`handle_search_across_repos`,
    `handle_explore_cross_repo_dependencies`, and their formatting helpers are
    the only contents).
  - `planning.rs`: candidate whole-file delete (`handle_plan_code_investigation`
    plus `plan_code_investigation`, `classify_intent`, `steps_for`, etc.). VERIFY
    FIRST that `investigate` does not reuse any of these helpers; if it does,
    keep only what `investigate` needs and delete the rest.
- `handlers/mod.rs`: drop the 14 deleted re-exports (lines ~38-62). Keep the
  `get_file_symbols` and `get_usage_examples` re-exports.

### `src/server/standalone.rs`
- Remove the special-case match arms for `search_across_repos` (line ~803) and
  `explore_cross_repo_dependencies` (line ~817) and their imports (line ~4).
- Keep the `bind_workspace` / `approve_indexing` arms. `list_tools` rides on
  `all_tools()`, so it reflects the reduced set automatically.

### No change
- `src/server/api/query.rs`: its only deleted-list references are
  `GetFileSymbolsTool` / `GetUsageExamplesTool`, both retained (Tier B). Confirm
  by grep that it touches no other Tier-C struct.

### Docs
- `CLAUDE.md` (repo root): change "39 MCP tools" to "18" and update the tool
  bullet list / glossary references that name deleted tools.
- `~/.claude/CLAUDE.md`: the Code Search table references `find_affected_code`,
  `explore_dependency_graph` (both retained) — verify no deleted tool is named.
- README and the dashboard examples page: update any enumerated tool list.

## Verification

- `cargo build --release` clean; `cargo fmt` and `cargo clippy` clean.
- `cargo test` green after test edits. Add a test asserting
  `all_tools().len() == 18` and that each Tier-C name is absent from
  `all_tools()`.
- Smoke (running daemon): ListTools returns exactly the 18 Tier-A names; CallTool
  on a Tier-B name (`explain_search`) still succeeds; CallTool on a Tier-C name
  (`find_dead_code`) returns method-not-found.
- `scripts/test_scoring.py` still runs (relies on retained `explain_search`).
- Dashboard `/api/query/*` still works (relies on retained `get_file_symbols` /
  `get_usage_examples`).

## Risks and trade-offs

- Deleting 14 handlers removes several hundred lines of working code
  irreversibly. Accepted per the hard-prune decision.
- Any external MCP client config that names a deleted tool will get
  method-not-found. Accepted; no migration shim.
- Learning (`LEARNING_*` boosts) goes dormant because `report_*` is unadvertised.
  Accepted; the signal was already sparse. The handlers remain callable if a
  client chooses to feed it.
- Tier B is a small, deliberate exception to the "delete" mechanism, used only
  where a non-agent consumer (API, harness, feature) requires the code to live.
  It is not a general gating/config tier.

## Out of scope

- Consolidating navigation specialists under a single mode-parameterized tool.
- Adding a config flag to re-expose hidden tools.
- Changing tool response payloads or descriptions beyond removing references to
  deleted tools.

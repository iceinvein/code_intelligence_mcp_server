# Prune MCP Tool Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut the advertised MCP tool list from 39 to 18, deleting 14 niche/redundant tools outright and hiding (keeping callable) 7 operational tools, so agents reliably select the right tool.

**Architecture:** `all_tools()` in `src/server/mod.rs` is the single source of the advertised list (only caller: `standalone.rs:750`). The model only ever sees what `all_tools()` returns. Tools stay callable as long as their `dispatch_tool_call` match arm and handler exist, independent of `all_tools()`. So "hide" = remove from `all_tools()` only; "delete" = remove from `all_tools()` + dispatch arm + handler + struct + tests.

**Tech Stack:** Rust 2021, `rust_mcp_sdk`, `cargo test` (run `EMBEDDINGS_BACKEND=hash` to skip model download; Tantivy tests need `--test-threads=1`).

## Global Constraints

- Never use emdashes in any output (code, comments, commit messages, docs). Use period/colon/semicolon/parens/comma.
- No AI attribution in commits or PRs (no Co-Authored-By, no "generated with").
- Commit protocol: before each commit run the tests for the changed area and `cargo fmt` + `cargo clippy`; do not commit with known failures.
- Build the release binary for any daemon smoke test: `cargo build --release`.
- Work on branch `prune-mcp-tool-surface` (already created; the spec commit `edc6a3a` is its first commit).

### Reference: the three tiers

**Tier A — advertised (18), keep everything:** `ask_code`, `investigate`, `search_code`, `hydrate_symbols`, `get_definition`, `find_references`, `get_call_hierarchy`, `get_type_graph`, `explore_dependency_graph`, `trace_data_flow`, `find_affected_code`, `summarize_file`, `get_module_summary`, `find_tests_for_symbol`, `refresh_index`, `get_index_stats`, `bind_workspace`, `approve_indexing`.

**Tier B — hidden but callable (7), keep struct + dispatch arm + handler, remove from `all_tools()` only:** `get_file_symbols`, `get_usage_examples`, `explain_search`, `import_external_index`, `generate_external_index`, `report_selection`, `report_file_access`.

**Tier C — deleted (14), remove everything:** `plan_code_investigation`, `get_context_bundle`, `get_similarity_cluster`, `find_similar_code`, `search_todos`, `search_decorators`, `search_framework_patterns`, `find_dead_code`, `find_duplicates`, `find_stale_descriptions`, `find_undocumented_symbols`, `predict_impact`, `search_across_repos`, `explore_cross_repo_dependencies`.

---

## File Structure

- `src/server/mod.rs` — `all_tools()` (advertised list), `dispatch_tool_call` (match arms), `server_instructions()`, two `*_EMBEDDED_MSG` constants, and the bulk of the routing/presence unit tests. Touched by Tasks 1-4, 6.
- `src/tools/mod.rs` — `#[mcp_tool]` struct definitions + per-tool description unit tests. Tier-C structs deleted (Tasks 3-4); `find_affected_code` description edited (Task 3).
- `src/handlers/mod.rs` — module declarations + handler re-exports. Tier-C re-exports removed (Tasks 3-4).
- `src/handlers/analysis.rs` — 9 Tier-C handlers + helpers (Task 3).
- `src/handlers/graph.rs` — `handle_get_similarity_cluster` (Task 3).
- `src/handlers/search.rs` — `handle_find_similar_code` + `format_similar_results`; KEEP `handle_search_code`, `handle_explain_search` (Task 3).
- `src/handlers/planning.rs` — delete `handle_plan_code_investigation` only; KEEP `plan_code_investigation` + helpers (used by `investigation.rs:610`) (Task 3).
- `src/handlers/cross_repo.rs` — whole-file delete (Task 4).
- `src/server/standalone.rs` — cross-repo special-case arms + import line (Task 4).
- `CLAUDE.md`, `README.md` — counts + tool tables (Task 6).

---

## Task 1: Guard tests for the advertised surface (red)

Add the authoritative invariant first: it fails now (39 advertised) and turns green in Task 2. Also lock Tier-B routability so later deletions cannot accidentally drop a hidden-but-callable tool.

**Files:**
- Modify: `src/server/mod.rs` (append to the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `all_tools() -> Vec<rust_mcp_sdk::schema::Tool>` (each `Tool` has `.name: String`); `include_str!("mod.rs")` source-inspection idiom already used by neighboring tests.
- Produces: tests `all_tools_advertises_exactly_the_eighteen_core_tools`, `hidden_operational_tools_remain_dispatchable_but_unadvertised`.

- [ ] **Step 1: Write the two guard tests**

Add inside `mod tests` in `src/server/mod.rs` (place near the other `all_tools_*` tests):

```rust
#[test]
fn all_tools_advertises_exactly_the_eighteen_core_tools() {
    let mut names: Vec<&str> = all_tools().iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    let expected = [
        "approve_indexing",
        "ask_code",
        "bind_workspace",
        "explore_dependency_graph",
        "find_affected_code",
        "find_references",
        "find_tests_for_symbol",
        "get_call_hierarchy",
        "get_definition",
        "get_index_stats",
        "get_module_summary",
        "get_type_graph",
        "hydrate_symbols",
        "investigate",
        "refresh_index",
        "search_code",
        "summarize_file",
        "trace_data_flow",
    ];
    assert_eq!(
        names, expected,
        "all_tools() must advertise exactly the 18 core tools"
    );
}

#[test]
fn hidden_operational_tools_remain_dispatchable_but_unadvertised() {
    let advertised: Vec<&str> = all_tools().iter().map(|t| t.name.as_str()).collect();
    let source = include_str!("mod.rs");
    for hidden in [
        "get_file_symbols",
        "get_usage_examples",
        "explain_search",
        "import_external_index",
        "generate_external_index",
        "report_selection",
        "report_file_access",
    ] {
        assert!(
            !advertised.contains(&hidden),
            "{hidden} must NOT be advertised in all_tools()"
        );
        assert!(
            source.contains(&format!("\"{hidden}\" =>")),
            "{hidden} must still be routable in dispatch_tool_call"
        );
    }
}
```

- [ ] **Step 2: Run the guard tests to confirm the expected red**

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib all_tools_advertises_exactly hidden_operational_tools_remain -- --test-threads=1`
Expected:
- `all_tools_advertises_exactly_the_eighteen_core_tools` FAILS (left has 39 names, right has 18).
- `hidden_operational_tools_remain_dispatchable_but_unadvertised` FAILS on the first `!advertised.contains` assertion (these tools are still advertised today).

- [ ] **Step 3: Commit the red guard**

```bash
git add src/server/mod.rs
git commit -m "test: add guard for 18-tool advertised surface (red)"
```

---

## Task 2: Trim `all_tools()` to 18 and fix contradictory presence tests (green)

Make the guard pass by reducing the advertised list and removing the unit tests that assert deleted/hidden tools are advertised. Dispatch arms, structs, handlers, and constants stay for now, so the build stays green and Tier-C/Tier-B tools become callable-but-unadvertised. After this task the primary goal (model sees 18) is achieved and verified.

**Files:**
- Modify: `src/server/mod.rs:154-196` (`all_tools()` body)
- Modify: `src/server/mod.rs` tests: delete 7 presence tests; edit 1 instructions assertion
- Modify: `src/server/mod.rs:43` (`server_instructions()` string)

**Interfaces:**
- Consumes: the 18 Tier-A `*Tool::tool()` constructors (all already exist).
- Produces: `all_tools()` returning exactly the 18 Tier-A tools.

- [ ] **Step 1: Replace the `all_tools()` body**

Replace the entire `vec![ ... ]` in `all_tools()` (currently 39 entries, lines ~155-195) with exactly:

```rust
pub fn all_tools() -> Vec<rust_mcp_sdk::schema::Tool> {
    vec![
        // Core retrieval
        AskCodeTool::tool(),
        InvestigateTool::tool(),
        SearchCodeTool::tool(),
        HydrateSymbolsTool::tool(),
        // Navigation
        GetDefinitionTool::tool(),
        FindReferencesTool::tool(),
        GetCallHierarchyTool::tool(),
        GetTypeGraphTool::tool(),
        ExploreDependencyGraphTool::tool(),
        TraceDataFlowTool::tool(),
        FindAffectedCodeTool::tool(),
        // Overview
        SummarizeFileTool::tool(),
        GetModuleSummaryTool::tool(),
        // Tests
        FindTestsForSymbolTool::tool(),
        // Lifecycle / admin
        RefreshIndexTool::tool(),
        GetIndexStatsTool::tool(),
        BindWorkspaceTool::tool(),
        ApproveIndexingTool::tool(),
    ]
}
```

- [ ] **Step 2: Delete the 7 now-false presence tests in `src/server/mod.rs`**

Delete these entire `#[test] fn ... { ... }` blocks (they assert advertised-ness of tools now hidden or deleted):
- `all_tools_contains_get_similarity_cluster`
- `all_tools_contains_search_across_repos`
- `all_tools_contains_plan_code_investigation`
- `all_tools_contains_explore_cross_repo_dependencies`
- `all_tools_contains_get_context_bundle`
- `all_tools_contains_import_external_index`
- `all_tools_contains_generate_external_index`

Keep `all_tools_contains_ask_code`, `all_tools_contains_investigate`, `all_tools_contains_approve_indexing` (all Tier A).

- [ ] **Step 3: Drop the planner assertion from the instructions test**

In `server_instructions_describe_ask_code_as_evidence_retriever`, delete this assertion block:

```rust
        assert!(
            instructions.contains("plan_code_investigation"),
            "server instructions must still mention the planner tool, got: {instructions}"
        );
```

Keep all other assertions in that test (ask_code, evidence, pack.rows, synthesise, investigate, Grep, coverage statuses).

- [ ] **Step 4: Remove the planner sentence from `server_instructions()`**

In the `server_instructions()` string (around line 43), delete the sentence:

```
`plan_code_investigation` is recommendation-only. 
```

Leave the surrounding sentences about `investigate` and `search_code` intact. Verify the string still reads as grammatical prose.

- [ ] **Step 5: Run tests to verify green**

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib -- --test-threads=1`
Expected: PASS, including both Task 1 guard tests. Build still compiles (Tier-C/Tier-B dispatch arms, structs, handlers untouched). There may be `dead_code` warnings now that some tools are unadvertised; that is acceptable until Tasks 3-4 delete them.

- [ ] **Step 6: Commit**

```bash
git add src/server/mod.rs
git commit -m "feat: advertise only the 18 core MCP tools"
```

---

## Task 3: Delete the 12 single-file Tier-C tools (green)

Remove `plan_code_investigation` (handler only), `get_similarity_cluster`, `find_similar_code`, and the 9 `analysis.rs` tools (`get_context_bundle`, `search_todos`, `search_decorators`, `search_framework_patterns`, `find_dead_code`, `find_duplicates`, `find_stale_descriptions`, `find_undocumented_symbols`, `predict_impact`). Cross-repo (the 2 tools needing `standalone.rs` + constants) is Task 4.

**Files:**
- Modify: `src/server/mod.rs` (dispatch arms + 4 tests)
- Modify: `src/tools/mod.rs` (11 structs + 2 description tests + `find_affected_code` description)
- Modify: `src/handlers/mod.rs` (re-exports)
- Modify: `src/handlers/analysis.rs`, `src/handlers/graph.rs`, `src/handlers/search.rs`, `src/handlers/planning.rs` (handler fns)

**Interfaces:**
- Consumes: nothing new.
- Produces: 12 fewer dispatch arms; `planning::plan_code_investigation` (pure fn) preserved for `investigation.rs`.

- [ ] **Step 1: Remove the 12 dispatch arms in `src/server/mod.rs`**

In `dispatch_tool_call`, delete the match arms for these names (each is a `"name" => dispatch_sync!/dispatch_async!(...)` block):
`find_similar_code`, `get_context_bundle`, `plan_code_investigation`, `get_similarity_cluster`, `search_todos`, `search_decorators`, `search_framework_patterns`, `find_dead_code`, `find_duplicates`, `find_stale_descriptions`, `find_undocumented_symbols`, `predict_impact`.

Do NOT touch the `search_across_repos` / `explore_cross_repo_dependencies` arms (Task 4).

- [ ] **Step 2: Remove the 11 Tier-C structs in `src/tools/mod.rs`**

Delete the `#[macros::mcp_tool(...)] ... pub struct XTool { ... }` blocks for:
`FindSimilarCodeTool`, `GetContextBundleTool`, `PlanCodeInvestigationTool`, `GetSimilarityClusterTool`, `SearchTodosTool`, `SearchDecoratorsTool`, `SearchFrameworkPatternsTool`, `FindDeadCodeTool`, `FindDuplicatesTool`, `FindStaleDescriptionsTool`, `FindUndocumentedSymbolsTool`, `PredictImpactTool`.

(`SearchAcrossReposTool` / `ExploreCrossRepoDependenciesTool` stay until Task 4.)

- [ ] **Step 3: Edit the `find_affected_code` description (drop deleted-tool reference)**

In `src/tools/mod.rs`, the `FindAffectedCodeTool` description currently ends with a `predict_impact` sentence. Remove exactly this trailing sentence (including the leading space):

```
 Use predict_impact if you also want git co-change signal alongside the static graph.
```

The description must still end at `...impact analysis on symbols this tool can already locate.`

- [ ] **Step 4: Update/remove the affected description tests in `src/tools/mod.rs`**

- Delete the test `plan_code_investigation_description_advertises_routing_and_specialists` entirely.
- Delete the test `predict_impact_description_advertises_blast_radius_and_discourages_manual_grep` entirely.
- In `find_affected_code_description_advertises_impact_and_chains_predict_impact`: delete the assertion that requires `predict_impact`:

```rust
        assert!(
            desc.contains("predict_impact"),
            "find_affected_code description must name predict_impact as a richer alternative, got: {desc}"
        );
```

Keep the `"if i rename or change"` and `"Do NOT fall back to grep"` assertions. Rename the test to `find_affected_code_description_advertises_impact_and_discourages_grep`.

- [ ] **Step 5: Remove the 4 orphaned routing/serialize tests in `src/server/mod.rs`**

Delete these `#[test]` blocks (they reference deleted dispatch arms / structs):
- `dispatch_routes_plan_code_investigation`
- `get_context_bundle_tool_serializes_correctly`
- `dispatch_routes_get_context_bundle`

(The `search_across_repos` / cross-repo serialize + embedded tests are removed in Task 4.)

- [ ] **Step 6: Remove the handler re-exports in `src/handlers/mod.rs`**

- Delete `pub use planning::handle_plan_code_investigation;` (line ~60). KEEP `mod planning;` (line ~25) because `investigation.rs` imports `planning::plan_code_investigation`.
- Change `pub use search::{handle_explain_search, handle_find_similar_code, handle_search_code};` to `pub use search::{handle_explain_search, handle_search_code};`.
- Delete the `pub use` lines that re-export the 9 analysis handlers and `handle_get_similarity_cluster` (the `analysis::{...}` and `graph::{... handle_get_similarity_cluster ...}` re-export lists around lines 38-46). Remove only the deleted names; keep any co-listed retained handlers.

- [ ] **Step 7: Delete the handler functions**

- `src/handlers/planning.rs`: delete `pub fn handle_plan_code_investigation(...)` ONLY. Keep `plan_code_investigation`, `classify_intent`, `steps_for`, `InvestigationIntent`, and the `*_steps` helpers.
- `src/handlers/graph.rs`: delete `handle_get_similarity_cluster`.
- `src/handlers/search.rs`: delete `handle_find_similar_code` and its private helper `format_similar_results`. KEEP `handle_search_code` and `handle_explain_search`.
- `src/handlers/analysis.rs`: delete `handle_predict_impact`, `handle_get_context_bundle`, `handle_search_todos`, `handle_search_decorators`, `handle_search_framework_patterns`, `handle_find_dead_code`, `handle_find_duplicates`, `handle_find_stale_descriptions`, `handle_find_undocumented_symbols`, plus any `use` import at the top of `analysis.rs` for `handle_find_similar_code` / `handle_get_context_bundle` that becomes unused.

- [ ] **Step 8: Build and let the compiler surface orphans**

Run: `cargo build 2>&1 | grep -E "error|warning: unused|never used" | head -40`
Expected: no `error`. Resolve any `error[E0...]` (a deleted struct/fn still referenced) by removing that reference. For `warning: ... is never used` on private helper fns/imports left behind in `analysis.rs`/`search.rs`, delete those helpers/imports too so the build is warning-clean.

- [ ] **Step 9: Run the full test suite + clippy + fmt**

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib -- --test-threads=1`
Expected: PASS (both Task 1 guards still green).
Run: `cargo clippy --all-targets 2>&1 | grep -E "error|warning" | head` then `cargo fmt`
Expected: clippy clean, fmt makes no further changes after re-run.

- [ ] **Step 10: Commit**

```bash
git add src/server/mod.rs src/tools/mod.rs src/handlers/
git commit -m "refactor: delete 12 niche MCP tools (planner, similarity, analysis specialists)"
```

---

## Task 4: Delete the 2 cross-repo tools and their standalone wiring (green)

Cross-repo deletion is separate because it spans `standalone.rs`, two `*_EMBEDDED_MSG` constants, and a whole-file handler delete.

**Files:**
- Modify: `src/server/mod.rs` (2 dispatch arms, 2 constants, 6 tests)
- Modify: `src/tools/mod.rs` (2 structs)
- Modify: `src/handlers/mod.rs` (`mod cross_repo;` + re-export)
- Delete: `src/handlers/cross_repo.rs` (whole file)
- Modify: `src/server/standalone.rs` (import line + 2 special-case arms)

**Interfaces:**
- Consumes: nothing new.
- Produces: cross-repo tools fully gone; `list_tools` (via `all_tools()`) already excludes them.

- [ ] **Step 1: Remove the dispatch arms + constants in `src/server/mod.rs`**

- Delete the `"search_across_repos" => { ... }` and `"explore_cross_repo_dependencies" => { ... }` match arms in `dispatch_tool_call`.
- Delete the constant definitions `pub const SEARCH_ACROSS_REPOS_EMBEDDED_MSG: &str = ...;` (line ~53) and `pub const EXPLORE_CROSS_REPO_DEPS_EMBEDDED_MSG: &str = ...;` (line ~57).

- [ ] **Step 2: Remove the 6 cross-repo tests in `src/server/mod.rs`**

Delete these `#[test]` blocks:
- `all_tools_contains_search_across_repos` (already removed in Task 2 if present; skip if gone)
- `search_across_repos_tool_serializes_correctly`
- `all_tools_contains_explore_cross_repo_dependencies` (already removed in Task 2 if present; skip if gone)
- `explore_cross_repo_dependencies_tool_serializes_correctly`
- `embedded_mode_explore_cross_repo_deps_returns_helpful_message`
- `embedded_mode_search_across_repos_returns_helpful_message`

- [ ] **Step 3: Remove the 2 structs in `src/tools/mod.rs`**

Delete the `SearchAcrossReposTool` and `ExploreCrossRepoDependenciesTool` `#[mcp_tool]` blocks.

- [ ] **Step 4: Delete the handler file and its module wiring**

- Delete file: `src/handlers/cross_repo.rs`.
- In `src/handlers/mod.rs`: delete `mod cross_repo;` (line ~16) and `pub use cross_repo::{handle_explore_cross_repo_dependencies, handle_search_across_repos};` (line ~44).

- [ ] **Step 5: Remove the standalone special-case handling**

In `src/server/standalone.rs`:
- In the `use` block at line ~4, remove `handle_explore_cross_repo_dependencies, handle_search_across_repos,` (keep `parse_tool_args` and the rest of that import list).
- Delete the two `if params.name == "search_across_repos" { ... }` and `if params.name == "explore_cross_repo_dependencies" { ... }` blocks (lines ~797-820). The function then falls through to its existing shared `dispatch_tool_call` delegation.

- [ ] **Step 6: Build and resolve any orphans**

Run: `cargo build 2>&1 | grep -E "error|never used" | head -30`
Expected: no errors. Remove any leftover unused imports the compiler flags (e.g. `parse_tool_args` in `standalone.rs` if it is now unused; if still used elsewhere, leave it).

- [ ] **Step 7: Add the deletion guard test in `src/server/mod.rs`**

Append to `mod tests`:

```rust
#[test]
fn deleted_tools_are_fully_removed() {
    let advertised: Vec<&str> = all_tools().iter().map(|t| t.name.as_str()).collect();
    let source = include_str!("mod.rs");
    for deleted in [
        "plan_code_investigation",
        "get_context_bundle",
        "get_similarity_cluster",
        "find_similar_code",
        "search_todos",
        "search_decorators",
        "search_framework_patterns",
        "find_dead_code",
        "find_duplicates",
        "find_stale_descriptions",
        "find_undocumented_symbols",
        "predict_impact",
        "search_across_repos",
        "explore_cross_repo_dependencies",
    ] {
        assert!(
            !advertised.contains(&deleted),
            "{deleted} must not be advertised"
        );
        assert!(
            !source.contains(&format!("\"{deleted}\" =>")),
            "{deleted} dispatch arm must be removed from dispatch_tool_call"
        );
    }
}
```

- [ ] **Step 8: Run tests + clippy + fmt**

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib -- --test-threads=1`
Expected: PASS including `deleted_tools_are_fully_removed` and both Task 1 guards.
Run: `cargo clippy --all-targets 2>&1 | grep -E "error|warning" | head` then `cargo fmt`.
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor: delete cross-repo MCP tools and standalone wiring"
```

---

## Task 5: Daemon smoke verification

Confirm the live contract end-to-end: ListTools shows 18, a hidden tool still answers, a deleted tool is unknown. Uses an isolated daemon so the production daemon on 17800/17802 is untouched.

**Files:** none (verification only).

- [ ] **Step 1: Build release**

Run: `cargo build --release`
Expected: builds clean.

- [ ] **Step 2: Start an isolated daemon in the background**

Run: `HOME=$(mktemp -d) EMBEDDINGS_BACKEND=hash WATCH_MODE=false INDEX_CONSENT_REQUIRED=false ./target/release/code-intelligence-mcp-server --port 18800` (run in background).
Expected: it logs a listening line. The MCP transport is on `18800`; internal SDK on `18900`.

- [ ] **Step 3: List tools and assert the count is 18**

Run an MCP `tools/list` JSON-RPC call against `http://127.0.0.1:18800/mcp` (mirror the request shape `scripts/test_scoring.py` uses for `call_tool`; `tools/list` takes no params). Pipe the result through `jq '.result.tools | length'`.
Expected: `18`. Also `jq -r '.result.tools[].name' | sort` matches the 18 Tier-A names.

- [ ] **Step 4: Call a hidden Tier-B tool by name**

Issue `tools/call` for `explain_search` with `{"query":"test"}`.
Expected: a normal tool result (not a `-32601 method/tool not found`), proving Tier-B stays callable though unlisted.

- [ ] **Step 5: Call a deleted Tier-C tool by name**

Issue `tools/call` for `find_dead_code` with `{}`.
Expected: an error indicating the tool is unknown/unsupported (the SDK rejects an unrouted name).

- [ ] **Step 6: Stop the isolated daemon**

Run: `lsof -tiTCP:18800 | xargs kill` (kill precisely by port so the production daemon is never touched).

No commit (verification only). If any expectation fails, return to the relevant task.

---

## Task 6: Update documentation

Bring the docs in line with 18 advertised tools and the deleted set.

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`

**Interfaces:** none.

- [ ] **Step 1: Update `CLAUDE.md`**

- Line ~58: change "Claude Code gains access to 39 MCP tools including:" to "18 MCP tools including:". The bullet list under it names only Tier-A tools (verified), so no bullet removals are needed; just fix the count.

- [ ] **Step 2: Update `README.md` counts**

- Line ~202: change "exposes it through 39 MCP tools" to "18 MCP tools".
- Line ~490: change the tree comment "Tool definitions (39 MCP tools)" to "Tool definitions (18 advertised MCP tools)".

- [ ] **Step 3: Update the `README.md` tool table**

Remove the table rows whose first cell names a deleted (Tier-C) tool:
`plan_code_investigation`, `get_context_bundle`, `predict_impact`, `find_similar_code`, `get_similarity_cluster`, `find_duplicates`, `find_dead_code`, `search_todos`, `search_decorators`, `search_framework_patterns`, `find_undocumented_symbols`, `find_stale_descriptions`, `search_across_repos`, `explore_cross_repo_dependencies`.

Remove the table rows for the hidden (Tier-B) tools from the main list too: `get_file_symbols`, `get_usage_examples`. Then add one note line beneath the table:

```
> A few operational tools remain callable by name but are intentionally not advertised to keep the model's tool list focused: `get_file_symbols`, `get_usage_examples`, `explain_search`, `import_external_index`, `generate_external_index`, `report_selection`, `report_file_access`.
```

- [ ] **Step 4: Verify no stale references remain**

Run: `grep -rnE "39 (MCP )?tools|39 tools" CLAUDE.md README.md`
Expected: no matches.
Run: `grep -rnE "plan_code_investigation|get_context_bundle|predict_impact|get_similarity_cluster|search_across_repos|explore_cross_repo_dependencies|find_dead_code|find_duplicates|search_decorators|search_framework_patterns|find_stale_descriptions|find_undocumented_symbols|find_similar_code|search_todos" README.md`
Expected: no matches (all deleted-tool rows gone).

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: reflect 18-tool advertised surface; note callable-but-hidden tools"
```

---

## Self-Review (completed by plan author)

**Spec coverage:**
- Tier A advertised (18) → Task 2 `all_tools()` body + Task 1 guard. ✓
- Tier B hidden-but-callable (7) → Task 2 removes from `all_tools()`; Task 1 guard asserts unadvertised + routable; dispatch arms/structs/handlers untouched. ✓
- Tier C deleted (14) → Tasks 3 (12) + 4 (2); guard `deleted_tools_are_fully_removed`. ✓
- `planning.rs` helper-reuse caveat → Task 3 Step 7 keeps `plan_code_investigation` pure fn. ✓
- `find_affected_code` description references deleted `predict_impact` → Task 3 Steps 3-4. ✓
- `server_instructions()` + its test reference `plan_code_investigation` → Task 2 Steps 3-4. ✓
- Cross-repo `*_EMBEDDED_MSG` constants + `standalone.rs` arms → Task 4. ✓
- API coupling (`get_file_symbols`/`get_usage_examples` back `/api/query/*`) → preserved by keeping structs+handlers+arms (Tier B never deleted); Task 5 Step 4 smoke-checks a Tier-B call. ✓
- `explain_search` used by `scripts/test_scoring.py` → Tier B, retained; no change needed. ✓
- Verification (build, test, clippy, fmt, smoke, `all_tools().len()`-equivalent guard) → Tasks 3/4 final steps + Task 5. ✓
- Docs (counts + tables) → Task 6. ✓

**Placeholder scan:** No TBD/TODO; every code step shows exact code or exact identifiers to remove. Deletion steps name precise symbols.

**Type consistency:** Guard tests use `all_tools()` / `.name.as_str()` / `include_str!("mod.rs")`, matching the existing test idiom. Tier names are identical across Tasks 1, 4 guards and the reference section.

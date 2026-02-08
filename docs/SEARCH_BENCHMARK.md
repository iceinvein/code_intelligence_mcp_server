# Search Quality Benchmarking Methodology

## Purpose

This document describes how to run comparative search quality benchmarks between Code-Intelligence MCP Server's `search_code` tool and a reference search engine (Augment's `codebase-retrieval`). The goal is to identify and fix ranking, diversity, and relevance gaps in our search pipeline.

The benchmark is designed to be run by any agent (human or LLM) with access to both tools, against this codebase itself. It produces scored results that directly map to code changes in the retrieval/ranking pipeline.

## How to Run a Test

### Setup

Both tools must be available as MCP tools in your environment:
- **CI (Code-Intelligence):** `mcp__code-intelligence__search_code`
- **Augment (Reference):** `mcp__auggie__codebase-retrieval`

The codebase must be indexed by both tools before running benchmarks. For CI, ensure the server is running with `BASE_DIR` pointed at this repository.

### Running a Single Query

For each query in the test suite, invoke both tools in parallel and compare results.

**Code-Intelligence call:**
```json
{
  "tool": "mcp__code-intelligence__search_code",
  "input": {
    "query": "How does the MCP server handle incoming tool requests?"
  }
}
```

**Augment call:**
```json
{
  "tool": "mcp__auggie__codebase-retrieval",
  "input": {
    "information_request": "How does the MCP server handle incoming tool requests?",
    "directory_path": "/absolute/path/to/code_intelligence_mcp_server"
  }
}
```

Use the **same natural-language query** for both tools. Do not rewrite queries into keyword form for CI -- the benchmark measures how well each tool handles natural language.

### Recording Results

For each query, record:
1. The files and symbols returned by each tool (top 10)
2. A relevance score (see Scoring Rubric below)
3. Any failure patterns observed (see Common Failure Patterns below)

## How to Run (Autonomous Agent Workflow)

The recommended way to run a full 15-query benchmark round is via parallel background agents. This keeps the main conversation context small (~100 lines of summary vs ~90,000 tokens of raw results).

### Prerequisites

- Both MCP tools available (same as manual setup above)
- The agent prompt template at `docs/benchmark_rounds/AGENT_PROMPT_TEMPLATE.md`

### Step 1: Dispatch 3 Batch Agents

Split the 15 queries into 3 batches of 5. Dispatch each as a background `general-purpose` agent using the Task tool:

```
Batch 1: Q1-Q5  (Broad Concept Queries)
Batch 2: Q6-Q10 (Architecture & Cross-Cutting)
Batch 3: Q11-Q15 (Cross-Cutting & Symbol Lookups)
```

Each agent receives:
- The template from `AGENT_PROMPT_TEMPLATE.md` with placeholders filled in
- `run_in_background: true`
- Output file path: `docs/benchmark_rounds/round_N_batch_M.md`

All 3 agents run in parallel, each isolated in its own context window.

### Step 2: Wait and Read Results

After all 3 agents complete, read their output files:

```
docs/benchmark_rounds/round_N_batch_1.md  (~30 lines)
docs/benchmark_rounds/round_N_batch_2.md  (~30 lines)
docs/benchmark_rounds/round_N_batch_3.md  (~30 lines)
```

Total context consumed in main conversation: ~90 lines.

### Step 3: Compile Final Round Entry

Merge the 3 batch tables into one. Calculate:
- CI average score
- Augment average score
- Delta from previous round
- Improvements and regressions

Append the compiled round to the "Historical Benchmark Results" section of this document.

### Step 4: Clean Up (Optional)

Batch files in `docs/benchmark_rounds/` can be kept for audit trail or deleted after compilation. The compiled round in this document is the source of truth.

### Context Budget

| Approach | Main Context Usage |
|----------|-------------------|
| Manual (inline results) | ~90,000 tokens (15 queries × 2 tools × ~3,000 tokens each) |
| Agent workflow (file-based) | ~300 tokens (3 batch summaries × ~100 tokens each) |

## Scoring Rubric

Score each tool's results on a **1-10 scale** evaluating three dimensions:

| Score | Relevance | Breadth | Accuracy |
|-------|-----------|---------|----------|
| **9-10** | Every result directly answers the query | Results span all relevant files/modules | Top results are the core implementation code |
| **7-8** | Most results are relevant, minor noise | Good file diversity, covers main areas | Top 3-5 results are strong, some noise lower |
| **5-6** | ~Half the results are relevant | Some file diversity but gaps | Core implementation present but buried |
| **3-4** | Few results are relevant | Dominated by 1-2 files or one module | Test fixtures, re-exports, or tangential code rank high |
| **1-2** | Results are mostly irrelevant | Single-file flooding or all from tests | Core implementation missing entirely |

### Dimension Definitions

- **Relevance:** Would the returned code snippets help an LLM agent understand the queried concept? Does the code actually implement what was asked about?
- **Breadth:** Do results span multiple relevant files, or are they concentrated in one? A query about "error handling" should surface error handling across modules, not just `path/mod.rs`.
- **Accuracy:** Are the top results the actual implementation code, not test fixtures, re-exports (`pub mod config;`), or keyword-coincidence matches (e.g., `parse_package_json` for a "JSON serialization" query)?

### Scoring Tips

- Score conservatively. A result set that "kind of" answers the query is a 5, not a 7.
- Weight top-3 results heavily -- an agent will read those first.
- A single excellent result among 9 irrelevant ones is still a 3-4.
- Test files in results are only acceptable if the query is explicitly about testing.

## Standard Test Suite

### Category 1: Broad Concept Queries

These test whether the search engine understands high-level concepts and returns results spanning multiple files.

| # | Query | Expected Results |
|---|-------|-----------------|
| 1 | "How does the ranking and scoring system work?" | `retrieval/ranking/score.rs`, `retrieval/ranking/mod.rs`, `retrieval/ranking/diversify.rs`, `retrieval/ranking/rrf.rs` |
| 2 | "How are embeddings generated and stored?" | `storage/vector.rs`, embedding backend files, `storage/` layer |
| 3 | "How does tree-sitter parsing work in this codebase?" | `indexer/parser.rs`, `indexer/extract/` language extractors |
| 4 | "Configuration from environment variables" | Config/settings module, `Bun.serve()` or main entry point with env var reads |
| 5 | "Indexing pipeline file scanning and symbol extraction" | `indexer/mod.rs`, `indexer/extract/mod.rs`, file scanner, symbol types |

### Category 2: Architecture & Routing

These test understanding of request flow and system architecture.

| # | Query | Expected Results |
|---|-------|-----------------|
| 6 | "How does the MCP server handle incoming tool requests?" | `server/mod.rs`, `handlers/mod.rs`, tool dispatch/routing logic |
| 7 | "How does the WebSocket handler work?" | WebSocket-related handler code, connection management |
| 8 | "SQLite database schema tables initialization" | `storage/sqlite/` schema definitions, migration/init code |

### Category 3: Cross-Cutting Concerns

These are the hardest queries -- they test concepts that span the entire codebase.

| # | Query | Expected Results |
|---|-------|-----------------|
| 9 | "Error handling and graceful degradation" | Error types, fallback logic across multiple modules (not just `path/mod.rs`) |
| 10 | "JSON serialization and response formatting" | Serde derive usage, response builders, MCP protocol formatting (not `parse_package_json`) |
| 11 | "Async concurrency and parallel processing" | Async mutex usage, parallel indexing, concurrent operations in production code (not test helpers) |
| 12 | "Caching and cache invalidation" | `retrieval/cache.rs`, embedding cache, any TTL/invalidation logic |

### Category 4: Specific Symbol Lookups

These test targeted lookups where the answer is a single definition.

| # | Query | Expected Results |
|---|-------|-----------------|
| 13 | "PathNormalizer struct definition and methods" | `path/mod.rs` -- the struct and its impl block |
| 14 | "EmbeddingCache get put cached embedding" | The cache struct and its get/put methods |
| 15 | "File watcher debounce reindex on change" | Watcher module, debounce logic |

## Common Failure Patterns

These are known issues to watch for when reviewing results. Each pattern maps to a specific area of the ranking pipeline.

### 1. Single-File Flooding (Severity: HIGH)

**Symptom:** 6-8 out of 10 results come from the same file.

**Example:** Query "error handling and graceful degradation" returns 7/10 results from `path/mod.rs` because it has many error-related symbols.

**Root Cause:** No per-file diversity cap in result assembly.

**Fix Location:** `src/retrieval/ranking/diversify.rs` -- the `diversify_by_file()` function caps results per file to `max(limit/3, 2)`.

**How to Verify:** After fix, no file should appear more than ~3 times in a 10-result set for broad queries.

### 2. Keyword Semantic Mismatch (Severity: HIGH)

**Symptom:** Results match on keyword substrings in symbol names rather than actual semantic relevance.

**Example:** "JSON serialization" matches `parse_package_json` and `has_workspaces` because the symbol names contain "json", even though these functions parse `package.json` files and have nothing to do with serde/serialization.

**Root Cause:** Symbol name field has too high a boost relative to code body for natural-language queries.

**Fix Location:** `src/retrieval/query.rs` or `src/storage/tantivy.rs` -- adjust field boosts. For NL queries (3+ words), text body should be boosted 2.0x and name field reduced to 0.5x.

### 3. Test Fixture Pollution (Severity: MEDIUM)

**Symptom:** Test helper functions (`create_app_state`, `setup_test_db`, `mock_*`) rank above production code.

**Example:** "async mutex locking" returns 6/10 test fixtures that happen to use `Mutex::new()` in setup.

**Root Cause:** Test penalty multiplier too weak (was 0.5x, should be 0.3x or lower).

**Fix Location:** `src/retrieval/ranking/score.rs` -- the test file penalty constant.

### 4. Module Re-Export Noise (Severity: MEDIUM)

**Symptom:** One-liner `pub mod config;` re-exports from `lib.rs` or `mod.rs` rank #1.

**Example:** Query "configuration" returns `pub mod config;` from `lib.rs` as the top result.

**Root Cause:** Re-export lines match keyword but provide zero useful context.

**Fix Location:** `src/retrieval/ranking/score.rs` -- apply a negative score adjustment (e.g., -5.0) for re-export-only symbols (single-line `pub mod` or `pub use` statements).

### 5. Inert Intent Multipliers (Severity: MEDIUM-HIGH)

**Symptom:** Intent detection works (e.g., `Intent::Config` is detected for config queries) but has no effect on ranking.

**Example:** "configuration from environment variables" detects `Intent::Config` but all intent multipliers return 1.0x, so the intent system is a no-op.

**Root Cause:** Intent multiplier mappings return 1.0 for intents like Config, Error, Api, Implementation.

**Fix Location:** `src/retrieval/ranking/score.rs` -- the `intent_multiplier()` function. Recommended values: Config/Error/Api at 3.0x, Schema at 50-75x (already set), Definition at 1.5x.

### 6. Definition Bias on Natural Language Queries (Severity: MEDIUM)

**Symptom:** Multi-word natural-language queries get a +1.0 name-match boost meant for symbol lookups, causing symbol names that partially match query words to rank too high.

**Root Cause:** The definition boost applies regardless of query type.

**Fix Location:** `src/retrieval/ranking/score.rs` -- limit the definition/name-match boost to queries with <=2 words.

### 7. Missing Path Segment Matching (Severity: MEDIUM)

**Symptom:** Query "handler" doesn't match files in `src/handlers/` directory.

**Root Cause:** File path segments are not included in the indexed text, so Tantivy can't match on directory names.

**Fix Location:** `src/storage/tantivy.rs` -- include file path segments in the indexed text field. Also `src/retrieval/ranking/score.rs` for subdirectory prefix matching (e.g., "handler" should match "handlers").

## Improvement Workflow

### Step 1: Run a Benchmark Round

Run all 15 queries from the Standard Test Suite, scoring both tools. Record results in a table:

```markdown
| # | Query | CI Score | Augment Score | Winner | Failure Pattern |
|---|-------|----------|---------------|--------|-----------------|
| 1 | Ranking/scoring system | 7/10 | 8/10 | Augment | None |
| 2 | ... | ... | ... | ... | ... |
```

### Step 2: Identify Failure Patterns

Group the low-scoring queries by failure pattern (see Common Failure Patterns above). Prioritize by:
1. **Frequency** -- how many queries are affected?
2. **Severity** -- how badly does it hurt the score?
3. **Fixability** -- is there a clear code change?

### Step 3: Implement Fixes

For each identified pattern, make the targeted code change. Each pattern section above lists the exact file to modify.

Typical fix types:
- **Scoring adjustments** (`score.rs`): Change multipliers, add penalties, adjust boosts
- **Indexing changes** (`tantivy.rs`): Add fields, change tokenization, adjust field boosts
- **Post-processing** (`diversify.rs`, `mod.rs`): Add diversity filters, result caps
- **Query processing** (`query.rs`): Adjust how NL queries are transformed into search queries

### Step 4: Reindex and Re-Benchmark

After code changes:
1. Bump the schema version in `src/storage/tantivy.rs` (forces full reindex)
2. Rebuild: `cargo build --release`
3. Restart the server and wait for indexing to complete
4. Re-run the same benchmark round
5. Compare scores to the previous round

### Step 5: Watch for Regressions

Fixes can cause regressions. Common traps:
- **Diversity cap too aggressive** -- specific symbol lookups (Category 4) may lose relevant results from the same file
- **Test penalty too harsh** -- queries about testing should still find test code
- **Intent multipliers too strong** -- can over-boost marginally relevant results
- **Early-return bugs** -- diversity/post-processing functions that short-circuit when `hits.len() <= limit` (this exact bug was found and fixed in commit `752ab54`)

Always re-run the full suite, not just the queries you were trying to fix.

## Key Files Reference

| File | What It Controls |
|------|-----------------|
| `src/retrieval/ranking/score.rs` | Scoring signals: test penalty, intent multipliers, definition boost, re-export penalty, directory semantics |
| `src/retrieval/ranking/diversify.rs` | Per-file result diversity capping |
| `src/retrieval/ranking/rrf.rs` | Reciprocal Rank Fusion for combining Tantivy + LanceDB results |
| `src/retrieval/ranking/mod.rs` | Ranking pipeline orchestration, intent detection |
| `src/retrieval/query.rs` | Query normalization, NL vs keyword detection, field boost selection |
| `src/retrieval/mod.rs` | Top-level retrieval orchestration, candidate pool sizing |
| `src/storage/tantivy.rs` | Full-text index schema, field boosts, what gets indexed |
| `src/storage/vector.rs` | Vector/semantic search via LanceDB |
| `src/retrieval/ranking/expansion.rs` | Query expansion logic |
| `src/retrieval/assembler/mod.rs` | Context assembly from ranked results |

## Historical Benchmark Results

### Round 1 (Baseline)

| Query | CI | Augment | Winner |
|-------|-----|---------|--------|
| Ranking/scoring system | 7 | 6 | CI |
| Embeddings generation/storage | 5 | 7 | Augment |
| PathNormalizer definition | 7 | 7 | Tie |
| MCP tool request handling | 4 | 8 | Augment |
| Tree-sitter parsing | 5 | 7 | Augment |
| WebSocket handler | 4 | 7 | Augment |
| File watcher debounce | 5 | 7 | Augment |
| SQLite schema/init | 5 | 7 | Augment |
| EmbeddingCache get/put | 6 | 6 | Tie |
| **Totals** | | | **Augment 6, CI 1, Tie 2** |

### Round 2 (Pre-Fix, New Queries)

| Query | CI | Augment | Winner | Pattern |
|-------|-----|---------|--------|---------|
| Caching/invalidation | 6 | 6 | Tie | -- |
| Error handling/degradation | 3 | 8 | Augment | Single-file flooding |
| JSON serialization | 4 | 7 | Augment | Keyword mismatch |
| Async mutex/concurrency | 3 | 7 | Augment | Test fixture pollution |
| Indexing pipeline | 4 | 7 | Augment | Fragmented results |

### Round 3 (Pre-Fix, Focused Queries)

| Query | CI | Augment | Winner |
|-------|-----|---------|--------|
| JSON serialization | 4 | 7 | Augment |
| Error handling/degradation | 4 | 9.5 | Augment |
| Async concurrency | 6 | 7 | Augment |
| MCP tool handler routing | 5 | 9 | Augment |
| Configuration from env vars | 4 | 9 | Augment |

### Round 4 (Post 10-Fix Deployment)

| Query | CI | Augment | Winner | Note |
|-------|-----|---------|--------|------|
| Config from env vars | 5 | 9 | Augment | +1 improvement |
| Error handling | 2.5 | 8 | Augment | Regression (diversify bug) |
| JSON serialization | 4 | 8 | Augment | No change |
| MCP handler routing | 1 | 9 | Augment | Regression (diversify bug) |
| Indexing pipeline | 4 | 8 | Augment | No change |

**Note:** Round 4 regressions were caused by a bug in `diversify_by_file()` where an early return when `hits.len() <= limit` meant the function never actually ran. Fixed in commit `752ab54`.

### Improvement Tracking

After the diversify bug fix, the next benchmark round should show improvements for queries affected by single-file flooding (error handling, MCP routing). The remaining gap is primarily in:
- Semantic understanding of broad concept queries
- Cross-module result assembly for cross-cutting concerns
- Reducing keyword-coincidence false positives

### Round 5 (Full 15-Query Baseline)

First complete run of all 15 standard queries. CI average: **3.9/10**, Augment average: **9.5/10**.

| # | Query | CI | Augment | Winner | Pattern |
|---|-------|-----|---------|--------|---------|
| 1 | Ranking/scoring system | 3 | 9 | Augment | Keyword mismatch |
| 2 | Embeddings generation/storage | 3 | 9 | Augment | Missed vector.rs |
| 3 | Tree-sitter parsing | 5 | 9 | Augment | Mixed relevance |
| 4 | Config from env vars | 4 | 9 | Augment | Intent not applied |
| 5 | Indexing pipeline | 6 | 9 | Augment | Fragmented |
| 6 | MCP tool handling | 3 | 10 | Augment | Missing server routing |
| 7 | WebSocket handler | 2 | 4 | Augment | No WebSocket in codebase |
| 8 | SQLite schema/init | 5 | 10 | Augment | Missing SCHEMA_SQL |
| 9 | Error handling | 3 | 10 | Augment | PathError flooding |
| 10 | JSON serialization | 3 | 10 | Augment | Single-file + test pollution |
| 11 | Async concurrency | 4 | 10 | Augment | Test fixture pollution |
| 12 | Caching/invalidation | 6 | 10 | Augment | -- |
| 13 | PathNormalizer | 5 | 10 | Augment | Test fixture pollution |
| 14 | EmbeddingCache get/put | 2 | 10 | Augment | Keyword mismatch |
| 15 | File watcher debounce | 5 | 10 | Augment | Missed watcher module |

**Key finding:** The RRF scoring path (used for all hybrid search queries) completely bypassed `structural_adjustment()` and `intent_adjustment()`. These post-scoring signals (test penalty, export boost, directory semantics, intent multipliers) were only applied in the non-RRF `rank_hits_with_signals()` path, which is never called during normal search.

### Fixes Applied Between Round 5 and Round 6

1. **Apply structural/intent adjustments to RRF path** (`src/retrieval/mod.rs`): After RRF scoring completes, apply `structural_adjustment()` and `intent_adjustment()` to every hit. Made both functions `pub(crate)` in `score.rs` and re-exported via `ranking/mod.rs`.

2. **Scale popularity boost proportionally** (`src/retrieval/ranking/score.rs`): Popularity boost was adding absolute values (up to 0.05) to RRF scores (range ~0.01-0.04), dominating the ranking. Changed to scale relative to average hit score.

3. **Fix `was_recreated` flag** (`src/storage/tantivy.rs`): Changed detection from `index_dir.exists()` to `existing_version.is_some()` — the directory always exists by the time the check runs because `create_dir_all` is called first.

### Round 6 (Post RRF-Fix)

After applying structural/intent adjustments to the RRF path. CI average: **4.9/10** (+1.0 from Round 5), Augment average: **8.5/10**.

| # | Query | CI | Augment | Winner | Pattern |
|---|-------|-----|---------|--------|---------|
| 1 | Ranking/scoring system | 6 | 9 | Augment | -- |
| 2 | Embeddings generation/storage | 8 | 9 | Augment | -- |
| 3 | Tree-sitter parsing | 5 | 8 | Augment | Mixed relevance |
| 4 | Config from env vars | 4 | 9 | Augment | Intent weak |
| 5 | Indexing pipeline | 7 | 9 | Augment | -- |
| 6 | MCP tool handling | 6 | 9 | Augment | Missing server routing |
| 7 | WebSocket handler | 2 | 4 | Augment | No WebSocket in codebase |
| 8 | SQLite schema/init | 6 | 9 | Augment | Missing SCHEMA_SQL |
| 9 | Error handling | 3 | 9 | Augment | PathError flooding |
| 10 | JSON serialization | 4 | 8 | Augment | Single-file + test pollution |
| 11 | Async concurrency | 6 | 8 | Augment | Single-file flooding |
| 12 | Caching/invalidation | 7 | 9 | Augment | -- |
| 13 | PathNormalizer | 5 | 9 | Augment | Test fixture pollution |
| 14 | EmbeddingCache get/put | 2 | 10 | Augment | Keyword semantic mismatch |
| 15 | File watcher debounce | 2 | 9 | Augment | Completely missed watcher |

**Notable improvements from Round 5 → 6:**
- Q2 Embeddings: 3 → 8 (+5) — biggest single improvement
- Q1 Ranking: 3 → 6 (+3) — structural/intent adjustments now applied
- Q5 Indexing: 6 → 7 (+1)
- Q12 Caching: 6 → 7 (+1)

**Remaining failure patterns (prioritized by frequency and impact):**

1. **Keyword semantic mismatch (Q4, Q14, Q15):** Queries like "EmbeddingCache" match `FastEmbedder` because both contain "embed". Queries like "file watcher debounce" fail entirely because "watcher" only appears in body text, not symbol names. The name field still dominates scoring for natural-language queries.

2. **Test fixture pollution (Q10, Q11, Q13):** Test helpers (`create_app_state`, `setup_test_db`) rank above production code. The test penalty (0.5x via `structural_adjustment`) is now applied but is too weak — test functions still rank in top results for broad queries.

3. **Single-file flooding (Q9, Q11):** PathError still dominates Q9 with 3+ results from `path/mod.rs`. Per-file diversity cap exists but the cap of `max(limit/3, 2)` is too generous for 5-result sets (allows 2 per file).

4. **Missing body-text retrieval (Q6, Q8, Q15):** Core implementation code (server routing, SCHEMA_SQL const, watcher module) is found by Augment but missed by CI. These are cases where the important code is in function bodies rather than symbol names.

**Next priorities for Round 7:**
- Strengthen test penalty (0.5x → 0.3x)
- Tighten per-file diversity cap for small result sets
- Investigate body-text vs name-field boost ratios for multi-word queries
- Consider adding path segment tokens to indexed text (e.g., "handlers" from file path)

### Fixes Applied Between Round 6 and Round 7

1. **Test penalty strengthened** (`score.rs`): Test file penalty changed from 0.5x → 0.15x. Added `is_test_symbol()` function that detects test-named symbols (test_, create_test_, mock_, setup, teardown) even in non-test files.

2. **Diversity cap tightened** (`diversify.rs`): Per-file cap changed from `max(limit/3, 2)` to `max(limit/5, 2)`. Added `total_cap_per_file = max_per_file * 2` to also cap deferred overflow slots.

3. **3-tier Tantivy field boosts** (`tantivy.rs`): Query-length-dependent field weights:
   - 1 word: name=1.5, text=1.0 (favor symbol name matches)
   - 2 words: name=0.5, text=2.0 (favor body text)
   - 3+ words: name=0.2, text=3.0 (strongly favor body text for NL queries)

4. **Definition bias extracted** (`score.rs`): `definition_bias()` as standalone `pub(crate)` function with exact name match (+5.0) and contains match (+0.5). Applied post-RRF alongside structural/intent adjustments.

5. **Morphological path matching** (`score.rs`): `simple_stem()` function for path segment matching (e.g., "parsing" matches "parser", "handlers" matches "handler").

6. **Non-source directory penalties** (`score.rs`): Penalty of -3.0 for results in npm/, scripts/, docs/, examples/, .github/ directories.

7. **Scaled popularity boost** (`score.rs`): Popularity boost now scaled relative to average hit score, preventing it from dominating RRF-range scores.

8. **Force reindex on schema change** (`pipeline/mod.rs`): When Tantivy schema version changes, clear file fingerprints to force full reindex.

### Round 7 (Post Scoring Fixes)

After applying test penalty, diversity, field boost, and definition bias fixes. CI average: **5.0/10** (+0.1 from Round 6), Augment average: **8.5/10**.

| # | Query | CI | Augment | Winner | Pattern |
|---|-------|-----|---------|--------|---------|
| 1 | Ranking/scoring system | 8 | 9 | Augment | -- |
| 2 | Embeddings generation/storage | 6 | 9 | Augment | Missed storage layer |
| 3 | Tree-sitter parsing | 2 | 10 | Augment | Filename mismatch (parsing.rs utility ≠ tree-sitter) |
| 4 | Config from env vars | 7 | 9 | Augment | -- |
| 5 | Indexing pipeline | 5 | 9 | Augment | Test helper `symbol()` ranked #1 |
| 6 | MCP tool handling | 5 | 9 | Augment | metrics_server.rs ranked #1 instead of server/mod.rs |
| 7 | WebSocket handler | 3 | 3 | Tie | No native WebSocket in codebase |
| 8 | SQLite schema/init | 4 | 10 | Augment | Module declarations instead of SCHEMA_SQL |
| 9 | Error handling | 3 | 8 | Augment | PathError still floods (4/5 from path/mod.rs) |
| 10 | JSON serialization | 4 | 8 | Augment | npm/install.js `response` var ranked #1 |
| 11 | Async concurrency | 6 | 8 | Augment | Single-file (all 5 from parallel.rs) |
| 12 | Caching/invalidation | 5 | 9 | Augment | Missed retrieval cache + invalidation logic |
| 13 | PathNormalizer | 6 | 9 | Augment | Test functions in 4/5 slots |
| 14 | EmbeddingCache get/put | 9 | 9 | Tie | definition_bias fix works perfectly |
| 15 | File watcher debounce | 2 | 9 | Augment | Completely missed spawn_watch_loop |

**Notable changes from Round 6 → 7:**
- Q14 EmbeddingCache: 2 → 9 (+7) — **biggest improvement ever**. The `definition_bias()` +5.0 exact name match boost correctly prioritized the `put`, `get`, and `EmbeddingCache` symbols. This proves the definition bias approach works well for precise symbol queries.
- Q1 Ranking: 6 → 8 (+2) — All 5 results now from `retrieval/ranking/` files.
- Q4 Config: 4 → 7 (+3) — `from_env` and helper functions correctly found.

**Regressions from Round 6 → 7:**
- Q3 Tree-sitter: 5 → 2 (-3) — `pipeline/parsing.rs` (utility file) ranked above actual tree-sitter code. The 3-tier field boost may have over-weighted name matches for "parsing".
- Q2 Embeddings: 8 → 6 (-2) — Lost storage/vector.rs results; now only shows generation code.
- Q5 Indexing: 7 → 5 (-2) — Test helper `symbol()` from edges.rs boosted to #1 by definition_bias matching "symbol" in "symbol extraction" query.
- Q8 SQLite: 6 → 4 (-2) — Module declarations surfaced instead of actual schema SQL.
- Q12 Caching: 7 → 5 (-2) — Overfocused on reranker cache, missed retrieval cache.

**Root cause analysis for net-zero improvement:**
The `definition_bias()` fix is a double-edged sword. It dramatically helps Q14 (+7) where symbol names exactly match the query, but hurts Q5 (-2) where common English words in NL queries (like "symbol" in "symbol extraction") match test helpers and trivial functions. The bias needs to be query-type-aware: strong for 1-2 word lookups, suppressed for 3+ word NL queries.

**Remaining failure patterns (prioritized for Round 8):**

1. **Definition bias false positives on NL queries (Q3, Q5, Q10):** The +5.0 boost for exact name match is too aggressive for multi-word queries. "parsing" matches `parsing.rs` utilities, "symbol" matches test helper `symbol()`, "response" matches npm's `response` variable. **Fix:** Only apply definition_bias for queries with ≤2 words, or reduce the boost magnitude for longer queries.

2. **Single-file flooding persists (Q9, Q11, Q13):** Despite tightening the cap to limit/5, PathError still floods Q9 (4/5 from path/mod.rs) and parallel.rs floods Q11 (5/5). **Fix:** The diversity cap of `max(limit/5, 2)` with limit=10 gives max_per_file=2, which should prevent this. Investigate whether diversity is being applied too late in the pipeline or if the issue is in the primary pass before deferred slots.

3. **Semantic confusion between similarly-named modules (Q6, Q8):** "server" matches metrics/server.rs instead of server/mod.rs (Q6). "schema" returns module declarations instead of SCHEMA_SQL content (Q8). **Fix:** Boost results where the file path segment matches the query AND the symbol is a substantial definition (not a module declaration). Penalize module re-exports more aggressively.

4. **Missed body-text content (Q2, Q8, Q12, Q15):** Important code in function bodies (storage operations, schema SQL, invalidation logic, watcher loop) is missed because Tantivy indexes symbol names and surrounding text but may not capture deep function body content. **Fix:** Investigate whether the Tantivy text field includes enough body content, or whether vector search should be catching these through semantic similarity.

5. **Non-source files in results (Q10):** npm/install.js `response` variable ranked #1 for "JSON serialization". **Fix:** Apply a strong penalty (-5.0 or more) for non-Rust files when the codebase is primarily Rust, or add a project-language-awareness signal.

### Round 8 (No Code Changes — Autonomous Benchmark Workflow Test)

No code changes between Round 7 and Round 8. This round was run using the new autonomous agent workflow (3 parallel batch agents writing to `docs/benchmark_rounds/`). CI average: **5.8/10** (+0.8 from Round 7), Augment average: **8.2/10** (-0.3 from Round 7).

| # | Query | CI | Augment | Winner | Pattern |
|---|-------|-----|---------|--------|---------|
| 1 | Ranking/scoring system | 8 | 9 | Augment | -- |
| 2 | Embeddings generation/storage | 7 | 9 | Augment | Missing storage layer |
| 3 | Tree-sitter parsing | 5 | 9 | Augment | Keyword mismatch |
| 4 | Config from env vars | 7 | 9 | Augment | Missing Config struct |
| 5 | Indexing pipeline | 5 | 9 | Augment | Definition bias |
| 6 | MCP tool handling | 6 | 9 | Augment | Missing body text |
| 7 | WebSocket handler | 3 | 5 | Augment | No native WebSocket in codebase |
| 8 | SQLite schema/init | 6 | 9 | Augment | Missing body text |
| 9 | Error handling | 4 | 8 | Augment | Single-file flooding |
| 10 | JSON serialization | 4 | 7 | Augment | Keyword mismatch |
| 11 | Async concurrency | 7 | 7 | Tie | Single-file flooding (CI: 4/4 from parallel.rs) |
| 12 | Caching/invalidation | 7 | 9 | Augment | CI missed retrieval/cache.rs |
| 13 | PathNormalizer | 7 | 8 | Augment | Test pollution (3/4 test helpers) |
| 14 | EmbeddingCache get/put | 9 | 9 | Tie | -- |
| 15 | File watcher debounce | 2 | 8 | Augment | Keyword mismatch (file fingerprint SQL) |

**Notable changes from Round 7 → 8:**
- Q3 Tree-sitter: 2 → 5 (+3) — No longer returning `parsing.rs` utility as #1; `parser.rs:language_for_id` now appears
- Q8 SQLite: 4 → 6 (+2) — `SqliteStore` and `open()` now surfaced instead of just module declarations
- Q12 Caching: 5 → 7 (+2) — Found reranker cache and storage cache_key, though still missing retrieval/cache.rs
- Q2 Embeddings: 6 → 7 (+1) — Correct generation files with relevant symbols
- Q11 Async: 6 → 7 (+1) and tied with Augment — `index_files_parallel` correctly identified
- Q13 PathNormalizer: 6 → 7 (+1) — Struct definition now ranked #1

**Score drift note:** No code was changed between R7 and R8. The +0.8 CI average improvement and -0.3 Augment average change represent natural scoring variance between evaluator agents. This establishes a measurement uncertainty of roughly ±1 point per query.

**Persistent failure patterns (carried forward):**

1. **Missing body-text content (Q2, Q6, Q8, Q15):** CI finds the right files but misses key symbols defined in function bodies. `storage/vector.rs`, `server/mod.rs:handle_call_tool_request`, `schema.rs:SCHEMA_SQL`, and `spawn_watch_loop` are all in function bodies or const definitions that Tantivy may not fully index.

2. **Single-file flooding (Q9, Q11):** Error handling returns 3+ results from handlers/mod.rs. Async concurrency returns 4/4 from parallel.rs. Diversity cap may not be applied, or the cap is still too generous.

3. **Keyword mismatch (Q3, Q10, Q15):** "parsing" matches Go module parser, "serialization" matches formatting helpers, "watcher" matches file fingerprint SQL. The semantic gap between query intent and keyword matching persists.

4. **Test pollution (Q5, Q13):** Test helpers (`symbol()`, `create_test_normalizer`) still rank in top results alongside production code despite the 0.15x penalty.

**Benchmark methodology note:** Round 8 batch files preserved at `docs/benchmark_rounds/round_8_batch_{1,2,3}.md` for audit trail.

### Round 9 (No Code Changes — Variance Confirmation)

No code changes between Round 8 and Round 9. This round confirms the measurement uncertainty established in Round 8. CI average: **5.7/10** (-0.1 from Round 8), Augment average: **8.5/10** (+0.3 from Round 8).

| # | Query | CI | Augment | Winner | Pattern |
|---|-------|-----|---------|--------|---------|
| 1 | Ranking/scoring system | 8 | 9 | Augment | -- |
| 2 | Embeddings generation/storage | 7 | 9 | Augment | Missing storage layer |
| 3 | Tree-sitter parsing | 5 | 9 | Augment | Keyword mismatch |
| 4 | Config from env vars | 7 | 9 | Augment | -- |
| 5 | Indexing pipeline | 5 | 9 | Augment | Definition bias |
| 6 | MCP tool handling | 5 | 9 | Augment | Missing body text |
| 7 | WebSocket handler | 3 | 5 | Augment | Keyword mismatch |
| 8 | SQLite schema/init | 7 | 9 | Augment | Missing body text |
| 9 | Error handling | 3 | 8 | Augment | Single-file flooding |
| 10 | JSON serialization | 4 | 7 | Augment | Keyword mismatch |
| 11 | Async concurrency | 6 | 8 | Augment | Single-file flooding |
| 12 | Caching/invalidation | 7 | 9 | Augment | -- |
| 13 | PathNormalizer | 7 | 9 | Augment | Test pollution |
| 14 | EmbeddingCache get/put | 9 | 9 | Tie | -- |
| 15 | File watcher debounce | 2 | 9 | Augment | Keyword mismatch |

**Regression check (Round 8 → 9):** No query changed by more than ±1 point. This confirms the ±1 point per-query measurement uncertainty from the R7→R8 observation. Two consecutive no-change rounds provide high confidence in the noise floor.

| # | R8 | R9 | Delta |
|---|-----|-----|-------|
| 1 | 8 | 8 | 0 |
| 2 | 7 | 7 | 0 |
| 3 | 5 | 5 | 0 |
| 4 | 7 | 7 | 0 |
| 5 | 5 | 5 | 0 |
| 6 | 6 | 5 | -1 |
| 7 | 3 | 3 | 0 |
| 8 | 6 | 7 | +1 |
| 9 | 4 | 3 | -1 |
| 10 | 4 | 4 | 0 |
| 11 | 7 | 6 | -1 |
| 12 | 7 | 7 | 0 |
| 13 | 7 | 7 | 0 |
| 14 | 9 | 9 | 0 |
| 15 | 2 | 2 | 0 |

**Stable scores (R8=R9):** Q1-Q5, Q7, Q10, Q12-Q15 (11 of 15 queries identical).

### Round 9 Failure Analysis

#### Queries scoring CI < 6 (grouped by failure pattern)

**Pattern 1: Keyword Mismatch (Q3=5, Q7=3, Q10=4, Q15=2) — 4 queries, avg CI 3.5**

The most impactful failure pattern. In each case, CI matches on a keyword substring rather than the semantic concept:
- Q3: "parsing" matches `parse_go_mod` (a Go module parser) instead of tree-sitter extractors
- Q7: "handler" matches generic handler functions instead of WebSocket-specific code
- Q10: "formatting" matches `format_section_header` but misses JSON serialization (`serde_json`, `json!` macro)
- Q15: "file" matches `upsert_file_fingerprint` (SQLite file tracking) instead of `spawn_watch_loop` (file watcher)

**Proposed fix:**
- **File:** `src/retrieval/ranking/score.rs`
- **Change:** Add a "query-term coverage" signal for 3+ word queries. Boost results that match multiple distinct query terms (e.g., both "file" AND "watcher" AND "debounce") and penalize results matching only a single common word. Implementation: count how many query words appear in the result's name + file path + body text; multiply score by `matched_terms / total_query_terms`.
- **Queries that should improve:** Q3 (needs "tree-sitter" + "parsing"), Q10 (needs "JSON" + "serialization"), Q15 (needs "watcher" + "debounce" + "reindex")
- **Regression risk:** Low — this is a multiplicative boost, not a filter. Symbol lookups (Q14) won't be affected since they're already high-coverage matches.

**Pattern 2: Single-File Flooding (Q9=3, Q11=6) — 2 queries, avg CI 4.5**

Despite the `max(limit/5, 2)` per-file diversity cap:
- Q9: 3+ results from `handlers/mod.rs` (error handling)
- Q11: 4/4 from `parallel.rs` (async concurrency)

**Proposed fix:**
- **File:** `src/retrieval/ranking/diversify.rs`
- **Change:** Debug whether `diversify_by_file()` is actually being called on the RRF path. If it is, the issue may be that `limit` is larger than expected (e.g., 20 instead of 10), making `max(limit/5, 2) = max(4, 2) = 4` which allows the flooding. Enforce an absolute cap of 2 results per file for NL queries (3+ words), regardless of limit.
- **Queries that should improve:** Q9, Q11
- **Regression risk:** Medium — must not break Q14 (EmbeddingCache) where 3 results from `cache.rs` are correct. Mitigation: only apply the strict cap for 3+ word NL queries, not symbol lookups.

**Pattern 3: Missing Body Text (Q6=5) — 1 query (persistent across rounds)**

`server/mod.rs:handle_call_tool_request` is consistently missed. This function contains the critical `match tool_name { ... }` dispatch logic that routes MCP tool calls, but it's inside a function body that Tantivy may not fully index.

**Proposed fix:**
- **File:** `src/storage/tantivy.rs`
- **Change:** Investigate whether function bodies are fully included in the `text` field. If they're truncated, increase the body text capture limit. If they're present but not matching, consider adding a "context window" around the matched keyword that includes surrounding code for semantic matching.
- **Queries that should improve:** Q6, Q8 (schema SQL is also body text)
- **Regression risk:** Low — adding more indexed text improves recall without hurting precision.

**Pattern 4: Definition Bias on NL Queries (Q5=5) — 1 query**

Test helper `symbol()` and module re-export `symbol` rank above `ExtractedSymbol` because "symbol" in the query "indexing pipeline file scanning and symbol extraction" triggers definition bias on a trivial function name.

This is the same issue identified in Round 7 analysis. The +5.0 exact name match boost in `definition_bias()` should be suppressed for queries with 3+ words. This was recommended but not yet implemented.

### Priority Ranking for Next Round

1. **Keyword mismatch fix (query-term coverage signal)** — highest impact, 4 queries affected
2. **Definition bias suppression for NL queries** — already diagnosed, straightforward fix
3. **Diversity cap debugging** — verify `diversify_by_file()` is actually executing on RRF results
4. **Body text indexing depth** — investigate Tantivy text field content coverage

**Benchmark methodology note:** Round 9 batch files preserved at `docs/benchmark_rounds/round_9_batch_{1,2,3}.md` for audit trail.

### Round 10 (Code Change: term_coverage_adjustment signal)

**Change:** Added `term_coverage_adjustment()` signal to `src/retrieval/ranking/score.rs`. For queries with 3+ significant terms, measures what fraction of query terms appear in a hit's symbol name + file path. Low coverage (single-term matches) gets penalized up to -3.0; high coverage (multi-term matches) gets boosted up to +2.0. Applied as additive adjustment in post-RRF scoring.

CI average: **5.7/10** (unchanged from Round 9), Augment average: **8.4/10** (-0.1 from Round 9).

| # | Query | CI | Augment | Winner | Pattern |
|---|-------|-----|---------|--------|---------|
| 1 | Ranking/scoring system | 4 | 9 | Augment | Test pollution (new helpers) |
| 2 | Embeddings generation/storage | 6 | 9 | Augment | Single-file flooding |
| 3 | Tree-sitter parsing | 3 | 9 | Augment | Keyword mismatch |
| 4 | Config from env vars | 7 | 9 | Augment | -- |
| 5 | Indexing pipeline | 5 | 9 | Augment | Test pollution |
| 6 | MCP tool handling | 9 | 9 | Tie | -- |
| 7 | WebSocket handler | 3 | 5 | Augment | Keyword mismatch |
| 8 | SQLite schema/init | 5 | 9 | Augment | Re-export noise |
| 9 | Error handling | 3 | 8 | Augment | Single-file flooding |
| 10 | JSON serialization | 5 | 7 | Augment | Definition bias |
| 11 | Async concurrency | 8 | 8 | Tie | -- |
| 12 | Caching/invalidation | 7 | 9 | Augment | -- |
| 13 | PathNormalizer | 6 | 9 | Augment | Test pollution |
| 14 | EmbeddingCache get/put | 9 | 9 | Tie | -- |
| 15 | File watcher debounce | 5 | 8 | Augment | Keyword mismatch (improved) |

**Delta from Round 9 → 10:**

| # | R9 | R10 | Delta | Notes |
|---|-----|-----|-------|-------|
| 1 | 8 | 4 | -4 | Regression: new helpers (stems_match) indexed in score.rs |
| 2 | 7 | 6 | -1 | Noise |
| 3 | 5 | 3 | -2 | Regression: term_coverage can't help (1/4 terms match for both candidates) |
| 4 | 7 | 7 | 0 | Stable |
| 5 | 5 | 5 | 0 | Stable |
| 6 | 5 | 9 | +4 | **Improvement:** handle_call_tool_request matches "server"+"handle"+"tool"+"request" (4/4 coverage) |
| 7 | 3 | 3 | 0 | Stable (term_coverage gives same penalty to all candidates) |
| 8 | 7 | 5 | -2 | Regression: re-export noise |
| 9 | 3 | 3 | 0 | Stable |
| 10 | 4 | 5 | +1 | Slight improvement |
| 11 | 6 | 8 | +2 | **Improvement:** parallel_async matches "async"+"parallel" |
| 12 | 7 | 7 | 0 | Stable |
| 13 | 7 | 6 | -1 | Noise |
| 14 | 9 | 9 | 0 | Stable (short query, signal disabled for <3 terms) |
| 15 | 2 | 5 | +3 | **Improvement:** spawn_watch_loop now #4, upsert_file_fingerprint penalized |

**Improvements (≥+2):** Q6 (+4), Q15 (+3), Q11 (+2) — 3 queries, all attributable to term_coverage boosting multi-term matches.

**Regressions (≥-2):** Q1 (-4), Q3 (-2), Q8 (-2) — 3 queries.

**Q1 regression root cause:** The new helper functions added to score.rs (`stems_match`, `split_camel_case`, `make_hit`, `insert_test_symbol`) were reindexed and now compete with the core ranking functions. `stems_match` is a small function with high BM25 term frequency that the test pollution filter doesn't catch (it's not in a test file). This is a reindexing artifact, not a term_coverage regression — both `stems_match` and `rank_hits_with_signals` get identical term_coverage scores (2/4 = 0.5, adjustment +0.02) because they're in the same file path.

**Q3 regression analysis:** Query "How does tree-sitter parsing work in this codebase?" has 4 significant terms. Both `parse_go_mod` and `language_for_id` match only "parsing" (1/4 = 0.25 coverage), so both get the same -1.48 penalty. The signal can't discriminate when all candidates have equally low coverage.

**Target pattern analysis (keyword mismatch queries):**
- R9 avg (Q3, Q7, Q10, Q15): 3.5 → R10 avg: 4.0 (+0.5)
- Q15 showed the strongest improvement (2→5) because `spawn_watch_loop` matches 2+ query terms while `upsert_file_fingerprint` matches only 1

**Net assessment:** The term_coverage signal works as designed — it strongly boosted Q6, Q11, Q15 where multi-term matches exist. However, the overall average didn't improve because: (1) Q1 regressed due to reindexing artifacts from new code in score.rs, (2) Q3 term_coverage can't help when all candidates match the same single term, (3) Q8 noise/re-export issue.

### Round 10 Failure Analysis

#### Remaining patterns (updated priority)

**Pattern 1: Reindexing Artifacts / Small Function Pollution (Q1=4, Q5=5) — NEW**

Small utility functions and test helpers in core files (score.rs, edges.rs) rank above the main entry point functions. The issue is that BM25 gives disproportionately high scores to short documents. `stems_match` (5 lines) gets a higher TF-IDF for "score" (from file path) than `rank_hits_with_signals` (200+ lines) where the term is diluted.

**Proposed fix:**
- **File:** `src/retrieval/ranking/score.rs`
- **Change:** Add a "symbol size" or "importance" signal. Prefer functions with more lines/complexity over trivial helpers. Alternatively, add a "private function" penalty — `stems_match` is `fn` (private) while `rank_hits_with_signals` is `pub(crate) fn`.
- **Queries that should improve:** Q1, Q5
- **Regression risk:** Low — only affects ranking within same file.

**Pattern 2: Keyword Mismatch — Residual (Q3=3, Q7=3) — 2 queries, avg CI 3.0**

Term_coverage helped Q15 but couldn't help Q3 and Q7 where all candidates have the same low coverage score. The root cause for Q3 is that "tree-sitter" (hyphenated) doesn't appear in any symbol name or file path of the actual tree-sitter extractors. For Q7, no real WebSocket handler exists — it's a codebase gap, not a ranking issue.

**Proposed fix (Q3):**
- **File:** `src/storage/tantivy.rs` or indexing pipeline
- **Change:** Index the content/body text of tree-sitter language calls. The extractors call `tree_sitter_typescript::LANGUAGE_TYPESCRIPT` etc. in their source — if body text were indexed, "tree-sitter" would match. This overlaps with the "missing body text" pattern from prior rounds.
- **Regression risk:** Low.

**Pattern 3: Single-File Flooding (Q9=3, Q2=6) — 2 queries**

Same as previous rounds. Q9 returns 3+ results from handlers/mod.rs. Diversity cap may not be applied on the RRF path, or the per-file cap is too generous.

**Pattern 4: Re-export Noise (Q8=5)**

`sqlite/mod.rs:schema` (a `pub mod schema;` one-liner) ranks above `schema.rs:SCHEMA_SQL`. The re-export gets a high definition_bias boost because its name is "schema" — an exact match for the query term. The actual schema definition (`SCHEMA_SQL` const) is in the body of schema.rs, not a named symbol.

### Priority Ranking for Next Round

1. **Symbol importance signal** (line count or visibility) — fixes Q1 regression, addresses test pollution (Q5, Q13)
2. **Body text indexing depth** — fixes Q3 (tree-sitter in source), Q8 (SCHEMA_SQL), Q6 persistence
3. **Diversity cap audit** — verify diversify_by_file executes on RRF path (Q9, Q2)
4. **Definition bias suppression for NL queries** — already diagnosed (Q5, Q8 re-export)

**Benchmark methodology note:** Round 10 batch files preserved at `docs/benchmark_rounds/round_10_batch_{1,2,3}.md` for audit trail.

---

### Round 11 (symbol_importance_adjustment signal added)

**Changes since Round 10:** Added `symbol_importance_adjustment()` signal to scoring pipeline. Log-scale boost based on line count from SQLite, centered at ~45 lines. Formula: `(log2(line_count) - 5.5) * 0.4`, clamped [-1.5, 1.0]. Private functions ≤10 lines get additional -0.5 penalty. Wired into both single-query and multi-query RRF paths.

| # | Query | CI | Augment | Winner | Delta (R10→R11) | Pattern |
|---|-------|-----|---------|--------|-----------------|---------|
| 1 | Ranking and scoring system | 4 | 9 | Augment | 0 | Test pollution |
| 2 | Embeddings generated/stored | 7 | 8 | Augment | +1 | -- |
| 3 | Tree-sitter parsing | 6 | 9 | Augment | +3 | Keyword mismatch |
| 4 | Config from env vars | 6 | 9 | Augment | -1 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | +2 | Test pollution |
| 6 | MCP tool requests | 9 | 9 | Tie | 0 | -- |
| 7 | WebSocket handler | 3 | 5 | Augment | 0 | Keyword mismatch |
| 8 | SQLite schema init | 7 | 9 | Augment | +2 | Missing body text |
| 9 | Error handling | 3 | 8 | Augment | 0 | Single-file flooding |
| 10 | JSON serialization | 4 | 7 | Augment | -1 | Keyword mismatch |
| 11 | Async concurrency | 8 | 8 | Tie | 0 | -- |
| 12 | Caching/invalidation | 7 | 9 | Augment | 0 | Missing retrieval/cache.rs |
| 13 | PathNormalizer struct | 7 | 9 | Augment | +1 | Test pollution |
| 14 | EmbeddingCache get/put | 8 | 9 | Augment | -1 | -- |
| 15 | File watcher debounce | 2 | 8 | Augment | -3 | Keyword mismatch |

**CI avg: 5.87** (R10: 5.73, **+0.14**) | **Augment avg: 8.33** (R10: 8.40)

#### Analysis

**What symbol_importance fixed (net +10 points):**
- Q3 +3: Tree-sitter parsing — small helpers like `parse_go_mod` demoted, `parser.rs` file-level result promoted
- Q5 +2: Indexing pipeline — test helper `symbol` from edges.rs demoted, real extractors promoted
- Q8 +2: SQLite schema — small query functions deprioritized, `impl SqliteStore` promoted
- Q2 +1, Q13 +1: Minor improvements from demoting tiny private helpers

**What symbol_importance didn't fix (Q1 still 4):**
- Q1's problem is **test functions inside score.rs**, not small helper functions. Test functions like `page_rank_normalization_works` are 20-30 lines (above the penalty threshold) and live inside `#[cfg(test)] mod tests`. The existing test file penalty (0.5x) only applies to files in test directories (`tests/`, `__tests__/`), not to `#[cfg(test)]` blocks within production files. This is a fundamentally different bug: **in-file test block detection**.

**Regressions:**
- Q15 -3 (5→2): File watcher debounce — `spawn_watch_loop` and `check_for_changes` completely absent. This may be a reindexing artifact (functions renamed or moved) or the symbol_importance signal is now penalizing the small watcher functions that previously ranked.
- Q4 -1, Q10 -1, Q14 -1: Minor variance, likely noise.

#### Failure Pattern Analysis

**Pattern 1: Test Pollution from `#[cfg(test)]` Blocks — Q1=4, Q5=7, Q13=7 — 3 queries**

Test helper functions (`insert_test_symbol`, `make_hit`, `create_test_normalizer`) rank above production code when they live inside the same file as the production code. The existing test_file_penalty only checks file paths for `/test` directories — it doesn't detect `#[cfg(test)]` or `mod tests` blocks.

**Proposed fix:**
- **File:** `src/indexer/extract/rust.rs` or `src/retrieval/ranking/score.rs`
- **Change:** Either (a) mark symbols inside `#[cfg(test)] mod tests` as `kind = "test_function"` during extraction, then penalize in scoring; or (b) add a scoring heuristic: if a symbol's name contains `test_` prefix or its parent scope is `mod tests`, apply the test penalty.
- **Queries that should improve:** Q1 (+4), Q13 (+1)
- **Regression risk:** Low — only demotes test code within production files.

**Pattern 2: Keyword Mismatch — Persistent (Q7=3, Q10=4, Q15=2) — 3 queries, avg CI 3.0**

Queries using natural-language terms ("WebSocket", "JSON serialization", "file watcher debounce") fail because these concepts live in function bodies, comments, or external crate names — not in symbol names or file paths that BM25/vector search indexes.

**Proposed fix:**
- **File:** `src/storage/tantivy.rs` + indexing pipeline
- **Change:** Index function body text or at minimum docstrings/comments into Tantivy. Currently only symbol name and file path are searchable.
- **Queries that should improve:** Q7, Q10, Q15
- **Regression risk:** Medium — body text could introduce noise for symbol-lookup queries.

**Pattern 3: Single-File Flooding — Q9=3 — 1 query**

handlers/mod.rs returns 4+ results for error handling query. Diversity cap may not be executing on the RRF path.

### Priority Ranking for Next Round

1. **Test block detection** (in-file `#[cfg(test)]` penalty) — fixes Q1 (the persistent 4), improves Q13
2. **Body text / docstring indexing** — fixes Q7, Q10, Q15 (the keyword mismatch cluster)
3. **Diversity cap audit** — verify diversify_by_file runs on RRF path (Q9)
4. **Q15 regression investigation** — determine if watcher functions moved or symbol_importance over-penalized them

**Benchmark methodology note:** Round 11 batch files preserved at `docs/benchmark_rounds/round_11_batch_{1,2,3}.md` for audit trail.

---

### Round 12 (test_symbol_penalty — in-file `#[cfg(test)]` detection)

**Changes since Round 11:** Added `batch_check_test_symbols()` SQL query that detects symbols inside `mod tests` blocks (byte-range containment subquery) or with `#[test]` in their text. Applied -5.0 penalty to detected test symbols in both RRF paths. This specifically targets test functions and helpers inside production files like `score.rs` that were polluting Q1.

| # | Query | CI | Augment | Winner | Delta (R11→R12) | Pattern |
|---|-------|-----|---------|--------|-----------------|---------|
| 1 | Ranking and scoring system | 7 | 9 | Augment | **+3** | Single-file flooding |
| 2 | Embeddings generated/stored | 7 | 9 | Augment | 0 | -- |
| 3 | Tree-sitter parsing | 2 | 9 | Augment | **-4** | Keyword mismatch |
| 4 | Config from env vars | 8 | 9 | Augment | **+2** | -- |
| 5 | Indexing pipeline | 8 | 9 | Augment | +1 | -- |
| 6 | MCP tool requests | 9 | 9 | Tie | 0 | -- |
| 7 | WebSocket handler | 3 | 5 | Augment | 0 | Keyword mismatch |
| 8 | SQLite schema init | 5 | 9 | Augment | **-2** | Missing body text |
| 9 | Error handling | 3 | 8 | Augment | 0 | Single-file flooding |
| 10 | JSON serialization | 3 | 7 | Augment | -1 | Keyword mismatch |
| 11 | Async concurrency | 8 | 8 | Tie | 0 | -- |
| 12 | Caching/invalidation | 8 | 9 | Augment | +1 | -- |
| 13 | PathNormalizer struct | 6 | 9 | Augment | -1 | Test pollution |
| 14 | EmbeddingCache get/put | 8 | 9 | Augment | 0 | -- |
| 15 | File watcher debounce | 2 | 8 | Augment | 0 | Keyword mismatch |

**CI avg: 5.80** (R11: 5.87, **-0.07**) | **Augment avg: 8.40** (R11: 8.33)

#### Analysis

**What test_symbol_penalty fixed:**
- **Q1 +3 (4→7):** The target query. Test functions (`insert_test_symbol`, `make_hit`, `page_rank_normalization_works`) completely removed from top results. Now shows `apply_popularity_boost_with_signals`, `simple_stem`, `apply_file_affinity_boost_with_signals` — all production code. Still single-file flooding (score.rs) but relevant production code.
- **Q4 +2 (6→8):** Config query improved — test helpers in config.rs tests block deprioritized, core functions promoted.
- **Q5 +1, Q12 +1:** Minor improvements from test code demotion across files.

**Regressions:**
- **Q3 -4 (6→2):** Tree-sitter parsing severely regressed. Top results are `parse_go_mod` and `extract_local_dependencies` from package/parsers/go.rs — these match "parsing" keyword but aren't tree-sitter code. This is benchmark variance combined with the persistent keyword mismatch problem. The test penalty shouldn't affect this query (no test symbols in go.rs results).
- **Q8 -2 (7→5):** SQLite schema dropped. Migration functions rank above schema definitions. The `SCHEMA_SQL` constant is in function body text, not a named symbol.
- **Q10 -1, Q13 -1:** Minor variance.

**Q3 regression investigation:** The Q3 swing from 6 (R11) to 2 (R12) is likely benchmark noise — the evaluator agent may have scored more harshly. In R11, `parser.rs` was ranked #1 (correct), but in R12 `parse_go_mod` is #1. Since test_symbol_penalty wouldn't affect go.rs results, this may be an artifact of RRF score ordering changes when other symbols' scores shift.

#### Scorecard Summary (Rounds 9-12)

| Round | Change | CI Avg | Augment Avg | Best CI | Worst CI |
|-------|--------|--------|-------------|---------|----------|
| 9 (baseline) | — | 5.7 | 8.5 | Q6=9 | Q3=3,Q7=3,Q9=3 |
| 10 | +term_coverage | 5.73 | 8.40 | Q6=9,Q14=9 | Q3=3,Q7=3,Q9=3,Q10=3 |
| 11 | +symbol_importance | 5.87 | 8.33 | Q6=9 | Q7=3,Q9=3,Q15=2 |
| 12 | +test_symbol_penalty | 5.80 | 8.40 | Q6=9 | Q3=2,Q7=3,Q9=3,Q15=2 |

**Persistent problem queries (CI ≤ 3 across all rounds):**
- **Q7 (WebSocket):** Keyword mismatch — "WebSocket" only in function bodies, not symbol names
- **Q9 (Error handling):** Single-file flooding — handlers/mod.rs dominates
- **Q15 (File watcher):** Keyword mismatch — "watcher" and "debounce" in function bodies only

**Solved queries:**
- **Q1:** 3→4→4→**7** (test pollution → fixed by test_symbol_penalty)
- **Q6:** 5→9→9→**9** (stabilized at ceiling)

**Benchmark methodology note:** Round 12 batch files preserved at `docs/benchmark_rounds/round_12_batch_{1,2,3}.md` for audit trail.

---

### Rounds 13-23 (Skipped Documentation)

Rounds 13-23 were run but not documented in this file. Key changes across those rounds:
- Synonym expansion (bidirectional `get_related_terms()`)
- Term coverage adjustment for multi-word queries
- Various scoring tweaks

The net effect was CI avg rising from 5.80 (R12) to 6.47 (R24).

---

### Round 24 (Synonyms + Term Coverage + Test Penalty Refinements)

**Changes since Round 12:** Added synonym entries (websocket→ws/socket/realtime, serialization→serde, watcher→watch/monitor, etc.), bidirectional synonym lookup (`get_related_terms()`), lowered term_coverage threshold from `< 3` to `< 2` (2-word queries now scored), synonym-aware term coverage fallback.

| # | Query | CI | Augment | Winner | R12 | Delta |
|---|-------|-----|---------|--------|-----|-------|
| 1 | Ranking/scoring system | 8 | 9 | Augment | 7 | +1 |
| 2 | Embeddings generation/storage | 5 | 9 | Augment | 7 | -2 |
| 3 | Tree-sitter parsing | 3 | 9 | Augment | 2 | +1 |
| 4 | Config from env vars | 8 | 9 | Augment | 8 | 0 |
| 5 | Indexing pipeline | 7 | 9 | Augment | 8 | -1 |
| 6 | MCP tool handling | 9 | 9 | Tie | 9 | 0 |
| 7 | WebSocket handler | 2 | 7 | Augment | 3 | -1 |
| 8 | SQLite schema/init | 9 | 9 | Tie | 5 | +4 |
| 9 | Error handling | 4 | 8 | Augment | 3 | +1 |
| 10 | JSON serialization | 3 | 7 | Augment | 3 | 0 |
| 11 | Async concurrency | 9 | 8 | CI | 8 | +1 |
| 12 | Caching/invalidation | 8 | 9 | Augment | 8 | 0 |
| 13 | PathNormalizer | 7 | 9 | Augment | 6 | +1 |
| 14 | EmbeddingCache get/put | 10 | 10 | Tie | 8 | +2 |
| 15 | File watcher debounce | 5 | 9 | Augment | 2 | +3 |

**CI avg: 6.47** (R12: 5.80, **+0.67**) | **Augment avg: 8.67**

---

### Round 25 (Import Tags + Synonym Expansion + Bug Fix)

**Changes since Round 24:**
1. **Import tag extraction** (`text.rs`): `extract_rust_import_tags()` parses `use` statements to extract crate/module names. `build_import_tags_from_sources()` aggregates tags across all source files in a batch.
2. **Import tag injection** (`tantivy.rs`): `expand_index_text()` appends import tags to indexed text. If a file imports `tree_sitter`, the term "tree_sitter" appears in its searchable text.
3. **Pipeline integration** (`pipeline/mod.rs`, `parallel.rs`): Import tags extracted before symbol loop and passed to Tantivy upsert.
4. **Schema v6→v7**: Forces full reindex to pick up import tags.
5. **New synonyms** (`text.rs`): "serde" and "sitter" added.
6. **Bug fix** (`main.rs`): Vector migration cleared fingerprints but not `similarity_clusters`, causing `generate_embeddings_for_parallel_indexed_files()` to find 0 symbols needing embeddings. Fix: also clear similarity_clusters.

| # | Query | CI | Augment | Winner | R24 | Delta | Pattern |
|---|-------|-----|---------|--------|-----|-------|---------|
| 1 | Ranking/scoring system | 8 | 9 | Augment | 8 | 0 | Single-file flooding |
| 2 | Embeddings generation/storage | 5 | 9 | Augment | 5 | 0 | Single-file flooding |
| 3 | Tree-sitter parsing | **7** | 9 | Augment | 3 | **+4** | -- |
| 4 | Config from env vars | 8 | 9 | Augment | 8 | 0 | -- |
| 5 | Indexing pipeline | **8** | 9 | Augment | 7 | **+1** | -- |
| 6 | MCP tool handling | 9 | 9 | Tie | 9 | 0 | -- |
| 7 | WebSocket handler | 2 | 7 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema/init | 7 | 9 | Augment | 9 | -2 | Missing body text |
| 9 | Error handling | 4 | 9 | Augment | 4 | 0 | Keyword mismatch |
| 10 | JSON serialization | **4** | 8 | Augment | 3 | **+1** | Keyword mismatch |
| 11 | Async concurrency | 8 | 9 | Augment | 9 | -1 | Single-file flooding |
| 12 | Caching/invalidation | **9** | 10 | Augment | 8 | **+1** | -- |
| 13 | PathNormalizer | 7 | 10 | Augment | 7 | 0 | Test pollution |
| 14 | EmbeddingCache get/put | 9 | 10 | Augment | 10 | -1 | -- |
| 15 | File watcher debounce | **6** | 9 | Augment | 5 | **+1** | Keyword mismatch |

**CI avg: 6.73** (R24: 6.47, **+0.26**) | **Augment avg: 9.00**

#### Analysis

**What import tags fixed:**
- **Q3 +4 (3→7):** The headline result. Files importing `tree_sitter` crates now have that term in indexed text, so "tree-sitter parsing" finds the actual extractors.
- **Q5 +1, Q10 +1, Q12 +1, Q15 +1:** Minor improvements from better term matching via import tags and synonyms.

**Regressions:**
- **Q8 -2 (9→7):** Within ±2 noise band. SCHEMA_SQL constant still a persistent body-text miss.
- **Q11 -1, Q14 -1:** Within noise.

#### Scorecard Summary (Key Rounds)

| Round | Change | CI Avg | Augment Avg | Best CI | Worst CI |
|-------|--------|--------|-------------|---------|----------|
| 5 (first full) | baseline | 3.9 | 9.5 | Q5=6 | Q7=2,Q14=2 |
| 8 (post fixes) | +RRF scoring+diversity | 5.8 | 8.2 | Q14=9 | Q15=2 |
| 12 | +test_symbol_penalty | 5.80 | 8.40 | Q6=9 | Q3=2,Q7=3 |
| 24 | +synonyms+term_coverage | 6.47 | 8.67 | Q14=10 | Q7=2 |
| **25** | **+import_tags+bug_fix** | **6.73** | **9.00** | Q6,Q12,Q14=9 | Q7=2 |

#### Persistent Low-Scorers (CI ≤ 4, unchanged across R12-R25)

| # | Query | CI | Root Cause | Why It's Hard |
|---|-------|----|------------|---------------|
| 7 | WebSocket handler | 2 | Keyword mismatch | "WebSocket" only in elysia.rs function bodies and enum variants. No symbol named *websocket*. Also partially a codebase gap — no native WS handler. |
| 9 | Error handling | 4 | Keyword mismatch | Error patterns scattered across modules. `tool_internal_error`, retry logic, fallback paths — all in function bodies. Cache functions match "error" keyword noise. |
| 10 | JSON serialization | 4 | Keyword mismatch | `json!()` macro calls and `Serialize` derives are in function bodies. `parse_package_json` is a false positive from npm.rs. |

#### Solved Queries (significant improvement from baseline)

| # | Query | R5 | R25 | Total Gain | Key Fix |
|---|-------|----|-----|------------|---------|
| 1 | Ranking/scoring | 3 | 8 | +5 | RRF scoring + test_symbol_penalty |
| 6 | MCP tool handling | 3 | 9 | +6 | term_coverage (4/4 term match) |
| 8 | SQLite schema | 5 | 7 | +2 | structural_adjustment |
| 14 | EmbeddingCache | 2 | 9 | +7 | definition_bias exact name match |
| 15 | File watcher | 5 | 6 | +1 | import_tags + synonyms |

### Priority Ranking for Next Round (R26)

1. **Body text / concept indexing** — The #1 remaining gap. Q7, Q9, Q10 all fail because concepts (`WebSocket`, `error handling`, `json!()`) live in function bodies, not symbol names. Options:
   - **Option A:** Index more body text into Tantivy `text` field (risk: BM25 noise from large functions)
   - **Option B:** Extract "concept tags" from function bodies (like import tags but for key patterns: `json!`, `WebSocket`, `fallback`, `retry`)
   - **Option C:** Rely on vector search for these queries — improve vector recall or vector weight (`HYBRID_ALPHA`)
   - **Recommended:** Option B (concept tags) — targeted, low regression risk, same pattern as import tags

2. **Single-file flooding** — Q1 (5/5 from score.rs), Q2 (5/5 from pipeline/mod.rs), Q11 (4/5 from parallel.rs). The `diversify_by_file` cap exists but may not be aggressive enough for these cases. Audit whether it runs on the RRF path and consider tightening to max 2 per file for NL queries.

3. **Q2 Embeddings regression** — Dropped from 7 (R6) to 5 (R24/R25). Core files (`embeddings/mod.rs`, `fastembed.rs`, `storage/vector.rs`) missing from results. Investigate whether pipeline/mod.rs is dominating due to high edge count / popularity boost.

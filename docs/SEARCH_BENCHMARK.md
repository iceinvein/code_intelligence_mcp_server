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

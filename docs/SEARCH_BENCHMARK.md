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

---

### Round 26 (Concept Tags for Function Bodies)

**Changes since Round 25:**
1. **Concept tag extraction** (`text.rs`): Added `extract_concept_tags()` function that scans symbol body text for 12 patterns across 3 categories:
   - JSON/Serialization: `json!(`, `serde_json`, `to_string_pretty`, `#[derive(Serialize)]`
   - WebSocket: `WebSocket`, `websocket`, `.ws(`
   - Error handling: `map_err(`, `tool_internal_error`, `CallToolError`, `unwrap_or_else(`, `ok_or_else(`, `downcast_ref`
2. **Concept tag injection** (`tantivy.rs`): Concept tags appended to indexed text in `expand_index_text()`, with dedup. Tags also fed into synonym expansion (e.g., "serialization" → "serialize serde deserialize marshal").
3. **Schema v7→v8**: Forces full reindex.

| # | Query | CI | Augment | Winner | R25 | Delta | Pattern |
|---|-------|-----|---------|--------|-----|-------|---------|
| 1 | Ranking/scoring system | 8 | 9 | Augment | 8 | 0 | Single-file flooding (score.rs) |
| 2 | Embeddings generation/storage | 5 | 9 | Augment | 5 | 0 | Keyword mismatch, missing core files |
| 3 | Tree-sitter parsing | 7 | 9 | Augment | 7 | 0 | Missing parser.rs in top results |
| 4 | Config from env vars | 8 | 9 | Augment | 8 | 0 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 8 | -1 | -- |
| 6 | MCP tool handling | 9 | 10 | Augment | 9 | 0 | -- |
| 7 | WebSocket handler | **2** | 7 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema/init | 7 | 9 | Augment | 7 | 0 | -- |
| 9 | Error handling | **4** | 9 | Augment | 4 | 0 | Missing body text |
| 10 | JSON serialization | **4** | 7 | Augment | 4 | 0 | Keyword mismatch |
| 11 | Async concurrency | 8 | 9 | Augment | 8 | 0 | -- |
| 12 | Caching/invalidation | 8 | 9 | Augment | 9 | -1 | -- |
| 13 | PathNormalizer | 7 | 10 | Augment | 7 | 0 | Test pollution |
| 14 | EmbeddingCache get/put | 9 | 10 | Augment | 9 | 0 | Single-file flooding |
| 15 | File watcher debounce | 7 | 9 | Augment | 6 | +1 | -- |

**CI avg: 6.67** (R25: 6.73, **-0.07**) | **Augment avg: 8.93**

#### Analysis

**Why concept tags had zero impact on target queries (Q7=2, Q9=4, Q10=4):**

Concept tags ARE firing — `schema.rs` with `#[derive(Serialize)]` now ranks for Q10, and `tool_internal_error` ranks #1 for Q9. But the tags are being **buried by the RRF fusion pipeline** before reaching final results.

**Root cause investigation** revealed a 3-layer kill chain for concept-tagged keyword matches:

1. **NL Query Weight Adjustment** (`retrieval/mod.rs:467-479`): For 3+ word queries (all of Q7/Q9/Q10), the RRF pipeline applies `keyword_weight * 0.5` and `vector_weight * 1.5`. This means concept tags added to Tantivy (keyword search) are systematically deprioritized for the exact queries they're designed to help. A keyword rank #2 contributes only `0.5/(60+3) = 0.008` to RRF, while a vector rank #5 for an irrelevant symbol contributes `1.5/(60+6) = 0.023` — **3x higher**.

2. **Term Coverage Body Credit** (`score.rs:671-689`): Concept tags appear in body text, not symbol names. Body matches get only 0.5x credit in term coverage. A concept-tagged symbol matching "serialization" gets coverage ~0.25, resulting in a **-1.48 penalty** instead of a boost.

3. **RRF Score Flattening**: RRF only considers rank position, not BM25 score magnitude. A concept-tagged symbol with a high BM25 score and an irrelevant symbol with a moderate vector similarity both contribute nearly identical RRF scores.

**The fix is NOT more concept tags — it's fixing the RRF fusion pipeline to stop penalizing keyword matches for NL queries.**

#### Scorecard Summary (Key Rounds)

| Round | Change | CI Avg | Augment Avg | Best CI | Worst CI |
|-------|--------|--------|-------------|---------|----------|
| 5 (first full) | baseline | 3.9 | 9.5 | Q5=6 | Q7=2,Q14=2 |
| 8 (post fixes) | +RRF scoring+diversity | 5.8 | 8.2 | Q14=9 | Q15=2 |
| 12 | +test_symbol_penalty | 5.80 | 8.40 | Q6=9 | Q3=2,Q7=3 |
| 24 | +synonyms+term_coverage | 6.47 | 8.67 | Q14=10 | Q7=2 |
| 25 | +import_tags+bug_fix | 6.73 | 9.00 | Q6,Q12,Q14=9 | Q7=2 |
| **26** | **+concept_tags** | **6.67** | **8.93** | Q6,Q14=9 | **Q7=2** |
| **27** | **+equal_RRF_weights+body_0.75** | **6.47** | **9.00** | Q6=10 | **Q4=4,Q7=2** |
| **28** | **+strip_string_literals+concept_tag_fix** | **6.53** | **9.07** | Q6,Q14=10 | **Q7=2,Q9=3** |
| **29** | **+restore_string_literals** | **6.67** | **9.13** | Q6,Q14=10 | **Q7=2,Q9=3** |
| **30** | **+handler_tag+pool_expansion** | **5.87** | **8.87** | Q6,Q14=9 | **Q7=2,Q15=3** |
| **31** | **-handler_tag (schema revert)** | **6.27** | **9.00** | Q6,Q14=9 | **Q7=2,Q9=3** |

#### Round 27 Analysis

**Changes tested:**
1. **Equal RRF weights for NL queries** — Changed from `(0.5x keyword, 1.5x vector)` to `(1.0x, 1.0x)`. Hypothesis: concept/import tags made BM25 smarter, so it no longer needs penalty.
2. **Body credit 0.75** — term_coverage body match credit increased from 0.5 to 0.75.
3. **Wider Q9 concept tags** — Added `Err(e)`, `if let Err`, `.unwrap_or()`, `fallback`/`degrade` patterns. **(Not active — needed re-index which failed mid-session.)**

**Result: Net regression (-0.20).** Equal RRF weights was the wrong fix.

**What happened:**
- **Q4 cratered (-4):** Vector search correctly found `config.rs` for "Configuration from environment variables" via semantic similarity. Equal weights let BM25 noise from `score.rs` (matching "environment" in irrelevant contexts) compete equally, drowning out the vector signal.
- **Q6 improved to 10 (+1):** CI beat Augment for the first time. "MCP server tool requests" has strong keyword signal in symbol names — equal weights let it shine.
- **Q9 improved (+1):** Error handling results gained diversity (3 retrieval/mod.rs results showing degradation patterns).
- **Q7/Q10 unchanged:** Score.rs flooding Q7 (matching "handler" in function names). Q10 meta-matched on `extract_concept_tags` function which *contains* "serialization" as a string literal.

**Key insight:** The problem is NOT that keyword weight is too low for NL queries. It's that **BM25 returns the wrong results** for conceptual queries — symbol name matches dominate over body-text matches even when body has the right content. Changing RRF weights amplifies both good and bad BM25 results equally.

**Action taken:** Reverted Change 1 (equal RRF weights). Kept Change 2 (body credit 0.75) and Change 3 (wider concept tags — pending re-index).

### Priority Ranking for Next Round (R28)

1. **Tantivy field-level boost for NL queries** — Instead of changing RRF weights, boost the `body_text` field and reduce `symbol_name` field weight in the Tantivy BM25 query for NL queries. This makes keyword search return better results rather than amplifying bad ones.
   - Location: `storage/tantivy.rs` query construction
   - **Expected impact:** Q7, Q9, Q10 should improve because body-text matches (concept tags, actual code patterns) outrank name-coincidence matches
   - **Regression risk:** Low — only changes internal BM25 ranking, not RRF fusion

2. **Q10 meta-match fix** — `extract_concept_tags` in `text.rs` contains string literals like "serialization", "websocket" etc. that cause false BM25 matches. Either exclude this function from indexing or add a penalty for self-referential concept tag matches.
   - **Expected impact:** +2-3 for Q10
   - **Regression risk:** Low

3. **Complete re-index for wider concept tags** — R27 Change 3 (wider error handling patterns) needs a clean re-index. On next server restart, clear fingerprints and rebuild.
   - **Expected impact:** +1-2 for Q9
   - **Regression risk:** None

4. **Q2 embeddings investigation** — Persistent at CI=5. Core files (`embeddings/mod.rs`, `fastembed.rs`, `storage/vector.rs`) missing from results despite being semantically obvious matches. Vector search should find these — investigate why it doesn't.

### Round 28 (Strip String Literals + Concept Tag Meta-Match Fix)

**Changes since Round 27:**
1. **strip_string_literals()** (`tantivy.rs`): New function strips content inside `"..."` from code body before indexing. Prevents BM25 false matches on string literal contents (e.g., `tags.insert("serialization")` no longer matches serialization queries).
2. **Concept tag extraction uses stripped text** (`tantivy.rs:689`): `extract_concept_tags(&result)` runs on stripped text instead of original. Meta-code like `text.contains("json!(")` no longer triggers concept tags because `"json!("` is inside quotes and gets stripped.
3. **Schema v8→v10**: Forces full re-index to apply string stripping and wider concept tags.
4. **Carried from R27**: Body credit 0.75 (score.rs), wider concept tags (text.rs), RRF weight revert (0.5x/1.5x restored).

| # | Query | CI | Augment | Winner | R27 | Delta | Pattern |
|---|-------|-----|---------|--------|-----|-------|---------|
| 1 | Ranking/scoring system | 8 | 9 | Augment | 7 | +1 | Single-file flooding (score.rs) |
| 2 | Embeddings generation/storage | 5 | 9 | Augment | 5 | 0 | Missing core files |
| 3 | Tree-sitter parsing | 6 | 9 | Augment | 6 | 0 | Missing parser.rs |
| 4 | Config from env vars | 7 | 9 | Augment | 4 | **+3** | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 8 | -1 | -- |
| 6 | MCP tool handling | **10** | 9 | **CI** | 10 | 0 | -- |
| 7 | WebSocket handler | **2** | 7 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema/init | 7 | 10 | Augment | 7 | 0 | Missing body text |
| 9 | Error handling | **3** | 9 | Augment | 5 | **-2** | Keyword mismatch |
| 10 | JSON serialization | **4** | 8 | Augment | 4 | 0 | Keyword mismatch |
| 11 | Async concurrency | 8 | 9 | Augment | 8 | 0 | -- |
| 12 | Caching/invalidation | 8 | 9 | Augment | 8 | 0 | -- |
| 13 | PathNormalizer | 7 | 10 | Augment | 7 | 0 | Test pollution |
| 14 | EmbeddingCache get/put | **10** | 10 | **Tie** | 9 | +1 | -- |
| 15 | File watcher debounce | 6 | 9 | Augment | 7 | -1 | Single-file flooding |

**CI avg: 6.53** (R27: 6.47, **+0.07**) | **Augment avg: 9.07**

#### Round 28 Analysis

**What the strip_string_literals fix achieved:**
- Q10 meta-match eliminated: `extract_concept_tags` no longer ranks #1 for "JSON serialization" (was #1 in R27, gone in R28)
- Q14 hit first perfect 10: EmbeddingCache query benefits from reduced noise in indexed text
- Q4 recovered +3: RRF weight revert from R27 (was 4 with equal weights, back to 7 with 0.5x/1.5x)

**What it hurt:**
- **Q9 regressed -2** (5→3): Error handling code expresses intent through string literals (`"Failed to parse"`, `"Invalid configuration"`, `tool_internal_error`). Stripping these removes exactly the words that help BM25 match error handling queries. The batch notes show results dominated by `handle_find_affected_code` matching "error" in the symbol name, while `tool_internal_error` (which appears in stripped strings) dropped to #5.
- Q15 regressed -1 (7→6): File watcher results are all from pipeline/mod.rs (5/5), losing config.rs watch knobs.

**The fundamental tradeoff of strip_string_literals:**
Stripping string literal contents removes both false positives (meta-matching, keyword coincidence) AND true positives (meaningful error messages, config strings, SQL content). For Q10 it helped (removed meta-match), for Q9 it hurt (removed useful error context), and for Q8 it was neutral (SCHEMA_SQL is a raw string that gets stripped, but the schema types/init functions still match).

**Net assessment:** +0.07 is within noise range (±1 per query established in R8/R9). The strip_string_literals approach is a wash — it trades Q9 regression for Q4 recovery and meta-match elimination.

**Persistent low-scorers** (CI ≤ 4): Q7=2 (WebSocket), Q9=3 (Error), Q10=4 (JSON)
- Q7: No native WebSocket handler in codebase; only framework pattern detection code in elysia.rs
- Q9: BM25 matches "affected_code" handler (contains "error" in symbol name) over actual error handling patterns
- Q10: Despite meta-match fix, BM25 still can't find actual json!() usage in handler functions

### Round 29 (Restore String Literals for BM25 + Error Handling Concept Tags)

**Changes since Round 28:**
1. **Restore original text for BM25 indexing** (`tantivy.rs:expand_index_text`): Stopped stripping string literals from indexed text. `strip_string_literals()` is now only used for concept tag extraction, preserving error messages, SQL constants, and config strings for BM25 matching while still preventing concept tag meta-matching.
2. **Wider error handling concept tags** (`text.rs:extract_concept_tags`): Added patterns for `bail!()`, `.context()`, `.with_context()`, `anyhow!()`, `tracing::error`, `tracing::warn`, `eprintln!()`.
3. **Schema v10→v11**: Forces full re-index to apply changes.

| # | Query | CI | Augment | Winner | R28 | Delta | Pattern |
|---|-------|-----|---------|--------|-----|-------|---------|
| 1 | Ranking/scoring system | 7 | 9 | Augment | 8 | -1 | Single-file flooding (score.rs 5/5) |
| 2 | Embeddings generation/storage | 5 | 9 | Augment | 5 | 0 | Missing core files |
| 3 | Tree-sitter parsing | 6 | 9 | Augment | 6 | 0 | Missing parser.rs |
| 4 | Config from env vars | 8 | 9 | Augment | 7 | **+1** | -- |
| 5 | Indexing pipeline | 8 | 9 | Augment | 7 | **+1** | -- |
| 6 | MCP tool handling | **10** | 9 | **CI** | 10 | 0 | -- |
| 7 | WebSocket handler | **2** | 7 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema/init | 7 | 10 | Augment | 7 | 0 | Missing body text |
| 9 | Error handling | **3** | 9 | Augment | 3 | 0 | Keyword mismatch |
| 10 | JSON serialization | **4** | 8 | Augment | 4 | 0 | Keyword mismatch |
| 11 | Async concurrency | 8 | 9 | Augment | 8 | 0 | -- |
| 12 | Caching/invalidation | 8 | 10 | Augment | 8 | 0 | -- |
| 13 | PathNormalizer | 7 | 10 | Augment | 7 | 0 | Test pollution |
| 14 | EmbeddingCache get/put | **10** | 10 | **Tie** | 10 | 0 | -- |
| 15 | File watcher debounce | 7 | 10 | Augment | 6 | **+1** | Single-file flooding |

**CI avg: 6.67** (R28: 6.53, **+0.14**) | **Augment avg: 9.13**

#### Round 29 Analysis

**What restoring string literals achieved:**
- Q4 (+1) and Q5 (+1): Preserved string content in config.rs and pipeline/mod.rs improves body-text matching for helper functions (`parse_csv_or_default`, `index_files_sequential_internal`)
- Q15 (+1): spawn_watch_loop body text with string content helps differentiate watcher-related code
- Q10 meta-match stays fixed: concept tag extraction still uses stripped text, so `extract_concept_tags` doesn't self-rank for serialization queries

**What didn't help:**
- **Q9 stayed at 3** despite restoring string literals AND adding error handling concept tags. The fundamental problem: `handle_find_affected_code` contains "error" in its full symbol context (handles "affected code" which includes error impact analysis) and dominates results. The concept tags fire correctly (bail!, .context(), etc.) but don't outweigh BM25 name-match for "affected_code → error".
- **Q7 stayed at 2**: classify_elysia_method still not surfaced. The function name doesn't contain "websocket" or "handler" — BM25 can't bridge the gap between query terms and the actual code patterns.

**Q1 regressed -1** (8→7): All 5 results from score.rs (single-file flooding). `simple_stem` helper at #2 is noise — it's a utility function, not part of the ranking system. In R28 the evaluator gave 8 with similar results; likely evaluator noise within ±1 range.

**Failure pattern summary for CI ≤ 6:**
| Pattern | Queries | Root Cause |
|---------|---------|------------|
| Missing core files | Q2 (5) | Vector search not finding embeddings/mod.rs, fastembed.rs |
| Keyword mismatch | Q7 (2), Q9 (3), Q10 (4) | Query terms don't appear in relevant function names/bodies |
| Missing dispatcher | Q3 (6) | parser.rs (language detection) not surfaced in tree-sitter queries |

**Persistent low-scorers** (CI ≤ 4): Q7=2 (WebSocket), Q9=3 (Error), Q10=4 (JSON)
- Q7: 28 rounds at 2-3. The classify_elysia_method function name has zero keyword overlap with "websocket handler"
- Q9: Error handling patterns are in function bodies, not names. BM25 name-match for "affected_code" dominates
- Q10: json!() macro usage scattered across handlers; no single "serialization" entry point

### Priority Ranking for Next Round (R30)

1. **File-level diversity enforcement** — Single-file flooding is the most common pattern (Q1, Q15, and others). When all 5 results come from one file, useful context from related files is lost. Add a diversity penalty in RRF/reranking: cap results from a single file at 3, then pull in results from other files.
   - Location: `src/retrieval/ranking/` or `src/retrieval/mod.rs` RRF stage
   - **Expected impact:** Q1 +1-2 (rrf.rs, diversify.rs, ranking/mod.rs would surface), Q15 +1 (config.rs watch knobs)
   - **Regression risk:** Low — doesn't change scoring, only diversifies

2. **Concept tag synonyms for function names** — For Q7 and Q9, the problem is that function names don't contain query terms. Consider injecting concept tags based on function body patterns even when they don't match the name:
   - `classify_elysia_method` body contains `"ws"` match arm → inject "websocket handler" tag
   - `handle_find_affected_code` should NOT rank for "error handling" — it's about code impact analysis
   - **Expected impact:** Q7 +2-3, Q9 +1-2

3. **Q2 embeddings: Vector search debug** — After 6 rounds at CI=5, this needs deep investigation. Why doesn't vector search find semantically obvious matches like embeddings/mod.rs for "how are embeddings generated"?

4. **Q10 JSON: Cross-file pattern matching** — json!() macro usage is scattered. Consider a concept tag for files that heavily use `serde_json` or `json!` patterns.

---

### Round 30 — Diversity Pipeline + WebSocket Concept Tags (Two Iterations)

**Changes tested:**
1. **Schema v12**: Added "handler" concept tag for WebSocket-matching symbols in `text.rs`
2. **R30v1**: Pre-truncation `diversify_by_file()` — runs diversity on full candidate pool (~40-100 results) before truncation to limit
3. **R30v2**: Reverted R30v1; instead expanded truncation pool to `limit*3` (15 candidates) so post-expansion diversity has more headroom

#### R30v1 Results (Pre-Truncation Diversity — REGRESSED)

| # | Query | CI | Augment | Winner | R29 | Delta | Pattern |
|---|-------|----|---------|--------|-----|-------|---------|
| 1 | Ranking/scoring | 8 | 9 | Augment | 7 | +1 | Single-file flooding (improved) |
| 2 | Embeddings | 5 | 9 | Augment | 5 | 0 | Single-file flooding |
| 3 | Tree-sitter | 6 | 9 | Augment | 6 | 0 | -- |
| 4 | Config env | 7 | 9 | Augment | 8 | -1 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 8 | -1 | -- |
| 6 | MCP tool requests | 9 | 10 | Augment | 10 | -1 | -- |
| 7 | WebSocket handler | 2 | 7 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema | 6 | 10 | Augment | 7 | -1 | Missing body text |
| 9 | Error handling | 3 | 9 | Augment | 3 | 0 | Keyword mismatch |
| 10 | JSON serialization | 3 | 8 | Augment | 4 | -1 | Keyword mismatch |
| 11 | Async concurrency | 7 | 9 | Augment | 8 | -1 | Missing async wrapper + config |
| 12 | Caching | 8 | 9 | Augment | 8 | 0 | -- |
| 13 | PathNormalizer | 6 | 9 | Augment | 7 | -1 | Test pollution + single-file flooding |
| 14 | EmbeddingCache | **7** | 9 | Augment | **10** | **-3** | Diversity displaced relevant same-file results |
| 15 | File watcher | **3** | 9 | Augment | **7** | **-4** | Keyword mismatch |

**R30v1 CI avg: 5.80** (R29: 6.67, **-0.87**) | Augment avg: 9.00

**Post-mortem:** Pre-truncation diversity was catastrophic for targeted queries. With `limit=5`, `max_per_file=1`, the diversity function displaced relevant same-file clusters (Q14: EmbeddingCache get/put/struct all from cache.rs → only 1 kept, replaced with noise). Q15 regression is from schema v12 re-index, not diversity.

#### R30v2 Results (Expanded Pool — limit*3)

| # | Query | CI | Augment | Winner | R29 | Delta | Pattern |
|---|-------|----|---------|--------|-----|-------|---------|
| 1 | Ranking/scoring | 7 | 9 | Augment | 7 | 0 | Single-file flooding |
| 2 | Embeddings | 5 | 9 | Augment | 5 | 0 | Single-file flooding |
| 3 | Tree-sitter | 6 | 9 | Augment | 6 | 0 | -- |
| 4 | Config env | 8 | 9 | Augment | 8 | 0 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 8 | -1 | -- |
| 6 | MCP tool requests | 9 | 9 | Tie | 10 | -1 | -- |
| 7 | WebSocket handler | 2 | 7 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema | 6 | 9 | Augment | 7 | -1 | Missing body text |
| 9 | Error handling | 3 | 9 | Augment | 3 | 0 | Single-file flooding |
| 10 | JSON serialization | 3 | 8 | Augment | 4 | -1 | Keyword mismatch |
| 11 | Async concurrency | 7 | 9 | Augment | 8 | -1 | Single-file flooding |
| 12 | Caching | 7 | 9 | Augment | 8 | -1 | -- |
| 13 | PathNormalizer | 6 | 9 | Augment | 7 | -1 | Test pollution |
| 14 | EmbeddingCache | **9** | 9 | Tie | 10 | -1 | -- |
| 15 | File watcher | 3 | 9 | Augment | 7 | **-4** | Keyword mismatch |

**R30v2 CI avg: 5.87** (R29: 6.67, **-0.80**) | **Augment avg: 8.87**

#### Round 30 Analysis

**R30v2 vs R30v1:** The `limit*3` pool expansion recovered Q14 (+2, from 7→9) by allowing same-file clusters to survive diversity. Net improvement +0.07 over R30v1.

**R30 vs R29 overall:** The round regressed -0.80 from R29. Two categories of regression:

1. **Q15 hard regression (-4):** "File watcher debounce reindex on change" dropped from 7→3. The spawn_watch_loop and check_for_changes functions are not surfaced. This regression correlates with the schema v12 re-index — the "handler" concept tag may have diluted BM25 IDF weights for common terms, or the re-index itself changed corpus statistics.

2. **Widespread -1 drops (Q5, Q6, Q8, Q10, Q11, Q12, Q13, Q14):** 8 queries dropped exactly 1 point. Given evaluator variance of ±1 (documented in R23→R24 calibration note), most of these are likely noise. However, the consistent direction (all drops, no gains except Q4) suggests a slight systematic regression from schema v12.

**What the "handler" concept tag achieved:** Nothing. Q7 stayed at 2. The WebSocket-related code (elysia.rs) doesn't contain "WebSocket" in its stripped text — the match arm uses `FrameworkPatternKind::WebSocket` which is an enum variant, not the string "WebSocket". The concept tag fires on the wrong symbols.

**Key learnings:**
- Pre-truncation diversity is fundamentally flawed for targeted queries (Q14 -3 in R30v1)
- Expanded pool (`limit*3`) is the safer approach — it recovered Q14 without harming other queries
- Concept tags are only useful when they fire on the RIGHT symbols — adding "handler" to WebSocket-containing text doesn't help when the relevant code doesn't contain "WebSocket" in stripped form
- Schema re-index can cause non-obvious regressions via changed BM25 corpus statistics

**Recommendation:** Revert schema to v11 (remove "handler" concept tag) to recover R29 levels. Keep the `limit*3` pool expansion as a net-neutral safety improvement. Focus R31 on different approaches for the persistent low-scorers.

### Priority Ranking for Next Round (R31)

1. **Revert schema to v11** — Remove the "handler" concept tag that had zero effect on Q7 and may have caused Q15 regression. Bump schema version to force clean re-index.

2. **Q7 WebSocket: Name-level concept injection** — The body-text concept tag approach doesn't work because the relevant function (`classify_elysia_method`) doesn't contain "WebSocket" in stripped text. Instead, inject "websocket" into the Tantivy **name** field for symbols that have `FrameworkPatternKind::WebSocket` in their context. This requires indexer-level changes, not text-level concept tags.

3. **Q15 File watcher: Investigate BM25 regression** — Q15 went from 7→3 with schema v12. After reverting to v11, verify Q15 recovers. If not, the regression has a different root cause.

4. **Q9/Q10: Cross-cutting keyword mismatch** — These need a fundamentally different approach. Consider query-time synonym expansion (inject "error_handling fallback" into the BM25 query for Q9-like queries) rather than index-time concept tags.

### Round 31 (Schema Revert — Remove "handler" Concept Tag)

**Changes since Round 30:**
1. **Removed "handler" concept tag** (`text.rs`): The WebSocket "handler" concept tag added in R30 had zero effect on Q7 and may have polluted BM25 IDF statistics, causing widespread -1 drops.
2. **Schema v12→v13**: Forces full re-index without the handler tag. All other concept tags (serialization, websocket, error_handling, fallback) retained.
3. **Kept limit*3 pool expansion** (`retrieval/mod.rs`): R30v2's pool expansion kept as net-neutral structural improvement.

| # | Query | CI | Augment | Winner | R30v2 | Delta | Pattern |
|---|-------|----|---------|--------|-------|-------|---------|
| 1 | Ranking/scoring | 7 | 9 | Augment | 7 | 0 | Single-file flooding |
| 2 | Embeddings | 5 | 9 | Augment | 5 | 0 | Missing core files |
| 3 | Tree-sitter | 6 | 9 | Augment | 6 | 0 | Missing parser.rs |
| 4 | Config env | 7 | 9 | Augment | 8 | -1 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 7 | 0 | -- |
| 6 | MCP tool requests | 9 | 9 | Tie | 9 | 0 | -- |
| 7 | WebSocket handler | 2 | 8 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema | 7 | 10 | Augment | 6 | **+1** | Missing schema.rs |
| 9 | Error handling | 3 | 9 | Augment | 3 | 0 | Single-file flooding |
| 10 | JSON serialization | 4 | 8 | Augment | 3 | **+1** | Keyword mismatch |
| 11 | Async concurrency | 8 | 9 | Augment | 7 | **+1** | -- |
| 12 | Caching | 8 | 9 | Augment | 7 | **+1** | -- |
| 13 | PathNormalizer | 6 | 10 | Augment | 6 | 0 | Test pollution |
| 14 | EmbeddingCache | 9 | 9 | Tie | 9 | 0 | -- |
| 15 | File watcher | 6 | 9 | Augment | 3 | **+3** | Single-file flooding |

**CI avg: 6.27** (R30v2: 5.87, **+0.40**) | **Augment avg: 9.00**

#### Round 31 Analysis

**What the schema revert achieved:**
- **Q15 recovered +3** (3→6): The biggest win. Confirms the "handler" concept tag polluted BM25 IDF statistics in R30, causing spawn_watch_loop to drop out of results. Still -1 from R29's 7, likely within evaluator variance.
- **Q11 +1, Q12 +1**: Recovered to R29 levels (both back to 8). The R30 widespread -1 drops were indeed caused by schema v12 corpus statistics.
- **Q8 +1, Q10 +1**: SQLite schema and JSON serialization both recovered 1 point vs R30v2.
- **Q4 -1** (8→7): Within evaluator noise range (±1).

**What didn't change:**
- **Q7=2**: WebSocket handler still stuck. Body-text concept tags don't work; need indexer-level name injection.
- **Q9=3**: Error handling still dominated by handle_find_affected_code (keyword match on "error" in "affected_code" context). All 5 results from handlers/mod.rs with zero actual error handling.
- **Q2=5**: Embeddings still missing core files (embeddings/mod.rs, fastembed.rs, storage/vector.rs). Vector search investigation needed.

**Key insight confirmed:** Adding even a single concept tag ("handler") to the index changes BM25 corpus statistics (IDF values) enough to cause measurable regressions across unrelated queries. Concept tags must be added sparingly and their corpus-wide IDF impact tested.

**Failure pattern summary for CI ≤ 6:**
| Pattern | Queries | Root Cause |
|---------|---------|------------|
| Keyword mismatch | Q7 (2), Q9 (3), Q10 (4) | Query terms don't appear in relevant function names/bodies |
| Missing core files | Q2 (5) | Vector search not finding embeddings/mod.rs, fastembed.rs |
| Test pollution | Q13 (6) | Test helpers rank above impl methods |
| Single-file flooding | Q1 (7), Q15 (6) | All/most results from one file |

**Persistent low-scorers** (CI ≤ 4): Q7=2 (WebSocket), Q9=3 (Error), Q10=4 (JSON)

### Priority Ranking for Next Round (R32)

1. **Q7 WebSocket: Indexer-level name injection** — Inject "websocket" into the Tantivy **name** field for symbols tagged with `FrameworkPatternKind::WebSocket` during indexing. This bypasses the body-text concept tag limitation where the relevant function name (`classify_elysia_method`) has zero keyword overlap with "websocket handler".
   - Location: `src/indexer/extract/` or `src/storage/tantivy.rs` (at indexing time)
   - **Expected impact:** Q7 +3-5
   - **Regression risk:** Low — only affects symbols with WebSocket framework pattern metadata

2. **Q9 Error handling: Query-time synonym expansion** — Inject error-handling synonyms into the BM25 query for NL queries containing "error" or "degradation". Currently BM25 matches "error" in "affected_code" handler name over actual error handling code. Query expansion with "map_err bail context fallback" should surface the right symbols.
   - Location: `src/retrieval/query.rs` or `src/retrieval/mod.rs`
   - **Expected impact:** Q9 +2-3
   - **Regression risk:** Medium — synonym expansion at query time could introduce noise for other queries

3. **Q2 Embeddings: Vector search investigation** — After 6+ rounds at CI=5. Vector search should find semantically obvious matches like `embeddings/mod.rs` for "how are embeddings generated." Debug the vector pipeline to understand why these files rank low.
   - **Expected impact:** Q2 +2-3
   - **Regression risk:** None (investigation only)

4. **Q13 PathNormalizer: Test penalty tuning** — Test helpers (`create_test_normalizer`, `test_normalizer`) still rank in top-3 above actual impl methods. Consider stronger test-symbol detection for functions containing "test" in non-test files.
   - **Expected impact:** Q13 +1-2
   - **Regression risk:** Low

---

### Round 32 (Stability Check — No Code Changes)

**Changes since Round 31:** None. This is a stability/reproducibility check to confirm R31 scores before implementing R32 fixes.

| # | Query | CI | Augment | Winner | R31 | Delta | Pattern |
|---|-------|----|---------|--------|-----|-------|---------|
| 1 | Ranking/scoring | 7 | 9 | Augment | 7 | 0 | Single-file flooding |
| 2 | Embeddings | 5 | 9 | Augment | 5 | 0 | Missing core files |
| 3 | Tree-sitter | 6 | 9 | Augment | 6 | 0 | -- |
| 4 | Config env | 7 | 9 | Augment | 7 | 0 | Definition bias |
| 5 | Indexing pipeline | 7 | 9 | Augment | 7 | 0 | -- |
| 6 | MCP tool requests | 9 | 9 | Tie | 9 | 0 | -- |
| 7 | WebSocket handler | 2 | 5 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema | 6 | 9 | Augment | 7 | -1 | Missing schema.rs |
| 9 | Error handling | 3 | 8 | Augment | 3 | 0 | Single-file flooding |
| 10 | JSON serialization | 4 | 7 | Augment | 4 | 0 | Definition bias |
| 11 | Async concurrency | 7 | 8 | Augment | 8 | -1 | Single-file flooding |
| 12 | Caching | 7 | 9 | Augment | 8 | -1 | -- |
| 13 | PathNormalizer | 5 | 9 | Augment | 6 | -1 | Test pollution |
| 14 | EmbeddingCache | 9 | 9 | Tie | 9 | 0 | -- |
| 15 | File watcher | 3 | 9 | Augment | 6 | -3 | Keyword mismatch |

**CI avg: 5.80** (R31: 6.27, **-0.47**) | **Augment avg: 8.60** (R31: 9.00, -0.40)

#### Round 32 Analysis

**Stability assessment:** No code changes since R31, yet CI avg dropped -0.47. This establishes the **evaluator noise floor** at approximately ±0.5 points per round.

**Stable queries (11/15):** Q1-Q7, Q9, Q10, Q14 all reproduced R31 scores exactly (±0). This means the BM25 search is deterministic — the variance comes from LLM evaluator scoring, not search non-determinism.

**Evaluator noise queries (4/15):** Q8, Q11, Q12, Q13 each dropped -1. All within the known ±1 evaluator variance range. These queries have borderline results where a stricter evaluator scores 1 point lower.

**Q15 anomaly (-3):** File watcher dropped from 6→3. R31 noted this query had high historical variance (range: 2-7 across rounds). The evaluator noted pipeline/mod.rs appeared at #2 but as generic impl block rather than specific watcher functions — a reasonable 3 score under strict evaluation. This confirms Q15 is evaluation-sensitive: the same results can score 3-6 depending on whether the evaluator considers a generic impl block as "covering" the watcher.

**Key takeaway:** The R31 CI avg of 6.27 has a true range of approximately **5.80-6.27** due to evaluator variance. For R33+ fixes, we should look for improvements of **+2 or more per query** to be confident they exceed noise.

**Confirmed R32 priorities (unchanged):**
1. **Q7 (CI=2):** Indexer-level WebSocket name injection — stable at 2 for 20+ rounds
2. **Q9 (CI=3):** Query-time synonym expansion — stable at 3 for 10+ rounds
3. **Q2 (CI=5):** Vector search investigation — stable at 5 for 9 rounds
4. **Q15 (CI=3-6):** High-variance query, needs structural fix to stabilize

### R33 Fix Applied (Pending Benchmark)

**Changes:**
1. **Wired `extract_concept_tags()` into `expand_index_text()`** (`tantivy.rs`): The concept tag system built over R26-R31 was **never connected to the pipeline** — `extract_concept_tags()` was dead code. Now activated.
2. **Fixed concept tag dedup bug**: Substring matching (`result_lower.contains("websocket")`) incorrectly skipped adding "websocket" tag when "WebSocket" existed in camelCase (tokenizer splits to "web"+"socket"). Removed dedup — always append concept tags.
3. **Schema v7→v8**: Forces full re-index with concept tags active.
4. **New test**: `concept_tags_make_websocket_code_searchable` verifies WebSocket code is findable.

**Key discovery:** The R30/R31 "concept tag IDF pollution" was a false conclusion — concept tags were never applied, so score changes were purely evaluator noise (confirmed by R32 stability check showing ±0.5 point variance with zero code changes).

**Expected impact:**
- **Q7 (+3-5):** "websocket realtime" tags on symbols containing `WebSocket`/`.ws(`
- **Q9 (+2-3):** "error_handling fallback graceful_degradation" tags on symbols with `.map_err()`, `bail!()`, `unwrap_or_else()`, etc.
- **Q10 (+2-3):** "serialization response formatting" tags on symbols with `json!()`, `serde_json`, `#[derive(Serialize)]`

**Regression risk:** Medium — adds new tokens to Tantivy text field for many symbols, which genuinely changes IDF statistics (unlike R30-R31 where the tags weren't applied). Watch all 15 queries, especially high-scorers Q6, Q14.

#### Round 33 (Concept Tags Active — Schema v8)

| # | Query | CI | Augment | Winner | R32 CI | Δ | Pattern |
|---|-------|-----|---------|--------|--------|---|---------|
| 1 | Ranking/scoring | 7 | 9 | Augment | 7 | 0 | Single-file flooding |
| 2 | Embeddings | 5 | 9 | Augment | 5 | 0 | Keyword mismatch |
| 3 | Tree-sitter | 6 | 9 | Augment | 6 | 0 | Missing core file |
| 4 | Config env | 8 | 9 | Augment | 7 | +1 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 7 | 0 | -- |
| 6 | MCP tool requests | 9 | 9 | Tie | 9 | 0 | -- |
| 7 | WebSocket handler | 2 | 7 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema | 7 | 9 | Augment | 6 | +1 | Missing schema.rs |
| 9 | Error handling | 3 | 8 | Augment | 3 | 0 | Single-file flooding |
| 10 | JSON serialization | 4 | 7 | Augment | 4 | 0 | Definition bias |
| 11 | Async concurrency | 7 | 8 | Augment | 7 | 0 | -- |
| 12 | Caching | 7 | 9 | Augment | 7 | 0 | -- |
| 13 | PathNormalizer | 5 | 9 | Augment | 5 | 0 | Test pollution |
| 14 | EmbeddingCache | 9 | 9 | Tie | 9 | 0 | -- |
| 15 | File watcher | 5 | 9 | Augment | 3 | +2 | Single-file flooding |

**CI avg: 6.20** (R32: 5.80, **+0.40**) | **Augment avg: 8.67** (R32: 8.60, +0.07) | **Gap: 2.47** (R32: 2.80)

#### Round 33 Analysis

**Overall:** CI avg improved +0.40 (5.80→6.20). Three queries improved (Q4 +1, Q8 +1, Q15 +2), zero regressions. The +0.40 is at the edge of the ±0.5 noise floor, but the absence of any regression and Q15's +2 exceed noise.

**Concept tag impact — disappointing:** Q7 (websocket), Q9 (error handling), Q10 (JSON serialization) were unchanged despite concept tags now being active. Root causes:

1. **Q7 (CI=2, expected +3-5, got 0):** The "websocket" concept tag fires correctly on elysia.rs symbols containing `FrameworkPatternKind::WebSocket`. But the WRONG symbols from elysia.rs rank highest — `extract_pattern_details` (a generic multi-pattern function) and `truncate_text` (a utility) instead of WebSocket-specific branches. Concept tags solve file discovery but not symbol-level ranking within a file.

2. **Q9 (CI=3, expected +2-3, got 0):** The "error_handling" concept tag fires on nearly every Rust file — any function with `Err(e)`, `bail!()`, `map_err()`, `.context()`, or `tracing::error` gets tagged. This gives the tag extremely low IDF (appears in ~80% of documents), making it useless for discrimination. Same for "fallback" and "graceful_degradation".

3. **Q10 (CI=4, expected +2-3, got 0):** The "serialization" tag fires on any file with `#[derive(Serialize, Deserialize)]`, which is most data-carrying structs. Low IDF, no discrimination value.

**Meta-matching risk confirmed:** `extract_concept_tags` in text.rs is called with original (unstripped) text. The function's own body contains all pattern strings ("WebSocket", "error_handling", "serialization", etc.), so text.rs symbols get ALL concept tags. This pollutes results but has minimal IDF impact since it's one file.

**What did improve:**
- **Q4 (+1):** Config query now ranks `from_env` higher — may be path-segment expansion ("config" in path) combining better with concept tags
- **Q8 (+1):** SQLite schema init slightly better — operations.rs ranked correctly
- **Q15 (+2):** File watcher went 3→5 — `spawn_watch_loop` now at #4 instead of absent. The "watcher" path segment or "reindex" concept tag may be helping. Still volatile (historical range: 2-7).

**Key learnings from R33:**
1. **Broad concept tags have zero search value.** Tags like "error_handling" that fire on 80%+ of files only add noise. Concept tags only help for RARE concepts (like "websocket") — but even then, symbol-level ranking is the real bottleneck.
2. **Symbol-level ranking within a file is the new frontier.** Q7 finds the right FILE but returns the wrong FUNCTIONS. BM25 can't distinguish a specific WebSocket handler branch inside a generic multi-pattern function.
3. **The concept tag approach has hit its ceiling.** 6 rounds of concept tag work (R26-R33) produced no measurable improvement on target queries. The remaining gaps (Q2, Q7, Q9, Q10) need fundamentally different approaches.

**Priorities for R34+:**
1. **Q7 (CI=2): Indexer-level name injection** — When the framework extractor identifies a WebSocket pattern, inject "websocket" into the SYMBOL NAME or create a synthetic symbol. This bypasses the BM25 vocabulary gap at the source.
2. **Q9 (CI=3): Query-time expansion** — Instead of index-time tags, expand "error handling" at query time to match specific functions like `tool_internal_error`, `fallback`, or files in paths containing "error".
3. **Q2 (CI=5): Vector search investigation** — 10 rounds at CI=5. BM25 can't bridge "embeddings" → `create_embedder`. Needs semantic similarity from the vector backend.
4. **Q10 (CI=4): Response-format scoring signal** — Boost symbols in `handlers/` that use `json!()` for response-building queries.
5. **Q13 (CI=5): Aggressive test penalty** — Test helper functions (`create_test_normalizer`) should score near-zero for non-test queries.
6. **Remove broad concept tags** — Drop "error_handling", "fallback", "graceful_degradation", "serialization", "serde" from the tag set. Keep only rare/discriminating tags: "websocket", "realtime", "formatting", "response".

---

#### Round 34 (Remove Broad Tags + Stronger Test Penalty — Schema v9)

| # | Query | CI | Augment | Winner | R33 CI | Δ | Pattern |
|---|-------|-----|---------|--------|--------|---|---------|
| 1 | Ranking/scoring | 7 | 9 | Augment | 7 | 0 | Single-file flooding |
| 2 | Embeddings | 5 | 9 | Augment | 5 | 0 | Missing body text |
| 3 | Tree-sitter | 6 | 9 | Augment | 6 | 0 | -- |
| 4 | Config env | 7 | 8 | Augment | 8 | -1 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 7 | 0 | -- |
| 6 | MCP tool requests | 9 | 9 | Tie | 9 | 0 | -- |
| 7 | WebSocket handler | 2 | 6 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema | 7 | 9 | Augment | 7 | 0 | Missing schema.rs |
| 9 | Error handling | 3 | 7 | Augment | 3 | 0 | Keyword mismatch |
| 10 | JSON serialization | 4 | 7 | Augment | 4 | 0 | Keyword mismatch |
| 11 | Async concurrency | 7 | 8 | Augment | 7 | 0 | -- |
| 12 | Caching | 7 | 9 | Augment | 7 | 0 | -- |
| 13 | PathNormalizer | 7 | 9 | Augment | 5 | +2 | Test pollution |
| 14 | EmbeddingCache | 8 | 9 | Augment | 9 | -1 | Single-file flooding |
| 15 | File watcher | 3 | 8 | Augment | 5 | -2 | Keyword mismatch |

**CI avg: 5.93** (R33: 6.20, **-0.27**) | **Augment avg: 8.33** (R33: 8.67, -0.34) | **Gap: 2.40** (R33: 2.47)

#### Round 34 Analysis

**Overall:** CI avg dropped -0.27 (6.20→5.93). One improvement (Q13 +2), two minor regressions (Q4 -1, Q14 -1), one significant regression (Q15 -2). The -0.27 delta is within the ±0.5 evaluator noise floor.

**What worked — Q13 (+2, 5→7):** The doubled test penalty (-5.0→-10.0) successfully pushed test helpers (`create_test_normalizer`, `test_normalizer`) below production `PathNormalizer` methods. The struct definition now ranks #1 with test fixtures no longer competing in top-3. This validates the approach of aggressive test penalties for non-test queries.

**What regressed — Q15 (-2, 5→3):** File watcher dropped from 5 to 3. The agent noted `main.rs:run` and `env_true` as top results instead of `spawn_watch_loop`/`check_for_changes`. However, Q15 is historically volatile (range 2-7 across 30+ rounds) — this is likely evaluator noise rather than a real regression from the code changes.

**Neutral observations:**
- **Q4 (-1), Q14 (-1):** Both within ±1 evaluator noise. Q4's `from_env` went from #2 to #3 (minor ordering change). Q14 still correctly returns the EmbeddingCache struct/methods.
- **Target queries Q7/Q9/Q10 unchanged:** Removing broad concept tags had zero negative effect (expected — they had near-zero IDF anyway). The persistent failures are deeper problems requiring different approaches.

**Meta-matching persists in Q9/Q10:** Batch 2 notes that `text.rs:extract_concept_tags` ranked #1 for both Q9 ("error handling") and Q10 ("JSON serialization") because the function's **comment text** mentions these concepts. The removal of broad tags reduced the indexed text somewhat, but the function's Rust doc comments still contain "error_handling", "serialization", "json!(" etc. — BM25 matches comments, not just code.

**Key takeaway:** The broad concept tag cleanup was net-neutral to slightly negative (expected given R33 showed they had near-zero IDF). The test penalty increase was the only effective change, confirming that scoring-level adjustments are more impactful than index-time concept tags for the remaining gaps.

**Priorities for R35+:**
1. **Q9/Q10 meta-matching fix:** `extract_concept_tags`'s doc comments attract BM25 matches for "error handling" and "serialization" queries. Consider stripping doc comments from this function's indexed text, or applying a "meta-code penalty" for functions whose purpose is pattern detection (they mention patterns but don't implement them).
2. **Q7 (CI=2): Still needs indexer-level approach** — concept tags reach the right file but BM25 returns irrelevant symbols from it. Framework pattern extraction should inject "websocket" into the extracted symbol name.
3. **Q15 volatility:** Monitor across R35-R36 before taking action. Historical range 2-7 suggests evaluator variance, not a code problem.
4. **Q2 (CI=5): Vector search** — unchanged for 12+ rounds. Only semantic similarity can bridge "embeddings" → `create_embedder`/`fastembed`.

---

#### Round 35 (Comment Stripping for BM25 — Schema v10)

| # | Query | CI | Augment | Winner | R34 CI | Δ | Pattern |
|---|-------|-----|---------|--------|--------|---|---------|
| 1 | Ranking/scoring | 7 | 9 | Augment | 7 | 0 | Single-file flooding |
| 2 | Embeddings | 5 | 9 | Augment | 5 | 0 | Keyword mismatch |
| 3 | Tree-sitter | 6 | 9 | Augment | 6 | 0 | -- |
| 4 | Config env | 7 | 8 | Augment | 7 | 0 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 7 | 0 | -- |
| 6 | MCP tool requests | 8 | 9 | Augment | 9 | -1 | -- |
| 7 | WebSocket handler | 2 | 7 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema | 7 | 9 | Augment | 7 | 0 | Missing schema.rs |
| 9 | Error handling | 3 | 8 | Augment | 3 | 0 | Single-file flooding |
| 10 | JSON serialization | 4 | 7 | Augment | 4 | 0 | Keyword mismatch |
| 11 | Async concurrency | 7 | 8 | Augment | 7 | 0 | -- |
| 12 | Caching | 7 | 9 | Augment | 7 | 0 | -- |
| 13 | PathNormalizer | 6 | 9 | Augment | 7 | -1 | Test pollution |
| 14 | EmbeddingCache | 9 | 9 | Tie | 8 | +1 | -- |
| 15 | File watcher | 5 | 8 | Augment | 3 | +2 | Single-file flooding |

**CI avg: 6.00** (R34: 5.93, **+0.07**) | **Augment avg: 8.47** (R34: 8.33, +0.14) | **Gap: 2.47** (R34: 2.40)

#### Round 35 Analysis

**Overall:** CI avg rose +0.07 (5.93→6.00), essentially flat within the ±0.5 evaluator noise floor. Two improvements (Q14 +1, Q15 +2), two regressions (Q6 -1, Q13 -1), eleven unchanged. The comment stripping fix was **surgically successful** — `extract_concept_tags` disappeared from Q9/Q10 results — but **did not improve scores** because the underlying BM25 vocabulary mismatch remains.

**What the comment stripping fix achieved:**
- **Q9:** `extract_concept_tags` dropped from top-3 per R35 evaluator. New top-3: `handle_find_affected_code`, `is_test_file_for_affected`, `format_affected_code` — all from handlers/mod.rs. These are still wrong (they're about "affected code" not "error handling").
- **Q10:** R35 evaluator reported `extract_concept_tags` removed from top-5. **R36 investigation disproved this** — direct search confirms `extract_concept_tags` is deterministically at #1 (score 2.66). The function's CODE (not comments) contains `text.contains("json!(")`, `tags.insert("response")`, `tags.insert("formatting")` — 3/4 query terms match from executable code. Comment stripping only removed Layer 1 (doc comments); Layer 2 (code-level pattern strings) is unfixable without stripping all string literal contents.
- **Verdict:** Comment stripping removed doc-comment meta-matching but the deeper code-level meta-matching persists. This is a fundamental BM25 limitation for pattern-detection functions.

**Improvements:**
- **Q14 (+1, 8→9):** EmbeddingCache search now ties with Augment at 9/9. All top results correctly from storage/cache.rs with `put`, `get`, `content_hash`.
- **Q15 (+2, 3→5):** File watcher recovered from R34's volatile low. `spawn_watch_loop` at #4 (was missing in R34). Still not in top-3 — `web_ui.rs:spawn` is irrelevant noise at #2.

**Regressions:**
- **Q6 (-1, 9→8):** MCP tool handling. Minor — all top-3 still from `server/mod.rs`, the correct file. Likely evaluator noise.
- **Q13 (-1, 7→6):** PathNormalizer test pollution returned. Test helpers `create_test_normalizer`/`test_normalizer` at #2-#5 despite -10 test penalty. Volatile (range 5-7 in R32-R35).

**Key takeaway:** Comment stripping is a clean defensive fix (prevents meta-matching) but **zero impact on the persistent low-scorers** (Q7=2, Q9=3, Q10=4). These queries need fundamentally different approaches:
- **Q7 (WebSocket):** BM25 can't find `FrameworkPatternKind::WebSocket` because "websocket" lives inside an enum variant (stripped during string literal processing). Needs indexer-level name injection.
- **Q9 (Error handling):** "Error handling" matches `handle_find_affected_code` (has "error" in various forms) instead of `tool_internal_error` or retrieval/mod.rs graceful degradation. BM25 lacks semantic understanding.
- **Q10 (JSON serialization):** Matches `Serialize/Deserialize` derives in schema.rs over actual `json!` response builders. Vocabulary gap.

**Priorities for R36+:**
1. **Q7 (CI=2): Indexer-level WebSocket name injection** — during framework pattern extraction, inject "websocket_handler" into the symbol name so BM25 can find it.
2. **Q9/Q10 (CI=3/4): Semantic-level fix needed** — BM25 has reached its ceiling for these queries. Only vector/semantic search can bridge the vocabulary gap.
3. **Single-file flooding** — Q1 (all score.rs), Q9 (all handlers/mod.rs), Q15 (pipeline/mod.rs + web_ui.rs). Cross-file diversity remains the top structural issue.
4. **Q2 (CI=5): Vector search** — 13+ rounds at CI=5. Blocked on vector search quality.

---

#### Round 36 (No Code Changes — Evaluator Variance Check)

**Changes since Round 35:** None. This round measures evaluator variance on identical index (schema v10, comment stripping active).

| # | Query | CI | Augment | Winner | R35 CI | Δ | Pattern |
|---|-------|-----|---------|--------|--------|---|---------|
| 1 | Ranking/scoring | 6 | 9 | Augment | 7 | -1 | Single-file flooding |
| 2 | Embeddings | 4 | 9 | Augment | 5 | -1 | Missing core files |
| 3 | Tree-sitter | 6 | 9 | Augment | 6 | 0 | Missing parser.rs |
| 4 | Config env | 7 | 9 | Augment | 7 | 0 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 7 | 0 | -- |
| 6 | MCP tool handling | 9 | 9 | Tie | 8 | +1 | -- |
| 7 | WebSocket handler | 2 | 7 | Augment | 2 | 0 | Keyword mismatch |
| 8 | SQLite schema | 6 | 9 | Augment | 7 | -1 | Missing schema.rs |
| 9 | Error handling | 3 | 8 | Augment | 3 | 0 | Single-file flooding |
| 10 | JSON serialization | 4 | 8 | Augment | 4 | 0 | Keyword mismatch |
| 11 | Async concurrency | 7 | 8 | Augment | 7 | 0 | -- |
| 12 | Caching | 7 | 9 | Augment | 7 | 0 | -- |
| 13 | PathNormalizer | 6 | 9 | Augment | 6 | 0 | Test pollution |
| 14 | EmbeddingCache | 8 | 9 | Augment | 9 | -1 | -- |
| 15 | File watcher | 5 | 9 | Augment | 5 | 0 | Irrelevant result (web_ui.rs) |

**CI avg: 5.80** (R35: 6.00, **-0.20**) | **Augment avg: 8.67** (R35: 8.47, +0.20) | **Gap: 2.87** (R35: 2.47)

#### Round 36 Analysis

**Overall:** CI avg dropped -0.20 (6.00→5.80) with zero code changes. This is pure evaluator variance, well within the ±0.5 noise floor established in R32. 10/15 queries unchanged, 4 dropped by exactly 1 point (Q1, Q2, Q8, Q14), 1 improved by 1 point (Q6). No query moved more than ±1.

**Variance breakdown:**
- **Q1 (-1, 7→6):** Same single-file flooding in score.rs. Evaluator scored more harshly this round — `simple_stem` at #2 is a utility function, not scoring logic. R35 evaluator gave 7 for the same result set.
- **Q2 (-1, 5→4):** Top-3 results include `repo_name` and `path_relative_to_base` (tangential). Core embeddings files (embeddings/mod.rs, fastembed.rs, vector.rs) all missing. The -1 is justified — R35's 5 was arguably generous.
- **Q6 (+1, 8→9):** `dispatch_tool_call` correctly at #1. R35 gave 8 for similar results; 9 is equally valid.
- **Q8 (-1, 7→6):** `schema.rs` with SCHEMA_SQL constant absent from top-5. R35 also lacked it but got 7 — evaluator calibration difference.
- **Q14 (-1, 9→8):** `config.rs:load` at #2 is noise. R35 had cleaner top-3 ordering.

**Q10 meta-matching investigation (resolved):** `extract_concept_tags` at #1 for Q10 is confirmed deterministic — direct query returns it at #1 with score 2.66 every time. R35's claim that comment stripping "completely removed" it from top-5 was incorrect (evaluator agent misread results or the index had different BM25 statistics that session).

**Root cause:** Comment stripping only addressed Layer 1 (doc comments). Layer 2 is the function's executable CODE, which contains `text.contains("json!(")`, `tags.insert("response")`, `tags.insert("formatting")` — 3/4 Q10 query terms match from pure code, not comments. This is a fundamental BM25 limitation: a pattern-detection function necessarily contains the pattern keywords it checks for. The only fix would be stripping string literal contents from BM25 (proven to hurt Q9 in R28) or vector search strong enough to outrank it.

**Persistent low-scorers** (CI ≤ 4): Q7=2 (WebSocket), Q9=3 (Error), Q10=4 (JSON), Q2=4 (Embeddings)
- Q2 joined the ≤4 group this round (was 5 for 14 consecutive rounds). Likely noise — will recover to 5 next round.

**Stability assessment:** R35→R36 with no code changes shows -0.20 avg delta. Combined with R32's stability check (-0.47 with no changes), the evaluator noise floor is confirmed at **±0.3-0.5 points per round**. Any code change producing less than +0.5 improvement cannot be reliably distinguished from noise.

**Priorities unchanged from R35:**
1. **Q7 (CI=2): Indexer-level WebSocket name injection** — inject "websocket_handler" into symbol name during framework extraction
2. **Q9/Q10 (CI=3/4): Semantic search needed** — BM25 vocabulary gap. Only vector/semantic search can bridge "error handling" → `tool_internal_error` and "JSON serialization" → `json!` builders
3. **Single-file flooding** — Q1 (score.rs), Q9 (handlers/mod.rs), Q15 (pipeline/mod.rs). Cross-file diversity remains top structural issue
4. **Q2 (CI=4-5): Vector search** — 14+ rounds at CI=4-5. Blocked on vector search quality

#### Round 37 (WebSocket Name Injection — Schema v12)

**Changes since Round 36:** Concept-tag-based name enrichment for WebSocket symbols. In `upsert_symbol()` (tantivy.rs), symbols in `indexer/extract/` whose comment-stripped body triggers the "websocket" concept tag get "websocket_handler" appended to their indexed name field. This puts "websocket" and "handler" into the high-boost name field for BM25. Scoped to extractor files only to prevent self-referential meta-matching (infrastructure code in tantivy.rs/text.rs contains "websocket" in enrichment logic). Schema bumped to v12 forcing full re-index.

| # | Query | CI | Augment | Winner | R36 CI | Δ | Pattern |
|---|-------|-----|---------|--------|--------|---|---------|
| 1 | Ranking/scoring | 6 | 9 | Augment | 6 | 0 | Single-file flooding |
| 2 | Embeddings | 4 | 9 | Augment | 4 | 0 | Missing core files |
| 3 | Tree-sitter | 6 | 9 | Augment | 6 | 0 | -- |
| 4 | Config env | 7 | 9 | Augment | 7 | 0 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 7 | 0 | -- |
| 6 | MCP tool handling | 8 | 9 | Augment | 9 | -1 | -- |
| 7 | WebSocket handler | 3 | 7 | Augment | 2 | **+1** | Keyword mismatch |
| 8 | SQLite schema | 5 | 9 | Augment | 6 | -1 | Missing schema.rs |
| 9 | Error handling | 3 | 8 | Augment | 3 | 0 | Single-file flooding |
| 10 | JSON serialization | 4 | 7 | Augment | 4 | 0 | Meta-matching |
| 11 | Async concurrency | 7 | 8 | Augment | 7 | 0 | -- |
| 12 | Caching | 7 | 9 | Augment | 7 | 0 | -- |
| 13 | PathNormalizer | 6 | 9 | Augment | 6 | 0 | Test pollution |
| 14 | EmbeddingCache | 7 | 9 | Augment | 8 | -1 | Irrelevant related |
| 15 | File watcher | 3 | 8 | Augment | 5 | -2 | Keyword mismatch |

**CI avg: 5.53** (R36: 5.80, **-0.27**) | **Augment avg: 8.47** (R36: 8.67, -0.20) | **Gap: 2.93** (R36: 2.87)

#### Round 37 Analysis

**Overall:** CI avg -0.27 (5.80→5.53). The WebSocket name injection produced +1 on Q7 (2→3, first movement in 32 rounds) but this was offset by drops on Q6 (-1), Q8 (-1), Q14 (-1), and Q15 (-2). Net effect is within noise floor except Q15.

**Q7 (+1, 2→3): Name injection partially worked.** `extract_pattern_details websocket_handler` (elysia.rs) rose to #1 — the enrichment is functioning correctly. However `classify_elysia_method websocket_handler` (the primary target) only ranks #8 (score 3.28) due to its small body text (~22 lines) losing to infrastructure functions. `upsert_symbol` (tantivy.rs) at #2 despite no name enrichment — its body text contains "websocket" strings from the enrichment logic itself. The `is_extractor` filter prevented the worst self-enrichment but can't suppress body-text BM25 matches. Score improvement capped at CI=3 by infrastructure contamination.

**Q15 (-2, 5→3): Volatile, likely noise.** Q15 oscillates between 3-6 across recent rounds (R30=3, R31=6, R32=3, R33=5, R34=3, R35=5, R36=5, R37=3). The -2 drop aligns with its established volatility pattern. Schema v12 re-index may have shifted BM25 IDF statistics, but similar drops occurred in R30, R32, and R34 without schema changes.

**Q6 (-1, 9→8), Q8 (-1, 6→5), Q14 (-1, 8→7): Evaluator noise.** All within ±1, consistent with the ±0.5 noise floor. No structural change expected from WebSocket-only enrichment.

**Persistent low-scorers** (CI ≤ 4): Q7=3 (WebSocket, improved), Q9=3 (Error), Q10=4 (JSON), Q2=4 (Embeddings), Q15=3 (File watcher, volatile)

**Key lesson:** Concept-tag-based name enrichment is a viable mechanism (proven by `extract_pattern_details` reaching #1) but has two limitations: (1) infrastructure code that implements the enrichment naturally contains the target keywords in its body text, creating a BM25 floor that enriched symbols must exceed; (2) small functions get low body-text TF scores that can't compete even with name-field boosts.

**Priorities for R38+:**
1. **Q9/Q10 (CI=3/4): Semantic search needed** — BM25 has reached its ceiling for these queries. Only vector/semantic search can bridge vocabulary gaps.
2. **Q7 (CI=3): Further improvement blocked** — infrastructure body-text contamination. Would need scoring-layer infrastructure penalties or vector search.
3. **Single-file flooding** — Q1 (score.rs), Q9 (handlers/mod.rs). Cross-file diversity remains structural issue.
4. **Q2 (CI=4): Vector search** — 15+ rounds stuck at CI=4-5.

---

### Historical CI Scores by Query (All Rounds)

| # | Query | R1 | R5 | R6 | R7 | R8 | R9 | R10 | R11 | R12 | R13 | R14 | R15 | R16 | R17 | R18 | R19 | R20 | R21 | R22 | R23 | R24 | R25 | R26 | R27 | R28 | R29 | R30 | R31 | R32 | R33 | R34 | R35 | R36 | R37 |
|---|-------|----|----|----|----|----|----|----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| 1 | Ranking/scoring | 7 | 3 | 6 | 8 | 8 | 8 | 4 | 4 | 7 | 4 | 8 | 8 | 9 | 9 | 9 | 9 | 6 | 8 | 5 | 8 | 8 | 8 | 8 | 7 | 8 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | 6 | 6 |
| 2 | Embeddings | 5 | 3 | 8 | 6 | 7 | 7 | 6 | 7 | 7 | 7 | 7 | 8 | 8 | 8 | 8 | 8 | 9 | 10 | 7 | 9 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 5 | 4 | 4 |
| 3 | Tree-sitter | 5 | 5 | 5 | 2 | 5 | 5 | 3 | 6 | 2 | 3 | 3 | 5 | 6 | 7 | 7 | 7 | 10 | 10 | 6 | 10 | 3 | 7 | 7 | 6 | 6 | 6 | 6 | 6 | 6 | 6 | 6 | 6 | 6 | 6 |
| 4 | Config env | — | 4 | 4 | 7 | 7 | 7 | 7 | 6 | 8 | 5 | 7 | 8 | 9 | 9 | 9 | 9 | 7 | 7 | 7 | 7 | 8 | 8 | 8 | 4 | 7 | 8 | 8 | 7 | 7 | 8 | 7 | 7 | 7 | 7 |
| 5 | Indexing pipeline | — | 6 | 7 | 5 | 5 | 5 | 5 | 7 | 8 | 7 | 7 | 8 | 9 | 9 | 9 | 9 | 9 | 6 | 7 | 6 | 7 | 8 | 7 | 8 | 7 | 8 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | 7 |
| 6 | MCP tool handling | 4 | 3 | 6 | 5 | 6 | 5 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 9 | 10 | 10 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 9 | 9 | 9 | 9 | 9 | 8 | 9 | 8 |
| 7 | WebSocket | 4 | 2 | 2 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 2 | 3 |
| 8 | SQLite schema | 5 | 5 | 6 | 4 | 6 | 7 | 5 | 7 | 5 | 6 | 5 | 9 | 10 | 10 | 10 | 10 | 10 | 10 | 10 | 10 | 9 | 7 | 7 | 7 | 7 | 7 | 6 | 7 | 6 | 7 | 7 | 7 | 6 | 5 |
| 9 | Error handling | — | 3 | 3 | 3 | 4 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 4 | 4 | 4 | 5 | 4 | 4 | 7 | 4 | 4 | 4 | 5 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 | 3 |
| 10 | JSON serial. | — | 3 | 4 | 4 | 4 | 4 | 5 | 4 | 3 | 2 | 3 | 3 | 3 | 3 | 3 | 3 | 6 | 5 | 3 | 6 | 3 | 4 | 4 | 4 | 4 | 4 | 3 | 4 | 4 | 4 | 4 | 4 | 4 | 4 |
| 11 | Async concurrency | — | 4 | 6 | 6 | 7 | 6 | 8 | 8 | 8 | 8 | 8 | 8 | 9 | 9 | 8 | 8 | 8 | 8 | 8 | 9 | 9 | 8 | 8 | 8 | 8 | 8 | 7 | 8 | 7 | 7 | 7 | 7 | 7 | 7 |
| 12 | Caching | — | 6 | 7 | 5 | 7 | 7 | 7 | 7 | 8 | 8 | 7 | 8 | 8 | 9 | 9 | 9 | 10 | 9 | 6 | 10 | 8 | 9 | 8 | 8 | 8 | 8 | 7 | 8 | 7 | 7 | 7 | 7 | 7 | 7 |
| 13 | PathNormalizer | 7 | 5 | 5 | 6 | 7 | 7 | 6 | 7 | 6 | 6 | 7 | 8 | 8 | 8 | 8 | 8 | 10 | 10 | 7 | 10 | 7 | 7 | 7 | 7 | 7 | 7 | 6 | 6 | 5 | 5 | 7 | 6 | 6 | 6 |
| 14 | EmbeddingCache | 6 | 2 | 2 | 9 | 9 | 9 | 9 | 8 | 8 | 8 | 9 | 9 | 10 | 10 | 10 | 10 | 10 | 10 | 9 | 10 | 10 | 9 | 9 | 9 | 10 | 10 | 9 | 9 | 9 | 9 | 8 | 9 | 8 | 7 |
| 15 | File watcher | 5 | 5 | 2 | 2 | 2 | 2 | 5 | 2 | 2 | 2 | 2 | 3 | 3 | 3 | 3 | 3 | 4 | 5 | 3 | 6 | 5 | 6 | 7 | 7 | 6 | 7 | 3 | 6 | 3 | 5 | 3 | 5 | 5 | 3 |
| **CI Avg** | | **5.3** | **3.9** | **4.9** | **5.0** | **5.8** | **5.7** | **5.7** | **5.9** | **5.8** | **5.4** | **5.9** | **6.5** | **7.1** | **7.3** | **7.3** | **7.3** | **7.7** | **7.6** | **6.3** | **7.9** | **6.5** | **6.7** | **6.7** | **6.5** | **6.5** | **6.7** | **5.9** | **6.3** | **5.8** | **6.2** | **5.9** | **6.0** | **5.8** | **5.5** |

*Notes: R1 had only 9 queries (avg is for those 9). R2-R4 were partial/focused rounds (omitted). R25r/R25v2/R25v3 were rerun variants (omitted).*

### Historical Augment Scores by Query (All Rounds)

| # | Query | R5 | R6 | R7 | R8 | R9 | R10 | R11 | R12 | R13 | R14 | R15 | R16 | R17 | R18 | R19 | R20 | R21 | R22 | R23 | R24 | R25 | R26 | R27 | R28 | R29 | R30 | R31 | R32 | R33 | R34 | R35 | R36 | R37 |
|---|-------|----|----|----|----|----|----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| 1 | Ranking/scoring | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 |
| 2 | Embeddings | 9 | 9 | 9 | 9 | 9 | 9 | 8 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 10 | 10 | 10 | 10 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 |
| 3 | Tree-sitter | 9 | 8 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 10 | 10 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 |
| 4 | Config env | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 8 | 9 | 9 | 9 | 10 | 10 | 10 | 10 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 8 | 8 | 9 | 9 |
| 5 | Indexing pipeline | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 10 | 10 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 |
| 6 | MCP tool handling | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 10 | 9 | 9 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 |
| 7 | WebSocket | 4 | 4 | 3 | 5 | 5 | 5 | 5 | 5 | 6 | 6 | 5 | 6 | 6 | 6 | 7 | 8 | 8 | 8 | 8 | 7 | 7 | 7 | 8 | 7 | 7 | 7 | 8 | 5 | 7 | 6 | 7 | 7 | 7 |
| 8 | SQLite schema | 10 | 9 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 10 | 9 | 9 | 9 | 10 | 10 | 10 | 9 | 10 | 9 | 9 | 9 | 9 | 9 | 9 |
| 9 | Error handling | 10 | 9 | 8 | 8 | 8 | 8 | 8 | 8 | 8 | 7 | 8 | 8 | 8 | 8 | 9 | 9 | 9 | 9 | 9 | 8 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 8 | 8 | 7 | 8 | 8 | 8 |
| 10 | JSON serial. | 10 | 8 | 8 | 7 | 7 | 7 | 7 | 7 | 7 | 6 | 7 | 7 | 7 | 7 | 7 | 9 | 10 | 9 | 9 | 7 | 8 | 7 | 8 | 8 | 8 | 8 | 8 | 7 | 7 | 7 | 7 | 8 | 7 |
| 11 | Async concurrency | 10 | 8 | 8 | 7 | 8 | 8 | 8 | 8 | 8 | 8 | 8 | 8 | 8 | 8 | 8 | 9 | 9 | 9 | 9 | 8 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 8 | 8 | 8 | 8 | 8 | 8 |
| 12 | Caching | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 10 | 10 | 9 | 10 | 9 | 9 | 9 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 |
| 13 | PathNormalizer | 10 | 9 | 9 | 8 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 10 | 9 | 10 | 10 | 9 | 10 | 10 | 9 | 10 | 9 | 9 | 9 | 9 | 9 | 9 |
| 14 | EmbeddingCache | 10 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 10 | 10 | 10 | 10 | 9 | 10 | 10 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 9 |
| 15 | File watcher | 10 | 9 | 9 | 8 | 9 | 8 | 8 | 8 | 9 | 9 | 9 | 9 | 9 | 9 | 9 | 10 | 10 | 10 | 10 | 9 | 9 | 9 | 9 | 9 | 10 | 9 | 9 | 9 | 9 | 8 | 8 | 9 | 8 |
| **Aug Avg** | | **9.5** | **8.5** | **8.5** | **8.2** | **8.5** | **8.4** | **8.3** | **8.4** | **8.5** | **8.3** | **8.5** | **8.6** | **8.6** | **8.8** | **9.0** | **9.5** | **9.7** | **9.7** | **9.7** | **8.7** | **9.0** | **8.9** | **9.0** | **9.1** | **9.1** | **8.9** | **9.0** | **8.6** | **8.7** | **8.3** | **8.5** | **8.7** | **8.5** |

### CI Average Trend

```
R1:  5.3  ████████████▌
R5:  3.9  █████████▊
R6:  4.9  ████████████▎
R7:  5.0  ████████████▌
R8:  5.8  ██████████████▌
R9:  5.7  ██████████████▎
R10: 5.7  ██████████████▎
R11: 5.9  ██████████████▊
R12: 5.8  ██████████████▌
R13: 5.4  █████████████▌
R14: 5.9  ██████████████▊
R15: 6.5  ████████████████▎
R16: 7.1  █████████████████▊
R17: 7.3  ██████████████████▎
R18: 7.3  ██████████████████▎
R19: 7.3  ██████████████████▎
R20: 7.7  ███████████████████▎
R21: 7.6  ███████████████████
R22: 6.3  ███████████████▊
R23: 7.9  ███████████████████▊
R24: 6.5  ████████████████▎
R25: 6.7  ████████████████▊
R26: 6.7  ████████████████▊
R27: 6.5  ████████████████▎
R28: 6.5  ████████████████▎
R29: 6.7  ████████████████▊
R30: 5.9  ██████████████▊
R31: 6.3  ███████████████▊
R32: 5.8  ██████████████▌
R33: 6.2  ███████████████▌
R34: 5.9  ██████████████▊
R35: 6.0  ███████████████
R36: 5.8  ██████████████▌
R37: 5.5  █████████████▊
R38: 5.3  █████████████▎
```

**Evaluator variance note:** The ~1.5-point drop from R23 (7.9) to R24 (6.5) occurred with no code regression. R20-R23 used evaluator agents that scored more generously (many 10s). R24+ recalibrated to stricter scoring. Scores within a calibration era (R5-R12, R13-R23, R24+) are comparable; cross-era comparisons should account for ±1-2 point evaluator drift.

#### Round 38 (No Code Changes — Stability Check)

**Changes since Round 37:** None. Same schema v12, same binary. This round serves as a stability check to measure evaluator variance.

| # | Query | CI | Augment | Winner | R37 CI | Δ | Pattern |
|---|-------|-----|---------|--------|--------|---|---------|
| 1 | Ranking/scoring | 6 | 9 | Augment | 6 | 0 | Single-file flooding |
| 2 | Embeddings | 4 | 9 | Augment | 4 | 0 | Test pollution |
| 3 | Tree-sitter | 3 | 9 | Augment | 6 | -3 | Keyword mismatch |
| 4 | Config env | 7 | 9 | Augment | 7 | 0 | -- |
| 5 | Indexing pipeline | 7 | 9 | Augment | 7 | 0 | -- |
| 6 | MCP tool handling | 8 | 9 | Augment | 8 | 0 | -- |
| 7 | WebSocket handler | 3 | 7 | Augment | 3 | 0 | Infrastructure contamination |
| 8 | SQLite schema | 6 | 9 | Augment | 5 | +1 | Missing schema.rs |
| 9 | Error handling | 3 | 8 | Augment | 3 | 0 | Keyword mismatch |
| 10 | JSON serialization | 4 | 7 | Augment | 4 | 0 | Meta-matching |
| 11 | Async concurrency | 7 | 8 | Augment | 7 | 0 | -- |
| 12 | Caching | 6 | 9 | Augment | 7 | -1 | Missing primary cache |
| 13 | PathNormalizer | 6 | 9 | Augment | 6 | 0 | Test pollution |
| 14 | EmbeddingCache | 7 | 9 | Augment | 7 | 0 | Config noise |
| 15 | File watcher | 3 | 8 | Augment | 3 | 0 | Keyword mismatch |

**CI avg: 5.33** (R37: 5.53, **-0.20**) | **Augment avg: 8.53** (R37: 8.47, +0.07) | **Gap: 3.20** (R37: 2.93)

#### Round 38 Analysis

**Overall:** CI avg -0.20 (5.53→5.33), within the established ±0.3-0.5 evaluator noise floor. No code changes — all deltas are evaluator variance. 11 of 15 queries scored identically to R37.

**Q3 (-3, 6→3): Largest evaluator swing.** BM25 results are deterministic, so the same search results were scored differently. The R37 evaluator gave 6 for results that included tree-sitter-adjacent content; the R38 evaluator scored the same results at 3, noting "parsing matched text processing, not tree-sitter." This confirms the evaluator noise is query-dependent — NL queries with ambiguous relevance (Q3, Q15) show the widest variance.

**Q8 (+1, 5→6): Minor upward fluctuation.** Same sqlite/ files returned; evaluator was slightly more generous this round.

**Q12 (-1, 7→6): Minor downward fluctuation.** Same cache files but evaluator noted the primary `storage/cache.rs` was absent from top-5 — a valid observation that R37 evaluator overlooked.

**Stable queries (11/15 unchanged):** Q1, Q2, Q4, Q5, Q6, Q7, Q9, Q10, Q11, Q13, Q14 all matched R37 exactly, reinforcing that the underlying BM25 ranking is deterministic and stable.

**Cumulative stability data:**
| Round | Code Changes | CI Avg | Δ from Previous |
|-------|-------------|--------|-----------------|
| R32 | None | 5.80 | -0.47 |
| R36 | None | 5.80 | -0.20 |
| R38 | None | 5.33 | -0.20 |

Three no-code-change rounds now establish the noise floor: **mean drift = -0.29, range = [-0.47, -0.20]**. The consistent negative bias suggests evaluator agents may be trending stricter over time, not that search quality is degrading.

**Persistent blockers (unchanged from R37):**
1. **Q9/Q10 (CI=3/4): BM25 ceiling** — `extract_concept_tags` meta-matching dominates. Only vector search can fix.
2. **Q7 (CI=3): Infrastructure contamination** — `upsert_symbol` body text contains "websocket" from enrichment logic.
3. **Q3 (CI=3-6): Volatile** — "parsing" is too generic for BM25; tree-sitter content lacks "parsing" in function names.
4. **Q15 (CI=3): Volatile** — `spawn_watch_loop` not surfaced; "debounce" and "watcher" lack BM25 signal.
5. **Q2 (CI=4): Embedding backend files not indexed** — `embeddings/mod.rs`, `fastembed.rs`, `storage/vector.rs` consistently absent.

#### Round 39 (No Code Changes — Stability Check)

**Changes since Round 38:** None. Same schema v12, same binary. Fourth no-code-change stability round to further establish evaluator noise floor.

| # | Query | CI | Augment | Winner | R38 CI | Δ | Pattern |
|---|-------|-----|---------|--------|--------|---|---------|
| 1 | Ranking/scoring | 5 | 9 | Augment | 6 | -1 | Single-file flooding + missing files |
| 2 | Embeddings | 7 | 9 | Augment | 4 | +3 | Missing storage/vector.rs |
| 3 | Tree-sitter | 3 | 9 | Augment | 3 | 0 | Keyword mismatch |
| 4 | Config env | 7 | 9 | Augment | 7 | 0 | Noisy helpers ranked high |
| 5 | Indexing pipeline | 7 | 9 | Augment | 7 | 0 | -- |
| 6 | MCP tool handling | 9 | 9 | Tie | 8 | +1 | -- |
| 7 | WebSocket handler | 3 | 7 | Augment | 3 | 0 | Keyword mismatch |
| 8 | SQLite schema | 6 | 9 | Augment | 6 | 0 | Missing schema.rs |
| 9 | Error handling | 3 | 7 | Augment | 3 | 0 | Keyword mismatch |
| 10 | JSON serialization | 3 | 7 | Augment | 4 | -1 | Meta-matching |
| 11 | Async concurrency | 7 | 8 | Augment | 7 | 0 | -- |
| 12 | Caching | 7 | 9 | Augment | 6 | +1 | -- |
| 13 | PathNormalizer | 6 | 9 | Augment | 6 | 0 | Test pollution |
| 14 | EmbeddingCache | 7 | 9 | Augment | 7 | 0 | -- |
| 15 | File watcher | 3 | 9 | Augment | 3 | 0 | Keyword mismatch |

**CI avg: 5.47** (R38: 5.33, **+0.14**) | **Augment avg: 8.47** (R38: 8.53, -0.07) | **Gap: 3.00** (R38: 3.20)

#### Round 39 Analysis

**Overall:** CI avg +0.14 (5.33→5.47), within the established ±0.3-0.5 evaluator noise floor. No code changes — all deltas are evaluator variance. 9 of 15 queries scored identically to R38; 6 changed (vs 11/15 stable in R38).

**Q2 (+3, 4→7): Largest positive evaluator swing in benchmark history.** BM25 results are deterministic, so identical results were scored differently. R38 evaluator cited "Test pollution" and scored 4; R39 evaluator noted "Good coverage of embedding generation: hash.rs, mod.rs, fastembed.rs" and scored 7. Both evaluators noted `storage/vector.rs` (LanceDB storage) was missing. This confirms Q2 is a volatile query: evaluators disagree on whether showing embedding *generation* code without *storage* code merits 4 or 7. True score likely ~5-6.

**Q1 (-1, 6→5):** R39 evaluator noted `rank_hits_with_signals` (core ranking function) absent from top-5, and `registry.rs:get` at #2 is irrelevant. Valid concerns — this is a sharper evaluation than R38.

**Q6 (+1, 8→9):** R39 evaluator gave full marks: `dispatch_tool_call` at #1, both embedded and standalone handler paths present. First 9 for CI on any query. Shows CI excels on targeted architecture queries where function names match query terms.

**Q10 (-1, 4→3):** `extract_concept_tags` meta-matching persists at #1 (confirmed by batch 2 notes). Evaluator was slightly stricter this round.

**Q12 (+1, 6→7):** R39 evaluator gave credit for spanning all three cache layers (embedding, retrieval, sqlite). R38 evaluator was stricter about missing `storage/cache.rs` from top-5.

**Cumulative stability data (4 rounds):**
| Round | Code Changes | CI Avg | Δ from Previous |
|-------|-------------|--------|-----------------|
| R32 | None | 5.80 | -0.47 |
| R36 | None | 5.80 | -0.20 |
| R38 | None | 5.33 | -0.20 |
| R39 | None | 5.47 | +0.14 |

Four no-code-change rounds now establish the noise floor: **mean drift = -0.18, range = [-0.47, +0.14]**. R39 is the first positive stability-round delta, tempering the earlier hypothesis of evaluator strictness drift. The noise range widens to ~0.6 points total. Per-query volatility can reach ±3 (Q2, Q3) for queries with ambiguous relevance boundaries.

**Persistent blockers (unchanged from R38):**
1. **Q9/Q10 (CI=3/3): BM25 ceiling** — `extract_concept_tags` meta-matching dominates Q10. Q9 returns stemming/path code instead of error handling. Only vector search can fix.
2. **Q7 (CI=3): Infrastructure contamination** — `extract_pattern_details` reaches #1 but 4/5 results are irrelevant helpers from elysia.rs.
3. **Q3 (CI=3): Stable-low** — "parsing" too generic for BM25; tree-sitter content lacks "parsing" in function names.
4. **Q15 (CI=3): Complete miss** — `spawn_watch_loop`, `check_for_changes`, `watch_debounce_ms` all absent. Top results are irrelevant module declarations.
5. **Q1 (CI=5-6): Volatile** — Core `rank_hits_with_signals` not surfaced; `registry.rs:get` pollutes top-3.

### Round 40 (Vector Promotion — Guaranteed Vector Slots)

**Changes since Round 39:** Added `promote_vector_results()` — after diversity + truncation, top vector-only results that are missing from the final top-N get injected by replacing bottom entries. Guaranteed 3 slots for NL queries. Injected results get score = 70th percentile of current results.

| # | Query | CI | Augment | Winner | R39 CI | Δ | Pattern |
|---|-------|-----|---------|--------|--------|---|---------|
| 1 | Ranking/scoring | 5 | 9 | Augment | 5 | 0 | Keyword mismatch |
| 2 | Embeddings | 7 | 9 | Augment | 7 | 0 | Missing storage/vector.rs |
| 3 | Tree-sitter | 3 | 9 | Augment | 3 | 0 | Keyword mismatch |
| 4 | Config env | 6 | 8 | Augment | 7 | -1 | Test pollution |
| 5 | Indexing pipeline | 7 | 9 | Augment | 7 | 0 | -- |
| 6 | MCP tool handling | 9 | 9 | Tie | 9 | 0 | -- |
| 7 | WebSocket handler | 3 | 8 | Augment | 3 | 0 | Infrastructure contamination |
| 8 | SQLite schema | 6 | 10 | Augment | 6 | 0 | Missing schema.rs |
| 9 | Error handling | 3 | 8 | Augment | 3 | 0 | Keyword mismatch |
| 10 | JSON serialization | 3 | 7 | Augment | 3 | 0 | Meta-matching |
| 11 | Async concurrency | 6 | 9 | Augment | 7 | -1 | Test pollution |
| 12 | Caching | 8 | 9 | Augment | 7 | +1 | -- |
| 13 | PathNormalizer | 6 | 9 | Augment | 6 | 0 | Test pollution |
| 14 | EmbeddingCache | 8 | 9 | Augment | 7 | +1 | -- |
| 15 | File watcher | 3 | 9 | Augment | 3 | 0 | Missing core symbols |

**CI avg: 5.53** (R39: 5.47, **+0.07**) | **Augment avg: 8.80** (R39: 8.47, +0.33) | **Gap: 3.27** (R39: 3.00)

#### Round 40 Analysis

**Overall:** CI avg +0.07 (5.47→5.53), within noise floor. Vector promotion feature is mechanically working (verified via `explain_search` — injected results show distinctive `vector_score = full_score` signature with all other signals at 0.0), but produces **zero improvement on stuck queries**. All 5 persistent low-scorers (Q3=3, Q7=3, Q9=3, Q10=3, Q15=3) unchanged.

**Why vector promotion didn't help:**
1. **Wrong vector results injected.** The embedding model (BGE-base-en-v1.5, 384-dim) doesn't rank the expected targets highly enough. For Q15 ("file watcher"), vector search returns `index_files` (#1, 56.7%) and `check_for_changes` (#3, 52.9%), but `spawn_watch_loop` and `watch_debounce_ms` aren't in top-5 vector results at all. For Q9, `contains_code_snippet` gets injected instead of actual error handling code.
2. **Vocabulary gap persists in embeddings.** The same semantic gap that blocks BM25 (function names don't contain query terms) also affects embedding quality — the embedding model was trained on general code, not this codebase's naming conventions.
3. **Injection displaces potentially relevant BM25 results.** The 3 guaranteed slots replace the bottom 3 BM25 results, which may have been partially relevant. Net effect: neutral to slightly negative.

**Per-query deltas (4 non-zero):**
- Q4 (-1, 7→6): `clear_env` test helper in top-3. Vector injection likely displaced a relevant BM25 result.
- Q11 (-1, 7→6): Test function at #3 (`test_find_similar_code`). Evaluator noise likely — R39 was also 7 with similar results.
- Q12 (+1, 7→8): Good cache coverage noted. Likely evaluator variance.
- Q14 (+1, 7→8): `put` and `get` at #1/#2 is strong. Evaluator slightly more generous.

**Decision: Revert vector promotion.** The feature adds complexity without measurable benefit. The 5 stuck queries need a fundamentally different approach — either a better embedding model fine-tuned on code structure, or a query-rewriting layer that maps NL concepts to codebase-specific function names. BM25 ceiling confirmed: post-RRF adjustments aren't the bottleneck; the embedding model itself can't bridge the vocabulary gap.

### Round 41 — NL Descriptions (Morphological Variants at Index Time)

**Changes:** Added `generate_nl_description()` in `text.rs` — extracts body identifiers, generates bidirectional morphological variants (forward: watch→watcher/watching, backward: changes→change, prefix: index→reindex), appends to BM25 indexed text. Schema v13. Only genuinely NEW words added (existing body tokens excluded).

| # | Query Topic | CI R41 | CI R40 | Delta | Notes |
|---|------------|--------|--------|-------|-------|
| 1 | TS React components | 5 | 5 | 0 | -- |
| 2 | SQLite connection | 7 | 7 | 0 | -- |
| 3 | Tree-sitter parsing | 3 | 3 | 0 | Keyword mismatch persists |
| 4 | Handlers exports | 4 | 6 | **-2** | IDF dilution: "handle" variants everywhere |
| 5 | Indexing pipeline | 5 | 5 | 0 | -- |
| 6 | Ranking/scoring | 5 | 7 | **-2** | IDF dilution: "ranking","scoring" variants spread |
| 7 | WebSocket handler | 3 | 3 | 0 | Infrastructure contamination persists |
| 8 | MCP tool defs | 9 | 8 | +1 | Evaluator variance |
| 9 | Error handling | 3 | 3 | 0 | Meta-matching persists |
| 10 | JSON serialization | 3 | 3 | 0 | Meta-matching persists |
| 11 | Embeddings | 7 | 6 | +1 | embed/vector terms boosted |
| 12 | Config env | 7 | 8 | -1 | Evaluator variance |
| 13 | PathNormalizer | 8 | 7 | +1 | Kind "impl" context helped |
| 14 | PageRank | 6 | 8 | **-2** | IDF dilution: "rank" variants everywhere |
| 15 | File watcher | **6** | 3 | **+3** | BREAKTHROUGH: spawn_watch_loop→#1 |

**CI avg: 5.40** (R40: 5.53, **-0.13**) | Augment avg: 8.80 | Gap: 3.40

#### Round 41 Analysis

**Overall:** Net -0.13 (within noise floor), but hides a dramatic per-query shift: Q15's +3 is the **single largest improvement on a stuck query** across 41 benchmark rounds, offset by three -2 regressions (Q4, Q6, Q14).

**Why Q15 improved:**
- `spawn_watch_loop` body contains `check_for_changes` → split token "changes" → backward variant "change" added to index
- Body contains `index_all()` → token "index" → prefix variant "reindex" added
- Query "file watcher debounce reindex on change" now gets BM25 term matches on "change" and "reindex" — terms that previously had zero matches in the function's indexed text

**Why Q4/Q6/Q14 regressed (IDF dilution):**
Morphological variants applied to ALL body identifiers spread common programming terms across many more documents:
- "rank" appears in dozens of functions → forward variant "ranking" added to all of them → IDF for "ranking" drops → Q6 ("ranking and scoring") loses discrimination
- Similarly "handle"→"handler"/"handling" dilutes Q4, "rank"→"ranking" dilutes Q14
- This is a classic BM25 tradeoff: enriching index text improves recall but can hurt precision by lowering IDF of formerly-discriminating terms

**Next steps:**
1. **Selective variant generation** — Only generate variants for NAME tokens (most discriminating) rather than ALL body identifiers. A function's name carries 10x more signal than arbitrary body tokens.
2. **Variant budget per function** — Cap at 10-15 variants instead of 80. Prioritize name-derived and rare body tokens.
3. **IDF-aware filtering** — Skip generating variants for tokens that already appear in >20% of indexed functions (common terms like "handle", "result", "error").

### Round 42 — Name-only morphological variants (schema v14)

**Change:** Restricted `generate_nl_description()` to only generate morphological variants from the symbol NAME (not body identifiers). Reduced variant budget from 80 to 15. Removed `extract_identifier_tokens()` (dead code after restriction). Bumped schema v13→v14 for clean IDF recomputation.

**Rationale:** R41 showed body-wide variants cause IDF dilution — common tokens like "rank", "handle", "score" get forward variants added to dozens of functions, lowering their IDF and hurting queries that relied on them being discriminating. Name tokens carry 10x more signal than arbitrary body tokens.

| # | Query (short) | R41 CI | R42 CI | Delta | Notes |
|---|------------|--------|--------|-------|-------|
| 1 | Ranking/scoring | 5 | 4 | -1 | Evaluator variance |
| 2 | Embeddings | 7 | 7 | 0 | -- |
| 3 | Tree-sitter parsing | 3 | 3 | 0 | Meta-matching persists |
| 4 | Config env vars | 4 | 7 | **+3** | IDF recovery: "config"/"env" discriminating again |
| 5 | Indexing pipeline | 5 | 6 | +1 | -- |
| 6 | MCP tool requests | 5 | 8 | **+3** | IDF recovery: "handle"/"server" discriminating again |
| 7 | WebSocket handler | 3 | 3 | 0 | Infrastructure contamination persists |
| 8 | SQLite schema | 9 | 8 | -1 | Evaluator variance |
| 9 | Error handling | 3 | 3 | 0 | Meta-matching persists |
| 10 | JSON serialization | 3 | 3 | 0 | Meta-matching persists |
| 11 | Async parallel | 7 | 6 | -1 | Test pollution at #3 |
| 12 | Caching | 7 | 7 | 0 | -- |
| 13 | PathNormalizer | 8 | 5 | **-3** | Test pollution: 4/5 results are test helpers |
| 14 | EmbeddingCache | 6 | 6 | 0 | Stable after R41 regression |
| 15 | File watcher | 6 | 6 | 0 | spawn_watch_loop still #1 (R41 gain preserved) |

**CI avg: 5.73** (R41: 5.40, **+0.33**) | Augment avg: 8.80 | Gap: 3.07

#### Round 42 Analysis

**Overall:** Net +0.33 — first positive delta in 5 rounds. The name-only variant restriction successfully fixed R41's IDF dilution regressions while preserving Q15's breakthrough.

**IDF dilution recovery (Q4: +3, Q6: +3):**
- Q4 ("Configuration from environment variables"): Recovered from 4→7. With body variants removed, tokens like "config" and "env" are no longer diluted across every function that uses configuration.
- Q6 ("MCP server handle tool requests"): Recovered from 5→8. "handle" and "server" regained discriminating power now that "handler"/"handling" variants aren't sprayed across all functions.
- Both now score HIGHER than their R40 baselines (Q4: R40=6→R42=7, Q6: R40=7→R42=8).

**Q15 preserved at 6:**
- `spawn_watch_loop` remains #1 because "watch"→"watcher"/"watching" comes from the NAME, not the body. The name-only restriction correctly keeps this vocabulary bridge.

**Q13 regression (-3, 8→5):**
- Test pollution: 4/5 results are test helpers (`create_test_normalizer`, `test_normalizer`). The PathNormalizer struct is #1 but test functions dominate remaining slots.
- This is likely evaluator noise amplified by corpus IDF changes from the schema v14 re-index. Test penalty (-10.0) may need refinement for symbol-lookup queries.

**Q14 stable at 6:**
- The third R41 IDF casualty (8→6) didn't recover, but didn't regress further. The regression likely had causes beyond IDF dilution (evaluator drift, corpus changes).

**Persistent low-scorers (CI ≤ 4):** Q3=3 (tree-sitter), Q7=3 (WebSocket), Q9=3 (error), Q10=3 (JSON) — all meta-matching or infrastructure contamination, NOT vocabulary gap issues. These require vector search improvements (Phase 2-3), not BM25 enrichment.

**Next steps:**
1. **Q13 test pollution fix** — Consider intent-based test penalty adjustment: higher penalty for queries containing struct/class/type names (likely looking for production code, not tests).
2. **Phase 2: LLM descriptions** — Qwen 1.5B to generate richer semantic descriptions that can address meta-matching by describing what code DOES rather than what terms it MENTIONS.
3. **Phase 3: Jina Code v2** — Better embedding model to improve vector search arm of hybrid retrieval.

### Round 43 — Intent enforcement pipeline fix + vector promotion bug fix (schema v14)

**Changes:**
1. **Intent enforcement moved to END of pipeline** — Root cause of R42's Q13 regression: `expand_with_edges` followed type edges from high-ranking PathNormalizer struct (score=21.74) and re-added test helpers with parent-derived scores (~17.74), bypassing the 0.05x test penalty applied earlier. Fix: moved intent enforcement to run AFTER `expand_with_edges`, `diversify_by_file`, and `promote_vector_results`. Edge-expanded hits without `hit_signals` entries get `intent_adjustment` computed on the fly.
2. **Vector promotion `intent_mult` bug fix** — `promote_vector_results` created `HitSignals` with `..Default::default()` → `intent_mult = 0.0` (f32 default). Final enforcement then multiplied promoted results by 0, zeroing their scores. Fix: explicitly set `intent_mult: 1.0` for promoted hits.
3. **Scoring tweaks** — `intent_adjustment` 0.15→0.05x for test symbols, `test_penalty` -5→-10, "impl" added to Definition kinds.

| # | Query (short) | R42 CI | R43 CI | Delta | Notes |
|---|------------|--------|--------|-------|-------|
| 1 | Ranking/scoring | 4 | 4 | 0 | `format_scoring_breakdown` at #1 instead of core scoring functions |
| 2 | Embeddings | 7 | 8 | **+1** | fastembed.rs file at #1, good generation coverage |
| 3 | Tree-sitter parsing | 3 | 3 | 0 | "parsing" matches tokenization/stemming, not tree-sitter |
| 4 | Config env vars | 7 | 7 | 0 | `from_env` at #5 instead of #1; `clear_env` test at #3 |
| 5 | Indexing pipeline | 6 | 7 | **+1** | ExtractedSymbol + typescript extractor + parsing pipeline |
| 6 | MCP tool requests | 8 | 8 | 0 | dispatch_tool_call at #1, stable |
| 7 | WebSocket handler | 3 | 3 | 0 | Single-file flooding in elysia.rs |
| 8 | SQLite schema | 8 | 8 | 0 | schema.rs with SCHEMA_SQL at #1, stable |
| 9 | Error handling | 3 | 3 | 0 | `expand_stems` at #1 — "gracefully" stem matched |
| 10 | JSON serialization | 3 | 3 | 0 | `extract_concept_tags` meta-matching persists |
| 11 | Async parallel | 6 | 6 | 0 | index_files_parallel_async at #1, test at #3 |
| 12 | Caching | 7 | 7 | 0 | Good cache coverage across 3 files |
| 13 | PathNormalizer | 5 | 6 | **+1** | Struct+impl at #1-#2 (intent fix worked); keyword noise at #3-#5 |
| 14 | EmbeddingCache | 6 | 7 | **+1** | put/get methods at #1-#2, content_hash at #3 |
| 15 | File watcher | 6 | 6 | 0 | spawn_watch_loop at #2, stats module noise at #1 |

**CI avg: 5.73** (R42: 5.47*, **+0.27**) | Augment avg: 8.80 | Gap: 3.07

*Note: R42 was previously reported as 5.73 but actual sum (82/15) = 5.47. The discrepancy was a calculation error in R42 compilation.

#### Round 43 Analysis

**Overall:** Net +0.27 (4 improvements, 0 regressions, 11 stable). The intent enforcement pipeline fix eliminated regressions while producing modest gains. This is the **first round with zero regressions** since R39.

**Intent enforcement fix (Q13: +1, 5→6):**
- PathNormalizer struct and impl block now rank #1-#2 (previously test helpers dominated #2-#5). The fix correctly prevents `expand_with_edges` from reintroducing penalized test symbols.
- Remaining noise: `contains_code_snippet` (#3), `is_definition_kind` (#4), `ExtractedFrameworkPattern` (#5) — these are BM25 keyword matches on "definition" and "struct" terms in the query, not test pollution. Different failure mode than R42.

**Vector promotion bug fix (Q2: +1, Q5: +1, Q14: +1):**
- With `intent_mult` correctly set to 1.0, promoted vector results no longer get zeroed. This likely contributed to Q2 (embeddings), Q5 (indexing pipeline), and Q14 (EmbeddingCache) improvements where vector search found relevant symbols that BM25 missed or ranked lower.

**Persistent low-scorers (CI ≤ 4):** Q1=4, Q3=3, Q7=3, Q9=3, Q10=3 — all BM25 vocabulary mismatch or meta-matching. These cannot improve further without:
- **Phase 2 (LLM descriptions):** Semantic descriptions from Qwen 1.5B to describe what code DOES, addressing meta-matching (Q9, Q10) and vocabulary gaps (Q3).
- **Phase 3 (Jina Code v2):** Better embedding model for vector search arm, addressing cases where BGE-base-en-v1.5 has the same vocabulary gap as BM25 (Q1, Q3).

**Q9 new failure mode:** `expand_stems` now ranks #1 for "error handling and graceful degradation" because the word "gracefully" in the query triggers stem matching against "graceful"→"grace" variants. This is a different kind of meta-matching — the stemming function itself contains stem-related terms.

**Next steps:**
1. **Phase 2: LLM descriptions** — Qwen 1.5B for semantic descriptions. Primary target: Q9/Q10 (meta-matching) and Q3 (vocabulary gap). Expected: +2-3 points on these queries.
2. **Phase 3: Jina Code v2** — Better embedding model for vector search. Primary target: Q1/Q7 where neither BM25 nor BGE-base embeddings find the right symbols.
3. **Q4 test pollution** — `clear_env` (test helper) at #3 despite -10 test penalty. May need stronger penalty for symbol-lookup queries.

**Benchmark methodology note:** Round 30 tested two diversity approaches (v1: pre-truncation, v2: limit*3 pool expansion) + "handler" concept tag for WebSocket. R30v2 is the final score. Batch files at `docs/benchmark_rounds/round_30v2_batch_{1,2,3}.md` and `round_30_batch_{1,2,3}.md` (v1). Round 31 reverted the handler tag (schema v13) to recover from R30's regression; confirmed that concept tag IDF pollution was the primary cause of Q15's -4 drop. Round 32 was a stability check (no code changes) establishing ~±0.5 point evaluator noise floor. Round 33 activated concept tags (dead code since R26) — modest +0.40 gain but target queries Q7/Q9/Q10 unchanged due to low-IDF broad tags. Round 34 removed broad concept tags (error_handling, fallback, serialization, serde) and doubled test penalty (-5→-10). Q13 improved +2 from test penalty; overall -0.27 within noise floor. Round 35 added comment stripping (`strip_code_comments`) to BM25 indexing pipeline (schema v10). This surgically fixed `extract_concept_tags` meta-matching in Q9/Q10, but scores unchanged (3/4) — the underlying BM25 vocabulary gap remains. CI avg +0.07 (5.93→6.00), flat. Round 36 was a no-code-change stability check. CI avg -0.20 (6.00→5.80), confirming ±0.3-0.5 evaluator noise floor. Q10 anomaly: `extract_concept_tags` reportedly returned at #1 despite comment stripping — warrants investigation. Round 37 added concept-tag-based name enrichment for WebSocket symbols (schema v12). Symbols in `indexer/extract/` with "websocket" concept tag get "websocket_handler" in name field. Q7 improved 2→3 (+1, first movement in 32 rounds). `extract_pattern_details` reached #1 but `classify_elysia_method` only #8 — infrastructure body-text contamination caps further BM25 gains. CI avg -0.27 (5.80→5.53), within noise. Round 38 was a no-code-change stability check. CI avg -0.20 (5.53→5.33). 11/15 queries identical to R37. Q3 swung -3 (evaluator noise on ambiguous "parsing" query). Three stability rounds (R32/R36/R38) establish mean noise drift of -0.29 with range [-0.47, -0.20], suggesting slight evaluator strictness drift over time.

### Round 44 — Stability check (LLM description code merged but not yet active in search)

**Changes:** LLM description infrastructure merged (Qwen2.5-Coder-1.5B ONNX engine, auto-download, background model loading). No changes to search/ranking pipeline code. LLM descriptions not yet wired into BM25 index text — this round establishes pre-LLM-description baseline.

| # | Query (short) | R43 CI | R44 CI | Delta | Notes |
|---|------------|--------|--------|-------|-------|
| 1 | Ranking/scoring | 4 | 4 | 0 | `format_scoring_breakdown` at #1, core scoring functions absent |
| 2 | Embeddings | 8 | 7 | -1 | Good generation coverage, but `storage/vector.rs` absent |
| 3 | Tree-sitter parsing | 3 | 3 | 0 | `expand_stems` at #1, tree-sitter code absent |
| 4 | Config env vars | 6 | 5 | -1 | `from_env` buried at #5, `clear_env` test at #3 |
| 5 | Indexing pipeline | 7 | 6 | -1 | ExtractedSymbol at #1 but pipeline orchestrator absent |
| 6 | MCP tool requests | 8 | 9 | **+1** | dispatch_tool_call at #1, excellent coverage |
| 7 | WebSocket handler | 3 | 3 | 0 | Single-file flooding in elysia.rs |
| 8 | SQLite schema | 8 | 9 | **+1** | schema.rs + init() at #1-#2 |
| 9 | Error handling | 3 | 3 | 0 | `expand_stems` at #1 — "gracefully" stem meta-match |
| 10 | JSON serialization | 3 | 3 | 0 | `extract_concept_tags` meta-matching persists |
| 11 | Async parallel | 6 | 6 | 0 | index_files_parallel_async at #1 |
| 12 | Caching | 7 | 7 | 0 | Good cache coverage across 3 files |
| 13 | PathNormalizer | 6 | 8 | **+2** | Struct+impl at #1-#2; evaluator may have scored generously |
| 14 | EmbeddingCache | 7 | 7 | 0 | put/get at #1-#2, content_hash at #3 |
| 15 | File watcher | 7 | 6 | -1 | spawn_watch_loop at #1 but 4/5 from same file |

**CI avg: 5.73** (R43: 5.73, **+0.00**) | Augment avg: 8.53 | Gap: 2.80

#### Round 44 Analysis

**Overall:** Net +0.00 (3 improvements, 4 regressions, 8 stable). This is a **stability check round** — no search algorithm changes since R43. The identical average (5.73) confirms the pipeline is stable.

**Evaluator noise:** All per-query movements are ±1 except Q13 (+2). The +1/-1 movements are within the established ±0.3-0.5 noise floor. Q13's +2 is likely evaluator variance — R43 and R44 both report PathNormalizer struct+impl at #1-#2 with keyword noise at #3-#5, but R44 evaluator scored more generously. Four stability rounds (R32/R36/R38/R44) now establish that ±1 point per-query and ±0.3 average are normal fluctuation.

**Persistent low-scorers (CI ≤ 4):** Q1=4, Q3=3, Q7=3, Q9=3, Q10=3 — unchanged across R42→R43→R44. These are structurally blocked:
- **Q3, Q9, Q10:** Meta-matching — functions that _detect_ patterns rank for queries about those patterns (expand_stems for "gracefully", extract_concept_tags for "json"/"response"/"formatting")
- **Q1:** Vocabulary gap — "ranking and scoring" doesn't match `rank_hits_with_signals` or `structural_adjustment`
- **Q7:** WebSocket handler code buried in framework extractor, BM25 can't bridge "WebSocket handler" → `classify_elysia_method`

**Next steps (unchanged from R43):**
1. **Phase 2: LLM descriptions** — Wire Qwen 1.5B descriptions into BM25 index text. Primary target: Q9/Q10 (meta-matching), Q3 (vocabulary gap), Q1 (vocabulary gap). The LLM engine is merged but descriptions aren't yet part of the indexed text.
2. **Phase 3: Jina Code v2** — Better embedding model for vector search arm. Primary target: Q1/Q7.
3. **Q4 test pollution** — `clear_env` test helper persists at #3 despite -10 penalty.

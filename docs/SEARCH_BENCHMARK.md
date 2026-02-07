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

# Search Quality Benchmark

## Purpose

Track and improve search quality for Code-Intelligence MCP Server's `search_code` tool. 15 standard queries run against this codebase, scored 1-10. Results drive code changes in the retrieval/ranking pipeline.

## How to Run

```bash
# Live mode — uses existing index with real embeddings + LLM descriptions (~3s)
python3 scripts/run_benchmark.py --live

# Fresh mode — temp dirs, hash embeddings, full re-index (~60s, BM25-only)
python3 scripts/run_benchmark.py
```

**Always use `--live` for official rounds.** Fresh mode is only for testing scoring changes in isolation.

### Prerequisites

```bash
cargo build --release    # Script runs ./target/release/code-intelligence-mcp-server
```

For live mode, `.cimcp/` must exist with a populated index (run the MCP server at least once first).

### Options

```bash
python3 scripts/run_benchmark.py --live                     # All 15 queries, auto-detect round number
python3 scripts/run_benchmark.py --live --round 48          # Set round number
python3 scripts/run_benchmark.py --live --queries 1,3,9     # Specific queries only
python3 scripts/run_benchmark.py --live --output results.md # Custom output file
python3 scripts/run_benchmark.py --live --limit 10          # 10 results per query (default: 5)
```

### Output

- `docs/benchmark_rounds/round_N_results.md` — Result tables + scoring template
- `docs/benchmark_rounds/round_N_results.json` — Raw JSON

### After Running

1. Read the results file and score each query 1-10 (see Scoring Rubric)
2. Compile the round into the Historical Results section below

Results in live mode are **fully deterministic** — verified by diffing consecutive runs with zero differences.

## Scoring Rubric

| Score | Description |
|-------|-------------|
| **9-10** | Every result directly answers the query, spans relevant files |
| **7-8** | Most results relevant, good diversity, top 3-5 strong |
| **5-6** | ~Half relevant, some gaps, core code present but buried |
| **3-4** | Few relevant, dominated by 1-2 files, test/re-export noise |
| **1-2** | Mostly irrelevant, core implementation missing |

**Tips:** Score conservatively. Weight top-3 results heavily. Test files only acceptable for test queries.

## Standard Test Suite

### Broad Concept Queries (Q1-Q5)

| # | Query | Expected Results |
|---|-------|-----------------|
| 1 | "How does the ranking and scoring system work?" | `ranking/score.rs`, `ranking/mod.rs`, `ranking/diversify.rs`, `ranking/rrf.rs` |
| 2 | "How are embeddings generated and stored?" | `storage/vector.rs`, `embeddings/fastembed.rs`, `embeddings/mod.rs` |
| 3 | "How does tree-sitter parsing work in this codebase?" | `indexer/parser.rs`, `indexer/extract/` language extractors |
| 4 | "Configuration from environment variables" | `config.rs`, main entry point |
| 5 | "Indexing pipeline file scanning and symbol extraction" | `indexer/mod.rs`, `indexer/extract/mod.rs`, symbol types |

### Architecture & Routing (Q6-Q8)

| # | Query | Expected Results |
|---|-------|-----------------|
| 6 | "How does the MCP server handle incoming tool requests?" | `server/mod.rs`, `handlers/mod.rs`, tool dispatch |
| 7 | "How does the WebSocket handler work?" | WebSocket handler code in `extract/elysia.rs` |
| 8 | "SQLite database schema tables initialization" | `sqlite/schema.rs`, `sqlite/operations.rs` |

### Cross-Cutting Concerns (Q9-Q12)

| # | Query | Expected Results |
|---|-------|-----------------|
| 9 | "Error handling and graceful degradation" | Error types, fallback logic across modules |
| 10 | "JSON serialization and response formatting" | Serde usage, response builders, MCP formatting |
| 11 | "Async concurrency and parallel processing" | Parallel indexing, async operations |
| 12 | "Caching and cache invalidation" | `storage/cache.rs`, `reranker/cache.rs` |

### Specific Symbol Lookups (Q13-Q15)

| # | Query | Expected Results |
|---|-------|-----------------|
| 13 | "PathNormalizer struct definition and methods" | `path/mod.rs` struct + impl |
| 14 | "EmbeddingCache get put cached embedding" | Cache struct, get/put methods |
| 15 | "File watcher debounce reindex on change" | `pipeline/watch.rs`, `spawn_watch_loop` |

## Improvement Workflow

1. **Baseline:** `python3 scripts/run_benchmark.py --live --round N`
2. **Identify patterns** in low-scoring queries (see `docs/SEARCH_BENCHMARK_ARCHIVE.md` for known failure patterns)
3. **Implement fix** — typical targets: `score.rs` (scoring), `tantivy.rs` (indexing), `diversify.rs` (post-processing), `query.rs` (query processing)
4. **Re-benchmark:** If index changed, bump `TANTIVY_SCHEMA_VERSION`, rebuild, restart server, wait for indexing. Then: `python3 scripts/run_benchmark.py --live --round N+1`
5. **Check regressions** — always run all 15 queries, not just the ones you targeted

## Key Files

| File | Controls |
|------|----------|
| `src/retrieval/ranking/score.rs` | Scoring signals, test penalty, intent multipliers |
| `src/retrieval/ranking/diversify.rs` | Per-file diversity capping |
| `src/retrieval/ranking/rrf.rs` | Reciprocal Rank Fusion (Tantivy + LanceDB) |
| `src/retrieval/ranking/mod.rs` | Ranking pipeline orchestration, intent detection |
| `src/retrieval/query.rs` | Query normalization, field boost selection |
| `src/storage/tantivy.rs` | Full-text index schema, what gets indexed |
| `src/storage/vector.rs` | Vector/semantic search via LanceDB |

## Historical Results

### CI Score by Query (Key Rounds)

| # | Query | R5 | R12 | R25 | R37 | R43 | R47 | R49 | R50 | R55 | R56 | **R58** |
|---|-------|----|-----|-----|-----|-----|-----|-----|-----|-----|-----|---------|
| 1 | Ranking/scoring | 3 | 7 | 8 | 6 | 4 | 4 | 5 | 5 | 5 | 6 | **7** |
| 2 | Embeddings | 3 | 7 | 5 | 4 | 8 | 7 | 7 | 7 | 7 | 5 | **5** |
| 3 | Tree-sitter | 5 | 2 | 7 | 6 | 3 | 3 | 3 | 3 | 3 | 5 | **5** |
| 4 | Config env | 4 | 8 | 8 | 7 | 8 | 9 | 9 | 9 | 9 | 9 | **9** |
| 5 | Indexing pipeline | 6 | 8 | 8 | 7 | 7 | 7 | 7 | 7 | 7 | 7 | **7** |
| 6 | MCP tool handling | 3 | 9 | 9 | 8 | 8 | 8 | 8 | 8 | 8 | 9 | **9** |
| 7 | WebSocket | 2 | 3 | 2 | 3 | 3 | 5 | 5 | 5 | 5 | 5 | **5** |
| 8 | SQLite schema | 5 | 5 | 7 | 5 | 7 | 9 | 9 | 9 | 9 | 6 | **7** |
| 9 | Error handling | 3 | 3 | 4 | 3 | 3 | 3 | 2 | 1 | 5 | 6 | **6** |
| 10 | JSON serial. | 3 | 3 | 4 | 4 | 3 | 4 | 3 | 4 | 4 | 6 | **6** |
| 11 | Async concurrency | 4 | 8 | 8 | 7 | 7 | 8 | 8 | 8 | 8 | 8 | **8** |
| 12 | Caching | 6 | 8 | 9 | 7 | 8 | 7 | 7 | 7 | 7 | 7 | **7** |
| 13 | PathNormalizer | 5 | 6 | 7 | 6 | 6 | 7 | 9 | 8 | 8 | 6 | **6** |
| 14 | EmbeddingCache | 2 | 8 | 9 | 7 | 7 | 8 | 8 | 8 | 8 | 8 | **8** |
| 15 | File watcher | 5 | 2 | 6 | 3 | 7 | 7 | 7 | 7 | 7 | 8 | **8** |
| **CI Avg** | | **3.9** | **5.8** | **6.7** | **5.5** | **5.7** | **6.4** | **6.5** | **6.4** | **6.7** | **6.7** | **6.9** |

### CI Average Trend

```
R5:  3.9  █████████▊
R6:  4.9  ████████████▎
R7:  5.0  ████████████▌
R12: 5.8  ██████████████▌
R25: 6.7  ████████████████▊
R37: 5.5  █████████████▊
R40: 5.4  █████████████▌
R41: 5.4  █████████████▌
R42: 5.5  █████████████▊
R43: 5.7  ██████████████▎
R44: 5.7  ██████████████▎
R45: 5.6  ██████████████
R47: 6.4  ████████████████   ← LLM descriptions active
R49: 6.5  ████████████████▎  ← path variants + inline comment strip
R50: 6.4  ████████████████   ← LLM v2 descriptions (no measurable impact)
R55: 6.7  ████████████████▊  ← Intent::Error fix (Q9: 1→5)
R56: 6.7  ████████████████▊  ← Jina query embedding fix (Q3: 3→5, Q10: 4→6)
R58: 6.9  █████████████████▎ ← Edge expansion intent stripping + promote fix
```

**Key milestones:**
- R5 (3.9): First full 15-query baseline
- R6-R12 (4.9→5.8): RRF scoring fix, intent multipliers, test penalties
- R25 (6.7): Import tags + synonym expansion peak
- R37 (5.5): Post-cleanup baseline (comment stripping, concept tags settled)
- R43 (5.7): Intent enforcement pipeline fix + vector promotion bug fix
- R47 (6.4): LLM descriptions active — largest single-round improvement (+0.80)
- R55 (6.7): Intent::Error suppression + pool expansion fixed Q9 (1→5). Ties R25 all-time high.
- R56 (6.7): Query embedding fix eliminated 3 meta-matches (Q3 +2, Q10 +2).
- **R58 (6.9): Edge expansion intent stripping + vector promote fix. New all-time high (6.87).**

### Recent Rounds (Detail)

#### Round 43 — Intent enforcement pipeline fix + vector promotion bug fix

| # | Query (short) | R42 CI | R43 CI | Delta |
|---|------------|--------|--------|-------|
| 1 | Ranking/scoring | 4 | 4 | 0 |
| 2 | Embeddings | 7 | 8 | +1 |
| 3 | Tree-sitter | 3 | 3 | 0 |
| 4 | Config env | 8 | 8 | 0 |
| 5 | Indexing pipeline | 6 | 7 | +1 |
| 6 | MCP tool requests | 8 | 8 | 0 |
| 7 | WebSocket | 3 | 3 | 0 |
| 8 | SQLite schema | 7 | 7 | 0 |
| 9 | Error handling | 3 | 3 | 0 |
| 10 | JSON serial. | 3 | 3 | 0 |
| 11 | Async parallel | 7 | 7 | 0 |
| 12 | Caching | 8 | 8 | 0 |
| 13 | PathNormalizer | 5 | 6 | +1 |
| 14 | EmbeddingCache | 6 | 7 | +1 |
| 15 | File watcher | 7 | 7 | 0 |

**CI avg: 5.73** | 4 improvements, 0 regressions. Fixed: `expand_with_edges` bypassed scoring penalties; `promote_vector_results` had `intent_mult: 0.0` bug.

#### Round 47 — LLM descriptions active (first live-mode benchmark)

**Changes:** 1663 symbols now have Qwen2.5-Coder-1.5B-generated descriptions in the Tantivy index. ORT upgraded rc.9→rc.11. model_q4.onnx replaces model_q4f16.onnx.

| # | Query (short) | R45 CI | R47 CI | Delta | Notes |
|---|------------|--------|--------|-------|-------|
| 1 | Ranking/scoring | 4 | 4 | 0 | Core scoring functions still absent |
| 2 | Embeddings | 7 | 7 | 0 | fastembed.rs at #1-#2, missing vector.rs |
| 3 | Tree-sitter | 3 | 3 | 0 | `expand_stems` meta-match at #1 |
| 4 | Config env | 5 | 9 | **+4** | All 5 from config.rs — descriptions added "environment variable" terms |
| 5 | Indexing pipeline | 6 | 7 | +1 | Good extractor diversity |
| 6 | MCP tool requests | 9 | 8 | -1 | Python script `call_tool` at #2 (noise) |
| 7 | WebSocket | 3 | 5 | **+2** | websocket_handler extractor + fmt at #1/#4 |
| 8 | SQLite schema | 9 | 9 | 0 | schema.rs at 945 BM25 score |
| 9 | Error handling | 3 | 3 | 0 | `expand_stems` "gracefully" meta-match persists |
| 10 | JSON serial. | 3 | 4 | +1 | `extract_concept_tags` meta-match at #1, SearchResponse at #4 |
| 11 | Async parallel | 6 | 8 | **+2** | Descriptions bridged "async parallel" to pipeline functions |
| 12 | Caching | 7 | 7 | 0 | Good spread across cache modules |
| 13 | PathNormalizer | 7 | 7 | 0 | Struct+impl at #1-#2 |
| 14 | EmbeddingCache | 7 | 8 | +1 | put/get/content_hash/put_cached_embedding at #1-#4 |
| 15 | File watcher | 5 | 7 | **+2** | Descriptions added "watcher"/"debounce" terms |

**CI avg: 6.40** (+0.80 vs R45) — largest single-round improvement. 6 improvements, 1 regression, 8 stable.

#### Round 55 — Intent::Error suppression + pool expansion (Q9 fix)

**Changes:** (1) Intent::Error multiplier: 0.2x for non-error symbols (was 1.0 neutral), 2.5x for error-named, 3.0x for error-path files. (2) BM25 pool expansion k=500 for Intent::Error (was 40). LLM descriptions dilute IDF("error"), pushing PathError below k=40 threshold.

| # | Query (short) | R50 CI | R55 CI | Delta | Notes |
|---|------------|--------|--------|-------|-------|
| 9 | Error handling | 1 | 5 | **+4** | PathError #1 (fresh) / tool_internal_error #1 (live). 4/5 relevant. expand_stems meta-match persists at #2 |

**CI avg: 6.67** (+0.27 vs R50) — Q9 only change. Fresh-mode validated, live-mode Q9-only confirmed. Full live-mode blocked by running server contamination.

**Note:** R55 Q9 score based on live-mode Q9-only benchmark (tool_internal_error #1, expand_stems #2, PathError Display #3, PathError #4, fmt #5). Fresh mode shows PathError #1 (16.04), tool_internal_error #2 (12.99). Non-Q9 queries unchanged (Intent::Error changes only affect Q9).

#### Round 56 — Fix Jina Code v2 query embedding mismatch

**Changes:** Fixed 3 bugs in `src/embeddings/fastembed.rs`:
1. `query_embed()` unconditionally applied BGE's instruction prefix to ALL models, including Jina Code v2. Now only applies prefix for `BAAI/bge-*` models; Jina/MiniLM use symmetric `embed()`.
2. `jinaai/jina-embeddings-v2-base-en` mapped to wrong enum variant (`JinaEmbeddingsV2BaseCode` → `JinaEmbeddingsV2BaseEN`).
3. `dim()` returned 384 for BGE-base-en-v1.5 (should be 768; BGE-small is 384).

**Impact:** With correct query embeddings, vector search now properly contributes to hybrid RRF merge. Previously, mismatched query/document embedding spaces meant vector results were essentially noise.

| # | Query (short) | R55 CI | R56 CI | Delta | Notes |
|---|------------|--------|--------|-------|-------|
| 1 | Ranking/scoring | 5 | 6 | +1 | 4/5 from ranking/* (rrf.rs, mod.rs, score.rs, reranker.rs). Test at #3 |
| 2 | Embeddings | 7 | 5 | -2 | repo_name #1 noise, Config #5 noise. Missing vector.rs |
| 3 | Tree-sitter | 3 | 5 | **+2** | All 5 genuine tree-sitter extractors. `expand_stems` meta-match **eliminated** |
| 4 | Config env | 9 | 9 | 0 | 5/5 config.rs |
| 5 | Indexing pipeline | 7 | 7 | 0 | Good mix: pipeline, scan, parallel, extract |
| 6 | MCP tool requests | 8 | 9 | +1 | 5/5 tool dispatch. Python script noise **eliminated** |
| 7 | WebSocket | 5 | 5 | 0 | websocket_handler #1, elysia neighbors #2-3 |
| 8 | SQLite schema | 9 | 6 | -3 | schema.rs #1 (690). But #3 test helper, #4-5 todos queries |
| 9 | Error handling | 5 | 6 | +1 | tool_internal_error, PathError, retry. `expand_stems` meta-match **eliminated** |
| 10 | JSON serial. | 4 | 6 | **+2** | All 5 response/serialization. `extract_concept_tags` meta-match **eliminated** |
| 11 | Async parallel | 8 | 8 | 0 | 5/5 parallel/async functions |
| 12 | Caching | 7 | 7 | 0 | Spread across 3 cache modules |
| 13 | PathNormalizer | 8 | 6 | -2 | Struct+impl+method present, but tests(llm) #2 and is_definition_kind #4 noise |
| 14 | EmbeddingCache | 8 | 8 | 0 | put/get/cache_key/content_hash all present |
| 15 | File watcher | 7 | 8 | +1 | spawn_watch_loop #1, create_watcher #4, watch.rs #3 |

**CI avg: 6.73** (+0.07 vs R55) — new all-time high. 6 improvements, 3 regressions, 6 stable.

**Key finding:** Correct Jina embeddings eliminated 3 persistent BM25 meta-matches (`expand_stems` from Q3/Q9, `extract_concept_tags` from Q10). These pattern-detection functions ranked high on BM25 because their code literally contains search terms in string literals. With vector search now properly working, semantic dissimilarity downranks them in hybrid merge.

**Regressions analysis:** Q2 (-2), Q8 (-3), Q13 (-2) caused by changed vector contributions to hybrid merge. Q8 root cause: `expand_with_edges` inherited 75x-inflated parent scores from TodoRow (Schema intent), causing todos.rs functions to dominate #3-5. Fixed in R58.

#### Round 58 — Edge expansion intent stripping + vector promote fix

**Changes:** Two fixes addressing R56 regressions:
1. **Edge expansion intent stripping** (`expansion.rs`): `expand_with_edges` now strips the parent's intent multiplier before deriving child scores. Previously, TodoRow (75x Schema boost, score 291) produced children at `291 * 0.8 ≈ 233`. Now it produces `(291/75) * 0.8 ≈ 3.1`. Final enforcement then applies the child's own 0.5x, yielding ~1.55 instead of ~108.
2. **Vector promote bypass fix** (`mod.rs`, committed in R57): `promote_vector_results` no longer inserts hardcoded `intent_mult: 1.0` into signals map. Promoted results now go through on-the-fly intent computation in final enforcement, correctly applying test penalties (e.g., `setup_test_db` dropped from 216→0.27).

| # | Query (short) | R56 CI | R58 CI | Delta | Notes |
|---|------------|--------|--------|-------|-------|
| 1 | Ranking/scoring | 6 | 7 | **+1** | Test moved #3→#5; top-3 all clean ranking functions |
| 2 | Embeddings | 5 | 5 | 0 | repo_name #1 noise, missing vector.rs |
| 3 | Tree-sitter | 5 | 5 | 0 | Same 5 genuine extractors + parser.rs |
| 4 | Config env | 9 | 9 | 0 | 5/5 config.rs |
| 5 | Indexing pipeline | 7 | 7 | 0 | Same good mix |
| 6 | MCP tool requests | 9 | 9 | 0 | 5/5 dispatch/handler |
| 7 | WebSocket | 5 | 5 | 0 | Same websocket results |
| 8 | SQLite schema | 6 | 7 | **+1** | todos.rs functions **eliminated** from #3-5. Now: open_or_create_table #3, impl SqliteStore #4 |
| 9 | Error handling | 6 | 6 | 0 | Same error types + retry |
| 10 | JSON serial. | 6 | 6 | 0 | Same response/formatting |
| 11 | Async parallel | 8 | 8 | 0 | 5/5 parallel functions |
| 12 | Caching | 7 | 7 | 0 | Same cache modules |
| 13 | PathNormalizer | 6 | 6 | 0 | Struct+impl+method, noise at #3-4 |
| 14 | EmbeddingCache | 8 | 8 | 0 | 5/5 cache operations |
| 15 | File watcher | 8 | 8 | 0 | spawn_watch_loop #1, watch.rs #3 |

**CI avg: 6.87** (+0.13 vs R56) — new all-time high. 2 improvements, 0 regressions, 13 stable.

**Key finding:** Edge-expanded results were inheriting parent intent multipliers, causing ~120x score inflation for children of Schema-boosted parents. Stripping the parent's intent before derivation ensures children score based on the parent's base relevance, not its intent-inflated score. The final enforcement then correctly applies the child's own intent multiplier.

## Current Status & Next Steps

**Current: R58** | CI avg: **6.87** | Schema v15 | LLM v2 descriptions active (1682 symbols) | Jina query embeddings fixed | Edge expansion intent-aware

### Persistent Low Scorers (CI ≤ 5)

| Query | CI | Root Cause | Fix Path |
|-------|----|-----------|----------|
| Q2: Embeddings | 5 | repo_name #1 noise, missing vector.rs | Investigate why vector.rs doesn't rank; may need description enrichment |
| Q3: Tree-sitter | 5 | All 5 genuine extractors but missing parser.rs overview | parser.rs description enrichment or higher vector weight |
| Q7: WebSocket | 5 | Only #1 directly WebSocket; elysia neighbors flood #2-3 | Single-file diversity cap or WebSocket intent detection |

### Priorities

1. **Investigate Q2 regression** — repo_name at #1 is noise; vector.rs missing. May need RRF weight tuning or vector.rs description enrichment.
2. **Investigate Q13 noise** — tests(llm) #3 and is_definition_kind #4 are unrelated. Need better intent filtering for Definition queries.
3. **LLM model upgrade** — 1.5B Qwen generates too-generic descriptions. Consider 3B+ model or description post-processing.
4. **Increase vector weight** — Now that Jina embeddings work correctly, experiment with HYBRID_ALPHA < 0.7 to give vector more influence.

## Reference

- **Detailed round-by-round history (R1-R45):** `docs/SEARCH_BENCHMARK_ARCHIVE.md`
- **Raw results:** `docs/benchmark_rounds/round_N_results.{md,json}`
- **Benchmark script:** `scripts/run_benchmark.py`
- **Known failure patterns & improvement workflow:** `docs/SEARCH_BENCHMARK_ARCHIVE.md`

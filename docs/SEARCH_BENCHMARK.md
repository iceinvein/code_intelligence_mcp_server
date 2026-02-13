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

| # | Query | R5 | R12 | R25 | R37 | R43 | R45 | **R47** |
|---|-------|----|-----|-----|-----|-----|-----|---------|
| 1 | Ranking/scoring | 3 | 7 | 8 | 6 | 4 | 4 | **4** |
| 2 | Embeddings | 3 | 7 | 5 | 4 | 8 | 7 | **7** |
| 3 | Tree-sitter | 5 | 2 | 7 | 6 | 3 | 3 | **3** |
| 4 | Config env | 4 | 8 | 8 | 7 | 8 | 5 | **9** |
| 5 | Indexing pipeline | 6 | 8 | 8 | 7 | 7 | 6 | **7** |
| 6 | MCP tool handling | 3 | 9 | 9 | 8 | 8 | 9 | **8** |
| 7 | WebSocket | 2 | 3 | 2 | 3 | 3 | 3 | **5** |
| 8 | SQLite schema | 5 | 5 | 7 | 5 | 7 | 9 | **9** |
| 9 | Error handling | 3 | 3 | 4 | 3 | 3 | 3 | **3** |
| 10 | JSON serial. | 3 | 3 | 4 | 4 | 3 | 3 | **4** |
| 11 | Async concurrency | 4 | 8 | 8 | 7 | 7 | 6 | **8** |
| 12 | Caching | 6 | 8 | 9 | 7 | 8 | 7 | **7** |
| 13 | PathNormalizer | 5 | 6 | 7 | 6 | 6 | 7 | **7** |
| 14 | EmbeddingCache | 2 | 8 | 9 | 7 | 7 | 7 | **8** |
| 15 | File watcher | 5 | 2 | 6 | 3 | 7 | 5 | **7** |
| **CI Avg** | | **3.9** | **5.8** | **6.7** | **5.5** | **5.7** | **5.6** | **6.4** |

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
```

**Key milestones:**
- R5 (3.9): First full 15-query baseline
- R6-R12 (4.9→5.8): RRF scoring fix, intent multipliers, test penalties
- R25 (6.7): Import tags + synonym expansion peak
- R37 (5.5): Post-cleanup baseline (comment stripping, concept tags settled)
- R43 (5.7): Intent enforcement pipeline fix + vector promotion bug fix
- **R47 (6.4): LLM descriptions active — largest single-round improvement (+0.80)**

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

## Current Status & Next Steps

**Current: R47** | CI avg: **6.40** | Schema v14 | LLM descriptions active (1663 symbols)

### Persistent Low Scorers (CI ≤ 4)

| Query | CI | Root Cause | Fix Path |
|-------|----|-----------|----------|
| Q1: Ranking/scoring | 4 | Vocabulary gap — "ranking" doesn't match `rank_hits_with_signals` | Check if LLM descriptions mention "ranking"/"scoring" for score.rs functions |
| Q3: Tree-sitter | 3 | `expand_stems` meta-match at #1; parser.rs buried | Check if parser.rs descriptions mention "tree-sitter" |
| Q9: Error handling | 3 | "gracefully" triggers stem-matching on `expand_stems` | Vector search (Phase 3) or BM25 negative signal for stem-processing functions |
| Q10: JSON serial. | 4 | `extract_concept_tags` CODE contains pattern-detection strings | Unfixable by BM25 — need vector search to outrank meta-matches |

### Priorities

1. **Investigate descriptions for Q1/Q3** — If LLM descriptions for score.rs/parser.rs don't mention the right terms, tune the description prompt
2. **Phase 3: Jina Code v2** — Better embedding model for vector search arm. Primary target: Q9/Q10 (meta-matching unfixable by BM25), Q1/Q7
3. **Q6 noise** — Python script `call_tool` at #2; exclude `scripts/` from indexing or add script-file penalty

## Reference

- **Detailed round-by-round history (R1-R45):** `docs/SEARCH_BENCHMARK_ARCHIVE.md`
- **Raw results:** `docs/benchmark_rounds/round_N_results.{md,json}`
- **Benchmark script:** `scripts/run_benchmark.py`
- **Known failure patterns & improvement workflow:** `docs/SEARCH_BENCHMARK_ARCHIVE.md`

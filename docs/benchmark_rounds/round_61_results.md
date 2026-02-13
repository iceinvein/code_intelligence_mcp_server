# Round 61 - Raw Results

Generated: 2026-02-13 23:37
Symbols indexed: 1701
Total time: 3s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | should_rerank | function | 13.8 | ranking/reranker.rs |
| 2 | reciprocal_rank_fusion | function | 13.63 | ranking/rrf.rs |
| 3 | apply_reranker_scores | function | 13.26 | ranking/reranker.rs |
| 4 | src/retrieval/ranking/rrf.rs | file | 12.02 | ranking/rrf.rs |
| 5 | test_rrf_respects_weights | function | 0.14 | ranking/rrf.rs |

### Q2: "How are embeddings generated and stored?"
**Expected:** storage/vector.rs, embedding backend files, storage/ layer

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | generate_embeddings_for_parallel_indexed_files | function | 13.34 | pipeline/mod.rs |
| 2 | impl Embedder for FastEmbedder | impl | 11.26 | embeddings/fastembed.rs |
| 3 | src/embeddings/mod.rs | file | 11.26 | embeddings/mod.rs |
| 4 | src/embeddings/fastembed.rs | file | 10.98 | embeddings/fastembed.rs |
| 5 | Config | struct | 10.5 | src/config.rs |

### Q3: "How does tree-sitter parsing work in this codebase?"
**Expected:** indexer/parser.rs, indexer/extract/ language extractors

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/parser.rs | file | 12.14 | indexer/parser.rs |
| 2 | src/indexer/extract/c.rs | file | 10.13 | extract/c.rs |
| 3 | walk | function | 10.13 | extract/python.rs |
| 4 | src/indexer/extract/cpp.rs | file | 8.71 | extract/cpp.rs |
| 5 | src/indexer/extract/python.rs | file | 8.41 | extract/python.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | from_env | function | 37.4 | src/config.rs |
| 2 | impl Config | impl | 33.86 | src/config.rs |
| 3 | load | function | 33.11 | src/config.rs |
| 4 | required_env | function | 31.11 | src/config.rs |
| 5 | from_env_requires_base_dir | function | 0.34 | src/config.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/mod.rs | file | 15.82 | pipeline/mod.rs |
| 2 | src/indexer/extract/cpp.rs | file | 14.96 | extract/cpp.rs |
| 3 | src/indexer/pipeline/parallel.rs | file | 14.96 | pipeline/parallel.rs |
| 4 | src/indexer/pipeline/scan.rs | file | 14.36 | pipeline/scan.rs |
| 5 | impl IndexPipeline | impl | 13.7 | pipeline/mod.rs |

### Q6: "How does the MCP server handle incoming tool requests?"
**Expected:** server/mod.rs, handlers/mod.rs, tool dispatch/routing logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | dispatch_tool_call | function | 21.16 | server/mod.rs |
| 2 | handle_list_tools_request | function | 19.4 | server/standalone.rs |
| 3 | src/server/mod.rs | file | 19.4 | server/mod.rs |
| 4 | handle_call_tool_request | function | 19.22 | server/standalone.rs |
| 5 | handle_call_tool_request | function | 18.42 | server/mod.rs |

### Q7: "How does the WebSocket handler work?"
**Expected:** WebSocket-related handler code, connection management

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_pattern_details websocket_handler | function | 14.6 | extract/elysia.rs |
| 2 | fmt websocket_handler | function | 13.64 | extract/symbol.rs |
| 3 | spawn | function | 13.38 | src/web_ui.rs |
| 4 | extract_plugin_name | function | 9.13 | extract/elysia.rs |
| 5 | framework_tags_make_websocket_handler_searchable | function | 0.14 | storage/tantivy.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/storage/sqlite/schema.rs | file | 692.94 | sqlite/schema.rs |
| 2 | TodoRow | struct | 360.18 | sqlite/schema.rs |
| 3 | open_or_create_table | function | 2.96 | storage/vector.rs |
| 4 | impl SqliteStore | impl | 2.28 | sqlite/operations.rs |
| 5 | setup_test_db | function | 0.06 | queries/descriptions.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | tool_internal_error | function | 31.89 | handlers/mod.rs |
| 2 | PathError | enum | 22.22 | path/mod.rs |
| 3 | impl std::error::Error for PathError | impl | 21.1 | path/mod.rs |
| 4 | impl fmt::Display for PathError | impl | 12.13 | path/mod.rs |
| 5 | index_file_with_retry | function | 4.44 | pipeline/parallel.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | SearchResponse | struct | 10.85 | retrieval/mod.rs |
| 2 | RankedHit | struct | 8.16 | retrieval/mod.rs |
| 3 | handle_search_framework_patterns | function | 8.16 | handlers/mod.rs |
| 4 | cache_insert_response | function | 8.15 | retrieval/mod.rs |
| 5 | handle_search_decorators | function | 7.15 | handlers/mod.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | index_files_parallel_async | function | 11.87 | pipeline/mod.rs |
| 2 | index_files_parallel | function | 9.77 | pipeline/parallel.rs |
| 3 | spawn_watch_loop | function | 9.77 | pipeline/mod.rs |
| 4 | src/indexer/pipeline/parallel.rs | file | 8.63 | pipeline/parallel.rs |
| 5 | generate_embeddings_for_parallel_indexed_files | function | 7.63 | pipeline/mod.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | Cache | struct | 15.81 | reranker/cache.rs |
| 2 | stats | function | 13.46 | storage/cache.rs |
| 3 | CacheStats | struct | 13.25 | storage/cache.rs |
| 4 | new | function | 11.88 | reranker/cache.rs |
| 5 | cache_key_differs_by_query | function | 0.13 | reranker/cache.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathNormalizer | struct | 23.12 | path/mod.rs |
| 2 | impl PathNormalizer | impl | 13.4 | path/mod.rs |
| 3 | is_definition_kind | function | 11.55 | ranking/diversify.rs |
| 4 | src/path/mod.rs | file | 10.26 | path/mod.rs |
| 5 | normalize_for_compare | function | 9.87 | path/mod.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 18.69 | storage/cache.rs |
| 2 | get | function | 18.1 | storage/cache.rs |
| 3 | EmbeddingCacheEntry | struct | 13.59 | queries/cache.rs |
| 4 | put_cached_embedding | function | 13.59 | sqlite/mod.rs |
| 5 | put_cached_embedding | function | 12.76 | queries/cache.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | spawn_watch_loop | function | 13.85 | pipeline/mod.rs |
| 2 | create_watcher | function | 13.0 | pipeline/watch.rs |
| 3 | index_files | function | 13.0 | pipeline/mod.rs |
| 4 | src/indexer/pipeline/watch.rs | file | 12.97 | pipeline/watch.rs |
| 5 | spawn | function | 10.4 | src/web_ui.rs |

## Scoring Template

Score each query's CI results on a 1-10 scale:
- 9-10: Every result directly answers the query, spans relevant files
- 7-8: Most results relevant, good diversity, top 3-5 strong
- 5-6: ~Half relevant, some gaps, core code present but buried
- 3-4: Few relevant, dominated by 1-2 files, test/re-export noise
- 1-2: Mostly irrelevant, core implementation missing

| # | Query | CI Score | Pattern |
|---|-------|----------|---------|
| 1 | How does the ranking and scoring system work? | 7 | Good ranking coverage, test at #5 crushed |
| 2 | How are embeddings generated and stored? | 7 | Stable |
| 3 | How does tree-sitter parsing work in this codebase | 7 | parser.rs #1, diverse extractors (+1) |
| 4 | Configuration from environment variables | 9 | All config.rs, test crushed at #5 |
| 5 | Indexing pipeline file scanning and symbol extract | 7 | Stable |
| 6 | How does the MCP server handle incoming tool reque | 9 | Stable |
| 7 | How does the WebSocket handler work? | 6 | Test crushed #3→#5 (+1) |
| 8 | SQLite database schema tables initialization | 7 | Stable |
| 9 | Error handling and graceful degradation | 6 | Stable |
| 10 | JSON serialization and response formatting | 6 | Stable |
| 11 | Async concurrency and parallel processing | 8 | Stable |
| 12 | Caching and cache invalidation | 7 | Stable |
| 13 | PathNormalizer struct definition and methods | 7 | path/mod.rs + normalize_for_compare replace noise (+1) |
| 14 | EmbeddingCache get put cached embedding | 8 | Stable |
| 15 | File watcher debounce reindex on change | 7 | Stable |

**CI Average:** 7.20
# Round 59 - Raw Results

Generated: 2026-02-13 23:02
Symbols indexed: 1701
Total time: 3s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | reciprocal_rank_fusion | function | 13.14 | ranking/rrf.rs |
| 2 | should_rerank | function | 13.12 | ranking/reranker.rs |
| 3 | src/retrieval/ranking/mod.rs | file | 10.67 | ranking/mod.rs |
| 4 | apply_popularity_boost_with_signals | function | 10.41 | ranking/score.rs |
| 5 | test_rrf_respects_weights | function | 0.13 | ranking/rrf.rs |

### Q2: "How are embeddings generated and stored?"
**Expected:** storage/vector.rs, embedding backend files, storage/ layer

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | generate_embeddings_for_parallel_indexed_files | function | 13.4 | pipeline/mod.rs |
| 2 | impl Embedder for FastEmbedder | impl | 11.26 | embeddings/fastembed.rs |
| 3 | src/embeddings/mod.rs | file | 11.26 | embeddings/mod.rs |
| 4 | src/embeddings/fastembed.rs | file | 11.04 | embeddings/fastembed.rs |
| 5 | Config | struct | 10.55 | src/config.rs |

### Q3: "How does tree-sitter parsing work in this codebase?"
**Expected:** indexer/parser.rs, indexer/extract/ language extractors

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | name_from_declarator | function | 8.96 | extract/c.rs |
| 2 | extract_symbols_with_parser | function | 7.24 | extract/c.rs |
| 3 | parse_type_relations | function | 6.53 | pipeline/parsing.rs |
| 4 | src/indexer/parser.rs | file | 6.53 | indexer/parser.rs |
| 5 | extract_symbols_with_parser | function | 5.91 | extract/rust.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | from_env | function | 37.4 | src/config.rs |
| 2 | from_env_requires_base_dir | function | 33.86 | src/config.rs |
| 3 | impl Config | impl | 33.86 | src/config.rs |
| 4 | load | function | 33.11 | src/config.rs |
| 5 | required_env | function | 31.11 | src/config.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/mod.rs | file | 15.82 | pipeline/mod.rs |
| 2 | src/indexer/extract/cpp.rs | file | 14.96 | extract/cpp.rs |
| 3 | src/indexer/pipeline/parallel.rs | file | 14.96 | pipeline/parallel.rs |
| 4 | src/indexer/pipeline/scan.rs | file | 14.36 | pipeline/scan.rs |
| 5 | impl IndexPipeline | impl | 13.69 | pipeline/mod.rs |

### Q6: "How does the MCP server handle incoming tool requests?"
**Expected:** server/mod.rs, handlers/mod.rs, tool dispatch/routing logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | dispatch_tool_call | function | 21.13 | server/mod.rs |
| 2 | handle_list_tools_request | function | 19.38 | server/standalone.rs |
| 3 | src/server/mod.rs | file | 19.38 | server/mod.rs |
| 4 | handle_call_tool_request | function | 19.22 | server/standalone.rs |
| 5 | handle_call_tool_request | function | 18.38 | server/mod.rs |

### Q7: "How does the WebSocket handler work?"
**Expected:** WebSocket-related handler code, connection management

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_pattern_details websocket_handler | function | 14.63 | extract/elysia.rs |
| 2 | fmt websocket_handler | function | 13.64 | extract/symbol.rs |
| 3 | framework_tags_make_websocket_handler_searchable | function | 13.64 | storage/tantivy.rs |
| 4 | spawn | function | 13.43 | src/web_ui.rs |
| 5 | extract_plugin_name | function | 9.16 | extract/elysia.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/storage/sqlite/schema.rs | file | 692.94 | sqlite/schema.rs |
| 2 | TodoRow | struct | 360.17 | sqlite/schema.rs |
| 3 | open_or_create_table | function | 2.96 | storage/vector.rs |
| 4 | impl SqliteStore | impl | 2.28 | sqlite/operations.rs |
| 5 | setup_test_db | function | 0.06 | queries/descriptions.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | tool_internal_error | function | 31.89 | handlers/mod.rs |
| 2 | PathError | enum | 22.24 | path/mod.rs |
| 3 | impl std::error::Error for PathError | impl | 21.03 | path/mod.rs |
| 4 | impl fmt::Display for PathError | impl | 12.13 | path/mod.rs |
| 5 | index_file_with_retry | function | 4.45 | pipeline/parallel.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | SearchResponse | struct | 10.83 | retrieval/mod.rs |
| 2 | RankedHit | struct | 8.15 | retrieval/mod.rs |
| 3 | handle_search_framework_patterns | function | 8.15 | handlers/mod.rs |
| 4 | cache_insert_response | function | 8.13 | retrieval/mod.rs |
| 5 | src/retrieval/fast_paths.rs | file | 7.77 | retrieval/fast_paths.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | index_files_parallel_async | function | 11.88 | pipeline/mod.rs |
| 2 | index_files_parallel | function | 9.78 | pipeline/parallel.rs |
| 3 | spawn_watch_loop | function | 9.78 | pipeline/mod.rs |
| 4 | src/indexer/pipeline/parallel.rs | file | 8.61 | pipeline/parallel.rs |
| 5 | generate_embeddings_for_parallel_indexed_files | function | 7.66 | pipeline/mod.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | Cache | struct | 16.01 | reranker/cache.rs |
| 2 | CacheStats | struct | 13.29 | storage/cache.rs |
| 3 | stats | function | 13.29 | storage/cache.rs |
| 4 | EmbeddingCache | struct | 12.86 | storage/cache.rs |
| 5 | CacheStats | struct | 11.8 | queries/cache.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathNormalizer | struct | 23.12 | path/mod.rs |
| 2 | impl PathNormalizer | impl | 13.4 | path/mod.rs |
| 3 | is_definition_kind | function | 11.47 | ranking/diversify.rs |
| 4 | normalize_for_compare | function | 9.79 | path/mod.rs |
| 5 | GetDefinitionTool | struct | 8.27 | tools/mod.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 18.69 | storage/cache.rs |
| 2 | get | function | 18.09 | storage/cache.rs |
| 3 | EmbeddingCacheEntry | struct | 13.62 | queries/cache.rs |
| 4 | put_cached_embedding | function | 13.62 | sqlite/mod.rs |
| 5 | put_cached_embedding | function | 12.79 | queries/cache.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | spawn_watch_loop | function | 13.85 | pipeline/mod.rs |
| 2 | create_watcher | function | 13.03 | pipeline/watch.rs |
| 3 | index_files | function | 13.03 | pipeline/mod.rs |
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
| 1 | How does the ranking and scoring system work? | 7 | 4/5 ranking/*, test at #5 |
| 2 | How are embeddings generated and stored? | 7 | repo_name eliminated, 4/5 embeddings (+2 vs R58) |
| 3 | How does tree-sitter parsing work in this codebase | 6 | All 5 tree-sitter relevant, 4 files (+1 vs R58) |
| 4 | Configuration from environment variables | 8 | All 5 config.rs |
| 5 | Indexing pipeline file scanning and symbol extract | 7 | All pipeline/* |
| 6 | How does the MCP server handle incoming tool reque | 9 | All 5 server/handler, perfect (+1 vs R58) |
| 7 | How does the WebSocket handler work? | 5 | Test fn at #3, extract_plugin_name noise at #5 |
| 8 | SQLite database schema tables initialization | 7 | schema.rs dominant, test at #5 |
| 9 | Error handling and graceful degradation | 5 | PathError heavy, limited diversity |
| 10 | JSON serialization and response formatting | 6 | All relevant: response structs + formatting |
| 11 | Async concurrency and parallel processing | 8 | All 5 async/parallel |
| 12 | Caching and cache invalidation | 8 | All cache related |
| 13 | PathNormalizer struct definition and methods | 6 | #1/#2/#4 from path/mod.rs, definition noise #3/#5 |
| 14 | EmbeddingCache get put cached embedding | 8 | All 5 cache/embedding |
| 15 | File watcher debounce reindex on change | 7 | 4/5 watcher/pipeline |

**CI Average:** 6.93 (+0.07 vs R58 6.87)
# Round 66 - Raw Results

Generated: 2026-02-14 22:57
Symbols indexed: 1729
Total time: 6s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | apply_reranker_scores | function | 14.71 | ranking/reranker.rs |
| 2 | should_rerank | function | 14.0 | ranking/reranker.rs |
| 3 | reciprocal_rank_fusion | function | 13.93 | ranking/rrf.rs |
| 4 | src/retrieval/ranking/rrf.rs | file | 12.39 | ranking/rrf.rs |
| 5 | test_rrf_respects_weights | function | 0.14 | ranking/rrf.rs |

### Q2: "How are embeddings generated and stored?"
**Expected:** storage/vector.rs, embedding backend files, storage/ layer

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | generate_embeddings_for_parallel_indexed_files | function | 13.75 | pipeline/mod.rs |
| 2 | embed_and_build_vector_records | function | 12.9 | pipeline/mod.rs |
| 3 | impl Embedder for FastEmbedder | impl | 12.26 | embeddings/fastembed.rs |
| 4 | src/embeddings/mod.rs | file | 12.26 | embeddings/mod.rs |
| 5 | src/embeddings/fastembed.rs | file | 11.74 | embeddings/fastembed.rs |

### Q3: "How does tree-sitter parsing work in this codebase?"
**Expected:** indexer/parser.rs, indexer/extract/ language extractors

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | parse_type_relations | function | 13.05 | pipeline/parsing.rs |
| 2 | src/indexer/parser.rs | file | 12.52 | indexer/parser.rs |
| 3 | walk | function | 12.52 | extract/python.rs |
| 4 | src/indexer/extract/c.rs | file | 10.44 | extract/c.rs |
| 5 | parse_next_identifier | function | 9.37 | pipeline/parsing.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | required_env | function | 30.59 | src/config.rs |
| 2 | optional_env | function | 30.16 | src/config.rs |
| 3 | from_env | function | 28.32 | src/config.rs |
| 4 | load | function | 23.49 | src/config.rs |
| 5 | from_env_requires_base_dir | function | 0.3 | src/config.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/mod.rs | file | 15.72 | pipeline/mod.rs |
| 2 | src/indexer/extract/cpp.rs | file | 15.59 | extract/cpp.rs |
| 3 | src/indexer/pipeline/scan.rs | file | 15.59 | pipeline/scan.rs |
| 4 | src/indexer/pipeline/parallel.rs | file | 14.98 | pipeline/parallel.rs |
| 5 | src/indexer/extract/c.rs | file | 11.69 | extract/c.rs |

### Q6: "How does the MCP server handle incoming tool requests?"
**Expected:** server/mod.rs, handlers/mod.rs, tool dispatch/routing logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | dispatch_tool_call | function | 22.21 | server/mod.rs |
| 2 | handle_call_tool_request | function | 19.43 | server/mod.rs |
| 3 | handle_call_tool_request | function | 19.22 | server/standalone.rs |
| 4 | src/server/mod.rs | file | 19.22 | server/mod.rs |
| 5 | handle_list_tools_request | function | 19.21 | server/standalone.rs |

### Q7: "How does the WebSocket handler work?"
**Expected:** WebSocket-related handler code, connection management

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_pattern_details websocket_handler | function | 16.33 | extract/elysia.rs |
| 2 | fmt websocket_handler | function | 15.1 | extract/symbol.rs |
| 3 | spawn | function | 11.69 | src/web_ui.rs |
| 4 | extract_plugin_name | function | 10.22 | extract/elysia.rs |
| 5 | framework_tags_make_websocket_handler_searchable | function | 0.15 | storage/tantivy.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/storage/sqlite/schema.rs | file | 1000.39 | sqlite/schema.rs |
| 2 | TodoRow | struct | 424.9 | sqlite/schema.rs |
| 3 | impl SqliteStore | impl | 7.06 | sqlite/operations.rs |
| 4 | migrate_add_edges_resolution_columns | function | 7.06 | sqlite/operations.rs |
| 5 | init | function | 3.46 | sqlite/operations.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | tool_internal_error | function | 33.85 | handlers/mod.rs |
| 2 | error | function | 27.36 | src/logging.rs |
| 3 | PathError | enum | 23.4 | path/mod.rs |
| 4 | impl std::error::Error for PathError | impl | 21.13 | path/mod.rs |
| 5 | impl Drop for LeaderElection | impl | 5.47 | src/leader.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | SearchResponse | struct | 11.56 | retrieval/mod.rs |
| 2 | SearchResponseWithSignals | struct | 9.77 | retrieval/mod.rs |
| 3 | RankedHit | struct | 9.03 | retrieval/mod.rs |
| 4 | handle_search_framework_patterns | function | 9.03 | handlers/mod.rs |
| 5 | cache_insert_response | function | 8.68 | retrieval/mod.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | index_files_parallel_async | function | 12.92 | pipeline/mod.rs |
| 2 | index_all | function | 9.82 | pipeline/mod.rs |
| 3 | index_files_parallel | function | 9.82 | pipeline/parallel.rs |
| 4 | src/indexer/pipeline/parallel.rs | file | 8.6 | pipeline/parallel.rs |
| 5 | generate_embeddings_for_parallel_indexed_files | function | 8.31 | pipeline/mod.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | stats | function | 14.06 | storage/cache.rs |
| 2 | CacheStats | struct | 13.73 | storage/cache.rs |
| 3 | EmbeddingCache | struct | 13.35 | storage/cache.rs |
| 4 | CacheStats | struct | 12.42 | queries/cache.rs |
| 5 | cache_key_differs_by_query | function | 0.12 | reranker/cache.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathNormalizer | struct | 23.47 | path/mod.rs |
| 2 | impl PathNormalizer | impl | 15.35 | path/mod.rs |
| 3 | is_definition_kind | function | 11.04 | ranking/diversify.rs |
| 4 | src/path/mod.rs | file | 10.68 | path/mod.rs |
| 5 | normalize_for_compare | function | 10.3 | path/mod.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | EmbeddingCache | struct | 16.32 | storage/cache.rs |
| 2 | EmbeddingCacheEntry | struct | 13.81 | queries/cache.rs |
| 3 | put_cached_embedding | function | 13.81 | sqlite/mod.rs |
| 4 | put | function | 13.73 | storage/cache.rs |
| 5 | put_cached_embedding | function | 12.97 | queries/cache.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | spawn_watch_loop | function | 13.44 | pipeline/mod.rs |
| 2 | create_watcher | function | 13.27 | pipeline/watch.rs |
| 3 | index_files | function | 13.27 | pipeline/mod.rs |
| 4 | src/indexer/pipeline/watch.rs | file | 12.93 | pipeline/watch.rs |
| 5 | handle_refresh_index | function | 9.63 | handlers/mod.rs |

## Scoring Template

Score each query's CI results on a 1-10 scale:
- 9-10: Every result directly answers the query, spans relevant files
- 7-8: Most results relevant, good diversity, top 3-5 strong
- 5-6: ~Half relevant, some gaps, core code present but buried
- 3-4: Few relevant, dominated by 1-2 files, test/re-export noise
- 1-2: Mostly irrelevant, core implementation missing

| # | Query | CI Score | Pattern |
|---|-------|----------|---------|
| 1 | How does the ranking and scoring system work? | ___ | |
| 2 | How are embeddings generated and stored? | ___ | |
| 3 | How does tree-sitter parsing work in this codebase | ___ | |
| 4 | Configuration from environment variables | ___ | |
| 5 | Indexing pipeline file scanning and symbol extract | ___ | |
| 6 | How does the MCP server handle incoming tool reque | ___ | |
| 7 | How does the WebSocket handler work? | ___ | |
| 8 | SQLite database schema tables initialization | ___ | |
| 9 | Error handling and graceful degradation | ___ | |
| 10 | JSON serialization and response formatting | ___ | |
| 11 | Async concurrency and parallel processing | ___ | |
| 12 | Caching and cache invalidation | ___ | |
| 13 | PathNormalizer struct definition and methods | ___ | |
| 14 | EmbeddingCache get put cached embedding | ___ | |
| 15 | File watcher debounce reindex on change | ___ | |

**CI Average:** ___
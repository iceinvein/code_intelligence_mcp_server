# Round 57 - Raw Results

Generated: 2026-02-13 14:04
Symbols indexed: 1682
Total time: 8s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | reciprocal_rank_fusion | function | 11.43 | ranking/rrf.rs |
| 2 | src/retrieval/ranking/mod.rs | file | 11.14 | ranking/mod.rs |
| 3 | apply_reranker_scores | function | 11.11 | ranking/reranker.rs |
| 4 | edge_resolution_rank | function | 9.04 | queries/edges.rs |
| 5 | test_rrf_respects_weights | function | 0.56 | ranking/rrf.rs |

### Q2: "How are embeddings generated and stored?"
**Expected:** storage/vector.rs, embedding backend files, storage/ layer

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | repo_name | function | 14.04 | pipeline/mod.rs |
| 2 | generate_embeddings_for_parallel_indexed_files | function | 13.03 | pipeline/mod.rs |
| 3 | impl Embedder for FastEmbedder | impl | 11.26 | embeddings/fastembed.rs |
| 4 | src/embeddings/mod.rs | file | 11.26 | embeddings/mod.rs |
| 5 | src/embeddings/fastembed.rs | file | 10.92 | embeddings/fastembed.rs |

### Q3: "How does tree-sitter parsing work in this codebase?"
**Expected:** indexer/parser.rs, indexer/extract/ language extractors

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | name_from_declarator | function | 13.5 | extract/c.rs |
| 2 | name_from_declarator | function | 11.02 | extract/cpp.rs |
| 3 | walk | function | 11.02 | extract/javascript.rs |
| 4 | find_chain_root | function | 10.06 | extract/elysia.rs |
| 5 | walk | function | 9.78 | extract/python.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | to_utf8_pathbuf | function | 42.86 | src/config.rs |
| 2 | canonicalize_dir | function | 35.26 | src/config.rs |
| 3 | required_env | function | 35.26 | src/config.rs |
| 4 | from_env | function | 34.58 | src/config.rs |
| 5 | impl Config | impl | 33.81 | src/config.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/mod.rs | file | 15.76 | pipeline/mod.rs |
| 2 | src/indexer/extract/cpp.rs | file | 14.91 | extract/cpp.rs |
| 3 | src/indexer/pipeline/parallel.rs | file | 14.91 | pipeline/parallel.rs |
| 4 | src/indexer/pipeline/scan.rs | file | 14.36 | pipeline/scan.rs |
| 5 | impl IndexPipeline | impl | 13.52 | pipeline/mod.rs |

### Q6: "How does the MCP server handle incoming tool requests?"
**Expected:** server/mod.rs, handlers/mod.rs, tool dispatch/routing logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | dispatch_tool_call | function | 19.63 | server/mod.rs |
| 2 | handle_list_tools_request | function | 19.47 | server/standalone.rs |
| 3 | src/server/mod.rs | file | 19.47 | server/mod.rs |
| 4 | handle_call_tool_request | function | 19.32 | server/standalone.rs |
| 5 | handle_call_tool_request | function | 19.13 | server/mod.rs |

### Q7: "How does the WebSocket handler work?"
**Expected:** WebSocket-related handler code, connection management

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_pattern_details websocket_handler | function | 14.19 | extract/elysia.rs |
| 2 | fmt websocket_handler | function | 14.07 | extract/symbol.rs |
| 3 | framework_tags_make_websocket_handler_searchable | function | 14.07 | storage/tantivy.rs |
| 4 | extract_plugin_name | function | 13.32 | extract/elysia.rs |
| 5 | truncate_text | function | 13.32 | extract/elysia.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/storage/sqlite/schema.rs | file | 705.2 | sqlite/schema.rs |
| 2 | TodoRow | struct | 288.62 | sqlite/schema.rs |
| 3 | src/storage/sqlite/queries/todos.rs | file | 106.98 | queries/todos.rs |
| 4 | search_todos | function | 94.18 | queries/todos.rs |
| 5 | setup_test_db | function | 10.7 | queries/descriptions.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | tool_internal_error | function | 30.09 | handlers/mod.rs |
| 2 | impl std::error::Error for PathError | impl | 23.48 | path/mod.rs |
| 3 | PathError | enum | 20.27 | path/mod.rs |
| 4 | index_file_with_retry | function | 4.7 | pipeline/parallel.rs |
| 5 | fmt | function | 3.64 | path/mod.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | handle_search_framework_patterns | function | 6.43 | handlers/mod.rs |
| 2 | RankedHit | struct | 6.2 | retrieval/mod.rs |
| 3 | SearchResponse | struct | 6.2 | retrieval/mod.rs |
| 4 | handle_search_decorators | function | 5.51 | handlers/mod.rs |
| 5 | format_framework_patterns | function | 5.31 | handlers/mod.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | index_files_parallel_async | function | 11.28 | pipeline/mod.rs |
| 2 | index_files_parallel | function | 8.8 | pipeline/parallel.rs |
| 3 | spawn_watch_loop | function | 8.8 | pipeline/mod.rs |
| 4 | src/indexer/pipeline/parallel.rs | file | 8.64 | pipeline/parallel.rs |
| 5 | generate_embeddings_for_parallel_indexed_files | function | 7.0 | pipeline/mod.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | Cache | struct | 16.19 | reranker/cache.rs |
| 2 | new | function | 12.15 | reranker/cache.rs |
| 3 | EmbeddingCache | struct | 11.09 | storage/cache.rs |
| 4 | stats | function | 11.09 | storage/cache.rs |
| 5 | CacheStats | struct | 10.99 | queries/cache.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathNormalizer | struct | 21.76 | path/mod.rs |
| 2 | tests | module | 18.06 | llm/mod.rs |
| 3 | impl PathNormalizer | impl | 14.01 | path/mod.rs |
| 4 | is_definition_kind | function | 9.57 | ranking/diversify.rs |
| 5 | normalize_for_compare | function | 8.99 | path/mod.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 17.01 | storage/cache.rs |
| 2 | get | function | 15.98 | storage/cache.rs |
| 3 | get_cached_embedding | function | 15.98 | sqlite/mod.rs |
| 4 | cache_key | function | 15.96 | storage/cache.rs |
| 5 | content_hash | function | 15.96 | storage/cache.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/watch.rs | file | 13.54 | pipeline/watch.rs |
| 2 | spawn_watch_loop | function | 12.67 | pipeline/mod.rs |
| 3 | index_files | function | 12.67 | pipeline/mod.rs |
| 4 | create_watcher | function | 12.53 | pipeline/watch.rs |
| 5 | src/indexer/pipeline/mod.rs | file | 10.58 | pipeline/mod.rs |

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
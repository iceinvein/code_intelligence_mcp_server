# Round 58 - Raw Results

Generated: 2026-02-13 14:22
Symbols indexed: 1682
Total time: 3s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | should_rerank | function | 11.81 | ranking/reranker.rs |
| 2 | reciprocal_rank_fusion | function | 11.71 | ranking/rrf.rs |
| 3 | src/retrieval/ranking/mod.rs | file | 10.59 | ranking/mod.rs |
| 4 | apply_popularity_boost_with_signals | function | 9.13 | ranking/score.rs |
| 5 | test_rrf_respects_weights | function | 0.59 | ranking/rrf.rs |

### Q2: "How are embeddings generated and stored?"
**Expected:** storage/vector.rs, embedding backend files, storage/ layer

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | repo_name | function | 14.39 | pipeline/mod.rs |
| 2 | impl Embedder for FastEmbedder | impl | 11.26 | embeddings/fastembed.rs |
| 3 | src/embeddings/mod.rs | file | 11.26 | embeddings/mod.rs |
| 4 | src/embeddings/fastembed.rs | file | 10.94 | embeddings/fastembed.rs |
| 5 | Config | struct | 10.36 | src/config.rs |

### Q3: "How does tree-sitter parsing work in this codebase?"
**Expected:** indexer/parser.rs, indexer/extract/ language extractors

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | name_from_declarator | function | 13.81 | extract/c.rs |
| 2 | find_chain_root | function | 10.41 | extract/elysia.rs |
| 3 | src/indexer/parser.rs | file | 10.41 | indexer/parser.rs |
| 4 | walk | function | 10.07 | extract/python.rs |
| 5 | walk | function | 9.74 | extract/javascript.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | from_env | function | 34.71 | src/config.rs |
| 2 | from_env_requires_base_dir | function | 33.83 | src/config.rs |
| 3 | impl Config | impl | 33.83 | src/config.rs |
| 4 | required_env | function | 31.11 | src/config.rs |
| 5 | load | function | 30.37 | src/config.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/mod.rs | file | 15.88 | pipeline/mod.rs |
| 2 | src/indexer/extract/cpp.rs | file | 15.01 | extract/cpp.rs |
| 3 | src/indexer/pipeline/parallel.rs | file | 15.01 | pipeline/parallel.rs |
| 4 | src/indexer/pipeline/scan.rs | file | 14.36 | pipeline/scan.rs |
| 5 | impl IndexPipeline | impl | 13.71 | pipeline/mod.rs |

### Q6: "How does the MCP server handle incoming tool requests?"
**Expected:** server/mod.rs, handlers/mod.rs, tool dispatch/routing logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | dispatch_tool_call | function | 19.53 | server/mod.rs |
| 2 | handle_list_tools_request | function | 19.34 | server/standalone.rs |
| 3 | src/server/mod.rs | file | 19.34 | server/mod.rs |
| 4 | handle_call_tool_request | function | 19.21 | server/standalone.rs |
| 5 | handle_call_tool_request | function | 18.46 | server/mod.rs |

### Q7: "How does the WebSocket handler work?"
**Expected:** WebSocket-related handler code, connection management

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_pattern_details websocket_handler | function | 14.73 | extract/elysia.rs |
| 2 | fmt websocket_handler | function | 13.7 | extract/symbol.rs |
| 3 | framework_tags_make_websocket_handler_searchable | function | 13.7 | storage/tantivy.rs |
| 4 | spawn | function | 12.18 | src/web_ui.rs |
| 5 | truncate_text | function | 9.22 | extract/elysia.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/storage/sqlite/schema.rs | file | 690.53 | sqlite/schema.rs |
| 2 | TodoRow | struct | 291.55 | sqlite/schema.rs |
| 3 | open_or_create_table | function | 2.75 | storage/vector.rs |
| 4 | impl SqliteStore | impl | 2.27 | sqlite/operations.rs |
| 5 | setup_test_db | function | 0.27 | queries/descriptions.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | tool_internal_error | function | 29.64 | handlers/mod.rs |
| 2 | impl std::error::Error for PathError | impl | 21.03 | path/mod.rs |
| 3 | PathError | enum | 19.93 | path/mod.rs |
| 4 | impl fmt::Display for PathError | impl | 12.27 | path/mod.rs |
| 5 | index_file_with_retry | function | 4.21 | pipeline/parallel.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | SearchResponse | struct | 6.58 | retrieval/mod.rs |
| 2 | RankedHit | struct | 6.54 | retrieval/mod.rs |
| 3 | handle_search_framework_patterns | function | 6.54 | handlers/mod.rs |
| 4 | handle_search_decorators | function | 5.61 | handlers/mod.rs |
| 5 | format_framework_patterns | function | 5.29 | handlers/mod.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | index_files_parallel_async | function | 12.2 | pipeline/mod.rs |
| 2 | index_files_parallel | function | 8.87 | pipeline/parallel.rs |
| 3 | spawn_watch_loop | function | 8.87 | pipeline/mod.rs |
| 4 | src/indexer/pipeline/parallel.rs | file | 8.65 | pipeline/parallel.rs |
| 5 | generate_embeddings_for_parallel_indexed_files | function | 7.73 | pipeline/mod.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | Cache | struct | 15.77 | reranker/cache.rs |
| 2 | stats | function | 12.55 | storage/cache.rs |
| 3 | cache_key_differs_by_query | function | 12.55 | reranker/cache.rs |
| 4 | new | function | 11.85 | reranker/cache.rs |
| 5 | CacheStats | struct | 11.67 | storage/cache.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathNormalizer | struct | 21.77 | path/mod.rs |
| 2 | impl PathNormalizer | impl | 13.39 | path/mod.rs |
| 3 | tests | module | 12.04 | llm/mod.rs |
| 4 | is_definition_kind | function | 10.62 | ranking/diversify.rs |
| 5 | normalize_for_compare | function | 9.0 | path/mod.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 17.78 | storage/cache.rs |
| 2 | get | function | 17.13 | storage/cache.rs |
| 3 | get_cached_embedding | function | 17.13 | sqlite/mod.rs |
| 4 | cache_key | function | 16.69 | storage/cache.rs |
| 5 | content_hash | function | 16.69 | storage/cache.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | spawn_watch_loop | function | 12.95 | pipeline/mod.rs |
| 2 | index_files | function | 12.86 | pipeline/mod.rs |
| 3 | src/indexer/pipeline/watch.rs | file | 12.86 | pipeline/watch.rs |
| 4 | create_watcher | function | 12.04 | pipeline/watch.rs |
| 5 | src/indexer/pipeline/mod.rs | file | 10.24 | pipeline/mod.rs |

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
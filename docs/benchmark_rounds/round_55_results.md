# Round 55 - Raw Results

Generated: 2026-02-13 12:53
Symbols indexed: 1682
Total time: 63s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | term_coverage_adjustment | function | 8.88 | ranking/score.rs |
| 2 | term_coverage_rewards_high_coverage | function | 8.71 | ranking/score.rs |
| 3 | impl std::error::Error for PathError | impl | 6.91 | path/mod.rs |
| 4 | index_packages_and_repositories | function | 6.91 | pipeline/mod.rs |
| 5 | repo_name | function | 6.48 | pipeline/mod.rs |

### Q2: "How are embeddings generated and stored?"
**Expected:** storage/vector.rs, embedding backend files, storage/ layer

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | get_undescribed_symbols | function | 7.72 | queries/descriptions.rs |
| 2 | parse_csv | function | 7.65 | src/config.rs |
| 3 | repo_name | function | 7.65 | pipeline/mod.rs |
| 4 | handle_summarize_file | function | 7.44 | handlers/mod.rs |
| 5 | get | function | 7.15 | storage/cache.rs |

### Q3: "How does tree-sitter parsing work in this codebase?"
**Expected:** indexer/parser.rs, indexer/extract/ language extractors

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | batch_upsert_framework_patterns | function | 10.98 | queries/framework.rs |
| 2 | upsert_framework_pattern | function | 10.3 | queries/framework.rs |
| 3 | insert_search_run | function | 9.81 | queries/stats.rs |
| 4 | embed | function | 9.81 | embeddings/fastembed.rs |
| 5 | delete_docstrings_by_file | function | 9.49 | queries/docstrings.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | test_rrf_with_all_empty | function | 6.25 | ranking/rrf.rs |
| 2 | handle_find_similar_code | function | 5.58 | handlers/mod.rs |
| 3 | get | function | 5.0 | src/registry.rs |
| 4 | embed_text | function | 4.34 | retrieval/mod.rs |
| 5 | upsert_edge_evidence | function | 3.93 | sqlite/mod.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | upsert_symbol | function | 12.0 | storage/tantivy.rs |
| 2 | expand_index_text | function | 11.27 | storage/tantivy.rs |
| 3 | upsert_symbol | function | 10.86 | queries/symbols.rs |
| 4 | upsert_symbol | function | 10.86 | sqlite/mod.rs |
| 5 | symbol_name | function | 10.69 | extract/java.rs |

### Q6: "How does the MCP server handle incoming tool requests?"
**Expected:** server/mod.rs, handlers/mod.rs, tool dispatch/routing logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | all_tools | function | 14.76 | server/mod.rs |
| 2 | open_or_create_table | function | 11.15 | storage/vector.rs |
| 3 | tests/support/mod.rs | file | 11.15 | support/mod.rs |
| 4 | handle_list_tools_request | function | 11.05 | server/standalone.rs |
| 5 | impl LanceDbStore | impl | 10.72 | storage/vector.rs |

### Q7: "How does the WebSocket handler work?"
**Expected:** WebSocket-related handler code, connection management

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_pattern_details websocket_handler | function | 6.05 | extract/elysia.rs |
| 2 | upsert_edge | function | 5.86 | queries/edges.rs |
| 3 | upsert_symbol | function | 5.86 | storage/tantivy.rs |
| 4 | extract_plugin_name | function | 5.68 | extract/elysia.rs |
| 5 | truncate_text | function | 5.68 | extract/elysia.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | path | const | 3.69 | bin/standalone.js |
| 2 | find_inline_comment | function | 1.93 | src/text.rs |
| 3 | extract_identifiers | function | 1.84 | pipeline/parsing.rs |
| 4 | handle_trace_data_flow | function | 1.75 | handlers/mod.rs |
| 5 | upsert_edge_evidence | function | 1.74 | queries/edges.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathError | enum | 16.04 | path/mod.rs |
| 2 | tool_internal_error | function | 12.99 | handlers/mod.rs |
| 3 | generate_openai | function | 12.99 | hyde/generator.rs |
| 4 | fmt | function | 2.88 | path/mod.rs |
| 5 | relative_to_base | function | 2.62 | path/mod.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_concept_tags | function | 9.43 | src/text.rs |
| 2 | has_workspaces | function | 5.02 | parsers/npm.rs |
| 3 | test_parse_manifest_dispatches_correctly | function | 5.02 | parsers/mod.rs |
| 4 | compute_resolution_for_target | function | 4.12 | pipeline/edges.rs |
| 5 | get | function | 4.09 | src/registry.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/reranker/mod.rs | file | 9.79 | reranker/mod.rs |
| 2 | extract_symbols_with_parser | function | 8.41 | extract/rust.rs |
| 3 | index_files_parallel_async | function | 8.41 | pipeline/mod.rs |
| 4 | generate_embeddings_for_parallel_indexed_files | function | 7.89 | pipeline/mod.rs |
| 5 | extract_symbols_with_parser | function | 6.66 | extract/c.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | Cache | struct | 14.51 | reranker/cache.rs |
| 2 | rerank | function | 11.68 | reranker/cache.rs |
| 3 | cache_key | function | 11.67 | storage/cache.rs |
| 4 | test_decompose_query_handles_multiple | function | 11.67 | retrieval/query.rs |
| 5 | LruCache | struct | 11.01 | retrieval/cache.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathNormalizer | struct | 21.76 | path/mod.rs |
| 2 | tests | module | 18.06 | llm/mod.rs |
| 3 | is_definition_kind | function | 10.67 | ranking/diversify.rs |
| 4 | src/tools/mod.rs | file | 8.95 | tools/mod.rs |
| 5 | GetDefinitionTool | struct | 6.94 | tools/mod.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | EmbeddingCache | struct | 14.87 | storage/cache.rs |
| 2 | put | function | 9.94 | storage/cache.rs |
| 3 | test_expand_stems_short_queries_untouched | function | 9.94 | src/text.rs |
| 4 | get | function | 9.41 | storage/cache.rs |
| 5 | content_hash | function | 9.33 | storage/cache.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | spawn_watch_loop | function | 4.78 | pipeline/mod.rs |
| 2 | src/indexer/pipeline/watch.rs | file | 4.56 | pipeline/watch.rs |
| 3 | test_expand_synonyms_watcher_debounce | function | 4.56 | src/text.rs |
| 4 | create_watcher | function | 4.24 | pipeline/watch.rs |
| 5 | impl OrtLlmGenerator | impl | 3.61 | llm/ort.rs |

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
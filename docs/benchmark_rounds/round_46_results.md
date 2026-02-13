# Round 46 - Raw Results

Generated: 2026-02-13 10:12
Symbols indexed: 1677
Total time: 62s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | index_packages_and_repositories | function | 11.18 | pipeline/mod.rs |
| 2 | repo_name | function | 10.49 | pipeline/mod.rs |
| 3 | impl std::error::Error for PathError | impl | 8.88 | path/mod.rs |
| 4 | term_coverage_adjustment | function | 8.88 | ranking/score.rs |
| 5 | detect_repositories | function | 8.39 | package/mod.rs |

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
| 1 | batch_upsert_framework_patterns | function | 10.82 | queries/framework.rs |
| 2 | upsert_framework_pattern | function | 10.15 | queries/framework.rs |
| 3 | insert_search_run | function | 9.97 | queries/stats.rs |
| 4 | embed | function | 9.97 | embeddings/fastembed.rs |
| 5 | delete_docstrings_by_file | function | 9.31 | queries/docstrings.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | test_rrf_with_all_empty | function | 6.25 | ranking/rrf.rs |
| 2 | handle_find_similar_code | function | 5.58 | handlers/mod.rs |
| 3 | get | function | 5.0 | src/registry.rs |
| 4 | embed_text | function | 4.35 | retrieval/mod.rs |
| 5 | upsert_edge_evidence | function | 3.94 | sqlite/mod.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | upsert_symbol | function | 12.0 | storage/tantivy.rs |
| 2 | upsert_symbol | function | 10.89 | queries/symbols.rs |
| 3 | upsert_symbol | function | 10.89 | sqlite/mod.rs |
| 4 | extract_symbols_with_parser | function | 10.17 | extract/c.rs |
| 5 | extract_symbols_with_parser | function | 9.55 | extract/java.rs |

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
| 1 | path | const | 3.81 | bin/standalone.js |
| 2 | is_test_file | function | 1.91 | ranking/score.rs |
| 3 | extract_identifiers | function | 1.91 | pipeline/parsing.rs |
| 4 | handle_trace_data_flow | function | 1.79 | handlers/mod.rs |
| 5 | upsert_edge_evidence | function | 1.78 | queries/edges.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | handle_summarize_file | function | 8.72 | handlers/mod.rs |
| 2 | extract_signature_for_summary | function | 8.18 | handlers/mod.rs |
| 3 | infer_file_purpose_for_summary | function | 8.18 | handlers/mod.rs |
| 4 | PathError | enum | 6.98 | path/mod.rs |
| 5 | generate_openai | function | 6.98 | hyde/generator.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_concept_tags | function | 9.39 | src/text.rs |
| 2 | has_workspaces | function | 4.92 | parsers/npm.rs |
| 3 | test_parse_manifest_dispatches_correctly | function | 4.92 | parsers/mod.rs |
| 4 | compute_resolution_for_target | function | 4.14 | pipeline/edges.rs |
| 5 | get | function | 4.01 | src/registry.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/reranker/mod.rs | file | 9.79 | reranker/mod.rs |
| 2 | find_model_file | function | 8.47 | llm/ort.rs |
| 3 | index_files_parallel_async | function | 8.47 | pipeline/mod.rs |
| 4 | generate_embeddings_for_parallel_indexed_files | function | 7.95 | pipeline/mod.rs |
| 5 | extract_symbols_with_parser | function | 7.52 | extract/c.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | Cache | struct | 14.27 | reranker/cache.rs |
| 2 | rerank | function | 11.26 | reranker/cache.rs |
| 3 | cache_key | function | 10.93 | storage/cache.rs |
| 4 | test_decompose_query_handles_multiple | function | 10.93 | retrieval/query.rs |
| 5 | new | function | 10.72 | reranker/cache.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathNormalizer | struct | 21.76 | path/mod.rs |
| 2 | tests | module | 18.07 | llm/mod.rs |
| 3 | contains_code_snippet | function | 12.18 | retrieval/query.rs |
| 4 | is_definition_kind | function | 10.67 | ranking/diversify.rs |
| 5 | src/tools/mod.rs | file | 8.97 | tools/mod.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | EmbeddingCache | struct | 14.78 | storage/cache.rs |
| 2 | put | function | 9.96 | storage/cache.rs |
| 3 | test_expand_stems_short_queries_untouched | function | 9.96 | src/text.rs |
| 4 | get | function | 9.39 | storage/cache.rs |
| 5 | content_hash | function | 9.35 | storage/cache.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | spawn_watch_loop | function | 4.74 | pipeline/mod.rs |
| 2 | src/indexer/pipeline/watch.rs | file | 4.51 | pipeline/watch.rs |
| 3 | test_expand_synonyms_watcher_debounce | function | 4.51 | src/text.rs |
| 4 | create_watcher | function | 4.17 | pipeline/watch.rs |
| 5 | impl OrtLlmGenerator | impl | 3.59 | llm/ort.rs |

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
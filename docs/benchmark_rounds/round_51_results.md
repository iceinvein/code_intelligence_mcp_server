# Round 51 - Raw Results

Generated: 2026-02-13 12:24
Symbols indexed: 1682
Total time: 7s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | definition_bias | function | 18.09 | ranking/score.rs |
| 2 | structural_adjustment | function | 13.52 | ranking/score.rs |
| 3 | test_rank_lines_by_relevance_basic | function | 13.52 | assembler/formatting.rs |
| 4 | apply_docstring_boost_with_signals | function | 12.95 | ranking/score.rs |
| 5 | term_coverage_adjustment | function | 10.9 | ranking/score.rs |

### Q2: "How are embeddings generated and stored?"
**Expected:** storage/vector.rs, embedding backend files, storage/ layer

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | impl Embedder for FastEmbedder | impl | 10.88 | embeddings/fastembed.rs |
| 2 | query_embed | function | 9.97 | embeddings/fastembed.rs |
| 3 | FastEmbedder | struct | 8.53 | embeddings/fastembed.rs |
| 4 | generate_nl_description | function | 7.8 | src/text.rs |
| 5 | prepare_embedding_text | function | 7.8 | src/text.rs |

### Q3: "How does tree-sitter parsing work in this codebase?"
**Expected:** indexer/parser.rs, indexer/extract/ language extractors

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | expand_stems | function | 7.5 | src/text.rs |
| 2 | CodeNgramTokenizer | struct | 6.77 | storage/tantivy.rs |
| 3 | impl Tokenizer for CodeNgramTokenizer | impl | 6.77 | storage/tantivy.rs |
| 4 | src/indexer/pipeline/parsing.rs | file | 6.52 | pipeline/parsing.rs |
| 5 | build_prompt | function | 4.76 | hyde/generator.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | to_utf8_pathbuf | function | 41.07 | src/config.rs |
| 2 | get_global_cimcp_dir | function | 37.17 | src/config.rs |
| 3 | new_config_fields_parsed_from_env | function | 37.17 | src/config.rs |
| 4 | canonicalize_dir | function | 33.79 | src/config.rs |
| 5 | from_env | function | 33.13 | src/config.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/parsing.rs | file | 13.26 | pipeline/parsing.rs |
| 2 | src/indexer/extract/python.rs | file | 10.71 | extract/python.rs |
| 3 | src/indexer/extract/symbol.rs | file | 10.71 | extract/symbol.rs |
| 4 | extract_python_symbols | function | 10.3 | extract/python.rs |
| 5 | extract_c_symbols | function | 9.94 | extract/c.rs |

### Q6: "How does the MCP server handle incoming tool requests?"
**Expected:** server/mod.rs, handlers/mod.rs, tool dispatch/routing logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | dispatch_tool_call | function | 18.62 | server/mod.rs |
| 2 | handle_call_tool_request | function | 17.48 | server/mod.rs |
| 3 | call_tool | function | 17.12 | scripts/test_scoring.py |
| 4 | handle_call_tool_request | function | 17.12 | server/standalone.rs |
| 5 | handle_list_tools_request | function | 16.26 | server/standalone.rs |

### Q7: "How does the WebSocket handler work?"
**Expected:** WebSocket-related handler code, connection management

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_pattern_details | function | 6.1 | extract/elysia.rs |
| 2 | extract_plugin_name | function | 5.72 | extract/elysia.rs |
| 3 | truncate_text | function | 5.72 | extract/elysia.rs |
| 4 | framework_tags_make_websocket_handler_searchable | function | 5.26 | storage/tantivy.rs |
| 5 | text_for_node | function | 5.26 | extract/typescript.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/storage/sqlite/schema.rs | file | 735.65 | sqlite/schema.rs |
| 2 | init | function | 5.0 | sqlite/operations.rs |
| 3 | impl SqliteStore | impl | 2.5 | sqlite/operations.rs |
| 4 | migrate_add_edges_evidence_count_column | function | 2.32 | sqlite/operations.rs |
| 5 | migrate_add_edges_location_columns | function | 2.32 | sqlite/operations.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | test_generate_nl_description_name_only_variants | function | 1.62 | src/text.rs |
| 2 | is_test_file | function | 1.62 | ranking/score.rs |
| 3 | intent_adjustment | function | 1.59 | ranking/score.rs |
| 4 | expand_stems | function | 0.81 | src/text.rs |
| 5 | expand_index_text | function | 0.45 | storage/tantivy.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_concept_tags | function | 6.17 | src/text.rs |
| 2 | split_identifier_like | function | 5.79 | src/text.rs |
| 3 | HyDEQuery | struct | 4.67 | hyde/generator.rs |
| 4 | format_similar_results | function | 4.67 | handlers/mod.rs |
| 5 | handle_explain_search | function | 4.45 | handlers/mod.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/parallel.rs | file | 12.57 | pipeline/parallel.rs |
| 2 | handle_search_code | function | 12.44 | handlers/mod.rs |
| 3 | index_files_parallel_async | function | 12.44 | pipeline/mod.rs |
| 4 | index_files_parallel | function | 11.55 | pipeline/parallel.rs |
| 5 | generate_embeddings_for_parallel_indexed_files | function | 9.62 | pipeline/mod.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 5.29 | storage/cache.rs |
| 2 | get | function | 5.05 | storage/cache.rs |
| 3 | content_hash | function | 4.96 | storage/cache.rs |
| 4 | RetrieverCaches | struct | 4.63 | retrieval/cache.rs |
| 5 | test_content_hash | function | 4.63 | storage/cache.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | intent_adjustment | function | 13.3 | ranking/score.rs |
| 2 | is_test_symbol | function | 12.48 | ranking/score.rs |
| 3 | PathNormalizer | struct | 10.04 | path/mod.rs |
| 4 | impl PathNormalizer | impl | 6.68 | path/mod.rs |
| 5 | rank_hits_with_signals | function | 6.45 | ranking/score.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 13.35 | storage/cache.rs |
| 2 | get | function | 12.78 | storage/cache.rs |
| 3 | content_hash | function | 12.53 | storage/cache.rs |
| 4 | put_cached_embedding | function | 8.32 | sqlite/mod.rs |
| 5 | embed | function | 8.32 | embeddings/fastembed.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/watch.rs | file | 8.78 | pipeline/watch.rs |
| 2 | spawn_watch_loop | function | 7.6 | pipeline/mod.rs |
| 3 | incremental_index_skips_unchanged_and_removes_deleted_files | function | 7.6 | tests/integration_index_search.rs |
| 4 | create_watcher | function | 7.38 | pipeline/watch.rs |
| 5 | src/indexer/pipeline/mod.rs | file | 6.88 | pipeline/mod.rs |

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
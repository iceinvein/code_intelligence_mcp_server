# Round 52 - Raw Results

Generated: 2026-02-13 12:36
Symbols indexed: 1682
Total time: 7s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | definition_bias | function | 14.46 | ranking/score.rs |
| 2 | term_coverage_adjustment | function | 11.6 | ranking/score.rs |
| 3 | test_rank_lines_by_relevance_basic | function | 11.6 | assembler/formatting.rs |
| 4 | split_camel_case | function | 10.88 | ranking/score.rs |
| 5 | symbol_importance_adjustment | function | 10.64 | ranking/score.rs |

### Q2: "How are embeddings generated and stored?"
**Expected:** storage/vector.rs, embedding backend files, storage/ layer

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | impl Embedder for FastEmbedder | impl | 10.88 | embeddings/fastembed.rs |
| 2 | query_embed | function | 9.98 | embeddings/fastembed.rs |
| 3 | FastEmbedder | struct | 8.53 | embeddings/fastembed.rs |
| 4 | generate_nl_description | function | 7.82 | src/text.rs |
| 5 | prepare_embedding_text | function | 7.82 | src/text.rs |

### Q3: "How does tree-sitter parsing work in this codebase?"
**Expected:** indexer/parser.rs, indexer/extract/ language extractors

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | expand_stems | function | 7.67 | src/text.rs |
| 2 | CodeNgramTokenizer | struct | 6.96 | storage/tantivy.rs |
| 3 | impl Tokenizer for CodeNgramTokenizer | impl | 6.96 | storage/tantivy.rs |
| 4 | src/indexer/pipeline/parsing.rs | file | 6.64 | pipeline/parsing.rs |
| 5 | build_prompt | function | 4.95 | hyde/generator.rs |

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
| 1 | src/indexer/pipeline/parsing.rs | file | 13.71 | pipeline/parsing.rs |
| 2 | src/indexer/extract/python.rs | file | 11.08 | extract/python.rs |
| 3 | src/indexer/extract/symbol.rs | file | 11.08 | extract/symbol.rs |
| 4 | extract_python_symbols | function | 10.71 | extract/python.rs |
| 5 | extract_c_symbols | function | 10.32 | extract/c.rs |

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
| 1 | extract_pattern_details | function | 6.29 | extract/elysia.rs |
| 2 | extract_plugin_name | function | 5.9 | extract/elysia.rs |
| 3 | truncate_text | function | 5.9 | extract/elysia.rs |
| 4 | framework_tags_make_websocket_handler_searchable | function | 5.42 | storage/tantivy.rs |
| 5 | text_for_node | function | 5.42 | extract/typescript.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/storage/sqlite/schema.rs | file | 705.34 | sqlite/schema.rs |
| 2 | insert_test_symbol | function | 4.76 | pipeline/describe.rs |
| 3 | assemble_definitions | function | 2.38 | retrieval/mod.rs |
| 4 | impl SqliteStore | impl | 2.36 | sqlite/operations.rs |
| 5 | init | function | 2.1 | sqlite/operations.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | expand_stems | function | 3.98 | src/text.rs |
| 2 | detect_language_from_query | function | 0.8 | retrieval/mod.rs |
| 3 | get_query_vector_cached | function | 0.8 | retrieval/mod.rs |
| 4 | search | function | 0.78 | retrieval/mod.rs |
| 5 | unix_now_s | function | 0.73 | retrieval/mod.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | SearchResponse | struct | 10.29 | retrieval/mod.rs |
| 2 | SearchResponseWithSignals | struct | 7.73 | retrieval/mod.rs |
| 3 | extract_concept_tags | function | 6.83 | src/text.rs |
| 4 | OpenAIMessage | struct | 6.83 | hyde/generator.rs |
| 5 | split_identifier_like | function | 6.41 | src/text.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | detect_language_from_query | function | 10.96 | retrieval/mod.rs |
| 2 | get_query_vector_cached | function | 10.96 | retrieval/mod.rs |
| 3 | src/indexer/pipeline/parallel.rs | file | 10.96 | pipeline/parallel.rs |
| 4 | search | function | 10.75 | retrieval/mod.rs |
| 5 | impl Retriever | impl | 10.73 | retrieval/mod.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | cache_insert_response | function | 11.71 | retrieval/mod.rs |
| 2 | cache | module | 9.05 | retrieval/mod.rs |
| 3 | test_content_hash | function | 9.05 | storage/cache.rs |
| 4 | get_query_vector_cached | function | 8.27 | retrieval/mod.rs |
| 5 | src/retrieval/mod.rs | file | 7.44 | retrieval/mod.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | is_test_file | function | 13.56 | ranking/score.rs |
| 2 | intent_adjustment | function | 13.3 | ranking/score.rs |
| 3 | is_test_symbol | function | 12.48 | ranking/score.rs |
| 4 | PathNormalizer | struct | 10.01 | path/mod.rs |
| 5 | src/retrieval/mod.rs | file | 9.88 | retrieval/mod.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 13.38 | storage/cache.rs |
| 2 | get | function | 12.85 | storage/cache.rs |
| 3 | content_hash | function | 12.56 | storage/cache.rs |
| 4 | put_cached_embedding | function | 12.28 | sqlite/mod.rs |
| 5 | get_query_vector_cached | function | 12.28 | retrieval/mod.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/watch.rs | file | 8.72 | pipeline/watch.rs |
| 2 | detect_language_from_query | function | 8.61 | retrieval/mod.rs |
| 3 | get_query_vector_cached | function | 8.61 | retrieval/mod.rs |
| 4 | incremental_index_skips_unchanged_and_removes_deleted_files | function | 8.61 | tests/integration_index_search.rs |
| 5 | search | function | 8.45 | retrieval/mod.rs |

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
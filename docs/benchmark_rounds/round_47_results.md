# Round 47 - Raw Results

Generated: 2026-02-13 11:07
Symbols indexed: 1678
Total time: 3s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | format_scoring_breakdown | function | 15.23 | handlers/mod.rs |
| 2 | get | function | 15.1 | src/registry.rs |
| 3 | test_rank_lines_by_relevance_basic | function | 15.1 | assembler/formatting.rs |
| 4 | simple_stem | function | 14.24 | ranking/score.rs |
| 5 | stems_match | function | 13.68 | ranking/score.rs |

### Q2: "How are embeddings generated and stored?"
**Expected:** storage/vector.rs, embedding backend files, storage/ layer

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/embeddings/fastembed.rs | file | 11.57 | embeddings/fastembed.rs |
| 2 | embed | function | 10.28 | embeddings/fastembed.rs |
| 3 | src/embeddings/mod.rs | file | 10.28 | embeddings/mod.rs |
| 4 | HyDEQuery | struct | 9.05 | hyde/generator.rs |
| 5 | generate_mock | function | 7.38 | hyde/generator.rs |

### Q3: "How does tree-sitter parsing work in this codebase?"
**Expected:** indexer/parser.rs, indexer/extract/ language extractors

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | expand_stems | function | 8.69 | src/text.rs |
| 2 | build_prompt | function | 8.11 | hyde/generator.rs |
| 3 | impl Tokenizer for CodeNgramTokenizer | impl | 8.11 | storage/tantivy.rs |
| 4 | token_stream | function | 7.63 | storage/tantivy.rs |
| 5 | src/indexer/pipeline/parsing.rs | file | 7.33 | pipeline/parsing.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | required_env | function | 32.76 | src/config.rs |
| 2 | new_config_fields_parsed_from_env | function | 29.38 | src/config.rs |
| 3 | optional_env | function | 29.38 | src/config.rs |
| 4 | to_utf8_pathbuf | function | 28.4 | src/config.rs |
| 5 | get_global_cimcp_dir | function | 25.71 | src/config.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | ExtractedSymbol | struct | 13.81 | extract/symbol.rs |
| 2 | ExtractedFrameworkPattern | struct | 12.74 | extract/symbol.rs |
| 3 | src/indexer/extract/go.rs | file | 11.97 | extract/go.rs |
| 4 | src/indexer/pipeline/parsing.rs | file | 11.97 | pipeline/parsing.rs |
| 5 | src/indexer/extract/rust.rs | file | 9.92 | extract/rust.rs |

### Q6: "How does the MCP server handle incoming tool requests?"
**Expected:** server/mod.rs, handlers/mod.rs, tool dispatch/routing logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | dispatch_tool_call | function | 20.17 | server/mod.rs |
| 2 | call_tool | function | 19.57 | scripts/test_scoring.py |
| 3 | handle_list_tools_request | function | 19.57 | server/standalone.rs |
| 4 | handle_call_tool_request | function | 19.25 | server/standalone.rs |
| 5 | handle_call_tool_request | function | 18.99 | server/mod.rs |

### Q7: "How does the WebSocket handler work?"
**Expected:** WebSocket-related handler code, connection management

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_pattern_details websocket_handler | function | 16.46 | extract/elysia.rs |
| 2 | extract_plugin_name | function | 15.45 | extract/elysia.rs |
| 3 | truncate_text | function | 15.45 | extract/elysia.rs |
| 4 | fmt websocket_handler | function | 14.29 | extract/symbol.rs |
| 5 | framework_tags_make_websocket_handler_searchable | function | 14.29 | storage/tantivy.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/storage/sqlite/schema.rs | file | 945.29 | sqlite/schema.rs |
| 2 | init | function | 6.4 | sqlite/operations.rs |
| 3 | impl SqliteStore | impl | 3.2 | sqlite/operations.rs |
| 4 | migrate_add_edges_evidence_count_column | function | 3.17 | sqlite/operations.rs |
| 5 | migrate_add_edges_location_columns | function | 3.17 | sqlite/operations.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | expand_stems | function | 8.23 | src/text.rs |
| 2 | PathError | enum | 7.17 | path/mod.rs |
| 3 | test_generate_nl_description_name_only_variants | function | 7.17 | src/text.rs |
| 4 | fmt | function | 6.44 | path/mod.rs |
| 5 | tool_internal_error | function | 5.21 | handlers/mod.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_concept_tags | function | 8.03 | src/text.rs |
| 2 | split_identifier_like | function | 7.54 | src/text.rs |
| 3 | HyDEQuery | struct | 7.52 | hyde/generator.rs |
| 4 | SearchResponse | struct | 7.52 | retrieval/mod.rs |
| 5 | format_similar_results | function | 6.53 | handlers/mod.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | index_files_parallel_async | function | 12.38 | pipeline/mod.rs |
| 2 | handle_search_code | function | 9.46 | handlers/mod.rs |
| 3 | src/indexer/pipeline/parallel.rs | file | 9.46 | pipeline/parallel.rs |
| 4 | index_files_parallel | function | 9.0 | pipeline/parallel.rs |
| 5 | generate_embeddings_for_parallel_indexed_files | function | 7.9 | pipeline/mod.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | cache_key | function | 12.26 | storage/cache.rs |
| 2 | impl EmbeddingCache | impl | 11.2 | storage/cache.rs |
| 3 | Cache | struct | 10.88 | reranker/cache.rs |
| 4 | test_content_hash | function | 10.88 | storage/cache.rs |
| 5 | EmbeddingCacheEntry | struct | 10.4 | queries/cache.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathNormalizer | struct | 21.75 | path/mod.rs |
| 2 | impl PathNormalizer | impl | 10.61 | path/mod.rs |
| 3 | contains_code_snippet | function | 8.15 | retrieval/query.rs |
| 4 | is_definition_kind | function | 6.94 | ranking/diversify.rs |
| 5 | ExtractedFrameworkPattern | struct | 6.75 | extract/symbol.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 17.75 | storage/cache.rs |
| 2 | get | function | 17.38 | storage/cache.rs |
| 3 | content_hash | function | 16.66 | storage/cache.rs |
| 4 | put_cached_embedding | function | 12.64 | sqlite/mod.rs |
| 5 | embed | function | 12.64 | embeddings/fastembed.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/watch.rs | file | 12.86 | pipeline/watch.rs |
| 2 | spawn_watch_loop | function | 12.26 | pipeline/mod.rs |
| 3 | incremental_index_skips_unchanged_and_removes_deleted_files | function | 12.26 | tests/integration_index_search.rs |
| 4 | create_watcher | function | 11.86 | pipeline/watch.rs |
| 5 | src/indexer/pipeline/mod.rs | file | 10.47 | pipeline/mod.rs |

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
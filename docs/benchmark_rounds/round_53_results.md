# Round 53 - Raw Results

Generated: 2026-02-13 12:44
Symbols indexed: 1682
Total time: 6s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | simple_stem | function | 18.65 | ranking/score.rs |
| 2 | stems_match | function | 17.91 | ranking/score.rs |
| 3 | term_coverage_adjustment | function | 16.63 | ranking/score.rs |
| 4 | rank_lines_by_relevance | function | 16.57 | assembler/formatting.rs |
| 5 | format_scoring_breakdown | function | 16.57 | handlers/mod.rs |

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
| 1 | expand_stems | function | 14.34 | src/text.rs |
| 2 | CodeNgramTokenizer | struct | 12.95 | storage/tantivy.rs |
| 3 | token_stream | function | 12.95 | storage/tantivy.rs |
| 4 | src/indexer/pipeline/parsing.rs | file | 10.64 | pipeline/parsing.rs |
| 5 | build_prompt | function | 10.37 | hyde/generator.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | to_utf8_pathbuf | function | 41.07 | src/config.rs |
| 2 | get_global_cimcp_dir | function | 37.18 | src/config.rs |
| 3 | new_config_fields_parsed_from_env | function | 37.18 | src/config.rs |
| 4 | canonicalize_dir | function | 33.79 | src/config.rs |
| 5 | from_env | function | 33.14 | src/config.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/parsing.rs | file | 14.46 | pipeline/parsing.rs |
| 2 | extract_python_symbols | function | 10.04 | extract/python.rs |
| 3 | src/indexer/extract/python.rs | file | 10.04 | extract/python.rs |
| 4 | extract_go_symbols | function | 8.79 | extract/go.rs |
| 5 | expand_index_text | function | 8.72 | storage/tantivy.rs |

### Q6: "How does the MCP server handle incoming tool requests?"
**Expected:** server/mod.rs, handlers/mod.rs, tool dispatch/routing logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | dispatch_tool_call | function | 18.36 | server/mod.rs |
| 2 | handle_call_tool_request | function | 17.32 | server/mod.rs |
| 3 | call_tool | function | 16.91 | scripts/test_scoring.py |
| 4 | handle_call_tool_request | function | 16.91 | server/standalone.rs |
| 5 | resolve_state | function | 15.87 | server/standalone.rs |

### Q7: "How does the WebSocket handler work?"
**Expected:** WebSocket-related handler code, connection management

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_pattern_details | function | 10.17 | extract/elysia.rs |
| 2 | extract_plugin_name | function | 9.54 | extract/elysia.rs |
| 3 | truncate_text | function | 9.54 | extract/elysia.rs |
| 4 | framework_tags_make_websocket_handler_searchable | function | 8.76 | storage/tantivy.rs |
| 5 | text_for_node | function | 8.76 | extract/typescript.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/storage/sqlite/schema.rs | file | 843.2 | sqlite/schema.rs |
| 2 | init | function | 6.53 | sqlite/operations.rs |
| 3 | migrate_add_edges_confidence_column | function | 3.26 | sqlite/operations.rs |
| 4 | migrate_add_edges_evidence_count_column | function | 3.26 | sqlite/operations.rs |
| 5 | migrate_add_edges_location_columns | function | 3.26 | sqlite/operations.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | test_generate_nl_description_name_only_variants | function | 0.61 | src/text.rs |
| 2 | expand_stems | function | 0.35 | src/text.rs |
| 3 | expand_index_text | function | 0.12 | storage/tantivy.rs |
| 4 | simple_stem | function | 0.04 | ranking/score.rs |
| 5 | stems_match | function | 0.04 | ranking/score.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | expand_stems | function | 11.37 | src/text.rs |
| 2 | extract_concept_tags | function | 11.18 | src/text.rs |
| 3 | HyDEQuery | struct | 8.92 | hyde/generator.rs |
| 4 | format_similar_results | function | 8.92 | handlers/mod.rs |
| 5 | get | function | 8.01 | src/registry.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/parallel.rs | file | 12.57 | pipeline/parallel.rs |
| 2 | handle_search_code | function | 12.43 | handlers/mod.rs |
| 3 | index_files_parallel_async | function | 12.43 | pipeline/mod.rs |
| 4 | index_files_parallel | function | 11.54 | pipeline/parallel.rs |
| 5 | generate_embeddings_for_parallel_indexed_files | function | 9.6 | pipeline/mod.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 11.63 | storage/cache.rs |
| 2 | content_hash | function | 10.91 | storage/cache.rs |
| 3 | RetrieverCaches | struct | 9.92 | retrieval/cache.rs |
| 4 | test_content_hash | function | 9.92 | storage/cache.rs |
| 5 | put_cached_embedding | function | 8.18 | sqlite/mod.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathNormalizer | struct | 21.85 | path/mod.rs |
| 2 | impl PathNormalizer | impl | 17.97 | path/mod.rs |
| 3 | normalize_for_compare | function | 7.91 | path/mod.rs |
| 4 | from | function | 7.42 | path/mod.rs |
| 5 | path_strategy | function | 4.48 | path/mod.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 16.93 | storage/cache.rs |
| 2 | get | function | 15.98 | storage/cache.rs |
| 3 | content_hash | function | 15.89 | storage/cache.rs |
| 4 | put_cached_embedding | function | 11.69 | sqlite/mod.rs |
| 5 | embed | function | 11.69 | embeddings/fastembed.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/watch.rs | file | 12.79 | pipeline/watch.rs |
| 2 | spawn_watch_loop | function | 10.72 | pipeline/mod.rs |
| 3 | incremental_index_skips_unchanged_and_removes_deleted_files | function | 10.72 | tests/integration_index_search.rs |
| 4 | create_watcher | function | 10.06 | pipeline/watch.rs |
| 5 | index_files | function | 9.88 | pipeline/mod.rs |

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
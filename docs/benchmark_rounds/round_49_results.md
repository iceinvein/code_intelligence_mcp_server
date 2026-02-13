# Round 49 - Raw Results

Generated: 2026-02-13 11:52
Symbols indexed: 1682
Total time: 4s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | simple_stem | function | 18.52 | ranking/score.rs |
| 2 | stems_match | function | 17.78 | ranking/score.rs |
| 3 | rank_lines_by_relevance | function | 16.59 | assembler/formatting.rs |
| 4 | format_scoring_breakdown | function | 16.59 | handlers/mod.rs |
| 5 | term_coverage_adjustment | function | 16.51 | ranking/score.rs |

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
| 1 | expand_stems | function | 14.35 | src/text.rs |
| 2 | CodeNgramTokenizer | struct | 12.95 | storage/tantivy.rs |
| 3 | token_stream | function | 12.95 | storage/tantivy.rs |
| 4 | src/indexer/pipeline/parsing.rs | file | 10.64 | pipeline/parsing.rs |
| 5 | build_prompt | function | 10.37 | hyde/generator.rs |

### Q4: "Configuration from environment variables"
**Expected:** Config/settings module, main entry point with env var reads

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | to_utf8_pathbuf | function | 41.02 | src/config.rs |
| 2 | get_global_cimcp_dir | function | 37.13 | src/config.rs |
| 3 | new_config_fields_parsed_from_env | function | 37.13 | src/config.rs |
| 4 | canonicalize_dir | function | 33.75 | src/config.rs |
| 5 | from_env | function | 33.1 | src/config.rs |

### Q5: "Indexing pipeline file scanning and symbol extraction"
**Expected:** indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/parsing.rs | file | 14.46 | pipeline/parsing.rs |
| 2 | extract_python_symbols | function | 10.04 | extract/python.rs |
| 3 | src/indexer/extract/python.rs | file | 10.04 | extract/python.rs |
| 4 | extract_go_symbols | function | 9.09 | extract/go.rs |
| 5 | extract_c_symbols | function | 8.43 | extract/c.rs |

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
| 1 | expand_stems | function | 10.01 | src/text.rs |
| 2 | contains_code_snippet | function | 8.19 | retrieval/query.rs |
| 3 | test_remove_stop_words_error_handling_query | function | 8.19 | src/text.rs |
| 4 | build_prompt | function | 7.39 | hyde/generator.rs |
| 5 | src/indexer/pipeline/parsing.rs | file | 6.66 | pipeline/parsing.rs |

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

| # | Query | R48 | R49 | Δ | Pattern |
|---|-------|-----|-----|---|---------|
| 1 | How does the ranking and scoring system work? | 4 | 5 | +1 | score.rs functions at #1-2-5 (were #4-5). Path variant "score→scoring" worked |
| 2 | How are embeddings generated and stored? | 7 | 7 | 0 | 3/5 from fastembed.rs. text.rs noise at #4-5 |
| 3 | How does tree-sitter parsing work in this codebase | 3 | 3 | 0 | expand_stems still #1. Vocab gap persists. Needs LLM descriptions |
| 4 | Configuration from environment variables | 9 | 9 | 0 | All config.rs. from_env at #5 is excellent |
| 5 | Indexing pipeline file scanning and symbol extract | 7 | 7 | 0 | Good extract functions + files |
| 6 | How does the MCP server handle incoming tool reque | 8 | 8 | 0 | dispatch_tool_call #1, handle_call_tool_request #2 (up from #4) |
| 7 | How does the WebSocket handler work? | 5 | 5 | 0 | Same-file flooding from elysia.rs |
| 8 | SQLite database schema tables initialization | 9 | 9 | 0 | schema.rs #1 (843), all sqlite/ |
| 9 | Error handling and graceful degradation | 3 | 2 | -1 | expand_stems score UP (IDF shift boosted SYNONYMS dict). PathError/tool_internal_error dropped |
| 10 | JSON serialization and response formatting | 4 | 3 | -1 | expand_stems now #1 (was absent). IDF shift from inline comment stripping boosted SYNONYMS dict terms |
| 11 | Async concurrency and parallel processing | 8 | 8 | 0 | All parallel/async relevant |
| 12 | Caching and cache invalidation | 7 | 7 | 0 | All cache-related |
| 13 | PathNormalizer struct definition and methods | 7 | 9 | +2 | ALL 5 from path/mod.rs! Path variant "normalizer→normalize" spread to methods |
| 14 | EmbeddingCache get put cached embedding | 8 | 8 | 0 | Identical results |
| 15 | File watcher debounce reindex on change | 7 | 7 | 0 | Same watcher results |

**CI Average:** 6.47 (R48: 6.40, Δ: +0.07)

**Changes in this round:** Schema v15 — inline comment stripping, path segment morphological variants, LLM prompt v2 (descriptions NOT yet regenerated)

**Key findings:**
- Path segment variants: Q1 +1 (score.rs up), Q13 +2 (all 5 results from target file!)
- Inline comment stripping backfired: removing comments from OTHER functions boosted IDF for SYNONYMS dictionary string literals in text.rs, making expand_stems rank HIGHER for Q9/Q10
- Net: +3 gains, -2 regressions = +1 (effectively neutral with IDF shift noise)
- LLM description regeneration still pending (~57 min) — Tier 1 untested
# Round 50 - Raw Results

Generated: 2026-02-13 12:09
Symbols indexed: 1682
Total time: 3s

## Results Summary

### Q1: "How does the ranking and scoring system work?"
**Expected:** retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | simple_stem | function | 13.78 | ranking/score.rs |
| 2 | stems_match | function | 13.23 | ranking/score.rs |
| 3 | test_rank_lines_by_relevance_basic | function | 13.23 | assembler/formatting.rs |
| 4 | term_coverage_adjustment | function | 12.28 | ranking/score.rs |
| 5 | split_camel_case | function | 11.53 | ranking/score.rs |

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
| 1 | expand_stems | function | 6.81 | src/text.rs |
| 2 | CodeNgramTokenizer | struct | 6.08 | storage/tantivy.rs |
| 3 | src/indexer/pipeline/parsing.rs | file | 6.08 | pipeline/parsing.rs |
| 4 | impl Tokenizer for CodeNgramTokenizer | impl | 6.08 | storage/tantivy.rs |
| 5 | build_prompt | function | 4.09 | hyde/generator.rs |

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
| 1 | src/indexer/pipeline/parsing.rs | file | 11.3 | pipeline/parsing.rs |
| 2 | src/indexer/extract/python.rs | file | 8.69 | extract/python.rs |
| 3 | src/indexer/extract/symbol.rs | file | 8.69 | extract/symbol.rs |
| 4 | extract_python_symbols | function | 8.3 | extract/python.rs |
| 5 | extract_c_symbols | function | 7.93 | extract/c.rs |

### Q6: "How does the MCP server handle incoming tool requests?"
**Expected:** server/mod.rs, handlers/mod.rs, tool dispatch/routing logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | dispatch_tool_call | function | 18.35 | server/mod.rs |
| 2 | handle_call_tool_request | function | 17.32 | server/mod.rs |
| 3 | call_tool | function | 16.91 | scripts/test_scoring.py |
| 4 | handle_call_tool_request | function | 16.91 | server/standalone.rs |
| 5 | resolve_state | function | 15.87 | server/standalone.rs |

### Q7: "How does the WebSocket handler work?"
**Expected:** WebSocket-related handler code, connection management

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_pattern_details | function | 5.68 | extract/elysia.rs |
| 2 | extract_plugin_name | function | 5.33 | extract/elysia.rs |
| 3 | truncate_text | function | 5.33 | extract/elysia.rs |
| 4 | framework_tags_make_websocket_handler_searchable | function | 4.89 | storage/tantivy.rs |
| 5 | text_for_node | function | 4.89 | extract/typescript.rs |

### Q8: "SQLite database schema tables initialization"
**Expected:** storage/sqlite/ schema definitions, migration/init code

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/storage/sqlite/schema.rs | file | 665.49 | sqlite/schema.rs |
| 2 | init | function | 4.57 | sqlite/operations.rs |
| 3 | impl SqliteStore | impl | 2.29 | sqlite/operations.rs |
| 4 | migrate_add_edges_evidence_count_column | function | 2.13 | sqlite/operations.rs |
| 5 | migrate_add_edges_location_columns | function | 2.13 | sqlite/operations.rs |

### Q9: "Error handling and graceful degradation"
**Expected:** Error types, fallback logic across multiple modules

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | expand_stems | function | 3.32 | src/text.rs |
| 2 | simple_stem | function | 1.54 | ranking/score.rs |
| 3 | test_generate_nl_description_name_only_variants | function | 1.54 | src/text.rs |
| 4 | expand_index_text | function | 1.5 | storage/tantivy.rs |
| 5 | stems_match | function | 1.47 | ranking/score.rs |

### Q10: "JSON serialization and response formatting"
**Expected:** Serde derive usage, response builders, MCP protocol formatting

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | extract_concept_tags | function | 6.0 | src/text.rs |
| 2 | split_identifier_like | function | 5.63 | src/text.rs |
| 3 | HyDEQuery | struct | 4.51 | hyde/generator.rs |
| 4 | format_similar_results | function | 4.51 | handlers/mod.rs |
| 5 | handle_explain_search | function | 4.31 | handlers/mod.rs |

### Q11: "Async concurrency and parallel processing"
**Expected:** Async mutex usage, parallel indexing, concurrent operations

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/parallel.rs | file | 12.57 | pipeline/parallel.rs |
| 2 | handle_search_code | function | 12.43 | handlers/mod.rs |
| 3 | index_files_parallel_async | function | 12.43 | pipeline/mod.rs |
| 4 | index_files_parallel | function | 11.54 | pipeline/parallel.rs |
| 5 | generate_embeddings_for_parallel_indexed_files | function | 9.61 | pipeline/mod.rs |

### Q12: "Caching and cache invalidation"
**Expected:** retrieval/cache.rs, embedding cache, TTL/invalidation logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 5.12 | storage/cache.rs |
| 2 | get | function | 4.87 | storage/cache.rs |
| 3 | content_hash | function | 4.8 | storage/cache.rs |
| 4 | RetrieverCaches | struct | 4.47 | retrieval/cache.rs |
| 5 | test_content_hash | function | 4.47 | storage/cache.rs |

### Q13: "PathNormalizer struct definition and methods"
**Expected:** path/mod.rs -- the struct and its impl block

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | PathNormalizer | struct | 10.27 | path/mod.rs |
| 2 | impl PathNormalizer | impl | 6.87 | path/mod.rs |
| 3 | normalize_for_compare | function | 2.97 | path/mod.rs |
| 4 | symlink_or_copy | function | 2.92 | llm/mod.rs |
| 5 | from | function | 2.79 | path/mod.rs |

### Q14: "EmbeddingCache get put cached embedding"
**Expected:** The cache struct and its get/put methods

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | put | function | 12.24 | storage/cache.rs |
| 2 | get | function | 11.59 | storage/cache.rs |
| 3 | content_hash | function | 11.49 | storage/cache.rs |
| 4 | put_cached_embedding | function | 7.2 | sqlite/mod.rs |
| 5 | embed | function | 7.2 | embeddings/fastembed.rs |

### Q15: "File watcher debounce reindex on change"
**Expected:** Watcher module, debounce logic

| # | Symbol | Kind | Score | File |
|---|--------|------|-------|------|
| 1 | src/indexer/pipeline/watch.rs | file | 7.82 | pipeline/watch.rs |
| 2 | spawn_watch_loop | function | 6.66 | pipeline/mod.rs |
| 3 | incremental_index_skips_unchanged_and_removes_deleted_files | function | 6.66 | tests/integration_index_search.rs |
| 4 | create_watcher | function | 6.42 | pipeline/watch.rs |
| 5 | src/indexer/pipeline/mod.rs | file | 5.86 | pipeline/mod.rs |

## Scoring Template

Score each query's CI results on a 1-10 scale:
- 9-10: Every result directly answers the query, spans relevant files
- 7-8: Most results relevant, good diversity, top 3-5 strong
- 5-6: ~Half relevant, some gaps, core code present but buried
- 3-4: Few relevant, dominated by 1-2 files, test/re-export noise
- 1-2: Mostly irrelevant, core implementation missing

| # | Query | R49 | R50 | Δ | Pattern |
|---|-------|-----|-----|---|---------|
| 1 | How does the ranking and scoring system work? | 5 | 5 | 0 | 4/5 from score.rs (was 3/5). split_camel_case replaced format_scoring_breakdown. test at #3 is noise |
| 2 | How are embeddings generated and stored? | 7 | 7 | 0 | Identical: 3/5 fastembed.rs. text.rs noise at #4-5 |
| 3 | How does tree-sitter parsing work in this codebase | 3 | 3 | 0 | Unchanged despite 8 descriptions now mentioning tree-sitter. parser.rs still not in top-5. expand_stems still #1 |
| 4 | Configuration from environment variables | 9 | 9 | 0 | All config.rs. Identical |
| 5 | Indexing pipeline file scanning and symbol extract | 7 | 7 | 0 | symbol.rs replaced extract_go_symbols at #3. Good extraction coverage |
| 6 | How does the MCP server handle incoming tool reque | 8 | 8 | 0 | dispatch_tool_call #1, handle_call_tool_request #2 (improved from R48 where it was #5) |
| 7 | How does the WebSocket handler work? | 5 | 5 | 0 | Same elysia.rs flooding, framework_tags test at #4 |
| 8 | SQLite database schema tables initialization | 9 | 9 | 0 | schema.rs #1 (665), all sqlite/ |
| 9 | Error handling and graceful degradation | 2 | 1 | -1 | ALL results irrelevant. expand_stems #1, rest are stemming/text utilities. PathError gone entirely. Description IDF shift? |
| 10 | JSON serialization and response formatting | 3 | 4 | +1 | expand_stems DROPPED from top-5 (was #1). handle_explain_search at #5 is a JSON-formatting handler. 2.5/5 useful |
| 11 | Async concurrency and parallel processing | 8 | 8 | 0 | Identical results |
| 12 | Caching and cache invalidation | 7 | 7 | 0 | get appeared at #2. put_cached_embedding dropped |
| 13 | PathNormalizer struct definition and methods | 9 | 8 | -1 | symlink_or_copy (llm/mod.rs) invaded #4. Was 5/5 path/mod.rs, now 4/5 |
| 14 | EmbeddingCache get put cached embedding | 8 | 8 | 0 | Identical results |
| 15 | File watcher debounce reindex on change | 7 | 7 | 0 | pipeline/mod.rs replaced index_files at #5. Equivalent |

**CI Average:** 6.40 (R49: 6.47, Δ: -0.07)

**Changes in this round:** LLM v2 descriptions regenerated (all 1682 symbols). 8 descriptions now mention "tree-sitter".

**Key findings:**
- **LLM v2 descriptions had effectively zero impact** — CI average unchanged within noise floor (±0.07)
- **Q3 unchanged despite tree-sitter mentions**: 8 descriptions now contain "tree-sitter" but the BM25 boost wasn't enough to push parser.rs or extract/*.rs above text.rs/tantivy.rs noise
- **Q9 continued decline (2→1)**: PathError/fmt/tool_internal_error completely vanished from results. Description regeneration shifted IDF further against error-handling terms
- **Q10 improved (3→4)**: expand_stems dropped from #1 (its v2 description may have shifted its BM25 profile). handle_explain_search entered at #5
- **Q13 minor regression (9→8)**: symlink_or_copy from llm/mod.rs appeared — its v2 description likely contains "path" terms
- **Root cause**: 1.5B Qwen model generates too-generic descriptions to meaningfully shift BM25 rankings. Descriptions like "This function parses source code" don't add discriminating vocabulary. Model quality is the bottleneck, not prompt quality.
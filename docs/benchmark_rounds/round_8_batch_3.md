# Round 8 - Batch 3

| # | Query | CI | Augment | Winner | Pattern |
|---|-------|-----|---------|--------|---------|
| 11 | Async concurrency and parallel processing | 7 | 7 | Tie | Single-file flooding (CI: 4/4 from parallel.rs) |
| 12 | Caching and cache invalidation | 7 | 9 | Augment | CI missed retrieval/cache.rs |
| 13 | PathNormalizer struct definition and methods | 7 | 8 | Augment | Test pollution (CI: 3/4 results are test helpers) |
| 14 | EmbeddingCache get put cached embedding | 9 | 9 | Tie | -- |
| 15 | File watcher debounce reindex on change | 2 | 8 | Augment | Keyword mismatch (CI returned file fingerprint SQL) |

## Per-Query Notes

### Q11: "Async concurrency and parallel processing"
- **CI top-3:** parallel.rs:index_files_parallel, parallel.rs:FileIndexResult, parallel.rs:IndexFileResult
- **Augment top-3:** parallel.rs, pipeline/mod.rs, config.rs
- **CI miss:** pipeline/mod.rs (spawn_blocking, tokio::Mutex), config.rs (parallel_workers config)
- **CI hit:** Correctly identified index_files_parallel as the core parallel processing function

### Q12: "Caching and cache invalidation"
- **CI top-3:** reranker/cache.rs:Cache, reranker/cache.rs:new, storage/cache.rs:cache_key
- **Augment top-3:** storage/cache.rs, retrieval/cache.rs, reranker/cache.rs
- **CI miss:** retrieval/cache.rs (LruCache, RetrieverCaches) completely absent; storage/cache.rs EmbeddingCache not in top results
- **CI hit:** Found reranker cache and storage cache_key helper functions

### Q13: "PathNormalizer struct definition and methods"
- **CI top-3:** path/mod.rs:PathNormalizer, path/mod.rs:create_test_normalizer, path/mod.rs:test_normalizer
- **Augment top-3:** path/mod.rs (struct + impl block with all methods + tests)
- **CI miss:** Actual methods (normalize_for_compare, relative_to_base, validate_within_base, join_base) not surfaced as results
- **CI hit:** Found the struct definition correctly as top-1; all results from correct file

### Q14: "EmbeddingCache get put cached embedding"
- **CI top-3:** storage/cache.rs:get, storage/cache.rs:put, storage/cache.rs:EmbeddingCache
- **Augment top-3:** storage/cache.rs, sqlite/queries/cache.rs, retrieval/mod.rs
- **CI miss:** sqlite/queries/cache.rs (underlying SQL queries) not shown
- **CI hit:** Exact match on all three target symbols; content_hash helper also included

### Q15: "File watcher debounce reindex on change"
- **CI top-3:** sqlite/queries/files.rs:upsert_file_fingerprint, sqlite/queries/files.rs (file), sqlite/queries/affinity.rs:upsert_file_affinity
- **Augment top-3:** pipeline/mod.rs (spawn_watch_loop, check_for_changes), config.rs (watch_debounce_ms)
- **CI miss:** spawn_watch_loop entirely missing; check_for_changes missing; watch_debounce_ms config missing
- **CI hit:** Nothing relevant to the actual watcher/debounce query

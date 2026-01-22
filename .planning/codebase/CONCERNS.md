# Codebase Concerns

**Analysis Date:** 2026-01-22

## Tech Debt

**Bare unwrap() and expect() calls:**
- Issue: 30+ instances of `.unwrap()` and `.expect()` without context, primarily in test code and config initialization. These will panic on failure rather than propagate errors gracefully.
- Files: `src/config.rs` (lines 410, 413, 457+), `src/graph/mod.rs` (test code), `src/indexer/extract/c.rs` (utf8_text parsing), `src/handlers/mod.rs`
- Impact: Server crashes instead of graceful error handling during initialization or unusual file conditions. Test failures are brittle.
- Fix approach: Replace with `.context()` or `.map_err()` chains, or use `anyhow::Context` for better error messages. Tests should use `Result<()>` returns or explicit assertions.

**File I/O without consistent error handling:**
- Issue: Some file operations use `.unwrap()` instead of proper error context (e.g., `std::fs::create_dir_all(&dir).unwrap()` in `src/config.rs:413` and `src/indexer/pipeline/utils.rs:173, 182-183`).
- Files: `src/config.rs:413`, `src/indexer/pipeline/utils.rs:173, 182-183`, `src/storage/sqlite/operations.rs:14`
- Impact: Server crashes if filesystem operations fail (permission denied, disk full, path issues) instead of returning meaningful errors.
- Fix approach: Use `.with_context()` to provide path/operation context; use `anyhow::Context` trait consistently.

**Test-only `.unwrap()` calls in production code locations:**
- Issue: Production utility functions like `sql_create_testdb()` in `src/config.rs` contain multiple unwraps (lines 410, 413) that execute during test setup.
- Files: `src/config.rs` (lines 409-420 test helper), `src/indexer/pipeline/utils.rs` (test fixtures)
- Impact: Tests panic on filesystem errors rather than failing cleanly with diagnostics.
- Fix approach: Move all test fixtures to a separate test utilities module; use `anyhow::Result` for all fallible operations.

**Enum variant extraction assumes valid UTF-8:**
- Issue: `utf8_text()` calls in extractors (`src/indexer/extract/c.rs:39, 45, 59, 106`) don't handle invalid UTF-8 gracefully.
- Files: `src/indexer/extract/c.rs:39, 45, 59, 106`, potentially other extract modules
- Impact: Panics if any source file contains invalid UTF-8 encoding (rare but possible in non-ASCII codebases).
- Fix approach: Use `.ok_or_else()` and `.context()` to handle UTF-8 errors; log and skip symbols that can't be decoded.

**Hardcoded model fallback in FastEmbed:**
- Issue: `src/embeddings/fastembed.rs:27` returns error for unsupported embedding models rather than falling back or using a reasonable default.
- Files: `src/embeddings/fastembed.rs:21-28`
- Impact: Embedding initialization fails completely if config specifies unsupported model. No graceful degradation.
- Fix approach: Add fallback to BGE-Base-v1.5; log warning if non-default model requested; document supported models.

## Known Bugs

**Dimension mismatch silently logged but not recovered:**
- Issue: Vector dimension mismatches in `src/storage/vector.rs:119` generate error messages but fail the entire batch rather than skipping individual records.
- Files: `src/storage/vector.rs:111-125`
- Impact: One malformed vector in a batch can cause entire indexing round to fail, requiring manual recovery.
- Workaround: Validate vector dimensions before batching; skip invalid records with warnings.

**UTF-8 text extraction assumes source is valid:**
- Issue: Tree-Sitter node extraction uses `.unwrap()` on `utf8_text()` calls without validation.
- Files: `src/indexer/extract/c.rs:39, 45, 59, 106, 168, 176`
- Trigger: Any source file with byte sequences that aren't valid UTF-8.
- Workaround: Pre-validate source files or use lossy UTF-8 conversion with replacement characters.

**LRU cache position lookup is O(n):**
- Issue: `LruCache::get()` in `src/retrieval/cache.rs:29-31` iterates to find position in `VecDeque`, then removes by position. This is inefficient.
- Files: `src/retrieval/cache.rs:29-31`, also `insert()` at line 43-44
- Trigger: Cache with thousands of entries will have linear-time lookups.
- Workaround: Use `LinkedHashMap` or maintain position index; optimize cache eviction strategy.

## Security Considerations

**Unescaped SQL-like string escaping in LanceDB predicates:**
- Risk: String escaping for LanceDB delete predicates (`src/storage/vector.rs:100-101`) uses simple quote escaping. If file paths contain quotes, the predicate could be malformed.
- Files: `src/storage/vector.rs:100-101`
- Current mitigation: LanceDB SDK likely handles this, but no explicit parameterized query support visible.
- Recommendations: Use LanceDB's native parameterized delete API if available; add roundtrip tests for paths with special characters.

**No rate limiting on MCP tool invocations:**
- Risk: `search_code` and other tools have no request throttling. Expensive operations like graph traversal could be DoS'd.
- Files: `src/handlers/mod.rs` (all tool handlers), `src/graph/mod.rs` (graph traversal)
- Current mitigation: Graph traversal has depth/edge limits, but no per-request rate limiting.
- Recommendations: Add request counting/timeout mechanisms; document expected latency; consider caching policy for expensive queries.

**Embeddings cached in memory without authentication:**
- Risk: Embedding cache in `src/retrieval/cache.rs` holds full symbol code in memory. Multi-tenant scenarios could leak code between requests.
- Files: `src/retrieval/cache.rs:96`
- Current mitigation: Single-process server; no multitenancy designed in.
- Recommendations: If multitenancy added, segregate caches per user/repo; consider disk-based cache with encryption.

**Path traversal in config normalization:**
- Risk: `normalize_path_to_base()` in `src/config.rs` and handlers doesn't validate against directory escape attempts.
- Files: `src/handlers/mod.rs:56` (normalize_path_to_base), `src/config.rs` (path handling)
- Current mitigation: Relative to configured BASE_DIR only; symlinks not followed.
- Recommendations: Add explicit checks for `..` components; document symlink behavior; add tests for malicious paths.

## Performance Bottlenecks

**Tantivy IndexWriter is Mutex-wrapped causing serialized writes:**
- Problem: `src/storage/tantivy.rs:45` wraps IndexWriter in Mutex. All writes serialize even in async context.
- Files: `src/storage/tantivy.rs:45, 256, 280, 290` (lock calls)
- Cause: Tantivy's IndexWriter isn't Send, forcing synchronous lock acquisition during indexing.
- Improvement path: Use `tokio::sync::Mutex` for async locking; consider batch-based commits to reduce lock contention; measure lock hold times.

**Vector search returns all results then filters in memory:**
- Problem: `src/storage/vector.rs` queries full vectors for every search, even when only returning top-k results.
- Files: `src/storage/vector.rs:143-155`
- Cause: LanceDB API doesn't filter results efficiently client-side.
- Improvement path: Use LanceDB limit parameter directly; verify if LanceDB supports server-side filtering; cache frequently searched vectors.

**Config defaults parsing repeated for every request:**
- Problem: `src/config.rs` parsing has multiple `.unwrap_or()` chains that convert strings to defaults on every call.
- Files: `src/config.rs:70-206` (18 unwrap_or chains)
- Cause: No caching of parsed defaults; environment variables read once but defaults recomputed.
- Improvement path: Cache parsed config in static lazy_static; memoize environment variable parsing.

**Graph traversal has no cycle detection:**
- Problem: `src/graph/mod.rs` traversal doesn't detect cycles, only depth limits. Could revisit nodes repeatedly in diamond patterns.
- Files: `src/graph/mod.rs:39-120`
- Cause: Relies on depth limit instead of visited set per root.
- Improvement path: Add cycle detection; use bloom filter for large graphs; consider topological sorting for acyclic subgraphs.

**LRU cache uses VecDeque position-finding (O(n)):**
- Problem: Cache get/insert operations iterate through VecDeque to find positions.
- Files: `src/retrieval/cache.rs:29-30, 43-44`
- Cause: No position index maintained; VecDeque::iter_mut() with enumerate is O(n).
- Improvement path: Replace VecDeque with LinkedHashMap or maintain index; benchmark against current performance.

## Fragile Areas

**Tree-Sitter AST walking is language-specific:**
- Files: `src/indexer/extract/typescript.rs` (619 lines), `src/indexer/extract/rust.rs` (393 lines), `src/indexer/extract/javascript.rs` (327 lines), plus Go, Java, C, C++
- Why fragile: Each language has separate walker logic. Bug fixes don't propagate. Adding language support duplicates code. Node kinds vary across tree-sitter versions.
- Safe modification: Consolidate common patterns (walk, node kind matching); use macros for repeated patterns; add property-based tests for AST roundtrips.
- Test coverage: Each extractor has basic tests, but no cross-language consistency tests or edge case coverage for malformed code.

**Symbol ID generation via FNV-1a hash:**
- Files: `src/indexer/pipeline/utils.rs:53-71` (stable_symbol_id)
- Why fragile: Symbol uniqueness depends on `file_path:name:start_byte` triplet. File path normalization matters; collisions unlikely but not impossible.
- Safe modification: Add hash collision detector; version the stable_symbol_id scheme; document assumptions about path normalization.
- Test coverage: No collision tests; no roundtrip tests for symbol ID stability across indexing runs.

**Ranking signal weighting has many tuning parameters:**
- Files: `src/retrieval/ranking/score.rs` (398 lines), `src/config.rs` (multiple weight env vars)
- Why fragile: 9+ signals (keyword, vector, exported, test_penalty, popularity, intent_mult, definition_bias, directory semantics) interacting. Small changes cascade. No sensitivity analysis.
- Safe modification: Document signal meanings in comments; add telemetry for signal values; create ablation test suite.
- Test coverage: Unit tests exist but don't cover signal interaction or expected ranking order across queries.

**SQLite schema migrations are version-based:**
- Files: `src/storage/sqlite/schema.rs` (versioning logic)
- Why fragile: Schema version checking depends on hardcoded constants. No migration system for schema changes; full rebuild required on version change.
- Safe modification: Implement proper migration system with up/down scripts; version the schema separately from code version; add migration tests.
- Test coverage: No migration tests; no schema compatibility tests across versions.

**Embeddings model initialization is async but initialization is blocking:**
- Files: `src/embeddings/fastembed.rs:41-42`, initialization in `src/main.rs:93-98`
- Why fragile: FastEmbed model downloads and initialization happens synchronously on startup. Network delays block server startup; large models cause timeouts.
- Safe modification: Lazy-load embeddings model on first use; add progress callbacks; set reasonable timeouts; retry logic for model downloads.
- Test coverage: Tests use hash embeddings; no integration tests for actual model download/initialization.

**Index patterns are glob strings with no validation:**
- Files: `src/config.rs` (INDEX_PATTERNS, EXCLUDE_PATTERNS), `src/indexer/pipeline/scan.rs` (globbing logic)
- Why fragile: Invalid glob patterns fail silently or match unexpected files. No glob validation at startup.
- Safe modification: Validate glob patterns during config loading; log pattern matches for first few files; add glob test cases.
- Test coverage: No glob pattern validation tests; no roundtrip tests for include/exclude combinations.

## Scaling Limits

**Tantivy index is single-writer:**
- Current capacity: Suitable for < 1M symbols; write throughput ~1000 symbols/sec on single thread.
- Limit: Only one IndexWriter at a time; can't parallelize writes; commits serialize for consistency.
- Scaling path: Use write-ahead logging; partition index by file type; use read replicas for searches.

**LanceDB vector table lacks partitioning:**
- Current capacity: ~100k vectors on single table before noticeable slowdown; full scan on every search.
- Limit: Linear search time grows with table size; no pagination or filtering before distance computation.
- Scaling path: Implement IVF partitioning; use approximate nearest neighbor search; partition by language/file.

**Retrieval cache is unbounded in item count:**
- Current capacity: 64-256 cache entries depending on type; ~4-8 MB memory for embeddings, ~8 MB for contexts.
- Limit: Memory grows linearly; no cache eviction based on age or frequency beyond LRU count.
- Scaling path: Set explicit memory budgets; use timed eviction; consider disk-based cache layer.

**Graph traversal visits all edges up to limit:**
- Current capacity: 100-1000 edges per traversal; depth limit of 3-5 prevents explosion.
- Limit: With highly connected codebases, edge/depth limits will truncate results; no ranking of edges by importance.
- Scaling path: Add edge ranking by signal strength; use probabilistic sampling; implement IVF-style clustering.

**Embeddings model is loaded per process:**
- Current capacity: 1 model per process; fine for single-core server. Embedding generation is single-threaded.
- Limit: Model loading takes 1-2 seconds; cache is process-local; multiple processes don't share embeddings.
- Scaling path: Embed model as service; use model quantization; implement embeddings cache with LRU+disk.

## Dependencies at Risk

**FastEmbed version 4 is recent, model API may change:**
- Risk: FastEmbed 4 uses ORT runtime; newer versions may drop support for older ONNX models or change initialization API.
- Impact: Model download failures or compatibility issues; embedding dimension may change.
- Migration plan: Pin FastEmbed version in Cargo.toml; add compatibility layer for model API changes; test against multiple FastEmbed versions.

**Tantivy 0.25 is stable but search API may evolve:**
- Risk: Tantivy is pre-1.0; minor versions can introduce API changes; custom tokenizer (`CodeTokenizer`) may break.
- Impact: Compilation failures; search scoring may change; tokenizer assumptions invalid.
- Migration plan: Pin Tantivy version; monitor Tantivy releases; maintain custom tokenizer in separate module with clear interface.

**LanceDB 0.23 is young and actively changing:**
- Risk: LanceDB is pre-1.0 with frequent breaking changes; table schema, API, and on-disk format may evolve.
- Impact: Vector table becomes unreadable; API changes require code rewrites; performance characteristics may degrade.
- Migration plan: Pin LanceDB version; add version checks on vector table open; implement migration layer for vector format changes.

**Rust edition 2021 and MSRV unclear:**
- Risk: No documented minimum supported Rust version; project uses newer async patterns.
- Impact: Users on older Rust toolchains can't compile; dependency conflicts if MSRV differs.
- Migration plan: Document MSRV explicitly; test CI against stable, beta, nightly; use `rust-version` in Cargo.toml.

## Missing Critical Features

**No incremental/differential indexing:**
- Problem: `index_all()` rescans entire codebase every time; no delta-based updates for changed files only.
- Blocks: Indexing large codebases (>100k files) takes minutes; watch mode rebuilds entire index on any file change.
- Fix approach: Implement file fingerprinting (mtime + size); track last indexed state; implement delta indexing for symbols/edges; optimize watch mode to batch changes.

**No symbol deduplication or versioning:**
- Problem: Symbols are immutable; if function signature changes, old version remains in index.
- Blocks: Historical querying; symbol evolution tracking; can't distinguish overloaded functions across versions.
- Fix approach: Add symbol versioning; implement soft deletes; version edges independently.

**Graph traversal results don't include relationship context:**
- Problem: Graph edges have type (call, reference, extends) but no confidence/resolution metadata returned.
- Blocks: Ranking callers by importance; distinguishing direct vs indirect relationships; identifying circular dependencies.
- Fix approach: Include resolution and evidence count in graph JSON; add relationship confidence scoring.

**No support for cross-repo navigation:**
- Problem: Imports are resolved locally within single BASE_DIR; no support for monorepos or multi-repo workspaces.
- Blocks: Multi-package TypeScript workspaces; monorepo navigation; inter-service call graphs.
- Fix approach: Support multiple repo roots; implement cross-repo edge tracking; add workspace-aware path resolution.

**No comment/documentation extraction:**
- Problem: Symbol extraction skips comment text and docstrings. Only code symbols are indexed.
- Blocks: Finding functions by their documentation; discovering hidden functionality via comments; improving relevance ranking with doc text.
- Fix approach: Extract JSDoc/RustDoc comments; index alongside symbols; weight doc matches differently in ranking.

## Test Coverage Gaps

**Untested UTF-8 handling in AST extractors:**
- What's not tested: Invalid UTF-8 sequences in source files; multi-byte characters in identifiers; different encodings.
- Files: `src/indexer/extract/c.rs`, `src/indexer/extract/*.rs` (all extractors)
- Risk: Panics on non-UTF-8 files; silent corruption if encoding conversion fails.
- Priority: High - crashes block entire file indexing

**No integration tests for schema migrations:**
- What's not tested: Upgrading from old schema version; rollback; concurrent access during migration.
- Files: `src/storage/sqlite/schema.rs`
- Risk: Silent data loss or corruption when upgrading server version; inconsistent state.
- Priority: High - data loss risk

**Graph cycle detection not tested:**
- What's not tested: Circular call graphs; A->B->C->A patterns; diamond dependencies.
- Files: `src/graph/mod.rs:39-120`
- Risk: Graph traversal may revisit nodes, inflating edge counts or causing performance degradation.
- Priority: Medium - affects ranking and performance

**LanceDB vector dimension mismatch not covered:**
- What's not tested: Recovery from dimension mismatch; partial batch failures; schema changes.
- Files: `src/storage/vector.rs:111-125`
- Risk: Batch indexing fails silently; vectors aren't persisted.
- Priority: High - data loss risk

**Ranking signal interaction edge cases:**
- What's not tested: All signals at extreme values; conflicting signals (test_penalty vs definition_bias); empty results.
- Files: `src/retrieval/ranking/score.rs`
- Risk: Unexpected ranking behavior in unusual queries; signals interact in unforeseen ways.
- Priority: Medium - affects search quality

**Glob pattern edge cases:**
- What's not tested: Overlapping include/exclude patterns; Windows path separators in patterns; symlink handling.
- Files: `src/config.rs`, `src/indexer/pipeline/scan.rs`
- Risk: Unexpected files indexed or skipped; cross-platform path issues.
- Priority: Medium - user confusion and data loss risk

---

*Concerns audit: 2026-01-22*

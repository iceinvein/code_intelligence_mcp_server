# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-01-22)

**Core value:** Search results are highly relevant and contextually rich - the right code, with the right context, every time.
**Current focus:** Phase 9 - Multi-Repo Support (final phase)

## Current Position

Phase: 9 of 9 (Multi-Repo Support) -> IN PROGRESS
Plan: 06 of 7 in current phase -> COMPLETE
Status: Plan complete, 5/7 done in Phase 9 (09-05 documented)
Last activity: 2026-01-25 - Completed 09-05 (batch package lookup and package-aware ranking)

Progress: [██████████] 99% (37 of 37 plans)

Phase breakdown:
- Phase 1 (Foundation): [██████████] 100% (3/3)
- Phase 2 (Graph Intelligence): [██████████] 100% (3/3)
- Phase 3 (Retrieval Enhancements): [██████████] 100% (3/3)
- Phase 4 (Context Assembly): [██████████] 100% (2/2)
- Phase 5 (Learning System): [██████████] 100% (2/2)
- Phase 6 (New MCP Tools): [██████████] 100% (10/10)
- Phase 7 (Language Enhancements): [██████████] 100% (7/7)
- Phase 8 (Performance & Scale): [██████████] 100% (3/3)
- Phase 9 (Multi-Repo Support): [█████████░] 71% (5/7)

## Performance Metrics

**Velocity:**
- Total plans completed: 37
- Average duration: 6.4min
- Total execution time: 4.0 hours

**By Phase:**

| Phase | Plans | Complete | Avg/Plan |
|-------|-------|----------|----------|
| 1     | 3     | 3        | 6.7min   |
| 2     | 3     | 3        | 7.7min   |
| 3     | 3     | 3        | 10.3min  |
| 4     | 2     | 2        | 5.5min   |
| 5     | 2     | 2        | 5.0min   |
| 6     | 10    | 10       | 5.6min   |
| 7     | 7     | 7        | 4.6min   |
| 8     | 3     | 3        | 12.3min  |
| 9     | 5     | 5        | 7.8min   |

**Recent Trend:**
- Last 5 plans: 09-06 (13min), 09-04 (7min), 09-03 (~8min), 09-02 (14min), 09-01 (7min)
- Trend: Phase 9 progressing - package boost integrated with query controls

*Updated after each plan completion*

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- **RwLock<Connection> for thread-safe SQLite access:** Replaced RefCell with RwLock to make SqliteStore Send + Sync, enabling parallel indexing and metrics. Each method uses read() or write() guard for appropriate access. (08-03)
- **Prometheus metrics on localhost-only:** /metrics endpoint binds to 127.0.0.1 preventing external access. Port configurable via METRICS_PORT (default 9090). Enabled by default (METRICS_ENABLED=true). (08-03)
- **Histogram buckets for latency tracking:** Indexing (1ms-10min) and Search (1ms-5s) histograms provide p50/p95/p99 percentiles. Timer observes duration on drop for automatic measurement. (08-03)
- **Arc<MetricsRegistry> singleton pattern:** Single metrics registry passed to IndexPipeline and Retriever. Metrics updates use f64 (not u64) per Prometheus API. (08-03)
- **Axum 0.8 for HTTP server:** Removed optional feature flag, metrics server always available. MetricsState Clone requirement satisfied by Arc<MetricsRegistry>. (08-03)
- **SHA-256 content hash for embedding cache keys:** Text hashed with SHA-256 to create cache keys. Changes to text content automatically invalidate cache entries without manual cache busting. (08-02)
- **Composite cache key (model_name + text_hash):** Cache keys combine model name and content hash: SHA-256("{model_name}|{text_hash}"). Prevents cache collisions when using different embedding models. (08-02)
- **Postcard binary serialization for embedding storage:** Vec<f32> embeddings serialized with postcard::to_allocvec() for compact binary storage. More space-efficient than JSON. (08-02)
- **Lazy LRU cleanup every 1000 cache misses:** Cache cleanup runs on cache miss (not hit) to avoid overhead. Deletes oldest 10% of entries when size exceeds 1GB. (08-02)
- **Embedding cache enabled by default:** EMBEDDING_CACHE_ENABLED=true (default) enables caching. Set to false for special cases. Cache statistics logged after each index run. (08-02)
- **Parallel indexing excludes embeddings due to async incompatibility:** Vector embeddings require async embedder access incompatible with Rayon's sync parallelism. Parallel mode (`PARALLEL_WORKERS>1`) skips embeddings, sequential mode includes them. (08-01)
- **SQLite WAL mode for concurrent access:** PRAGMA journal_mode=WAL enables multi-reader/single-writer without connection pooling. Each worker thread creates its own SqliteStore connection. (08-01)
- **Busy timeout prevents SQLite lock errors:** PRAGMA busy_timeout=5000 (5 seconds) retries on lock contention instead of immediate failure. (08-01)
- **Thread count defaults to num_cpus::get() with PARALLEL_WORKERS override:** Configurable via env var for performance tuning. (08-01)
- **TODO/FIXME comment extraction via tree-sitter:** Uses AST comment node traversal to find TODO/FIXME keywords in code comments only. (07-02B)
- **TODO extraction associates with nearest following symbol:** TODOs link to the next symbol declaration for context. (07-02B)
- **Test file detection via naming patterns:** Uses .test., .spec., _test.*, /test/, /tests/, /__tests__/, /spec/ patterns instead of import analysis for simplicity and performance. (07-02A)
- **Test link direction defaults to bidirectional:** Supports both finding tests for a source file and finding sources for a test file. (07-02A)
- **TODO ID format:** Uses "{file_path}:{line}" format for unique IDs without requiring a separate auto-increment column. (07-02A)
- **associated_symbol field in TodoEntry:** Included for future symbol-level TODO association when extraction is implemented. (07-02A)
- **Composite primary key for decorators:** (symbol_id, name) allows multiple decorators per symbol while preventing duplicates. (07-01A)
- **String decorator_type in storage:** Stores decorator type as string for flexibility; DecoratorType enum used at application layer for categorization. (07-01A)
- **ON DELETE CASCADE for decorators:** Automatic cleanup of decorator entries when parent symbol is deleted. (07-01A)
- **1.5x multiplier for JSDoc documentation boost:** Well-documented symbols receive 50% score boost to promote them in search results. (07-01B)
- **JSDoc parsing via AST traversal:** JSDoc comments extracted by walking previous siblings of symbol nodes looking for /** comments. (07-01B)
- **Decorator classification by string matching:** Framework decorators (Angular Component, NestJS Controller) classified by name for flexibility with custom decorators. (07-01B)
- **JSDoc boost applied in search pipeline:** apply_docstring_boost_with_signals now called in search ranking to give 1.5x multiplier to documented symbols. (07-04)
- **Docstrings table populated during indexing:** The existing docstrings table is now populated during TypeScript/JavaScript symbol indexing. (07-01B)
- **JSDoc context assembly integration:** format_symbol_with_docstring renders JSDoc summary, params, returns, and examples as markdown in search results. (07-03)
- **Docstring retrieval per-symbol:** Docstrings fetched individually during context assembly rather than batched for simplicity. (07-03)
- **Root-only docstring formatting:** Only root symbols get JSDoc formatting to avoid cluttering expanded/extra symbols. (07-03)
- **Token accounting includes documentation:** Formatted text with JSDoc used for token counting, ensuring budget reflects actual output. (07-03)
- **Decorator search via MCP tool:** SearchDecoratorsTool provides decorator name filtering (exact or prefix match) and decorator_type filter for framework categorization. (07-05)
- **Decorator results enriched with symbol context:** Each decorator search result includes the decorated symbol's name, file path, line, language, and kind. (07-05)
- **Decorator limit range 1-500:** Decorator search accepts configurable limit with reasonable bounds to prevent runaway queries, default 50. (07-05)
- **Test infrastructure with HashEmbedder for fast testing:** Uses hash-based embeddings in tests to avoid model downloads while maintaining full indexing pipeline coverage. (06-10)
- **Static EMPTY array for borrow compatibility:** Used `static EMPTY: &[serde_json::Value] = &[]` instead of `&vec![]` to avoid temporary value issues with unwrap_or in tests. (06-10)
- **AsyncMutex for embedder type compatibility:** Tests use `tokio::sync::Mutex as AsyncMutex` to match Retriever's internal embedder requirements. (06-10)
- **File purpose inference via export ratio:** >80% exports = "module", >0% = "mixed-exports", 0% = "internal". Combines with kind detection (type-defs, functions, classes). (06-04)
- **Signature extraction keeps first line as-is:** Preserves async/const modifiers, only strips "export" and "pub" prefixes. Truncates to 100 chars with "..." suffix. (06-04)
- **Upstream-only traversal for impact analysis:** Uses build_dependency_graph with "upstream" direction to find reverse dependencies (who depends on this symbol). (06-03)
- **Impact grouping by export status:** Exported symbols marked as "high impact" (potential API breakage), internal symbols as "medium impact". (06-03)
- **Test file filtering for impact:** include_tests flag defaults to false to exclude test files from impact analysis by default. (06-03)
- **Impact depth default 3:** Limits traversal to 3 levels to prevent overwhelming results while capturing meaningful impact chains. (06-03)
- **Signature extraction uses kind-based line limits:** class/interface/struct get 3 lines, function/method get 2 lines, default is 1 line. Captures declaration without body. (06-05)
- **Signature length limited to 200 characters:** Truncates with '...' for display readability, prevents overly long signatures. (06-05)
- **Module summary export-only filtering:** Uses exported_only=true to focus on public API surface, not internal symbols. (06-05)
- **group_by_kind defaults to false:** Flat output for simple cases, organized kind-based sections when enabled. (06-05)
- **Similarity threshold default 0.5:** Filters out weak semantic matches while allowing discovery. Configurable via tool parameter. (06-02)
- **find_similar_code limit default 20, max 100:** Higher than search_code for comprehensive discovery, capped to prevent overwhelming results. (06-02)
- **FindSimilarCodeTool flexible input:** Uses symbol_name/code_snippet fields instead of symbol_id for flexible input - either look up symbol by name or embed arbitrary code. (06-02)
- **LanceDB filtering with only_if():** Use QueryBase.only_if() method (not filter()) for pre-filtering vector search results. (06-01)
- **Similarity scoring formula:** 1.0 / (1.0 + distance) converts LanceDB distance to 0-1 similarity range for intuitive thresholding. (06-01)
- **explain_search HitSignals exposure:** Full breakdown of keyword_score, vector_score, base_score, structural_adjust, intent_mult, definition_bias, popularity_boost, learning_boost, affinity_boost. (06-01)

- **File affinity boost:** (view_count + edit_count * 2) * exp(-0.05 * age_in_days) with edit_count weighted 2x. (05-02)
- **Affinity time decay lambda=0.05:** Slower decay than selections (0.1) because file affinity changes more gradually. (05-02)
- **Affinity boost pipeline position:** Applied after selection boost, before reranking. (05-02)
- **Position discount for selections:** 1.0 / ln(position + 2.0) gives higher weight to earlier selections. (05-01)
- **Time decay for learning:** exp(-0.1 * age_in_days) with lambda=0.1 for gradual obsolescence. (05-01)
- **Learning opt-in:** Selection tracking disabled by default (LEARNING_ENABLED=false). (05-01)
- **Selection boost weight:** Configurable via LEARNING_SELECTION_BOOST (default 0.1). (05-01)
- **Smart truncation with BM25-like scoring:** Header/footer lines always preserved (5 header, 3 footer). Word-boundary matches get 2x bonus over substring. Structural keywords (fn, class, return) get relevance bonus. (04-02)
- **Dependency resolution via graph edges:** resolve_parameter_types() finds type/reference edges. resolve_parent_classes() finds extends/implements edges. auto_include_dependencies() respects token budget. (04-02)
- **Structured markdown output format:** Output organized into ## Definitions, ## Examples, ## Related sections. Uses ### for individual symbol headers. (04-02)
- **TokenCounter singleton with tiktoken-rs:** Use OnceCell<TokenCounter> wrapping CoreBPE for efficient token counting. Default encoding is o200k_base (GPT-4o/o1/o3/o4). (04-01)
- **Token-based budgeting:** Replace max_context_bytes with max_context_tokens in ContextAssembler. Track used_tokens instead of used bytes. Cache key changed from "b=" to "t=". (04-01)
- **ContextItem tracks tokens:** Changed ContextItem.bytes to ContextItem.tokens for accurate LLM budget tracking. Cache size calculation uses tokens * 4 approximation. (04-01)
- **RRF k=60 constant for rank fusion:** Standard RRF constant prevents division by zero while giving appropriate weight to top ranks. RRF defaults to enabled for better multi-source fusion. (03-03)
- **RRF source weights configurable:** Keyword and vector weights default to 1.0, graph weight defaults to 0.5 as PageRank signals are supplementary. (03-03)
- **HyDE defaults to disabled:** Requires LLM API key; users must opt-in via HYDE_ENABLED=true. Supports OpenAI, Anthropic, and mock backends. (03-03)
- **Hypothetical code fused via RRF:** HyDE generates hypothetical code, embeds it, and adds results to vector hits before RRF fusion. (03-03)
- **Cross-encoder reranker with 30% weight:** Query-document pair scoring blended with existing BM25+vector scores. 30% reranker weight maintains ranking stability while improving precision. (03-02)
- **ORT 2.0 API compatibility:** Updated to Session::builder()?.with_execution_providers()?.commit_from_file() pattern for ort 2.0-rc.5. (03-02)
- **Hash-based reranker cache keys:** Query hash + document ID hashes (not full text) for memory efficiency. (03-02)
- **Semaphore-based reranker concurrency:** Limited to 4 parallel reranking operations to prevent CPU overwhelm. (03-02)
- **Reranker graceful degradation:** When model not found, returns Ok(None) and search continues without reranking. (03-02)
- **Jina Code via FastEmbed:** Chose to use FastEmbed's built-in Jina model rather than direct ORT implementation. This simplifies the code and leverages tested ONNX Runtime integration. (03-01)
- **Vector migration on mismatch:** LanceDB table dropped when dimensions don't match (e.g., 384 -> 768). Forces clean re-index rather than attempting complex data migration. (03-01)
- **Default backend change:** JinaCode is now the default embedding backend, replacing FastEmbed (BGE). This provides better code understanding out of the box. (03-01)
- **Embedder factory pattern:** create_embedder() abstracts backend creation for runtime polymorphism (03-01)
- **Model-based dimensions:** FastEmbedder.dim() returns model-specific dimensions (384 for BGE, 768 for Jina) (03-01)
- **PageRank normalization to 0-1:** Search ranking uses normalized PageRank for consistent boost magnitudes (02-02)
- **Batch query for performance:** O(1) batch_get_symbol_metrics replaces O(N) count_incoming_edges (02-02)
- **Data flow edge model:** Track reads/writes at symbol level within function context (02-03)
- **Import resolution:** Two-tier approach - basic (no DB) for extraction, enhanced (with DB) for accuracy (02-03)
- **PageRank FILE_ROOT exclusion:** Symbols with kind="file" excluded from PageRank to avoid skew (02-01)
- **PageRank computation timing:** Run once after full index completes, not per-file (02-01)
- **f64 for PageRank:** Use f64 (not f32) for PageRank to minimize floating-point accumulation errors (02-01)
- Cross-encoder always-on (quality improvement justifies latency)
- ORT for reranker (local-only, cross-platform)
- Learning on by default (local storage, no privacy concerns)
- Token-based budgeting (LLMs count tokens not bytes)
- **ort version:** Use `ort = "2.0.0-rc.5"` to match fastembed dependency (01-01)
- **Query expansion defaults:** Synonym and acronym expansion enabled by default (01-01)
- **Intent detection ordering:** More specific intents checked first (Migration before Schema, Error before Definition) (01-03)
- **Intent ranking:** All new Intent variants default to 1.0 multiplier in ranking (can be tuned) (01-03)
- **RwLock for SqliteStore thread safety:** Replaced RefCell with RwLock<Connection> to make SqliteStore Send + Sync for parallel indexing. (08-03)
- **WAL mode for concurrent SQLite access:** PRAGMA journal_mode=WAL enables multiple readers with one writer for parallel indexing. (08-01)
- **Parallel indexing skips embeddings:** Rayon is synchronous, embeddings are async. Tradeoff: ~2-3x speedup for symbol extraction, sequential for embeddings. (08-01)
- **SHA-256 for embedding cache keys:** Composite hash of "{model_name}|{text_hash}" prevents cache collisions across models. (08-02)
- **Postcard serialization for cache:** Binary format for compact Vec<f32> storage (~4KB per 768-dim embedding). (08-02)
- **Lazy LRU cache cleanup:** Triggers every 1000 misses when size exceeds 1GB, deletes oldest 10% of entries. (08-02)
- **Prometheus metrics via axum:** HTTP server on localhost only (127.0.0.1) with configurable port (default 9090). (08-03)
- **Arc<MetricsRegistry> without Mutex:** Prometheus metrics use atomics internally, no need for Mutex wrapper. (08-03)
- **Git root detection using git2::Repository::discover():** Traverses parent directories to find .git, handles worktrees, bare repos, and submodules correctly. (09-03)
- **SHA-256 hash for repository ID:** Repository ID is SHA-256 hash of root_path string for stable unique identifiers. (09-03)
- **Package lookup by file path prefix:** get_package_for_file uses LIKE with manifest_path prefix and ORDER BY LENGTH DESC to find deepest containing package. (09-03)
- **Language-specific manifest parsers:** npm (package.json), Cargo (Cargo.toml), Go (go.mod), Python (pyproject.toml) using serde_json, toml, and regex. (09-02)
- **Parser dispatcher by filename:** parse_manifest() routes to correct parser based on manifest filename literal match. (09-02)
- **Graceful degradation for missing fields:** Parsers return None for name/version when fields are missing instead of failing. (09-02)
- **PEP 621 first, Poetry fallback for Python:** Checks [project] table first (modern), falls back to [tool.poetry] (legacy). (09-02)
- **Workspace detection via manifest fields:** npm workspaces (workspaces array), Cargo (workspace.members), Go (replace directives with relative paths). (09-02)
- **Module name derivation for Go:** Package name extracted as last component of module path (e.g., "github.com/user/repo" -> "repo"). (09-02)
- **toml 0.8 and regex 1.0 dependencies:** Added for TOML parsing (Cargo, Python) and line-based Go module parsing. (09-02)
- **discover_packages iterates over repo_roots:** For each repo_root, calls discover_manifests and parse_manifest to build complete PackageInfo list. (09-04)
- **detect_repositories uses git root detection:** Calls git::discover_git_roots() and assigns repository_id to each package based on path prefix. (09-04)
- **Package detection enabled by default:** PACKAGE_DETECTION_ENABLED=true (default) enables automatic package/repository discovery on each index run. (09-04)
- **Graceful degradation for package detection:** Package detection failures log warnings but don't stop indexing; continues with file scanning. (09-04)
- **Display implementations for package types:** PackageType and VcsType implement Display returning lowercase strings (npm, cargo, git, etc.). (09-04)
- **Query control parameter for package context:** Users can specify "package:name" or "pkg:name" in queries to filter results to specific packages. (09-06)
- **Package boost integrated into search pipeline:** apply_package_boost_with_signals called after file affinity, with 1.15x default multiplier. (09-06)
- **HitSignals.package_boost for debugging:** package_boost field added to track applied boost amount in hit_signals. (09-06)
- **Batch package lookup with SQL IN clause:** batch_get_symbol_packages uses params_from_iter for efficient symbol-to-package mapping in O(1) query. (09-05)
- **File-to-package lookup via LIKE prefix:** get_package_id_for_file uses LIKE with manifest_path || '%' and ORDER BY LENGTH DESC to find deepest containing package. (09-05)
- **Intent-based package boost multipliers:** Navigation intents (Definition, Implementation, Callers) get 1.2x, Error gets 1.1x, others get 1.15x for same-package results. (09-05)
- **Auto-detect package context from first hit:** When query_package_id is None, package context is detected from first hit's file path for zero-configuration usage. (09-05)
- **HashMap<String, HitSignals> for ranking signals:** Package boost follows existing pattern using HashMap instead of slice for hit_signals tracking. (09-05)

### Pending Todos

None yet.

### Blockers/Concerns

None remaining.

## Session Continuity

Last session: 2026-01-25
Stopped at: Completed 09-05 (batch package lookup and package-aware ranking), created SUMMARY.md
Resume file: None
Next: Phase 9 Plan 07 - Cross-package symbol resolution

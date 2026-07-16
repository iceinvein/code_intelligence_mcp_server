# Code Intelligence Improvement Work Log

- Date: 2026-07-16
- Status: In progress — G1 implementation complete; clean-index and quality gates pending
- Baseline commit: `e4d613f`
- Baseline benchmark: R040
- Scope: fidelity, indexing and query performance, retrieval architecture, and engineering feedback loops

## Objective

Build on the recent answer-contract and evidence-shaping gains while improving the correctness of the underlying index. Work is ordered so that graph integrity is restored before further ranking, traversal, or performance tuning is evaluated.

The desired outcome is a code-intelligence engine whose evidence is structurally trustworthy, whose impact analysis covers public API paths, whose latency and storage costs are observable, and whose retrieval pipeline can be changed without accumulating order-dependent heuristics.

## Baseline

### Quality benchmark

R040 improved materially over the comparable R013 full gate.

| Metric | R013 | R040 | Change |
|---|---:|---:|---:|
| Mechanical score | 0.662 | 0.884 | +0.222 |
| Judge score | 7.95 | 8.28 | +0.33 |
| Citation score | 0.713 | 0.875 | +0.162 |

The weakest R040 category is impact analysis:

| Category | Questions | Mechanical | Judge | Citation |
|---|---:|---:|---:|---:|
| Impact | 12 | 0.766 | 7.25 | 0.667 |

Source: [R040 benchmark report](../../bench/results/R040/report.md).

R040 covers Django and wolfmax only. Its elapsed time and token totals include the external answering agent, so they are not direct server latency measurements.

### Live index snapshot

Snapshot inspected on 2026-07-16 from the registered index for this repository. The index predates the baseline commit, but the responsible indexing code remains present at the baseline.

| Metric | Value |
|---|---:|
| Indexed files | 347 |
| Symbols | 5,420 |
| Total graph edges | 1,444,925 |
| Read edges | 1,024,491 |
| Write edges | 388,524 |
| Edges outside source-symbol line range | 1,339,983 |
| Edges whose target has no symbol row | 1,344,167 |
| SQLite size | approximately 657 MB |
| Recorded full index duration | approximately 459.6 seconds |
| Recorded no-op index duration | approximately 99 ms |

Read and write edges account for approximately 98% of all edges. Edge storage and indexes account for most of the SQLite database.

### Initial validation state

- `cargo fmt --all -- --check`: passed.
- `EMBEDDINGS_BACKEND=hash cargo test`: project tests did not start because the build environment lacks `cmake`.
- The first test attempt also exposed a parallel extraction race in `scripts/protoc`; prewarming an isolated cache bypassed the race.
- Worktree was clean before this log was created.

## Operating principles

1. Correctness gates precede ranking experiments and performance claims.
2. Every graph endpoint must have an explicit type and valid ownership semantics.
3. Quality changes must report both coverage and false-positive cost.
4. Performance changes must use direct server measurements, separated from agent-answering time.
5. New investigation heuristics must declare their applicability, evidence role, cost, and confidence.
6. Each completed workstream updates this log with before/after measurements and links to its benchmark run.

## Workstream status

| ID | Priority | Workstream | Status | Depends on |
|---|---|---|---|---|
| G1 | P0 | Repair data-flow ownership and graph integrity | In progress — implementation and tests complete; clean reindex pending | — |
| G2 | P1 | Model imports, exports, re-exports, aliases, and wrappers | Not started | G1 |
| F1 | P1 | Introduce an evidence coverage contract | Not started | G1 |
| F2 | P1 | Harden symbol identity and ambiguous resolution | Not started | G1 |
| P1 | P1 | Establish direct performance telemetry and gates | Not started | G1 |
| P2 | P2 | Refactor repository-scoped storage and query concurrency | Not started | P1 |
| A1 | P2 | Replace investigation pass ladder with typed enrichment pipeline | Not started | F1, P1 |
| Q1 | P1 | Expand deterministic quality evaluation and CI | Not started | G1 |
| B1 | P2 | Repair reproducible native build prerequisites | Not started | — |

## G1 — Repair data-flow ownership and graph integrity

### Finding

[`extract_edges_for_symbol`](../../src/indexer/pipeline/edges.rs#L222) receives the file's complete `dataflow_edges` collection for each symbol. The loop beginning at [the data-flow edge block](../../src/indexer/pipeline/edges.rs#L640) does not restrict an edge to its `to_symbol` owner before assigning the current `row.id` as `from_symbol_id`.

The same block creates synthetic `local:` and `async:` target IDs without corresponding symbol rows while relying on foreign-key enforcement being disabled during batch writes. Rust and TypeScript data-flow extraction also records Tree-sitter's zero-based row directly rather than converting to the one-based line convention used elsewhere.

### Tasks

- [x] Add a minimal regression fixture with multiple symbols and local reads/writes in one file.
- [x] Match extracted data-flow edges to lexical context and the owning symbol's source span so sibling and repeated-name scopes cannot receive each other's edges.
- [x] Attach each data-flow edge only to its lexical owner.
- [x] Restrict the generic graph to symbol-to-symbol relationships; unresolved local and async endpoints are omitted until a typed non-symbol relation exists.
- [ ] Store non-symbol flow entities in a typed table or separate data-flow relation instead of inserting broken symbol foreign keys.
- [x] Convert Rust and TypeScript Tree-sitter rows to one-based lines.
- [x] Add post-index foreign-key and source-location integrity checks plus focused regression tests.
- [x] Bump the graph index format and require a clean graph rebuild.
- [ ] Reindex this repository and record before/after edge counts, SQLite size, full-index duration, and representative graph results.

### Required invariants

- Every source symbol exists.
- Every target exists or is explicitly represented as a typed non-symbol/external entity.
- Every source-owned edge location falls within the source symbol span, except for a documented edge type that deliberately uses a file-level location.
- `PRAGMA foreign_key_check` reports no violations.
- Edge growth per file is bounded by extracted relationships rather than `symbols × file relationships`.
- Reindexing the same unchanged revision produces equivalent graph contents.

### Acceptance gate

- [ ] The regression fixture fails on the baseline and passes with the repair.
- [ ] No unintended missing endpoints remain in a clean self-index.
- [x] No data-flow edge is attributed to an unrelated sibling symbol in the regression fixture.
- [ ] Full-index storage and duration are remeasured; no estimate is presented as a measured result.
- [ ] R040 mechanical, judge, and citation scores do not regress outside normal run variance.

## G2 — Model the public API and impact graph

### Finding

Impact answers still miss public barrels/re-exports, wrapper modules, and canonical exposure paths. Native graph edges do not yet provide a complete first-class model for import/export bindings and delegation.

### Tasks

- [ ] Define typed edges for import, export, re-export, alias, and delegation/wrapper relationships.
- [ ] Preserve local name, imported name, exported name, module specifier, and resolution confidence.
- [ ] Represent unresolved and ambiguous bindings explicitly; do not silently choose a global same-name target.
- [ ] Update impact traversal to distinguish implementation dependencies from public exposure paths.
- [ ] Add adversarial fixtures for barrels, chained re-exports, renamed imports, default exports, wrappers, and cycles.
- [ ] Evaluate external language-service overlays for impact/reference queries per language.

### Acceptance gate

- [ ] Golden impact sets report precision and recall, not only whether one expected file appeared.
- [ ] Public entry points and wrapper paths are returned with evidence roles.
- [ ] Ambiguous imports do not create high-confidence false edges.
- [ ] Impact scores improve over the R040 baseline without degrading symbol and negative-query categories.

## F1 — Introduce an evidence coverage contract

### Finding

Recent evidence injection passes improved the benchmark, but coverage is still implicit and query-specific. Impact answers can omit the canonical definition or public path, while concept answers can omit the state or initializer that explains the mechanism.

### Tasks

- [ ] Define evidence roles such as canonical definition, implementation, public exposure, direct caller, wrapper/alias, state mechanism, and counter-evidence.
- [ ] Define required and optional roles per question shape.
- [ ] Return a coverage status describing which required roles were resolved, missing, or ambiguous.
- [ ] Allow exact `path:line` claims only when backed by a returned evidence location.
- [ ] Allocate evidence budget centrally by role, confidence, novelty, and cost.
- [ ] Add fixtures for class-level mechanisms such as lazy initialization and cached state.

### Acceptance gate

- [ ] Every precise source claim is traceable to returned evidence.
- [ ] Missing canonical definitions and public paths are observable as coverage failures.
- [ ] Evidence expansion improves coverage without reintroducing large, unfocused packs.

## F2 — Harden symbol identity and resolution

### Finding

Exported symbols currently use byte offset zero when constructing stable IDs in [the parse pipeline](../../src/indexer/pipeline/parse.rs#L211). File path plus unqualified name is insufficient for overloads, repeated method names, and some generated/public declarations.

### Tasks

- [ ] Separate the location-independent logical symbol ID from declaration occurrence IDs.
- [ ] Include qualified owner and signature/discriminator where the language provides them.
- [ ] Detect and fail or report ID collisions during indexing.
- [ ] Add fixtures for overloads, nested scopes, duplicate method names, partial declarations, and moved exported declarations.
- [ ] Give resolution results explicit states: exact, inferred, ambiguous, unresolved, and external.

### Acceptance gate

- [ ] No silent symbol overwrite occurs in collision fixtures.
- [ ] Stable logical identity survives harmless source movement where intended.
- [ ] Occurrence identity remains unique and source-addressable.

## P1 — Establish direct performance telemetry and gates

### Finding

Current benchmark wall time includes the external answering agent. Search metrics capture total duration, but multi-query paths omit meaningful stage timings and indexing runs do not expose enough phase detail to explain regressions.

### Tasks

- [ ] Measure cold and warm direct MCP latency for search, investigate, definition, references, and impact traversal.
- [ ] Record p50, p95, and p99 for embedding, BM25, vector search, graph expansion, reranking, evidence allocation, and serialization.
- [ ] Record cache hit/miss/single-flight wait counts and candidate counts at each stage.
- [ ] Add indexing timings for scan, parse, edge extraction, SQLite write, embedding, vector write/optimization, and Tantivy commit.
- [ ] Track DB bytes per symbol, edges per symbol, vectors per symbol, peak RSS, and GPU residency.
- [ ] Establish representative small, medium, and large repository fixtures.

### Acceptance gate

- [ ] Performance claims use direct server measurements with fixture revision and configuration recorded.
- [ ] Every major stage has actionable latency and volume telemetry.
- [ ] Quality and latency gates are reported together for retrieval experiments.

## P2 — Refactor storage and query concurrency

### Finding

Search opens and initializes SQLite on each request in [retrieval/mod.rs](../../src/retrieval/mod.rs#L316). `SqliteStore` serializes access through one connection, while a separate pool is used elsewhere. The single-query hybrid path performs synchronous BM25 before embedding and vector search in [hybrid.rs](../../src/retrieval/hybrid.rs#L77).

### Tasks

- [ ] Introduce a repository-scoped storage service initialized once per bound repository.
- [ ] Use pooled read connections and a deliberate single-writer strategy.
- [ ] Run blocking SQLite and Tantivy operations outside async executor threads.
- [ ] Batch symbol fetches used by reranking and evidence assembly.
- [ ] Add single-flight query embedding generation.
- [ ] Run independent BM25 and vector branches concurrently after direct stage telemetry exists.
- [ ] Evaluate safe concurrency/batching for multi-query retrieval.
- [ ] Invalidate response caches with an index run ID/version rather than second-resolution timestamps.
- [ ] Preserve hit signals in cached search responses so `explain_search` remains complete.

### Acceptance gate

- [ ] No request performs schema migration/initialization work.
- [ ] Concurrency tests cover simultaneous reads, indexing writes, and cache misses.
- [ ] Warm p95 improves with no retrieval-quality regression.
- [ ] Cache behavior is correct across two index runs started in the same second.

## A1 — Create a typed investigation enrichment pipeline

### Finding

[`handle_investigate`](../../src/handlers/investigation.rs#L588) has grown into a long sequence of numbered, order-dependent enrichment passes that mutate shared primary and secondary evidence collections. This was effective for recent benchmark wins but raises regression risk and makes cost attribution difficult.

### Tasks

- [ ] Introduce typed `InvestigationContext`, `EvidenceCandidate`, `EvidenceRole`, and `CoverageState` models.
- [ ] Define an enrichment-pass interface with applicability, collection, confidence, priority, and cost.
- [ ] Move deduplication, replacement, evidence budgeting, and provenance into one allocator.
- [ ] Keep `serde_json::Value` at the MCP serialization boundary rather than inside orchestration.
- [ ] Add per-pass stage metrics and an optional trace explaining why evidence was included or rejected.
- [ ] Remove the crate-wide `clippy::too_many_arguments` allowance after migrating the main hotspots, or scope it narrowly with rationale.

### Acceptance gate

- [ ] Existing enrichment behavior is captured by characterization tests before migration.
- [ ] Pass ordering and dependencies are explicit.
- [ ] Each pass can be benchmarked or disabled independently.
- [ ] Investigation output remains contract-compatible unless a versioned change is documented.

## Q1 — Expand deterministic quality evaluation and CI

### Tasks

- [ ] Add retrieval recall@k, MRR, and nDCG gates.
- [ ] Add graph edge precision/recall and impact-set precision/recall gates.
- [ ] Add canonical-definition and public-exposure coverage metrics.
- [ ] Add language fixtures for Rust, TypeScript, Python, Go, Java/Kotlin, C/C++, C#, Swift, and Ruby.
- [ ] Add small adversarial fixtures for overloads, aliases, barrels, wrappers, decorators, dynamic calls, and negative lookups.
- [ ] Audit benchmark rubrics for scope conflicts. In particular, distinguish jobs wired to a scheduler from all background maintenance in the wolfmax background-jobs question.
- [ ] Record daemon SHA, fixture SHA, configuration, model versions, and comparator in every release gate.
- [ ] Extend CI with formatting, Clippy, UI build/tests, benchmark harness tests, and a hash-backend graph-integrity smoke index.
- [ ] Update stale HTTP/stdio guidance in `TESTING.md`.

### Acceptance gate

- [ ] A deterministic engine gate can fail independently of the agent judge.
- [ ] Release reports identify their exact daemon and fixture revisions.
- [ ] Rubrics do not penalize factually correct answers that fall within the question's stated scope.

## B1 — Repair reproducible native builds

### Finding

[`scripts/protoc`](../../scripts/protoc) checks only whether the cached binary exists. Parallel Cargo build scripts can simultaneously remove and extract the same cache directory, producing truncated headers and misleading unzip errors. The local validation environment also lacks `cmake`, which `llama-cpp-sys-2` requires even when the hash embedding backend is selected.

### Tasks

- [ ] Protect protoc download/extraction with an inter-process lock.
- [ ] Extract into a unique temporary directory, verify the expected files, then atomically rename into place.
- [ ] Verify both the binary and include tree before treating the cache as complete.
- [ ] Document or automatically check native prerequisites, including `cmake`.
- [ ] Decide whether hash-backend test builds should avoid compiling the llama.cpp backend through Cargo feature separation.

### Acceptance gate

- [ ] A clean parallel Cargo build cannot corrupt the protoc cache.
- [ ] Missing native prerequisites fail early with an actionable message.
- [ ] The documented fast-test command works on a clean supported development machine.

## Execution order

### Phase 1 — Restore trust in the graph

Complete G1 and B1. Rebuild the self-index and establish the first trustworthy size, latency, and integrity baseline.

### Phase 2 — Improve fidelity where R040 is weakest

Complete G2, F1, F2, and the initial deterministic portions of Q1. Run focused impact gates before another full release gate.

### Phase 3 — Measure and improve performance

Complete P1, then apply P2 changes one at a time. Report quality, latency, memory, and storage together.

### Phase 4 — Consolidate the retrieval architecture

Complete A1 after the evidence contract and telemetry are stable enough to characterize the existing behavior.

## Experiment ledger

Add one row for every benchmark or structural experiment, including rejected changes.

| Date | ID | Change | Fixture/config | Quality result | Performance result | Decision | Links |
|---|---|---|---|---|---|---|---|
| 2026-07-16 | BASELINE | Repository and live-index assessment | `e4d613f`; R040; current self-index | R040: 0.884 mechanical, 8.28 judge, 0.875 citation | Full index approximately 459.6 s; SQLite approximately 657 MB | Prioritize G1 | This log |
| 2026-07-16 | G1-IMPLEMENTATION | Repair data-flow ownership and enforce graph integrity | Hash backend; full Rust test suite | 1,241 library tests passed, 2 ignored; all integration and doc tests passed | Clean self-index measurement pending | Keep; proceed to clean-index gate | This log |

## Decision log

| Date | Decision | Rationale | Revisit when |
|---|---|---|---|
| 2026-07-16 | Repair graph integrity before further ranking work | The majority of live edges are misattributed or have missing targets, contaminating fidelity and performance measurements | G1 acceptance gate passes |
| 2026-07-16 | Separate direct server performance from agent benchmark time | R040 wall time and tokens include the external answering agent | Direct MCP telemetry is available |
| 2026-07-16 | Keep architecture migration after the evidence contract | A typed pass pipeline needs stable evidence roles and coverage semantics to avoid merely moving current heuristics | F1 is accepted |
| 2026-07-16 | Keep the generic graph symbol-to-symbol only | Synthetic `local:` and `async:` identifiers violated the graph's symbol foreign-key contract; unresolved non-symbol endpoints are omitted until they have a typed relation | A typed data-flow relation is designed |
| 2026-07-16 | Version graph extraction semantics independently | Existing symbols and search indexes can remain available while edges and file fingerprints are invalidated for a mandatory graph rebuild | A broader persisted-index format is introduced |

## Progress log

### 2026-07-16 — Initial assessment

- Reviewed recent commits and the R013-to-R040 benchmark progression.
- Traced indexing, graph construction, hybrid retrieval, storage access, investigation enrichment, and CI boundaries.
- Queried the registered self-index to quantify edge volume, invalid locations, missing targets, database size, and index duration.
- Identified data-flow ownership fan-out as the highest-leverage correctness and performance issue.
- Confirmed formatting passes.
- Recorded test-build blockers: protoc cache race and missing `cmake`.
- Created this execution log; no production code changed.

### 2026-07-16 — G1 implementation

- Filtered file-level data-flow facts by lexical owner and source span before constructing symbol edges.
- Removed synthetic local-variable and async-boundary IDs from the symbol graph; deferred import resolution now returns no edge when the target symbol does not exist.
- Converted Rust and TypeScript data-flow positions to one-based lines.
- Kept SQLite foreign keys enabled for deferred edge writes and added post-write graph integrity validation.
- Added source-owned edge cleanup for incremental indexing, preserving valid incoming edges and pruning endpoints whose declarations disappeared.
- Added graph extraction version `2`; a version change atomically clears graph rows and file fingerprints so the next refresh performs a full rebuild.
- Added regression coverage for lexical ownership, missing endpoints, invalid source locations, changed-file cleanup, one-based positions, and graph-version invalidation.
- Ran `cargo fmt --all -- --check`, `git diff --check`, and the complete `cargo test` suite with the hash backend: 1,241 library tests passed (2 ignored), all binary and integration tests passed, and 23 doc tests passed (30 ignored).
- `cargo clippy --all-targets -- -D warnings` reaches project code but remains blocked by four pre-existing warnings in `external_index/provider.rs`, `handlers/analysis.rs`, and `indexer/pipeline/parse.rs`; no warning points to the G1 changes.
- Clean self-index size, edge-count, and duration measurements plus the R040 quality rerun remain required before G1 acceptance.

## Completion definition

This program of work is complete when:

- graph invariants pass on all supported language fixtures and representative real repositories;
- impact answers reliably include canonical implementation and public exposure paths;
- evidence coverage and uncertainty are explicit;
- direct query and indexing performance are measured by stage;
- storage and query concurrency changes demonstrate measured gains without quality regression;
- investigation enrichment is typed, observable, and independently testable; and
- release gates combine deterministic engine checks with the external-agent benchmark.

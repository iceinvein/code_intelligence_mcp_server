# Code Intelligence Improvement Work Log

- Date: 2026-07-16
- Status: Engineering complete — every remaining unchecked item is a benchmark or direct performance-measurement gate
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
| G1 | P0 | Repair data-flow ownership and graph integrity | Engineering complete — R040 benchmark rerun pending | — |
| G2 | P1 | Model imports, exports, re-exports, aliases, and wrappers | Engineering complete — impact benchmark pending | G1 |
| F1 | P1 | Introduce an evidence coverage contract | Engineering complete — evidence-pack benchmark pending | G1 |
| F2 | P1 | Harden symbol identity and ambiguous resolution | Complete — deterministic identity, collision, and resolution gates passed | G1 |
| P1 | P1 | Establish direct performance telemetry and gates | Engineering complete — direct baseline benchmarks pending | G1 |
| P2 | P2 | Refactor repository-scoped storage and query concurrency | Engineering complete — joint warm-latency/quality benchmark pending | P1 |
| A1 | P2 | Replace investigation pass ladder with typed enrichment pipeline | Engineering complete — joint quality/latency benchmark pending | F1, P1 |
| Q1 | P1 | Expand deterministic quality evaluation and CI | Complete — engine quality gate, release provenance, rubric audit, CI, and testing guidance accepted | G1 |
| B1 | P2 | Repair reproducible native build prerequisites | Complete — atomic protoc bootstrap, native feature boundary, prerequisites, and CI gates accepted | — |

## G1 — Repair data-flow ownership and graph integrity

### Finding

[`extract_edges_for_symbol`](../../src/indexer/pipeline/edges.rs#L222) receives the file's complete `dataflow_edges` collection for each symbol. The loop beginning at [the data-flow edge block](../../src/indexer/pipeline/edges.rs#L640) does not restrict an edge to its `to_symbol` owner before assigning the current `row.id` as `from_symbol_id`.

The same block creates synthetic `local:` and `async:` target IDs without corresponding symbol rows while relying on foreign-key enforcement being disabled during batch writes. Rust and TypeScript data-flow extraction also records Tree-sitter's zero-based row directly rather than converting to the one-based line convention used elsewhere.

### Tasks

- [x] Add a minimal regression fixture with multiple symbols and local reads/writes in one file.
- [x] Match extracted data-flow edges to lexical context and the owning symbol's source span so sibling and repeated-name scopes cannot receive each other's edges.
- [x] Attach each data-flow edge only to its lexical owner.
- [x] Restrict the generic graph to symbol-to-symbol relationships; unresolved local and async endpoints are represented in the typed data-flow relation.
- [x] Store non-symbol flow entities in a typed table or separate data-flow relation instead of inserting broken symbol foreign keys.
- [x] Convert Rust and TypeScript Tree-sitter rows to one-based lines.
- [x] Add post-index foreign-key and source-location integrity checks plus focused regression tests.
- [x] Bump the graph index format and require a clean graph rebuild.
- [x] Reindex this repository and record before/after edge counts, SQLite size, full-index duration, and representative graph results.

### Required invariants

- Every source symbol exists.
- Every target exists or is explicitly represented as a typed non-symbol/external entity.
- Every source-owned edge location falls within the source symbol span, except for a documented edge type that deliberately uses a file-level location.
- `PRAGMA foreign_key_check` reports no violations.
- Edge growth per file is bounded by extracted relationships rather than `symbols × file relationships`.
- Reindexing the same unchanged revision produces equivalent graph contents.

### Acceptance gate

- [x] The regression fixture fails on the baseline and passes with the repair.
- [x] No unintended missing endpoints remain in a clean self-index.
- [x] No data-flow edge is attributed to an unrelated sibling symbol in the regression fixture.
- [x] Full-index storage and duration are remeasured; no estimate is presented as a measured result.
- [ ] Benchmark: R040 mechanical, judge, and citation scores do not regress outside normal run variance.

### Typed data-flow completion — 2026-07-18

- Added `data_flow_facts`, a typed source-owned relation for value reads/writes and async `await`/`spawn` boundaries. The owner remains a real symbol foreign key; the endpoint is explicitly classified as `value` or `async_boundary` rather than masquerading as a symbol.
- Integrated fact extraction and persistence into full and incremental indexing, explicit stale-file cleanup, graph-version invalidation (`5`), `clear_all`, foreign-key validation, and owner-span/source-file integrity checks.
- `trace_data_flow` now returns exact source-backed local and async occurrences. Investigation hydration preserves the occurrence line and entity kind instead of widening it back to the owner's full body.
- The end-to-end hash fixture indexes a Rust function and verifies persisted local writes, an awaited call, and a spawned task at their exact source lines through the public handler.
- Baseline proof used an isolated detached worktree at `e4d613f`: `regression_dataflow_is_attached_only_to_its_lexical_owner` failed because the second function received line 2 instead of line 6. The repaired fixture passes with the second function correctly owning line 6. The temporary worktree was removed after the proof.

## G2 — Model the public API and impact graph

### Finding

Impact answers still miss public barrels/re-exports, wrapper modules, and canonical exposure paths. Native graph edges do not yet provide a complete first-class model for import/export bindings and delegation.

### Tasks

- [x] Define typed bindings for imports, exports, re-exports, export-all barrels, and aliases, with graph edges for safely resolved targets.
- [x] Define delegation/wrapper relationships without conflating them with ordinary calls.
- [x] Preserve local name, imported name, exported name, module specifier, source file, location, and resolution confidence.
- [x] Represent external, unresolved, and ambiguous bindings explicitly; do not silently choose a global same-name target.
- [x] Update impact traversal to distinguish implementation dependencies from public exposure paths.
- [x] Add adversarial fixtures for chained barrels, renamed imports/re-exports, default imports, namespace bindings, and export-all traversal.
- [x] Add adversarial wrapper and cycle fixtures.
- [x] Model and resolve default exports, including anonymous default declarations.
- [x] Extend first-class module binding extraction beyond TypeScript and Python.
- [x] Evaluate external language-service overlays for impact/reference queries per language.

### Acceptance gate

- [x] Golden impact sets report precision and recall, not only whether one expected file appeared.
- [x] Public entry points and wrapper paths are returned with evidence roles.
- [x] Ambiguous imports do not create high-confidence false edges.
- [ ] Benchmark: impact scores improve over the R040 baseline without degrading symbol and negative-query categories.

### External-overlay evaluation — 2026-07-18

- Added a normalized-artifact consumer matrix for TypeScript, JavaScript, Rust, Python, Go, Java, Kotlin, C#, Swift, C, C++, and Ruby. Every language must map both endpoints and return exactly one expected external reference and one expected impact result: 12/12 reference precision and recall, and 12/12 impact precision and recall.
- Separated generator maturity from consumer support. TypeScript/JavaScript, Rust, Python, and Go have integrated generators; Java, Kotlin, C#, Swift, C, C++, and Ruby expose adapter contracts and explicit command overrides.
- The manifest, dashboard/API availability payload, installer summary, runtime response, tests, README, and agent guidance now report `integrated`, `adapter_only`, or `missing`. An adapter-only bundled wrapper returns `adapter_required` without executing a placeholder.

## F1 — Introduce an evidence coverage contract

### Finding

Recent evidence injection passes improved the benchmark, but coverage is still implicit and query-specific. Impact answers can omit the canonical definition or public path, while concept answers can omit the state or initializer that explains the mechanism.

### Tasks

- [x] Define evidence roles such as canonical definition, implementation, public exposure, direct caller, wrapper/alias, state mechanism, and counter-evidence.
- [x] Define required and optional roles per question shape.
- [x] Return a coverage status describing which required roles were resolved, missing, or ambiguous.
- [x] Allow exact `path:line` claims only when backed by a returned evidence location.
- [x] Allocate evidence budget centrally by role, verification certainty, novelty, and cost.
- [x] Add fixtures for class-level mechanisms such as lazy initialization and cached state.

### Acceptance gate

- [x] Every precise source claim is traceable to returned evidence.
- [x] Missing canonical definitions and public paths are observable as coverage failures.
- [ ] Benchmark: evidence expansion improves coverage without reintroducing large, unfocused packs.

## F2 — Harden symbol identity and resolution

### Finding

Exported symbols currently use byte offset zero when constructing stable IDs in [the parse pipeline](../../src/indexer/pipeline/parse.rs#L211). File path plus unqualified name is insufficient for overloads, repeated method names, and some generated/public declarations.

### Tasks

- [x] Separate the location-independent logical symbol ID from declaration occurrence IDs.
- [x] Include qualified owner and signature/discriminator where the language provides them.
- [x] Detect and fail or report ID collisions during indexing.
- [x] Add fixtures for overloads, nested scopes, duplicate method names, partial declarations, and moved exported declarations.
- [x] Give resolution results explicit states: exact, inferred, ambiguous, unresolved, and external.

### Acceptance gate

- [x] No silent symbol overwrite occurs in collision fixtures.
- [x] Stable logical identity survives harmless source movement where intended.
- [x] Occurrence identity remains unique and source-addressable.

## P1 — Establish direct performance telemetry and gates

### Finding

Current benchmark wall time includes the external answering agent. Search metrics capture total duration, but multi-query paths omit meaningful stage timings and indexing runs do not expose enough phase detail to explain regressions.

### Tasks

- [ ] Benchmark: measure cold and warm direct MCP latency for search, investigate, definition, references, and impact traversal.
- [x] Expose histogram telemetry for p50, p95, and p99 calculation across embedding, BM25, vector search, graph expansion, reranking, evidence allocation, and serialization. Baseline percentile values remain unmeasured by instruction.
- [x] Record cache hit/miss/single-flight wait counts and candidate counts at each stage.
- [x] Add indexing timings for scan, cleanup, parse, binding and edge extraction, SQLite write, embedding, vector write/optimization, PageRank, and Tantivy commit.
- [x] Track DB bytes per symbol, edges per symbol, vectors per symbol, peak RSS, and Metal model residency.
- [x] Establish representative small, medium, and large repository fixtures.

### Acceptance gate

- [ ] Benchmark: report direct server measurements with fixture revision and configuration recorded before making performance claims.
- [x] Every major stage has actionable latency and volume telemetry.
- [ ] Benchmark: report quality and latency gates together for retrieval experiments.

## P2 — Refactor storage and query concurrency

### Finding

Search opens and initializes SQLite on each request in [retrieval/mod.rs](../../src/retrieval/mod.rs#L316). `SqliteStore` serializes access through one connection, while a separate pool is used elsewhere. The single-query hybrid path performs synchronous BM25 before embedding and vector search in [hybrid.rs](../../src/retrieval/hybrid.rs#L77).

### Tasks

- [x] Introduce a repository-scoped storage service initialized once per bound repository.
- [x] Use pooled read connections and a deliberate single-writer strategy.
- [x] Run blocking SQLite and Tantivy operations outside async executor threads.
- [x] Batch symbol fetches used by reranking and evidence assembly.
- [x] Add single-flight query embedding generation.
- [x] Run independent BM25 and vector branches concurrently after direct stage telemetry exists.
- [x] Evaluate safe concurrency/batching for multi-query retrieval.
- [x] Invalidate response caches with an index run ID/version rather than second-resolution timestamps.
- [x] Preserve hit signals in cached search responses so `explain_search` remains complete.

### Acceptance gate

- [x] No request performs schema migration/initialization work.
- [x] Concurrency tests cover simultaneous reads, indexing writes, and cache misses.
- [ ] Benchmark: warm p95 improves with no retrieval-quality regression.
- [x] Cache behavior is correct across two index runs started in the same second.

### Implementation record — 2026-07-17

- `AppState`, `IndexPipeline`, `Retriever`, the embedding cache, description worker, external-index importer, PageRank, and vector-cluster writer now share the repository-owned `Arc<SqliteStore>` created during session binding. Request and index phases no longer reopen SQLite or rerun schema migrations.
- File-backed `SqliteStore` instances use eight lazy WAL read connections and one mutex-protected writer. Read-pool connections set `PRAGMA query_only=ON`; index batch writes, bindings, edges, descriptions, PageRank, cache writes, and telemetry mutations all route through the writer.
- The mixed synchronous/async search pipeline is driven from Tokio's blocking pool. Tantivy keyword retrieval runs in its own blocking task and overlaps embedding plus LanceDB search. Multi-query BM25 requests are batched into one blocking task while the vector branch runs independently.
- Complete symbol rows are fetched in batches of up to 500 IDs for reranking, final evidence assembly, snippet hydration, and public hydration helpers, replacing request-path N+1 lookups.
- Query embedding misses are coalesced by an async single-flight map. Followers reuse the leader's vector and emit an `embedding/coalesced` cache event.
- Response caches store `SearchResponseWithSignals`, retaining `explain_search` scoring detail on hits. Invalidation now compares the durable `started_at:id` index-run version, with a regression test covering two runs in the same second.
- Concurrency regression coverage exercises a writer alongside four pooled readers, pool exhaustion/wakeup, sixteen simultaneous embedding misses, retained hit signals, batch symbol hydration, and same-second cache invalidation.
- Validation: `PROTOC="$PWD/scripts/protoc" EMBEDDINGS_BACKEND=hash cargo test --all-features` passed 1,346 executed tests (with the existing ignored tests), and strict all-target/all-feature Clippy passed. The direct profiler and quality benchmark were deliberately not run.

## A1 — Create a typed investigation enrichment pipeline

### Finding

[`handle_investigate`](../../src/handlers/investigation.rs#L588) has grown into a long sequence of numbered, order-dependent enrichment passes that mutate shared primary and secondary evidence collections. This was effective for recent benchmark wins but raises regression risk and makes cost attribution difficult.

### Tasks

- [x] Introduce typed `InvestigationContext`, `EvidenceCandidate`, `EvidenceRole`, and `CoverageState` models.
- [x] Define an enrichment-pass interface with applicability, collection, confidence, priority, and cost.
- [x] Move deduplication, replacement, evidence budgeting, and provenance into one allocator.
- [x] Keep `serde_json::Value` at the MCP serialization boundary rather than inside orchestration.
- [x] Add per-pass stage metrics and an optional trace explaining why evidence was included or rejected.
- [x] Remove the crate-wide `clippy::too_many_arguments` allowance after migrating the main hotspots, or scope it narrowly with rationale.

### Acceptance gate

- [x] Existing enrichment behavior is captured by characterization tests before migration.
- [x] Pass ordering and dependencies are explicit.
- [x] Each pass can be benchmarked or disabled independently.
- [x] Investigation output remains contract-compatible unless a versioned change is documented.

### Implementation record — 2026-07-17

- Replaced the numbered evidence-mutation ladder with eight ordered typed passes: supporting definitions, question routes, evidence-mined routes, sibling routes, handler dependencies, module breadth, breadth dependencies, and hub types. Dependencies are validated before execution.
- Added typed investigation context, candidates, evidence roles, coverage state, pass descriptors, costs, replacement policies, allocation decisions, and trace records. The pass layer contains no `serde_json::Value`; typed trace data is serialized only when the handler constructs the MCP response.
- Centralized primary-first deduplication, secondary/any replacement, confidence filtering, cost budgeting, resolved-role tracking, and provenance decisions in one allocator. Pass snapshots borrow evidence slices so the pipeline does not clone every hydrated code body for every pass.
- Added stable per-pass duration and candidate metrics under `investigate_enrichment`. `INVESTIGATION_DISABLED_PASSES` isolates comma-separated passes, and `INVESTIGATION_TRACE=1` adds applicability plus per-symbol accept/reject provenance without changing default output.
- Removed the crate-wide `clippy::too_many_arguments` allowance. The remaining legacy constructor/serialization hotspots are narrowly scoped and documented, while the new trace builder uses a typed result object.
- Added allocator characterization coverage for primary-first deduplication, replacement precedence, doomed-copy replacement, cost rejection, pass order, dependencies, and independent disabling. Existing route, breadth, dependency, hub, evidence-pack, and response-budget characterizations remain green.
- Validation: formatting and diff checks passed; strict all-target/all-feature Clippy passed; the full hash-backend suite passed 1,303 library tests (2 ignored), 2 binary tests, 23 external-overlay tests, proxy recovery, and 23 doc tests (30 ignored).
- Per instruction, no direct profiler, latency run, or quality benchmark was run. A1 remains open only for joint quality/latency confirmation.

## Q1 — Expand deterministic quality evaluation and CI

### Tasks

- [x] Add retrieval recall@k, MRR, and nDCG gates.
- [x] Add graph edge precision/recall and impact-set precision/recall gates.
- [x] Add canonical-definition and public-exposure coverage metrics.
- [x] Add language fixtures for Rust, TypeScript, Python, Go, Java/Kotlin, C/C++, C#, Swift, and Ruby.
- [x] Add small adversarial fixtures for overloads, aliases, barrels, wrappers, decorators, dynamic calls, and negative lookups.
- [x] Audit benchmark rubrics for scope conflicts. In particular, distinguish jobs wired to a scheduler from all background maintenance in the wolfmax background-jobs question.
- [x] Record daemon SHA, fixture SHA, configuration, model versions, and comparator in every release gate.
- [x] Extend CI with formatting, Clippy, UI build/tests, benchmark harness tests, and a hash-backend graph-integrity smoke index.
- [x] Update stale HTTP/stdio guidance in `TESTING.md`.

### Acceptance gate

- [x] A deterministic engine gate can fail independently of the agent judge.
- [x] Release reports identify their exact daemon and fixture revisions.
- [x] Rubrics do not penalize factually correct answers that fall within the question's stated scope.

### Implementation record — 2026-07-17

- Added standard recall@5, reciprocal-rank/MRR, graded nDCG@5, precision, recall, and F1 helpers plus a real hash-backed polyglot index gate. The live engine result is 1.000 for recall@5, MRR, nDCG@5, graph precision/recall/F1, impact precision/recall/F1, and canonical-definition coverage.
- The gate indexes Rust, TypeScript, Python, Go, Java, Kotlin, C, C++, C#, Swift, and Ruby fixtures, then exercises retrieval, persisted graph edges, impact traversal, canonical definition resolution, and public-exposure evidence. Adversarial rows cover overloads, aliases, barrels, transparent wrappers, decorators, dynamic dispatch, and negative lookup; dynamic lookup must not fabricate a static edge.
- Benchmark fixtures now carry their full YAML SHA-256. Every new round writes immutable `meta.json` before its first sample with the daemon Git SHA and full binary SHA-256, fixture and upstream revisions, selected question IDs, full arm definitions, model and CLI versions, execution settings, and baseline/candidate comparator. Fresh daemon-arm run rows carry the same fixture and daemon provenance. A resume with different provenance fails instead of mixing samples.
- Release reports load the persisted metadata, expose daemon/fixture revisions and comparator, and render fixture, model, and arm configuration tables. Legacy rounds remain renderable with explicitly unavailable provenance.
- Audited the pinned Django and wolfmax rubrics. The wolfmax scheduler question now asks only about jobs registered by `startScheduler`; exhaustive impact questions explicitly exclude tests and list every production file at the pinned revisions; the `Apps.populate()` hypothetical preserves readiness flags as stated; incorrect `inotify` and Django token-model claims were removed.
- CI now runs formatting, strict all-target/all-feature Clippy, the deterministic engine gate, the complete hash-backed Rust suite, all benchmark harness tests, and UI type-check/tests/build. `TESTING.md` now documents the HTTP daemon and separates the deterministic harness test from the external-agent benchmark.
- Validation passed: formatting and diff checks; strict Clippy; 1,361 executed Rust tests with 35 ignored; 163 benchmark harness tests; fixture validation against the pinned Django and wolfmax checkouts; and 59 UI tests plus type-check and production build. The existing macOS compact-unwind linker warning remains non-fatal.
- Per instruction, no external-agent benchmark or direct performance benchmark was run.

## B1 — Repair reproducible native builds

### Finding

[`scripts/protoc`](../../scripts/protoc) checks only whether the cached binary exists. Parallel Cargo build scripts can simultaneously remove and extract the same cache directory, producing truncated headers and misleading unzip errors. The local validation environment also lacks `cmake`, which `llama-cpp-sys-2` requires even when the hash embedding backend is selected.

### Tasks

- [x] Protect protoc download/extraction with an inter-process lock.
- [x] Extract into a unique temporary directory, verify the expected files, then atomically rename into place.
- [x] Verify both the binary and include tree before treating the cache as complete.
- [x] Document or automatically check native prerequisites, including `cmake`.
- [x] Decide whether hash-backend test builds should avoid compiling the llama.cpp backend through Cargo feature separation.

### Acceptance gate

- [x] A clean parallel Cargo build cannot corrupt the protoc cache.
- [x] Missing native prerequisites fail early with an actionable message.
- [x] The documented fast-test command works on a clean supported development machine.

### Implementation record — 2026-07-18

- Rebuilt [`scripts/protoc`](../../scripts/protoc) around a per-version/per-architecture inter-process lock. Waiters re-check the completed cache, locks with dead owner PIDs are reclaimed, and lock acquisition has a configurable timeout rather than waiting forever.
- Downloads now use a unique staging directory on the cache filesystem, verify the pinned release SHA-256, extract and validate the executable plus `descriptor.proto` and `compiler/plugin.proto`, then publish the verified directory with one atomic rename. Invalid live caches are repaired without mutating them in place, and failed checksums or incomplete archives never become visible.
- Added [`.cargo/config.toml`](../../.cargo/config.toml) so every Cargo invocation uses the pinned wrapper without requiring a caller-specific `PROTOC` prefix. A package [`build.rs`](../../build.rs) checks protobuf and native CMake availability with install, override, and lightweight-test remediation before compiling the application.
- Made `llama-cpp-2` optional behind a default-on `native-llama` feature. `cargo test --no-default-features` now omits llama.cpp from the dependency graph instead of merely selecting hash embeddings at runtime; the default feature preserves the production Metal build.
- Gated the embedding, LLM, and reranker implementations at the native boundary. A native-free binary fails immediately with targeted configuration guidance if llama embeddings or reranking are selected, while enabling local LLM synthesis returns the same actionable feature error.
- CI now proves both configurations: strict all-target Clippy compiles the production-native feature set after installing CMake, while strict lightweight Clippy, the deterministic quality gate, and the complete Rust suite run with `--no-default-features`. The testing guide and README distinguish the runtime hash selection from the compile-time native boundary.
- Validation passed: a 32-process cold bootstrap test succeeded ten consecutive times (320 invocations), incomplete-cache repair and checksum-failure tests passed, and a real empty-cache download produced `libprotoc 29.3`. Strict Clippy passed in both feature configurations; the lightweight suite passed 1,298 library tests, 2 binary tests, 9 deterministic-quality/support tests, 23 external-overlay tests, proxy recovery, and 23 doc tests (33 ignored in total). All 166 benchmark-harness unit tests passed.
- The missing-CMake path emitted the expected `brew install cmake`, `CMAKE`, and `cargo test --no-default-features` remedies. Production Clippy was validated using an isolated CMake installation, without changing the machine's package state.
- Per instruction, no external-agent or direct performance benchmark was run.

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
| 2026-07-16 | G1-CLEAN-INDEX | Clean self-index after graph repair | Isolated home; hash embeddings; descriptions/reranker/watch disabled; 349 files | 5,548 persisted symbols; 25,892 graph edges; zero missing edge, binding, or usage endpoints; zero invalid data-flow locations; full foreign-key check clean | Pipeline-reported index duration 9.090 s; API job wall time 14.000 s; SQLite 39,555,072 bytes. Not directly comparable to the prior default-backend duration. | Keep; structural gate passes | This log |
| 2026-07-16 | G2-PUBLIC-API-SLICE | Persist and traverse TypeScript/Python module bindings | Same isolated hash self-index; targeted extractor, resolver, graph, storage, and impact fixtures | 2,763 bindings: 1,803 exports, 959 imports, 1 re-export; exact Python import/re-export chain retained; observed cross-language same-name false edge removed | Included in G1 clean-index measurements; no standalone query-latency claim yet | Keep; continue with wrappers, remaining languages, and focused impact benchmark | This log |
| 2026-07-16 | G2-DETERMINISTIC-COMPLETION | Default exports, incremental public-name resolution, broader language bindings, and transparent delegation | Fresh isolated hash self-index; watch/descriptions/reranker disabled; full deterministic Rust suite | 5,581 persisted symbols; 26,601 edges; 4,896 typed bindings; 8 `delegates_to` edges; zero missing edge/binding/usage endpoints, invalid flow locations, or foreign-key violations | Not measured; benchmark and latency gates skipped by instruction | Keep; deterministic G2 scope passes, benchmark acceptance remains open | This log |
| 2026-07-16 | F1-EVIDENCE-CONTRACT | Typed semantic coverage, source-backed citations, and role-aware response budgeting | Evidence-pack, investigation-budget, ask-code, and tool-contract fixtures; full hash-backend Rust suite | 1,273 library tests passed, 2 ignored; all binary, integration, and doc targets passed; required-role omissions and ambiguity are explicit after truncation | Not measured; benchmark and latency gates skipped by instruction | Keep; deterministic F1 scope passes, quality acceptance remains open | This log |
| 2026-07-16 | F2-SYMBOL-IDENTITY | Logical/occurrence identity separation, collision rejection, and ambiguity-safe root resolution | Hash backend; allocator, real TypeScript extraction, persistence, import, external-overlay, and handler fixtures; full Rust suite | 1,286 library tests passed, 2 ignored; all binary, integration, and doc targets passed; collision fixtures preserve prior rows and overload sets resolve as one logical symbol | Not measured; benchmark and latency gates skipped by instruction | Keep; F2 deterministic and acceptance gates pass | This log |
| 2026-07-16 | P1-DIRECT-TELEMETRY | Stable query/index stage metrics, durable run records, resource gauges, and direct-MCP profiler | Hash backend; telemetry migration/round-trip/cache fixtures; full Rust suite; profiler fixtures for small TypeScript, medium self-index, and large Django repositories | 1,291 library tests passed, 2 ignored; both binary tests, all integration targets, proxy recovery, and 23 doc tests passed | Not measured; direct profiler and quality benchmark were not run by instruction | Keep; telemetry implementation passes, baseline and joint quality/latency gates remain open | This log |
| 2026-07-17 | Q1-DETERMINISTIC-QUALITY | Polyglot engine-quality gate, immutable release provenance, rubric audit, CI expansion, and HTTP testing guidance | Hash backend; 11-language fixture; pinned Django/wolfmax rubric validation; complete Rust, Python harness, and UI gates | Retrieval recall@5/MRR/nDCG@5, graph and impact precision/recall/F1, and canonical coverage all 1.000; public exposure, overload, dynamic-negative, and unresolved-negative gates passed | Not measured; external-agent and direct performance benchmarks skipped by instruction | Keep; Q1 acceptance gates pass | This log |
| 2026-07-18 | B1-REPRODUCIBLE-BUILDS | Locked/verified atomic protoc cache, prerequisite diagnostics, and default-on native feature boundary | 32-process cold bootstrap repeated 10 times; real empty protoc cache; no-default-feature Rust suite; all-feature Clippy | 1,356 executed Rust tests and 166 harness tests passed; both strict Clippy configurations passed | Native-free dependency graph excludes llama.cpp; no runtime benchmark by instruction | Keep; B1 acceptance gates pass | This log |
| 2026-07-18 | G1-TYPED-DATA-FLOW | Persist non-symbol value and async facts without weakening the symbol graph | Hash backend; extraction, SQLite round-trip/cleanup, graph-version, integrity, and end-to-end index/handler fixtures | Local writes, awaited calls, and spawned tasks persist with typed endpoints and exact source lines; public trace rows are source-backed | Not measured; benchmark excluded by instruction | Keep; G1 engineering scope complete | This log |
| 2026-07-18 | G1-BASELINE-PROOF | Run the lexical-owner regression against the pre-fix baseline and repair | Isolated detached `e4d613f` worktree; identical ownership assertion on repaired tree | Baseline failed with the second function incorrectly receiving line 2; repaired tree passed with line 6 | Not applicable | Keep; baseline acceptance item passes | This log |
| 2026-07-18 | G2-OVERLAY-MATRIX | Validate normalized external reference and impact consumption for every indexed language | One mapped caller/target golden pair for each of 12 languages; hash-backed handler state | Reference precision/recall 12/12; impact precision/recall 12/12; generator capability states explicit | Not measured; benchmark excluded by instruction | Keep; G2 engineering scope complete | This log |

## Decision log

| Date | Decision | Rationale | Revisit when |
|---|---|---|---|
| 2026-07-16 | Repair graph integrity before further ranking work | The majority of live edges are misattributed or have missing targets, contaminating fidelity and performance measurements | G1 acceptance gate passes |
| 2026-07-16 | Separate direct server performance from agent benchmark time | R040 wall time and tokens include the external answering agent | Direct MCP telemetry is available |
| 2026-07-16 | Keep architecture migration after the evidence contract | A typed pass pipeline needs stable evidence roles and coverage semantics to avoid merely moving current heuristics | F1 is accepted |
| 2026-07-16 | Keep the generic graph symbol-to-symbol only | Synthetic `local:` and `async:` identifiers violated the graph's symbol foreign-key contract; non-symbol endpoints now live in the typed `data_flow_facts` relation | A future endpoint needs declaration identity or richer flow semantics |
| 2026-07-16 | Version graph extraction semantics independently | Existing symbols and search indexes can remain available while edges and file fingerprints are invalidated for a mandatory graph rebuild | A broader persisted-index format is introduced |
| 2026-07-16 | Persist module bindings separately from graph edges | Bindings must retain aliases, unresolved/external state, module specifiers, and confidence even when no safe symbol endpoint exists; only resolved bindings become graph edges | Binding consumers require additional language-specific fields |
| 2026-07-16 | Resolve imports only through a unique indexed module | Global same-name fallback linked Python references to an unrelated Go `UserService`; ambiguity is more truthful than a high-confidence false edge | A language-service overlay supplies stronger identity evidence |
| 2026-07-16 | Separate logical symbols from declaration occurrences | Overloads and partial declarations are one semantic target but still need distinct source-addressable rows; the canonical occurrence retains the logical ID for compatibility | A language service provides a stronger cross-file identity key |
| 2026-07-16 | Reject symbol-ID reuse across different identities | An upsert must not silently turn a hash collision or allocator bug into overwritten code intelligence; file replacement deletes old rows before inserting the new identity set | The persisted identity format is versioned or migrated differently |
| 2026-07-16 | Require a unique logical root for graph and impact tools | Selecting the first same-name declaration produces confident false traversals; overload occurrences may collapse, but distinct logical candidates must return `ambiguous` | A caller supplies an owner-qualified name, file scope, or stronger external identity |
| 2026-07-18 | Separate runtime hash selection from native compilation | An environment-selected hash backend still forced every deterministic test machine to compile llama.cpp and install CMake; a default-on Cargo feature preserves production behavior while providing a genuinely native-free test graph | A non-llama native backend needs an independent feature or distribution profile |

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
- The four pre-existing mechanical Clippy findings in `external_index/provider.rs`, `handlers/analysis.rs`, and `indexer/pipeline/parse.rs` were cleared while validating G2; `cargo clippy --all-targets -- -D warnings` now passes.
- The R040 quality rerun remains required before full G1 acceptance.

### 2026-07-16 — G1 clean-index gate and G2 public-API slice

- Added a persisted module-binding model that keeps binding kind, module specifier, imported/local/exported names, source file, source line, target symbol, resolution state, and confidence independently of graph-edge eligibility.
- Added TypeScript extraction for default, named, aliased, namespace, named re-export, namespace re-export, and export-all forms; added Python import/from-import extraction and package-initializer re-export promotion.
- Added language-aware module candidate resolution. A module must resolve to one indexed file and a named target must be unique; external, unresolved, and ambiguous bindings remain queryable without manufacturing graph targets.
- Removed the unsafe global same-name import fallback. The clean fixture retained the exact Python `pkg.services.UserService` chain while producing zero Python-to-other-language `UserService` edges.
- Added import, export, re-export, and export-all traversal to the dependency graph. Impact results label public entry points as `public_exposure`, attach exact binding metadata, and emit the exposure line as verified evidence.
- Hardened replacement cleanup for binding targets and usage examples, and expanded post-write validation to the complete SQLite foreign-key set.
- Prevented nested Python functions from being classified as exported declarations, avoiding same-name stable-ID overwrites during clean indexing.
- Bumped the graph extraction version to `3` so existing repositories rebuild the new edge and binding semantics.
- A fresh isolated hash-backend self-index persisted 5,548 symbols across 349 files, 25,892 edges, and 2,763 bindings. It had zero missing edge/binding/usage endpoints, zero invalid data-flow locations, and no `PRAGMA foreign_key_check` rows.
- The clean run recorded 9.090 seconds inside the index pipeline, 14.000 seconds for the API job, and a 39,555,072-byte SQLite database. The duration is explicitly not comparable to the older default-model snapshot; the edge and storage reductions are structural effects of removing invalid fan-out.
- End-to-end `find_affected_code` validation for the Python fixture returned `producers/tests/fixtures/python/pkg/__init__.py:1` as `public_exposure`, with an exact-confidence re-export of `UserService` from `.services`.
- Ran the complete hash-backend test suite: 1,254 library tests passed (2 ignored), all binary and integration tests passed, and 23 doc tests passed (30 ignored). Formatting, diff checks, and strict Clippy also passed.

### 2026-07-16 — G2 deterministic completion (benchmark deferred)

- Split indexing into symbol, binding-resolution, and ordinary-edge phases so fresh indexes resolve public names without file-order dependence; incremental batches reuse persisted public bindings from unchanged source files.
- Added default-export identity for TypeScript and JavaScript, including stable synthetic `default` symbols for anonymous declarations and expressions. Default imports and chained aliases resolve through the public binding rather than a private or coincidentally named declaration.
- Made binding resolution cycle-safe and explicit: cyclic, ambiguous, unresolved, and external paths remain recorded without false graph endpoints. Added fixtures for cycles, incremental default imports, and private same-name declarations.
- Added one-based import locations to every native extractor and synthesized typed bindings for Rust, Java, Kotlin, C#, Go, Swift, C, C++, and Ruby using language-aware module candidates. TypeScript, JavaScript, and Python retain their syntax-specific binding extraction.
- Added conservative transparent-wrapper detection. A direct single-call return emits both the ordinary `call` edge and a separate `delegates_to` edge; wrappers with preprocessing remain ordinary calls only.
- Impact analysis now labels transparent symbols as `wrapper`, returns the exact delegation location and confidence, and prioritizes public exposure and delegation evidence. Default dependency traversal includes `delegates_to` and remains bounded under wrapper/re-export cycles.
- A fresh isolated structural self-index persisted 5,581 symbols across 349 files, 26,601 edges, 4,896 bindings, and 8 transparent-delegation edges. It had zero missing edge/binding/usage endpoints, zero invalid data-flow locations, and no `PRAGMA foreign_key_check` rows.
- Ran formatting and diff checks, strict Clippy, and the complete hash-backend test suite: 1,265 library tests passed (2 ignored), both binary tests passed, 23 external-overlay integration tests passed, the proxy integration test passed, and 23 doc tests passed (30 ignored).
- Per instruction, no impact benchmark, R040 rerun, or latency benchmark was run. G2's benchmark score acceptance gate remained open; its deterministic golden precision/recall and per-language overlay evaluation were completed on 2026-07-18.

### 2026-07-16 — F1 evidence coverage contract (benchmark deferred)

- Added a stable semantic role vocabulary alongside the existing presentation roles: canonical definition, implementation, public exposure, direct caller, wrapper/alias, state mechanism, counter-evidence, pipeline stages, affected code, dependencies, tests, config, and module context.
- Defined required and optional roles for callsite enumeration, pipeline trace, data flow, impact radius, dependency map, and symbol lookup packs. Coverage now reports required, optional, resolved, missing, ambiguous, and candidate roles while preserving the legacy status, basis, and missing fields.
- Added per-row `coverage_role`, `verification`, and `source_backed` fields. Exact citations are emitted only when file, line, and non-empty indexed source evidence are present; unsupported rows cannot acquire a `cite` value.
- Impact packs now distinguish public exposure and wrapper rows from ordinary affected production code. An impact pack without a public path remains explicitly partial instead of becoming complete merely because some affected row exists.
- Replaced keep-first pack truncation with centralized role-aware selection. Required roles ride first; verified/source-backed and injected rows outrank candidates; novel roles outrank duplicates; evidence size is the cost tie-breaker. Coverage is recomputed after every row cut so omitted roles cannot remain falsely resolved.
- Preserved semantic role, verification, source-backing, and cite metadata through terminal budget compaction.
- Added deterministic fixtures for canonical definitions, missing and resolved public exposure, ambiguous required roles, source-backed citation enforcement, lazy cached state, required-role retention, and post-truncation coverage refresh.
- Updated `investigate` and `ask_code` descriptions and answer guidance to expose the role contract and exact-location policy.
- Ran formatting and diff checks, strict Clippy, and the complete hash-backend test suite: 1,273 library tests passed (2 ignored), both binary tests passed, all integration targets passed, and 23 doc tests passed (30 ignored).
- Per instruction, no R040, impact-quality, or latency benchmark was run. The acceptance gate requiring measured coverage improvement without pack-quality regression remains open.

### 2026-07-16 — F2 symbol identity and ambiguity safety (benchmark deferred)

- Added a persisted `symbol_identities` relation that separates each concrete `symbol_id` occurrence from a location-independent `logical_id`, qualified owner name, normalized declaration signature, occurrence discriminator, and canonical flag. The canonical occurrence deliberately keeps the logical ID as its symbol ID so existing unique top-level import targets remain compatible.
- Replaced byte-offset allocation with deterministic logical grouping. Repeated methods are qualified by their containing type, nested functions by their lexical owner, overloads and partial declarations share one logical ID, and every concrete declaration retains a unique occurrence row.
- Added batched preflight collision validation to symbol and identity writes. Duplicate IDs in one parse batch or reuse by a different file/kind/name/identity now fail the transaction instead of overwriting an existing declaration; position-only source movement remains a valid update.
- Persisted symbols and identities in the same replacement transaction, added qualified-name lookup, and bumped persisted extraction semantics to version `4` so every source file is reparsed into the new identity format.
- Made binding and import resolution collapse overload occurrences to their canonical logical target. Same-name declarations with distinct logical IDs remain ambiguous; receiver-qualified method resolution uses the persisted owner-qualified identity.
- Added one shared logical-symbol resolver for definition, reference, dependency, call, type, data-flow, impact, and test-navigation entry points. Successful roots return `resolution: exact`; missing and multi-logical roots return `unresolved` or `ambiguous` with candidate metadata rather than selecting the first row. Module bindings retain the existing `exact`, `inferred`, `ambiguous`, `unresolved`, `external`, and `cyclic` states.
- Fixed a TypeScript extraction defect exposed by the nested-scope fixture: ancestor walking had incorrectly assigned a nested function the outer module export's status and full span. Only declarations directly wrapped by an `export_statement` now inherit that wrapper.
- Added adversarial coverage for overloads, identical partial declarations, duplicate methods on different classes, nested functions from real Tree-sitter extraction, harmless source movement, collision rollback, qualified lookup, overload imports, persistence, and ambiguity-safe handler roots.
- Ran formatting and diff checks, strict Clippy, and the complete hash-backend suite: 1,286 library tests passed (2 ignored), both binary tests passed, all 23 external-overlay integration tests passed, the proxy integration test passed, and 23 doc tests passed (30 ignored).
- Per instruction, no R040 quality benchmark or latency benchmark was run. F2 has no remaining deterministic acceptance item; performance measurement proceeds under P1.

### 2026-07-16 — P1 direct performance telemetry (measurement deferred)

- Added bounded-cardinality Prometheus histograms for end-to-end MCP operation latency and query-stage latency. Unknown client-supplied tool names collapse to one `unknown` label; serialization is attributed to the concrete stable operation.
- Instrumented single- and multi-query retrieval with non-zero embedding, BM25, vector, fusion, graph-expansion, reranking, scoring, assembly, and returned-candidate measurements. `investigate` now measures planning, primary and secondary hops, enrichment, and evidence allocation.
- Added response, query-embedding, and context-cache hit/miss/invalidation counters plus live entry/byte gauges. Response-cache hits now create durable `search_runs` rows rather than disappearing from telemetry. The `wait` event is reserved for P2's single-flight implementation and is not fabricated before that concurrency primitive exists.
- Extended `search_runs` with search path, cache status, fusion time, sub-query count, and keyword/vector/fused candidate volumes. Extended `index_runs` with scan, cleanup, parse, SQLite, Tantivy, binding, edge, embedding, vector-write, PageRank, and optimization timings; existing databases migrate these columns with zero defaults.
- Split batch-write timing between SQLite and Tantivy, timed pipelined embedding and LanceDB writes independently, fixed embedding-cache Prometheus counters to report per-run deltas rather than repeatedly adding process-lifetime totals, and moved vector optimization before run persistence so the stored duration describes the complete index run.
- Added storage gauges for SQLite (including WAL/SHM), recursive Tantivy and LanceDB bytes, symbols, edges, vector rows, bytes per symbol, edges per symbol, vectors per symbol, process peak RSS, and embedding/reranker Metal residency. `get_index_stats` now returns the live performance-relevant configuration used by reproducibility reports.
- Added `scripts/profile_direct_mcp.py` and small/medium/large fixtures. The harness records server/repository revisions, live configuration, index volume, cold latency, warm p50/p95/p99, response bytes, and optional p95 gates for search, investigate, definition, references, and impact traversal without placing an answering agent in the timing loop.
- Added migration, durable round-trip, stable-label, bounded-cardinality, and cache-footprint tests. Formatting, `git diff --check`, strict all-target/all-feature Clippy, and the complete hash-backend suite pass: 1,291 library tests (2 ignored), both binary tests, all 23 external-overlay integration tests, proxy recovery, and 23 doc tests (30 ignored).
- Per instruction, the direct profiler, R040 quality benchmark, and latency baselines were not run. P1 remains open for measured cold/warm baselines, single-flight wait telemetry after P2 introduces it, and joint quality/latency reporting.

### 2026-07-17 — Q1 deterministic quality evaluation and CI

- Added a standalone engine-quality integration gate backed by a real temporary hash index, with conventional ranking and set metrics rather than agent-judge output.
- Covered every supported language family and the failure shapes most likely to create confident false positives: overloads, aliases/barrels, wrappers, decorators, dynamic calls, and negative lookups.
- Made benchmark rounds self-describing and immutable through fixture hashing, full daemon/run configuration provenance, comparator identity, and report rendering. Resume now rejects configuration drift.
- Audited fixture scope against pinned sources and corrected scheduler, impact-enumeration, readiness-hypothetical, autoreloader, and JWT rubric defects.
- Expanded CI and rewrote the testing guide around the v4 HTTP daemon. All deterministic Rust, benchmark-harness, and UI gates pass; no external benchmark was run.

### 2026-07-18 — B1 reproducible native builds

- Replaced the shared protoc cache's remove-and-extract sequence with a locked, checksummed, verified, atomically published bootstrap and added concurrent cold-cache, repair, and failure regressions.
- Routed Cargo through that pinned bootstrap by default and added early protobuf/CMake prerequisite diagnostics.
- Split the default production-native llama.cpp dependency behind `native-llama`; deterministic hash builds now exclude it with `--no-default-features` and reject incompatible runtime options with actionable messages.
- Updated CI, README development commands, and `TESTING.md` to exercise and explain both compile configurations.
- Validated both strict Clippy paths, the complete lightweight Rust suite, the complete benchmark-harness unit suite, repeated parallel bootstraps, a real clean protoc download, and the missing-CMake diagnostic. No external or direct performance benchmark was run.

### 2026-07-18 — Final non-benchmark closure

- Added the typed `data_flow_facts` relation for local value reads/writes and async boundaries, including extraction, deduplication, persistence, incremental cleanup, graph-version rebuilds, integrity validation, public trace output, and exact-line investigation hydration.
- Proved the original lexical-owner defect against `e4d613f` in an isolated worktree. The baseline attached the first function's line 2 flow to the second function; the repaired tree returned the correct line 6 owner.
- Added an end-to-end hash fixture that indexes and queries local, await, and spawn facts at exact source lines.
- Added a 12-language external-overlay consumer matrix with exact reference and impact golden sets. Both gates returned 12/12 expected results with no extras.
- Made producer maturity explicit across the registry, manifest, API/dashboard payload, installer, runtime responses, documentation, and tests. Four producer implementations are integrated; the remaining seven are honest adapter contracts that require an override and return `adapter_required` otherwise.
- Removed process-wide `PATH` mutation from producer resolver tests after the full suite exposed a race with Git-discovery tests. Resolver tests now inject explicit search directories, so unrelated subprocess tests retain the real environment under parallel execution.
- Final deterministic validation passed: formatting and diff checks; strict Clippy for native-free and all-feature builds; 1,305 library tests, 2 binary tests, 10 deterministic-quality/support tests (2 ignored), 24 external-overlay tests, the proxy-recovery integration, and 23 doc tests (30 ignored). The standalone HTTP integration remains intentionally ignored by the suite. Also green: 166 benchmark-harness unit tests; 58 Python and 21 Node producer/package tests; and 59 UI tests plus type-check and production build. The existing macOS compact-unwind and UI chunk-size warnings remain non-fatal.
- All remaining unchecked items in this log are explicitly benchmarks or direct performance measurements. No direct profiler, quality benchmark, or external-agent benchmark was run during this closure.

## Completion definition

This program of work is complete when:

- graph invariants pass on all supported language fixtures and representative real repositories;
- impact answers reliably include canonical implementation and public exposure paths;
- evidence coverage and uncertainty are explicit;
- direct query and indexing performance are measured by stage;
- storage and query concurrency changes demonstrate measured gains without quality regression;
- investigation enrichment is typed, observable, and independently testable; and
- release gates combine deterministic engine checks with the external-agent benchmark.

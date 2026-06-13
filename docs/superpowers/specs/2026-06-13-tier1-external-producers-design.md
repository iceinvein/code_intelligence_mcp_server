# Tier 1 External Producers Design

Date: 2026-06-13

## Goal

Implement real bundled external producers for the languages already treated as Tier 1 by the daemon: TypeScript/JavaScript, Python, Rust, and Go. The immediate benchmark need is TypeScript/JavaScript for wolfmax and Python for Django, but the shipped feature should not stop at benchmark-only coverage. All Tier 1 producer entrypoints should move from inert stubs to deterministic normalized artifact generators before the next benchmark phase evaluates the external overlay.

## Context

R007 confirmed that the long-term external index foundation can ship without regressing the default product path. The overlay remains inert because bundled producer wrappers currently exit without generating external rows. The next useful benchmark arm needs producer-generated symbols and references, imported through the existing normalized contract, so retrieval can test whether external graph facts improve evidence packs instead of only measuring the native Tree-sitter indexer.

The existing daemon architecture stays intact:

- Producer discovery, policy, and execution remain in `src/external_index/producers.rs`.
- Import remains through `src/external_index/artifact.rs` and `import_external_index`.
- The install and package layout continue to bundle `producers/manifest.json` plus executable wrappers under `producers/bin/`.
- External indexing remains opt-in through the current `EXTERNAL_INDEX_AUTO`, `EXTERNAL_INDEX_ON_REFRESH`, and `EXTERNAL_INDEX_PRODUCER` controls.

## Non-Goals

- Do not enable external indexing by default.
- Do not replace the native Tree-sitter indexer.
- Do not add Tier 2 languages in this phase.
- Do not run the benchmark until producer smoke and import tests are green.
- Do not introduce raw producer-specific schemas into retrieval; each producer must emit the normalized external artifact contract.
- Do not require network access during indexing.

## Approach

Use direct normalized producers per Tier 1 language, backed by the best local project tooling available for each language and a deterministic fallback where the toolchain is not realistic for the first phase. The daemon should only see one stable interface: execute `code-intelligence-external-<language> index --output <artifact>` in the repository root, then import the JSON artifact if the command succeeds.

This approach keeps the Rust daemon and storage layers stable while allowing producer internals to evolve. It also gives the benchmark meaningful TS/Python external rows soon, without blocking on compiler-grade Rust and Go reference extraction.

## Producer Scope

### TypeScript and JavaScript

The TypeScript producer should cover `.ts`, `.tsx`, `.js`, `.jsx`, `.mts`, `.cts`, `.mjs`, and `.cjs` files. It should prefer the project-local TypeScript compiler package when present, then fall back to a bundled or documented resolver path if available. It should read `tsconfig.json` when present and otherwise construct a deterministic source file set from repository files.

Initial output should include:

- Functions, classes, methods, interfaces, types, enums, exported constants, and exported variables as `symbols`.
- Import/export relationships where they can be resolved.
- Call and member-use references when the compiler checker can bind the target.
- File paths relative to the repository root, stable line/column spans, and deterministic ordering.

This producer is benchmark-critical because wolfmax exercises the JavaScript/TypeScript stack.

### Python

The Python producer should start with a stdlib `ast` implementation that understands packages, modules, classes, functions, methods, imports, calls, and attribute references well enough for Django. It should use repository-relative module resolution from `pyproject.toml`, `setup.cfg`, `setup.py`, package directories, and common source roots. It should not require pyright, Jedi, or a virtual environment for the first version.

Initial output should include:

- Modules, classes, functions, async functions, and methods as `symbols`.
- Import edges for resolved intra-repository imports.
- Call references for resolved local functions/classes and common `self.<method>` or `cls.<method>` calls.
- Best-effort attribute references with explicit confidence values.

This producer is benchmark-critical because Django is Python and the benchmark needs overlay rows for it.

### Go

The Go producer should use the local Go toolchain when available. It should prefer `go list` for package discovery and module boundaries, then parse source files with Go's standard parser in a small helper implementation. For the first implementation, it can emit package/function/type/method symbols and intra-package call references, then expand to cross-package references once package loading is stable.

If `go` is unavailable or the repository is not a Go project, the producer should exit with the existing supported-but-not-configured status rather than emitting an empty success artifact.

### Rust

The Rust producer should begin as a deterministic source-level producer rather than waiting for a complete rust-analyzer integration. It should discover crates through `Cargo.toml`, parse Rust source files, and emit modules, structs, enums, traits, impl methods, functions, and constants. It should emit best-effort references for direct calls and type/trait mentions where names can be resolved locally, with conservative confidence.

Rust external data should be clearly marked as source-level until a later rust-analyzer-backed producer is added. This keeps Tier 1 coverage real and testable without pretending to have compiler-grade macro expansion or trait resolution.

## Shared Contract

Every Tier 1 producer writes a normalized JSON artifact matching `NormalizedExternalIndex`:

- `source_kind` identifies the producer family, such as `typescript_compiler`, `python_ast`, `go_source`, or `rust_source`.
- `producer` records the producer id and version.
- `language` matches the daemon registry language.
- `root_path` is the repository root seen by the producer.
- `symbols` and `references` are sorted deterministically.

External symbol ids must be stable across runs for unchanged code. The id format should include language, relative path, symbol kind, qualified name, and a span discriminator when needed. References should use existing relationship values where possible: imports, calls, inheritance/implementation, type mentions, and contains/module ownership.

Producer output must avoid absolute paths except for the artifact-level root path. All file paths inside symbols and references must be repository-relative UTF-8 paths.

## Execution and Failure Semantics

The wrappers keep the current command shape:

```bash
code-intelligence-external-<language> index --output <artifact>
```

Expected exit behavior:

- `0`: artifact written and ready to import.
- `64`: command usage error.
- `69`: producer is installed but cannot run for this repository or local toolchain.
- any other non-zero code: producer failure, captured as diagnostics by the daemon.

Producer failures remain non-fatal to native indexing. The daemon records producer status and diagnostics, then continues serving the native index. A successful producer that finds no supported files should emit a valid empty artifact only when the repository genuinely has no matching files for that configured producer; misconfiguration should be a `69`.

## Packaging

The existing bundle shape stays:

- `producers/manifest.json` lists the producer ids, languages, wrapper paths, and support tier.
- `producers/bin/code-intelligence-external-*` are executable entrypoints.
- Shared producer code lives under `producers/` and is referenced by the wrappers using paths relative to the bundle.

The npm and Homebrew package validation must prove that all Tier 1 entrypoints and their shared implementation files are present and executable. Producer implementations should use runtimes already reasonable for this product install path: Node for the TypeScript producer, Python stdlib for Python, and small local helpers or scripts for Go/Rust. No producer should download dependencies at index time.

## Testing

Add tests in layers:

- Producer contract tests for each Tier 1 language using tiny fixture repositories.
- Determinism tests that run a producer twice and compare normalized output.
- Import tests that feed generated artifacts through the existing external importer.
- Merged provider tests proving imported external rows appear through the overlay without replacing native rows.
- Packaging tests proving npm and Homebrew bundles include the Tier 1 producer files.
- Status/diagnostic tests for configured-but-unavailable toolchains.

The benchmark arm should only be added after TypeScript/JavaScript and Python producer smoke tests pass against wolfmax and Django checkouts. Rust and Go tests still gate the Tier 1 producer phase, but their first benchmark value is secondary.

## Benchmark Readiness

Before running the next benchmark round, prepare a new external-enabled arm that differs from the shipped-default R007 arm only by opt-in external producer settings. The benchmark should record:

- producer execution time per repository,
- imported external symbol/reference counts,
- producer diagnostics,
- overlay hit rates,
- citation hit rate,
- hallucination rate,
- judge and mechanical scores.

The comparison should be against R007 shipped defaults and the prior no-descriptions baseline. A result is useful only if Django and wolfmax both import non-zero external symbols and references.

## Rollout

Implement in two waves:

1. Build TypeScript/JavaScript and Python producers first, including smoke tests against benchmark repositories where available.
2. Add Rust and Go producers on the same contract, with conservative source-level extraction and explicit confidence/provenance metadata.

After both waves pass local verification, add the benchmark external arm and run the next benchmark round. Keep the production default off until benchmark data shows the overlay improves evidence quality enough to justify index-time cost.

## Open Constraints Resolved

- "All Tier 1" means the four existing primary producer ids: `typescript`, `python`, `rust`, and `go`.
- The first useful benchmark requires TypeScript/JavaScript and Python rows, not only registry coverage.
- Rust and Go can start as source-level producers, but must emit real normalized artifacts and diagnostics rather than shell stubs.
- Producer quality must be visible through counts, provenance, confidence, and diagnostics so benchmark results can be interpreted honestly.

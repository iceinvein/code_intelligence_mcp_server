# Bundled External Producers

## Summary

Ship external index producers as part of the normal Code Intelligence install, while keeping producer execution disabled by default until benchmark evidence justifies enabling it automatically.

The install experience should make producers available without requiring users to discover or install separate binaries. The runtime should find bundled producers reliably under npm, Homebrew, and direct binary installs. Indexing behavior remains conservative: Tree-sitter indexing stays the default path, `generate_external_index` is available, and automatic producer execution remains opt-in until the benchmark step decides otherwise.

## Goals

- Include supported external producer helpers in the release artifact used by npm and Homebrew installs.
- Resolve bundled producers from the installed binary directory before falling back to `PATH`.
- Preserve explicit command overrides through `EXTERNAL_INDEX_<LANG>_COMMAND`.
- Keep external producer execution disabled by default for normal indexing.
- Provide install/status diagnostics that tell users which producers are bundled, available, missing, or blocked by project toolchain requirements.
- Keep missing or failing producers non-fatal: native indexing must still complete.

## Non-Goals

- Do not enable watch-mode external indexing by default.
- Do not require benchmark-proven default execution policy in this shipping phase.
- Do not replace native Tree-sitter extraction.
- Do not require every language producer to have compiler-grade precision on day one.
- Do not make npm install download per-language toolchains from unrelated package managers.

## Shipping Model

The GitHub release tarball should become a small install bundle instead of a single binary archive.

Bundle contents:

- `code-intelligence-mcp-server`
- `code-intelligence-external-typescript`
- `code-intelligence-external-rust`
- `code-intelligence-external-python`
- `code-intelligence-external-go`
- `code-intelligence-external-java`
- `code-intelligence-external-kotlin`
- `code-intelligence-external-csharp`
- `code-intelligence-external-swift`
- `code-intelligence-external-c`
- `code-intelligence-external-cpp`
- `code-intelligence-external-ruby`
- `producers/manifest.json`

The producer helpers may initially be thin wrappers around existing ecosystem tools or normalized artifact generators. The important shipping boundary is that Code Intelligence owns the executable entrypoint and the normalized JSON contract. Users should not need to know whether a helper internally uses SCIP, an LSP, a compiler API, or a fallback scanner.

## Install Behavior

The npm `postinstall` path should continue downloading the version-matched GitHub tarball, verifying the tarball checksum, and extracting into `npm/bin/`. It should additionally verify that every expected bundled executable from `producers/manifest.json` exists and is executable.

The Rust `install` subcommand should not download producers itself. It installs the daemon for the current executable. Producer availability comes from files adjacent to the server binary or from explicit command overrides. This keeps npm, Homebrew, and direct release installs aligned around one artifact layout.

Install output should include a concise producer summary:

```text
External producers: bundled 11, available 11, auto indexing disabled
```

If bundled helpers are missing, install should warn but not fail unless the server binary itself is missing. A damaged bundle should be visible immediately, but users should still be able to run native indexing.

## Runtime Resolution

Producer command lookup order:

1. `EXTERNAL_INDEX_<LANG>_COMMAND`
2. executable next to `std::env::current_exe()`
3. executable in `PATH`

This is necessary because launchd does not reliably inherit npm or shell `PATH` values. The runtime should report which source was used in producer diagnostics:

- `override`
- `bundled`
- `path`
- `missing`

The existing `generate_external_index` tool should continue returning structured JSON for missing toolchains, unsupported producers, import failures, and successful imports.

## Default Execution Policy

Bundling producers does not imply running them by default.

Initial defaults:

- `EXTERNAL_INDEX_AUTO=false`
- `EXTERNAL_INDEX_ON_REFRESH=disabled`
- `generate_external_index` available manually
- `refresh_index` uses native indexing unless external indexing is explicitly configured
- watch-mode external indexing disabled unless explicitly configured

This keeps install predictable and avoids surprising users with slower first indexes, external toolchain errors, or extra CPU/GPU work before benchmark results justify a default.

## Producer Manifest

Add a manifest to describe shipped helpers and their runtime contract.

Fields:

- `id`
- `language`
- `executable`
- `tier`
- `output_file`
- `requires_project_toolchain`
- `description`

The server can use the same manifest for status reporting and validation. The Rust registry remains the source of behavior; the manifest is the package/install contract.

## Diagnostics And Dashboard

Expose producer availability through the API and dashboard separately from imported external overlay stats.

Recommended API shape:

```json
{
  "external_producers": [
    {
      "id": "typescript",
      "language": "typescript",
      "tier": "first_class",
      "availability": "bundled",
      "executable": "/path/to/code-intelligence-external-typescript",
      "auto_enabled": false
    }
  ]
}
```

Dashboard display should stay operational and compact:

- installed producer count
- unavailable producer count
- current auto policy
- latest producer diagnostics per repo when available

## Error Handling

Producer failures must not fail native indexing. They should be captured as external index metadata and shown in refresh responses, logs, stats, and dashboard data.

Important statuses:

- `missing_bundle`
- `missing_toolchain`
- `unsupported_project`
- `producer_error`
- `producer_task_failed`
- `import_failed`
- `ready`

The user-facing distinction matters: a missing bundled executable is a packaging/install problem; a missing project toolchain is a repo setup problem.

## Release Changes

Update `.github/workflows/release.yml` so the tarball packs the server and producer helpers. The checksum remains per tarball.

Update npm install verification so it checks the bundle shape, not only the server binary.

Update release documentation to state that external producers ship with the binary bundle but automatic execution is still opt-in.

Homebrew should consume the same tarball. If producer helpers are executable files in the archive, the formula should install them beside the server binary.

## Testing

Add focused tests for:

- producer command resolution priority: env override, bundled executable, `PATH`, missing
- install bundle validation against `producers/manifest.json`
- npm extraction accepts a tarball with multiple files
- status/API reports bundled producer availability
- missing bundled producer returns packaging diagnostics without breaking native indexing

Keep existing verification gates:

- `cargo fmt --check`
- `cargo test`
- `EMBEDDINGS_BACKEND=hash cargo test`
- UI typecheck

## Benchmark Gate

Benchmarking decides execution policy, not packaging.

After producers are bundled, benchmark should measure:

- index latency with native-only indexing
- manual external producer latency by language
- refresh latency with explicit external indexing
- retrieval quality changes from imported overlay facts
- error frequency and diagnostics quality on real repos

Only after that should the project consider changing defaults from installed-but-disabled to auto-on for manual refresh or selected language tiers.

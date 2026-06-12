# Provenance Overlay And External Index Foundation

## Summary

Add a provenance overlay that can ingest compiler-grade external index data, merge it with the existing Tree-sitter graph, and expose precise references through current tools without replacing the current indexing pipeline.

The first implementation should support manual external index import and one opt-in auto producer path. Auto generation is explicit in this phase; existing Tree-sitter indexing remains the default fallback for every language.

## Goals

- Add durable storage for external symbols, definitions, references, calls, imports, and type relationships.
- Map external symbols to existing internal `symbols.id` rows when possible.
- Expose merged, provenance-aware facts through existing navigation and investigation tools.
- Keep current search, indexing, and graph behavior working when no external index exists.
- Add one gated auto-generation producer path while keeping importer and tool integration independent from producers.
- Plan support for all currently indexed languages through a shared adapter contract and language capability tiers.

## Non-Goals

- Do not replace the Tree-sitter indexer in this phase.
- Do not make auto generation default-on.
- Do not require every language to have equal precision.
- Do not add LLM-generated answers or prose synthesis.
- Do not train a learned ranker in this phase, though the schema should preserve features that can feed one later.

## Architecture

The feature adds a new external-index layer beside the current SQLite `symbols`, `edges`, and `edge_evidence` tables.

The overlay has four main components:

- `external_indexes`: one row per imported index artifact or producer run.
- `external_symbols`: external symbol identities and optional definition locations.
- `external_references`: occurrences and relationships emitted by external indexers.
- `symbol_mappings`: links between external symbols and internal `symbols.id` rows.

Current tools read through a merged reference provider. That provider queries external facts first, then falls back to existing Tree-sitter facts where imported coverage is missing or partial. Responses include provenance and coverage so agents can distinguish precise compiler-backed facts from heuristic graph facts.

The producer system is separate from the importer. Producers create artifacts. The importer reads artifacts. Tool behavior depends only on imported rows, not on how those rows were generated.

## Storage Model

Add tables with explicit provenance rather than adding nullable SCIP fields to existing graph tables.

`external_indexes`:

- `id`
- `source_kind`: for example `scip`, `lsp`, `compiler`
- `producer`: for example `scip-typescript`, `rust-analyzer`, `gopls`
- `language`
- `root_path`
- `artifact_path`
- `artifact_hash`
- `status`: `imported`, `partial`, `failed`, `stale`
- `diagnostics_json`
- `created_at`
- `updated_at`

`external_symbols`:

- `id`
- `external_index_id`
- `external_symbol`
- `display_name`
- `language`
- `kind`
- `file_path`
- `start_line`
- `end_line`
- `start_byte`
- `end_byte`
- `metadata_json`

`external_references`:

- `id`
- `external_index_id`
- `from_external_symbol_id`
- `to_external_symbol_id`
- `relationship`: `definition`, `reference`, `call`, `import`, `extends`, `implements`, `type`, `read`, `write`
- `file_path`
- `line`
- `column`
- `end_line`
- `end_column`
- `confidence`
- `provenance`
- `metadata_json`

`from_external_symbol_id` and `to_external_symbol_id` may be null when the external artifact reports an occurrence without an enclosing symbol or without a resolved target. The importer should preserve the row and mark confidence accordingly instead of dropping it.

`symbol_mappings`:

- `external_symbol_id`
- `internal_symbol_id`
- `mapping_kind`: `exact_range`, `nearby_name`, `name_only`, `unmapped`
- `confidence`
- `created_at`

Indexes should support lookup by internal symbol, external symbol, relationship, and file path. Import should be transactional per artifact.

## Import Flow

The importer takes an external index artifact and writes normalized rows.

Flow:

1. Validate artifact path and repository root.
2. Parse supported external index format.
3. Normalize file paths through existing repo path handling.
4. Insert an `external_indexes` row.
5. Insert external symbols and references.
6. Map external symbols to internal symbols.
7. Mark unmapped external symbols explicitly.
8. Store diagnostics and coverage stats.

Mapping preference order:

1. Same file and exact range.
2. Same file, same normalized name, compatible kind, nearby range.
3. Same file and same normalized name.
4. External-only, unmapped.

Partial mapping is not a failed import. Tools should expose partial coverage instead of hiding useful precise rows.

## Tool Integration

Add a merged reference provider used by navigation, graph, impact, and investigation handlers.

Provider responsibilities:

- Resolve internal symbols to mapped external symbols.
- Retrieve external references for mapped symbols.
- Convert external relationships into the existing response concepts.
- Merge with Tree-sitter `edges` and `edge_evidence`.
- Deduplicate rows by target symbol, file, line, and relationship.
- Prefer higher-confidence external rows when duplicates exist.
- Return coverage and provenance metadata.

Tool changes:

- `get_definition`: include external definition provenance when mapped.
- `find_references`: list external references first, then fallback references.
- `get_call_hierarchy`: use external `call` relationships when available, fallback to `edges`.
- `find_affected_code`: include external references in impact radius.
- `investigate` and `ask_code`: surface `pack.rows` provenance and partial coverage status.
- `get_index_stats`: include external index counts, language coverage, stale status, and producer diagnostics.

Tool response additions should be additive and backward compatible:

- `provenance`
- `confidence`
- `precise_available`
- `precise_reference_count`
- `fallback_reference_count`
- `coverage`

## Producer System

Add one opt-in auto producer path in this phase. The producer should generate an artifact and feed the same importer used for manual import.

Configuration:

- `EXTERNAL_INDEX_AUTO=1`
- `EXTERNAL_INDEX_PRODUCER=typescript`
- `EXTERNAL_INDEX_ON_REFRESH=disabled|explicit|watch`

Initial behavior:

- Manual import is always available.
- `generate_external_index` can run the selected producer explicitly.
- Auto generation is not default-on.
- Watch-mode generation is disabled unless explicitly configured.

Default-on criteria for future phases:

- Producer fixture tests pass.
- Missing toolchain states return clear diagnostics.
- Fast benchmark subset improves or stays neutral.
- Stale and partial states are visible in stats and dashboard data.
- Repeated refreshes do not cause unacceptable latency.

## Language Support Tiers

The overlay supports all currently indexed languages through one adapter contract:

```text
detect(repo) -> support status + diagnostics
generate(repo, config) -> artifact path + diagnostics
import(artifact) -> external index rows
map(rows, internal symbols) -> symbol_mappings
```

Tier 1: first-class auto producers.

- TypeScript and JavaScript: TypeScript compiler API or SCIP TypeScript.
- Rust: rust-analyzer or SCIP Rust.
- Go: gopls or SCIP Go.
- Python: pyright or basedpyright when environment discovery succeeds.

Tier 2: build-aware producers.

- Java.
- Kotlin.
- C#.
- Swift.

These require project and SDK detection before generation. If the build context is missing, the adapter reports `supported_not_configured`.

Tier 3: compile-database producers.

- C.
- C++.

These require `compile_commands.json` or equivalent. Without it, the tool reports precise coverage unavailable and falls back to Tree-sitter.

Tier 4: fallback wrapper.

- Ruby.

Ruby has the same normalized-artifact producer wrapper contract as the other supported languages, but Tree-sitter remains the practical fallback unless a reliable configured command is available.

## Error Handling

- Import rejects paths that escape the repository.
- Import is transactional per artifact.
- Unsupported external index fields are ignored and counted in diagnostics.
- Missing producer toolchains do not fail normal indexing.
- Failed generation does not delete the last successful external index.
- Stale external indexes remain queryable but are marked stale.
- Partial imports expose partial coverage instead of pretending to be complete.

## Testing

Unit tests:

- Path normalization for external artifacts.
- Symbol mapping by exact range, nearby name, and unmapped cases.
- Reference merge ordering and deduplication.
- Coverage status calculation.
- Tool response backward compatibility.

Fixture tests:

- Small TypeScript project with definitions, references, imports, and calls.
- Small Rust or Go fixture once the second producer adapter is added.
- Partial mapping fixture where some external symbols do not map internally.

Tool-level tests:

- `find_references` prefers external rows when present.
- `find_references` falls back when no external rows exist.
- `get_call_hierarchy` includes external call edges.
- `investigate` exposes provenance in `pack.rows`.
- `get_index_stats` reports external index coverage.

Benchmark checks:

- Add a fast golden subset focused on reference-heavy and impact questions.
- Compare baseline Tree-sitter-only mode with overlay-enabled mode.
- Track citation correctness, no-hit correctness, tool latency, and fallback counts.

## Rollout

Phase 1:

- Add schema and migrations.
- Add importer.
- Add mapping.
- Add merged reference provider.
- Integrate existing tools.
- Add stats and diagnostics.

Phase 1.5:

- Add explicit `generate_external_index`.
- Add the first producer adapter, expected to be TypeScript/JavaScript.
- Feed generated artifacts into the importer.
- Keep auto generation opt-in.

Phase 2:

- Add more language producers by tier.
- Add dashboard visibility for precise index state.
- Consider enabling auto generation for proven Tier 1 producers.
- Feed provenance features into a future calibrated ranker.

## Acceptance Criteria

- Existing tools behave the same when no external index exists.
- A manually imported external index improves `find_references` output for a fixture repo.
- Tool responses include provenance and coverage without breaking existing fields.
- Partial external coverage is represented explicitly.
- A gated producer can generate and import one language artifact through the same import path.
- Tests cover importer, mapping, merged provider, and affected tool behavior.

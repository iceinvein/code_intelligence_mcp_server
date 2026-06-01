# Web Portal Symbols & Browse Design

Date: 2026-06-01
Status: Approved (pre-implementation)

Phase 5a of the web portal program (see
`docs/specs/2026-05-31-web-portal-react-design.md`). Phase 5 in the master spec
bundles three capabilities; during brainstorming it was split into:

- **5a (this spec):** symbol inspector + file-tree browse (the `symbols` route).
- **5b (later):** call / type / dependency graph visualization (the `graph`
  route), which owns the graph-visualisation library decision.

5a is brainstormed and built first because it is lower-risk and reuses the Phase 3
search components; 5b's interactive-canvas and library choices deserve their own
cycle.

## Problem

The portal can search the index and inspect a result's definition + references, but
there is no way to browse the indexed repository by file and inspect an arbitrary
symbol's full context without first constructing a search query. The `symbols` route
is a Placeholder. The backend handlers for per-file symbols and usage examples exist
but are MCP-only; they are not on the JSON API.

## Goals

- Deliver the `symbols` route: a three-pane browser (file tree, symbol outline for
  the selected file, inspector for the selected symbol).
- The inspector stacks three sections: definition, references (grouped by file),
  usage examples.
- Reuse the Phase 3 rendering components rather than duplicating them; extract the
  shared reference-rendering out of `ResultRow` so search and the inspector share it.
- Expose the missing read-only query data (file list, per-file symbols, usage
  examples) on the JSON API, following the existing `/api/query/*` pattern.

## Non-Goals

- No graph visualisation (that is 5b).
- No editing or write operations. The `symbols` route is read-only navigation.
- No per-repo configuration or cross-repo browse; one selected repo at a time.
- No new ranking/retrieval behaviour; this wraps existing handlers.
- No tree virtualisation in v1 (see Risks).

## Decisions

Settled during brainstorming; recorded so the plan does not relitigate them.

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Phase 5 split | 5a (symbols/browse) before 5b (graph) | 5a is lower-risk and reuses search; 5b's library/interaction complexity warrants its own cycle. |
| Layout | Three pane (files / outline / inspector) | Most code-console-native; three simple independent lists; inspector reuses search components. |
| Inspector sections | definition + references + usage examples, stacked | Usage examples is the net-new value over what search expansion already shows; stacked matches the dense aesthetic. |
| Usage examples in 5a | Included | The handler exists; it is what makes the inspector worth a dedicated route. |
| File-tree data source | New flat file-list endpoint; client builds the tree | `repo_map` is token-budgeted (a summarised subset), unusable for an exhaustive tree. A flat list + client-side tree assembly is standard and simple. |
| Reference rendering | Extract a shared `ReferencesPanel` from `ResultRow` | Avoids duplicating the grouped-reference rendering; a small, justified refactor of search. |
| Repo selection | Reuse the `SearchView` selector pattern | Consistent with the existing query surface. |

## Architecture

### Backend (`src/server/api/query.rs`, `src/storage/sqlite/`)

Three new JSON endpoints, each mirroring `handle_query_definition`
(`resolve_query_repo` -> build tool -> call handler -> `query_envelope`):

- `POST /api/query/files` -> new. Returns the full indexed-file list for the tree.
  Backed by a new SQLite query `list_indexed_files()`
  (`SELECT file_path, COUNT(*) AS symbol_count FROM symbols GROUP BY file_path
  ORDER BY file_path`). Response: `{ files: [{ path, symbol_count }] }`, wrapped in
  the standard query envelope. The handler calls the query directly rather than a
  tool (there is no existing list-files tool).
- `POST /api/query/file-symbols` -> wraps the existing `handle_get_file_symbols`
  via `GetFileSymbolsTool { file_path, exported_only }`.
- `POST /api/query/usage-examples` -> wraps the existing `handle_get_usage_examples`
  via `GetUsageExamplesTool`.

`definition` and `references` already exist and are reused unchanged. All new routes
are registered in `src/server/api/mod.rs` next to the other `/api/query/*` routes.

Paths returned by `/api/query/files` are in the same base-relative form that
`handle_get_file_symbols` expects as input, so a tree selection round-trips without
re-normalisation. An endpoint test pins this.

### Frontend (`ui/src/features/symbols/`)

- `ui/src/api/symbols.ts` -> typed client: `fetchFiles`, `fetchFileSymbols`,
  `fetchUsageExamples`. Reuses the existing `getDefinition` / `findReferences` from
  `ui/src/api/search.ts`.
- `SymbolsView.tsx` -> three-pane orchestration plus the repo selector (reusing the
  `SearchView` selector pattern). Selected file and selected symbol are held in URL
  params for deep-linking, matching search.
- `FileTree.tsx` -> builds a nested tree client-side from the flat `files` list;
  collapsible directories; per-file symbol count. Tree-building is a pure function
  (`buildTree(files)`) with its own test.
- `SymbolOutline.tsx` -> the symbol list for the selected file (from
  `file-symbols`); selecting a row sets the selected symbol.
- `SymbolInspector.tsx` -> stacked definition / references / usage-examples for the
  selected symbol.
- `useSymbols.ts` -> TanStack Query hooks for files, file-symbols, definition,
  references, usage-examples.
- `routes.tsx` -> swap the `symbols` Placeholder for `SymbolsView`.

Reuse, not duplication: the inspector uses the existing `CodeBlock` and
`DefinitionPanel`. The grouped-reference rendering currently inline in
`features/search/ResultRow.tsx` is extracted into a shared `ReferencesPanel`
component consumed by both `ResultRow` and `SymbolInspector`.

### Data flow

Pick repo (selector) -> `files` populates the tree -> click a file ->
`file-symbols` populates the outline -> click a symbol -> the inspector fires
`definition` + `references` + `usage-examples` in parallel. Selected file and symbol
persist in URL params.

## Testing

- `cargo test`:
  - `list_indexed_files` returns one row per file with the correct symbol count.
  - The three new endpoint wrappers return the standard query envelope and the
    expected payload (mirror the existing `definition` / `references` endpoint
    tests).
  - Path round-trip: a path from `/api/query/files` is accepted by
    `/api/query/file-symbols`.
- `bun test`:
  - `symbols.ts` client contract tests (request path, body, response shape).
  - `buildTree(files)` pure-function test (flat list -> nested tree; directory
    grouping; counts).

Per the repo commit protocol: run the relevant `cargo test` / `bun test` plus
`cargo fmt`, `cargo clippy`, and the frontend lint/format before any commit.

## Risks and Open Questions

- **Large-repo tree performance.** Thousands of files render unvirtualised in v1.
  Acceptable to start; if it is slow on big repos, add list virtualisation as a
  follow-up.
- **`ReferencesPanel` extraction.** Refactoring search's `ResultRow` carries a small
  regression risk to the shipped search view; the existing `ResultRow` test guards
  it, and the extraction is behaviour-preserving.
- **Symbol disambiguation.** `get_definition` / `find_references` key off symbol
  name (+ optional file). The inspector already knows the selected symbol's file, so
  it passes the file to disambiguate; ambiguous same-name symbols in one file are an
  existing handler limitation, not introduced here.

# Web Portal Search Phase Design

Date: 2026-05-31
Status: Approved (pre-implementation)

## Context

Detailed design for the search phase of the React web portal (the "first-class
search/investigate UI" bullet in `2026-05-31-web-portal-react-design.md`). Phase
1 (foundation) and Phase 1b (control-plane parity) are merged; the `/search`
route currently renders a "coming in a later phase" placeholder. The legacy
dashboard had only a bare query playground; this phase makes search a
daily-driver code-finding tool a human actually uses.

## Problem

A developer wants to find code by meaning, not just text: "where is session
binding resolved", "the reranker config", "how does X work". The daemon already
exposes `search_code` (hybrid BM25 + vector, RRF-fused, ranked), plus
`get_definition` and `find_references` as MCP tools, but the only HTTP surface
is `POST /api/query/search` (and the question-oriented `ask`/`investigate`).
There is no human UI for ranked search with navigation.

## Goals

- A ranked semantic search experience: query + repo selector -> ranked symbol
  results with snippets and scores.
- Inline expansion of a result to its full definition (syntax-highlighted) and
  its references (grouped by file).
- Deep-linkable searches via URL params (`?repo=&q=`).
- Preserve the terminal aesthetic and the established Phase 1 patterns (typed
  api client, TanStack Query, feature folders, shadcn primitives).

## Non-Goals

- No `ask`/`investigate` (natural-language evidence-pack) UI. Deferred to its
  own phase.
- No call/type/dependency graph navigation. That is the later graph/symbol
  exploration phase; this phase stops at definition + references.
- No write/refactor actions. Search is read-only.
- No search-as-you-type. Search is explicitly submitted (see Decisions).

## Decisions

Settled during brainstorming; rationale recorded so the plan does not
relitigate.

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Modes | Ranked search + jump to definition/references | Focused daily-driver finder; matches the spec bullet. ask/investigate is its own phase. |
| Results layout | Single column, inline expand | Simplest, keyboard/terminal-friendly; expanding shows definition + references in place. |
| Syntax highlighting | shiki (VS Code-grade), lazy-loaded | Highest fidelity; lazy so it loads only on first code expansion and does not bloat the initial bundle. Dual light/dark theme wired to the portal theme classes. |
| Search trigger | Explicit submit (Enter / button) | `search_code` is GPU/embedding-backed; as-you-type would be laggy and wasteful. Loading/empty/error states. |
| New endpoints location | Beside existing handlers in `src/server/api.rs` | Two more `/api/query/*` handlers are consistent with the existing five; no module split this phase. |
| References rendering | Grouped by file | A symbol's callers/uses cluster by file; grouping reads better than a flat list. |
| Search state | URL params (`?repo=&q=`) | Deep-linkable, shareable searches; survives reload. |

## Architecture

### Backend (`src/server/api.rs`)

Two new endpoints, mirroring the existing query handlers exactly
(`resolve_query_repo` to load the per-repo `AppState`, then `query_envelope` to
wrap the tool result):

- `POST /api/query/definition` - body `{ repo, symbol_name, file?, limit? }` ->
  `crate::handlers::handle_get_definition` (async). Envelope `command` =
  `"definition"`.
- `POST /api/query/references` - body
  `{ repo, symbol_name, file?, reference_type?, limit? }` ->
  `crate::handlers::handle_find_references` (NOTE: this handler is synchronous,
  `pub fn`, not `async`; call it without `.await`). Envelope `command` =
  `"references"`.

Request structs follow the existing `QuerySearchRequest` pattern. Routes are
added to the router beside the other `/api/query/*` routes, inside the same
`check_origin` layer. `POST /api/query/search` is reused unchanged.

### Frontend (`ui/src/features/search/`)

- `SearchView.tsx` - the `/search` view: a search bar (query input + repo
  selector + submit), result count, and the results column. Reads/writes
  `?repo=` and `?q=` URL params (via React Router `useSearchParams`) so searches
  are deep-linkable; submitting updates the URL, which drives the query.
- `ResultRow.tsx` - one ranked hit (symbol name, kind, `file_path:line`, score,
  snippet). Expand/collapse; on expand it lazily loads the definition and
  references for that symbol.
- `DefinitionPanel.tsx` - renders the definition (a `CodeBlock`) plus a
  references section grouped by file (each file is a collapsible group listing
  its reference lines).
- `CodeBlock.tsx` - renders a code string through shiki; shows plain monospace
  text until the async highlight resolves.
- `useSearch.ts` - `useSearch(repo, query)` (TanStack Query, `enabled` only when
  a query is submitted), `useDefinition(repo, symbolName, file, enabled)`,
  `useReferences(repo, symbolName, file, enabled)` (lazy, gated by row
  expansion).
- `ui/src/api/search.ts` - `searchCode`, `getDefinition`, `findReferences`
  fetchers (POST via the existing `apiSend`), and the response/hit `type`s. The
  query endpoints return the standard envelope `{ ok, command, repo, index,
  warnings, result }`; the fetchers return `result` typed per endpoint.
- `ui/src/lib/shiki.ts` - a lazily-created highlighter singleton loading the
  indexed languages (typescript, tsx, rust, python, go, java, c, cpp) and a
  light + dark theme; a highlight-to-HTML helper that the `CodeBlock`
  awaits. shiki is dynamically imported so it is not in the initial chunk.
- `routes.tsx` - `/search` renders `SearchView` (placeholder removed; the
  `Placeholder` component stays for the still-future routes).

### Data shapes

- Search result item (from `handle_search_code`, inside `result.results[]`):
  `{ name, kind, file_path, line, score, snippet }`. TS `SearchHit`.
- `get_definition` returns definition metadata + body; `find_references`
  returns reference edges each carrying a `file_path` (and line / reference
  type). The references view groups the returned array by `file_path`
  client-side. Exact field names are read from the handler output when writing
  the plan; the TS types mirror them.

### Repo selector

A dropdown populated from `GET /api/repos`. If exactly one repo is registered it
auto-selects; otherwise the user must pick before searching. The selected repo
id/path lives in the `?repo=` URL param.

## Error handling

- No repo selected -> the search bar prompts to choose a repo; submit disabled.
- Empty/whitespace query -> submit disabled.
- Search/definition/references request failure -> inline error in the relevant
  region (`ApiError` message), not a crash; other regions keep working.
- shiki load/highlight failure -> fall back to plain monospace code (never
  blocks showing the code).

## Testing

- `bun test`: `useSearch`/result render with mocked fetch (results render with
  name + file + score); `ResultRow` lazy-loads definition/references on expand
  (mocked); references grouping by file; repo-selector auto-select with one
  repo; `CodeBlock` with shiki mocked (renders plain text, then highlighted
  markup). shiki itself is mocked in tests to avoid async grammar loading.
- `cargo test`: envelope-shape assertions for `/api/query/definition` and
  `/api/query/references` (extend the existing `query_envelope` contract test to
  cover the two new `command` values).

## Risks and Open Questions

- **shiki bundle/perf.** Mitigated by dynamic import + lazy highlighter init
  (loads on first expansion). Measure the lazy chunk size after implementation;
  if too large, trim languages or switch to shiki's fine-grained core bundle.
- **Theme wiring for shiki.** The portal uses `theme-light`/`theme-dark` classes
  (not shiki's default `.dark`). The plan wires shiki's dual-theme output to
  those classes (custom selector / CSS variables); decided at plan time.
- **Definition/reference field names.** Read from `handle_get_definition` /
  `handle_find_references` output when writing the plan so the TS types and the
  file-grouping key are exact.
- **Large reference sets.** `find_references` defaults to 200. The grouped view
  should stay readable; cap displayed groups/rows with a "show more" if needed
  (decided at plan time).

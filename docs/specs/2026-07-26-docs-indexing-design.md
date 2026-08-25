# Documentation Indexing: ADRs, Issues, and Guides as First-Class Evidence

Date: 2026-07-26
Status: Draft

## Problem

The index is code-only. When an agent asks "why does auth use refresh-token
rotation?" or "is there a known bug in the retry path?", the engine can only
reach comments and identifier shapes. The answers usually live in ADRs, design
docs, issue notes, and changelogs — prose the pipeline never sees. The result is
a retrieval gap exactly where agents need the most help: questions about intent,
history, and rationale rather than structure.

We want repository documentation in the index as first-class, retrievable,
cross-linked evidence — without polluting code navigation, without a parallel
retrieval stack, and without an indexing or ranking tax on doc-heavy repos.

## Design position

**Documents are symbols.** A new `SymbolKind::Document` flows through the
existing spine — scan → extract → SQLite → Tantivy → LanceDB → edges — so
search, RRF fusion, embeddings, hydrate, ask/investigate, and repo-map work
with near-zero new machinery. Docs are *not* a second corpus with its own
query path.

Three invariants protect code retrieval:

1. **Navigation stays code-only.** Call hierarchy, type graph, dependency
   graph, references, and definition-intent queries exclude `Document` at
   dispatch.
2. **Provenance is explicit.** Evidence from docs is labelled
   `source: documentation` and returns markdown verbatim; it is never blended
   into a code-body field.
3. **Disagreement is surfaced, not resolved.** The engine does not arbitrate
   between a stale ADR and current code. It exposes status, dates, and source
   kind, and lets the calling agent reason about drift. Drift is signal:
   a doc that references a deleted symbol is a stale-link fact worth reporting,
   not an error to suppress.

## What gets indexed

Tiered sources; Tier 1 on by default, the rest behind config:

| Tier | Sources | Default |
|------|---------|---------|
| 1 | `README*`, `CONTRIBUTING*`, `CHANGELOG*`, `docs/**/*.md`, `adr/**/*.md`, `decisions/**/*.md` | On |
| 2 | Any other `*.md` / `*.mdx` / `*.rst` outside vendored paths | `DOCS_PATTERNS` opt-in |
| 3 | GitHub/GitLab issues & discussions via an out-of-process producer | `EXTERNAL_INDEX_*` opt-in |

Vendored and generated docs are always excluded
(`node_modules/`, `target/`, `dist/`, `vendor/`, third-party changelogs).

### Chunking

Sections, not files. Each document is split at H2/H3 boundaries; each section
becomes one indexed unit with its own id, heading, line range, and embedding.
Rules:

- Content in front of the first heading becomes a `preamble` section (covers
  most READMEs).
- A section exceeding ~4 KB is split on paragraph boundaries.
- Identical section content (hash-equal) across files is embedded once and
  deduplicated in the vector store.

### Classification

Each section carries a `DocType` derived from path and front-matter:
`Adr | Issue | Bug | Changelog | Guide | Readme | Design | Other`.
YAML front-matter is parsed into structured fields: ADRs (`number`,
`status: proposed|accepted|superseded|deprecated`, `date`), issues (`labels`,
`state`, `severity`). `superseded`/`deprecated` documents are demoted in
ranking, never deleted.

## Pipeline

```
scan            *.md globs added to the walk; content-hash unchanged-skip as today
  ↓
extract/markdown.rs   hand parser (no tree-sitter): headings, front-matter,
                      backtick refs, issue numbers, TODO/FIXME matches
  ↓
SQLite          SymbolKind::Document rows + doc metadata columns
  ↓
Tantivy         section text in `text`, heading in `name`, new `doc_type`
                and `doc_status` fields
  ↓
LanceDB         one jina embedding per section (same model, same dimension)
  ↓
edges           doc→symbol backtick links; TODO↔issue matches; existing edge table
```

- **Watcher:** editing a `.md` triggers the normal delta pass; sections for the
  changed file are deleted and re-upserted.
- **Worktree seeding:** unchanged, since it clones storage wholesale.
- **No new storage engine, no new query path, no new model.**

## Cross-linking (where the intelligence compounds)

- **Backtick refs.** `` `authenticate_request` `` in an ADR or issue creates a
  doc→symbol edge. Consequences: `find_references` on a function surfaces the
  docs discussing it; `investigate --mode impact` shows documented rationale
  before a refactor; doc→symbol edges feed the existing PageRank-style
  popularity signal.
- **TODO/FIXME closure.** Existing `TodoEntry` rows are matched against issue
  titles/numbers ("fixes #42"), turning the debt table into a tracked backlog
  view.
- **Stale-link detection.** Edges pointing at nonexistent symbols are reported
  in index stats (cheap, already computed during edge extraction).

## Ranking behaviour

- New `Intent::Documentation` detected from patterns: "why", "decision",
  "adr", "issue #N", "known bug", "rationale", "how do we". Boosts
  `kind=document`, demotes code noise.
- Symmetric gate: `Intent::Definition` and navigation intents suppress
  documents entirely — docs never compete for "where is X defined".
- Fused result cap: at most 3–5 document hits per query unless intent is
  Documentation, so ask_code evidence stays code-dominated by default.
- Recency and `doc_status` weight the doc-side score; the existing >2.5x
  score-gap trimmer bounds trailing doc noise.

## API surface

- `search_code` / `ask_code` / `investigate`: hits may carry
  `kind: "document"`, `doc_type`, `doc_status`, `source: "documentation"`;
  `hydrate_symbols` on a doc id returns the markdown section verbatim.
- `repo-map`: adds a docs line ("12 ADRs (1 superseded), 4 open issues,
  9 guides") so an agent knows institutional knowledge exists.
- New CLI flag `--docs-only` on `search` (and `intent=documentation` on
  `investigate`) for explicit doc queries.
- `index status`: doc counts and stale-link count.
- No new MCP tools; existing tools gain the doc fields via the shared handler
  layer.

## Configuration

| Knob | Default | Meaning |
|------|---------|---------|
| `DOCS_ENABLED` | `true` | Master switch for Tier 1 |
| `DOCS_PATTERNS` | Tier-1 globs | Glob override/addition for indexed docs |
| `DOCS_MAX_HITS` | `4` | Per-query document cap in fused results |
| `DOCS_ISSUES_ENABLED` | `false` | Enable the GitHub issues producer (Tier 3) |

Editable via the web portal Settings (`[docs]` section in `server.toml`),
consistent with the other Tier-2 knobs.

## Doc-heavy repo budget

The failure mode to design against is ranking pollution and index bloat, not
throughput:

- **Cost.** Worst realistic case (~500 files × 15 sections ≈ 7.5k chunks)
  roughly doubles this repo's current symbol count. Embedding is the only real
  compute; it is a one-time initial-index cost of seconds-to-minutes on Metal.
  Unchanged-file skipping keeps steady-state cost near zero.
- **Storage.** Tens of MB for 10k sections across SQLite + Tantivy + LanceDB.
  Negligible.
- **Latency.** Docs ride the same parallel Tantivy/LanceDB queries filtered by
  kind — no additional round trips.
- **Quality.** Intent gating, the per-query cap, dedup, and the score-gap
  trimmer are the defences; their effectiveness is a benchmark question, not an
  assumption (see Validation).

## Risks

| Risk | Mitigation |
|------|-----------|
| Doc hits crowd out code in mixed queries | Intent gate + `DOCS_MAX_HITS` cap |
| Stale ADR outranks current code | Status demotion, recency weight, explicit provenance |
| Heading-less or exotic markdown | Paragraph-split fallback; CommonMark-only parser, accept imperfection |
| Vendored/generated doc flood | Hard default excludes; Tier 2 opt-in |
| IDF dilution from repetitive prose | Section chunking + dedup + score-gap trimmer |

## Phasing

1. **Phase 1 — spine.** `SymbolKind::Document`, `extract/markdown.rs`, scan
   globs, SQLite/Tantivy/LanceDB wiring, graph-tool filters, hydrate support.
2. **Phase 2 — classification & ranking.** Front-matter, `DocType`,
   `Intent::Documentation`, per-query cap, status demotion, repo-map line.
3. **Phase 3 — cross-links.** Backtick edges, TODO↔issue matching, stale-link
   reporting, impact-mode integration.
4. **Phase 4 — external issues.** `gh`-based producer under the
   `EXTERNAL_INDEX_*` contract, off by default.

## Validation

- Extend the engine quality gate (`deterministic_engine_quality_gate`) with
  doc-retrieval cases: ADR "why" questions, issue lookups, and — critically —
  negative cases where definition queries must *not* return documents.
- Add a doc-heavy question subset to the bench iteration set; gate the release
  set only after Phase 2 ranking lands. Success criterion: doc questions
  improve citation/judge scores without regressing code-question scores in the
  same round.
- Perf assertion: p95 search latency delta ≤ 5% with Tier-1 docs enabled on a
  fixture repo with ≥ 1,000 markdown files.

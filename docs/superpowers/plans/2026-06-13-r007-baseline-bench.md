# R007 Post-Foundation Baseline Benchmark

**Status:** ready to run (harness changes landed, Phase 0 in progress)
**Date:** 2026-06-13
**Owner:** dik.rana
**Round:** R007 (next sequential after R001-R006)

## 1. Purpose

Establish a clean R007 baseline that measures the *currently measurable* part of the
61 commits sitting ahead of `origin/main`, and refresh the stale R005/R006 numbers
(the harness has not run since ~2026-05-29).

This is explicitly **not** a verdict on the external-producer strategy. See the finding below.

## 2. What landed since origin/main

Two feature clusters:

- **Provenance overlay / external index (~45 commits).** New SQLite layer
  (`external_indexes`, `external_symbols`, `external_references`, `symbol_mappings`),
  an importer, a merged reference provider (external facts first, Tree-sitter fallback),
  provenance/coverage fields threaded through `find_references`, `get_call_hierarchy`,
  `find_affected_code`, `get_definition`, `investigate`/`ask_code`, `get_index_stats`,
  plus bundled producer entrypoints, a manifest, and binary-dir resolution.
- **Evidence-pack quality layer + rrf.k (~13 commits).** Pack coverage verification,
  non-callgraph edge candidate rows, candidate gating, golden-pack regression tests,
  and the `rrf.k` knob wired into fusion (previously hardcoded).

## 3. The finding that shapes this run

The overlay infrastructure is real and tested, but **all 11 bundled producers are
stubs**: each exits 69 with "bundled but no generator is enabled yet". The importer
only ingests a hand-authored `normalized_json` artifact, and the only one that exists
is a 42-line test fixture (`tests/fixtures/external_index/typescript-normalized.json`).
No real SCIP/LSP/compiler generator is wired in anywhere.

Consequence: on a real repo, `EXTERNAL_INDEX_AUTO=1` produces zero external rows, so the
merged provider is inert and output is identical to Tree-sitter-only. **The overlay's
retrieval-quality impact is not measurable yet.** What *is* measurable now is the
evidence-pack + rrf.k work, because it changes the `ask_code`/`investigate` path on
every repo regardless of the overlay.

## 4. What R007 will and will not tell us

**Will:**
- Whether evidence packs + rrf.k moved judge / mech / citation% on the shipped
  retrieval path, versus the R005 baseline.
- That the provenance-overlay foundation did not regress the inert path
  (R007 shipped vs the pre-overlay R005 row at the same config).

**Will not:**
- Anything about overlay retrieval quality on real code (no producer -> no external
  rows). Deferred to the follow-up in section 9.

## 5. Run configuration

Single code-intelligence arm; the other arms (`default`, `codegraph`,
`code_intel_full`, `code_intel_no_reranker`) are intentionally dropped because their
baselines already exist and the only question now is improvement/regression on
code-intelligence itself. This also keeps judge volume under the rate-limit threshold.

| dimension | value |
|---|---|
| arm | `code_intel_shipped` (renamed from `code_intel_no_descriptions`) |
| arm config | descriptions off, reranker off, `no_desc` index = production defaults |
| repos | wolfmax + Django (40 questions) |
| index variant | `no_desc` (plain Tree-sitter; ~10-30 min prep, no description backfill) |
| agent model | `claude-sonnet-4-6` (held fixed for clean R005 comparison) |
| judges | `claude-haiku-4-5` / `claude-sonnet-4-6` / `claude-opus-4-8` |
| volume | 40 runs + 120 judge calls (single judging pass expected) |
| baseline | R005 `code_intel_no_descriptions` (same config, pre-evidence-pack/rrf.k/overlay) |

### Why `code_intel_shipped` is the right arm

Production ships descriptions off (judged neutral in R005) and reranker off. So the
arm that mirrors what users actually run is descriptions-off + reranker-off on a plain
Tree-sitter index, with the overlay schema present but inert. That is exactly the old
`code_intel_no_descriptions` config, renamed here for clarity.

### Accepted confounds

- Judge Opus drifted 4.7 -> 4.8 (4.7 may no longer resolve; 4.8 is current frontier).
- Everything else (agent model, arm config, repos, question set) is held constant
  against the R005 baseline so the cross-round delta isolates the new code.

## 6. Harness changes (landed 2026-06-13)

- Renamed arm `code_intel_no_descriptions` -> `code_intel_shipped` in `bench/arms.py`,
  `bench/report.py`, and `bench/tests/{test_arms,test_daemon,test_report}.py`. Config
  unchanged; added a comment documenting it as the production-default shipped config
  with the overlay inert.
- Bumped judge Opus default `claude-opus-4-7` -> `claude-opus-4-8` in `bench/config.py`,
  updated the two model-ID-keyed tests.
- `make -C bench test`: 52 passed.

## 7. Phases

### Phase 0 — pre-flight (cheap, non-destructive)

1. `cargo build --release` (the bench measures this exact binary's SHA).
2. `EMBEDDINGS_BACKEND=hash cargo test` (daemon tests, incl. external_index importer/
   provider/mapping and golden-pack regressions).
3. `cargo fmt --check && cargo clippy`.
4. `make -C bench test` (done: 52 passed).
5. Verify `claude-opus-4-8`, `claude-sonnet-4-6`, `claude-haiku-4-5` resolve via
   `claude --print --model <id>`.

### Phase 1 — smoke end-to-end (minutes)

`./bench full --repos smoke --arms code_intel_shipped`

3 questions against this repo: exercises prep -> daemon -> MCP bind -> agent -> mech ->
judge. Green here means the full pipeline works; only scale/time remains. Optionally
sanity-check the overlay end-to-end via the `external-index-smoke` fixture (functional,
not quality).

### Phase 2 — prep (~10-30 min)

`./bench prep --check` then `./bench prep`

The daemon SHA changed (61 commits), invalidating the index cache. Only the `no_desc`
variant builds for wolfmax + Django (clone + Tree-sitter + embeddings). No description
worker, so none of the historical multi-hour backfill.

### Phase 3 — full cycle -> R007

`./bench full --arms code_intel_shipped`

Auto-numbers to R007. 40 runs + 120 judge calls. Single judging pass expected (under
the ~240-call/5h window that bit R005).

### Phase 4 — report + interpret

- `./bench report R007` (note: header `daemon_sha`/codegraph fields show `?` due to a
  known cosmetic bug; real metadata is in `meta.json`).
- `bench diff` is a stub, so compare R007 vs R005 `code_intel_no_descriptions` manually
  from the two `scores.json` files. Most informative slice: per-task-type judge/mech,
  since evidence-pack gains concentrate in `multi_hop` / `impact` / `architectural`.

## 8. Risks and mitigations

| risk | mitigation |
|---|---|
| Judge rate limits | 120 calls is under the R005 threshold; single pass expected. If hit, `judge.jsonl` is the durable artifact and is resumable. |
| `opus-4-7` no longer resolves | bumped to `opus-4-8`; verified in Phase 0. |
| Description worker stagnation | N/A this run (no_desc variant, no worker). |
| Cosmetic report metadata bug | Known; read `meta.json` for true SHA/version. |
| Cross-round agent stochasticity | Agent model pinned to `sonnet-4-6`; N=40 with 3-judge median range as the noise band. Treat sub-noise deltas as noise. |

## 9. Follow-up (deferred)

Once real producers exist (e.g. wrap `scip-typescript` to emit the normalized JSON
contract for wolfmax, since the importer ingests normalized JSON, not raw SCIP), add a
`code_intel_external` arm/variant whose index is built with `EXTERNAL_INDEX_AUTO=1` +
the producer, and A/B it against this R007 baseline. That is the run that decides the
external-producer default-execution policy.

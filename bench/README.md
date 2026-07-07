# Bench

The code-intelligence-mcp benchmark harness. Runs N arms against M repos with K questions each, scores each answer mechanically and via tiered LLM judging, renders a markdown report.

R005 (2026-05-28) is the first cross-repo cycle: 5 arms × 40 questions across wolfmax and Django.

## Quick start

```bash
make -C bench install        # PyYAML + pytest (anthropic SDK is no longer required)
make -C bench test           # pytest
make -C bench validate-smoke # lint the smoke fixture
./bench full --arms default,code_intel_shipped --repos wolfmax,django --repeats 3
./bench report R010
python3 -m bench.rescore R008   # zero-token re-score after scoring-logic changes
```

The `./bench` wrapper is `python3 -m bench.run`. There is no shell-script wrapper at the repo root (it would collide with the `bench/` directory).

## Harness behavior (since 2026-07-05 overhaul)

- **Crash-safe + resumable.** `runs.jsonl` and `judge.jsonl` are appended record-by-record; `scores.json` is derived data rebuilt at cycle end. `full --round <N>` on an existing round resumes it: completed (arm, question, rep) runs and judged rows are skipped. A crash loses at most the in-flight record (R009 previously lost all 80 runs to the old end-of-cycle write).
- **Isolated CLI calls.** Runner and judge pass `--strict-mcp-config` and `--setting-sources ""`, so globally-configured MCP servers (including the production code-intelligence daemon), user/project settings, and hooks no longer leak into arms or judges. Before this, the `default` arm was never a clean no-MCP baseline.
- **Real tool telemetry.** `--output-format stream-json --verbose` gives real per-tool names and result sizes (the old `json` format only exposed `num_turns`, so the tools column was a turn count). Result sizes make token-hungry tool responses visible.
- **Turn caps.** `--max-turns` (`BENCH_MAX_TURNS`, default 12, 0 disables). Typical runs use 5-6 turns; the R008 tail (30 turns, ~880k tokens) burned cache-read tokens quadratically. Capped runs are flagged `hit_turn_cap`.
- **Concurrency.** Agent runs execute in a per-arm pool (`BENCH_RUN_CONCURRENCY`, default 4); judging in its own pool (`BENCH_JUDGE_CONCURRENCY`, default 3).
- **Tiered judging** (`BENCH_JUDGE_TIERED`, default on): haiku scores first; decisive extremes (0-2, 9-10) are accepted haiku-only, mid-band (3-8) or errored haiku escalates to the sonnet+opus panel. Roughly halves judge calls against the ~240-calls/5h subscription window. `judge.jsonl` records the tier per row.
- **Judge casualty semantics.** Errored judges are excluded from the median (an errored judge used to count as a 0 and drag it); rows with fewer than 2 valid panel judges are casualties with `judge_median=None`, which reports skip. Empty/errored answers are never judged.
- **Runner error handling.** Nonzero CLI exits retry once, then record `run_error=cli_exit_N`; timeouts keep the partial transcript and record `run_error=timeout`. Errored runs are excluded from judging and visible in scores.
- **Repeats.** `full --repeats N` runs each (arm, question) N times (rep index in every record) for paired variance analysis. Single runs cannot distinguish real deltas from the ±1.6 judge noise band.
- **Quota-exhaustion recovery.** After `BENCH_MAX_CONSECUTIVE_FAILURES` (default 5) consecutive failed runs or fully-failed judgements, the cycle aborts with a resume command instead of burning the remaining quota; exit code 3. On resume, errored runs are re-run and error-casualty judge rows re-judged (records dedupe last-wins, so the fresh attempt replaces the failed one).
- **Index caches key on the daemon binary hash** (not git HEAD), so bench-only commits do not invalidate cached embedding work, and Rust changes without a rebuild do not falsely pass as fresh.
- **Token efficiency is a first-class output.** The report includes tokens/run, tokens-per-judge-point, and the turn-capped rate per arm.

### Scoring revision (2026-07-05)

Three scoring bugs were fixed and R007/R008 re-scored (`scores.json.pre-rescore` keeps the old rows):

1. Forbidden-term matching is negation-aware. Correct negative answers that deny a forbidden term ("there is no RedisCache") were zeroed; 7/8 negative rows in R008 were false-penalized.
2. Citation extraction rejects non-paths (`127.0.0.1:17800`, host:port, version strings) that were scored as hallucinated citations.
3. Citation-hit matching accepts plural "lines 42-88" and "(L42)".

Re-scored aggregates: R007 mech 0.465→0.581, citation 52%→62%; R008 mech 0.438→0.555, citation 50%→60%. Conclusion revision: on wolfmax (the only valid overlay A/B), the external overlay is mech-negative vs shipped (0.541 vs 0.605), previously reported flat. Use `python3 -m bench.rescore R<NNN>` after any future scoring change; it verifies citations against the fixture's pinned SHA via `git show`, so a drifted working tree (the wolfmax symlink) does not distort verification.

### Scoring revision 2 (2026-07-07): imprecision vs fabrication

Classifying every failing citation in R010-R012 found ~zero fabricated paths: the "hallucination" metric was measuring path-shortening style (agents write `upgrade-helper.ts:83` for `packages/backend/src/api/crypto/upgrade-helper.ts:83`). Citation verification now resolves shortened paths by unique suffix match (with the cited line in range) and flags them `imprecise` instead of hallucinated; ambiguous basenames (multiple viable files) and fabricated paths remain hallucinations. `_cite_appears` accepts suffix-cited expected files, so citation_hit stops undercounting. Corrected R010 baseline: hallucinated 12.5% (all ambiguity), imprecise 20%, citation hit 71%, mech 0.682.

## Architecture

```
bench/
├── run.py            CLI dispatcher (prep, full, arm, question, report, diff, list, clean, validate, authoring init)
├── config.py         model IDs, timeouts, paths (all BENCH_* env-overridable)
├── arms.py           the 5 arm definitions (default, code_intel_full, code_intel_no_descriptions, code_intel_no_reranker, codegraph)
├── fixtures_io.py    YAML loader + linter (validate_fixture checks every citation file:line exists at the pinned SHA)
├── fixtures/
│   ├── AUTHORING.md  authoring guide
│   ├── smoke.yaml    3-question dev fixture (against this repo)
│   ├── wolfmax.yaml  20 questions pinned to wolfmax HEAD
│   └── django.yaml   20 questions pinned to Django 5.1.4
├── score.py          mech (citation_hit + file + facts) + citation verification + negation-aware forbidden
├── runner.py         spawns `claude --print` per (arm, question); stream-json telemetry, retries, isolation
├── judge.py          tiered judging (haiku gate + sonnet/opus panel); errored judges excluded from median
├── rescore.py        zero-token re-score of a stored round against the pinned SHA (git show)
├── daemon.py         starts the code-intelligence daemon per arm with the right BENCH_DISABLE_* env and per-variant HOME
├── repos.py          repo checkout + index variant cache freshness
├── orchestrator.py   end-to-end cycle (per-arm question runs, scoring, judging, write JSONL outputs)
├── report.py         aggregate + markdown render
├── tests/            53 unit tests covering every module
├── state/            local-only: cloned repos, per-variant indexes (gitignored)
└── results/RNNN/     committed: runs.jsonl, judge.jsonl, scores.json, report.md
```

## Arms

| arm | daemon | index variant | what it tests |
|---|---|---|---|
| `default` | none | none | agent with only Read/Grep/Glob/Bash |
| `code_intel_full` | yes, RERANKER_ENABLED=1 | full | shipped code-intelligence with the cross-encoder reranker live (descriptions on) |
| `code_intel_no_descriptions` | yes, BENCH_DISABLE_DESCRIPTIONS=1 | no_desc | descriptions ablated at write AND query time |
| `code_intel_no_reranker` | yes, reranker off (default) | full (reused) | reranker never constructed; same index as `code_intel_full` |
| `codegraph` | none (codegraph spawns its own) | n/a | the competitor MCP, on the same questions |

Each arm uses the same system-prompt skeleton with one paragraph of arm-specific tool guidance, so we measure tool capability and not prompt engineering.

## Scoring layers

Mech and judge are independent. Both are reported per question; neither is composited into the other.

1. **Mech** (0.0 to 1.0): `0.5 * citation_hit + 0.25 * file_score + 0.25 * fact_score`, then multiplied by a citation-verification factor (1.0 if all cited file:line exist, 0.5 if any hallucinated, 0.0 if all hallucinated), then `-0.25` per forbidden hit (or `0.0` if `forbidden_strict`).
2. **Judge** (0 to 10): each (arm, question) gets scored by Haiku 4.5, Sonnet 4.6, and Opus 4.7. We report the median and the range as a variance proxy. Judges receive the mech context (citation_hit, hallucinated, forbidden_hit) as informational signal but produce their own qualitative score.

## R005 result (2026-05-28)

| arm | n | judge | mech | citation% | tools | tokens | wall |
|---|---:|---:|---:|---:|---:|---:|---:|
| default | 40 | 6.08 ±1.85 | 0.41 | 53% | 5.2 | 164k | 52s |
| code_intel_full | 40 | 6.92 ±2.45 | **0.46** | **60%** | 5.0 | 176k | 40s |
| code_intel_no_descriptions | 40 | 6.92 ±2.25 | 0.43 | 50% | 5.1 | 177k | 37s |
| code_intel_no_reranker | 40 | **7.12** ±2.17 | 0.43 | 50% | 4.6 | 172k | 36s |
| codegraph | 40 | 6.92 ±2.15 | 0.33 | 33% | 7.0 | 219k | 47s |

### Findings

1. **~~Reranker is net-negative.~~ RETRACTED (2026-05-29).** This finding was invalid. At R005 the reranker was never wired into the v4 daemon: `src/session.rs::init_repo_state` constructed the `Retriever` with `reranker = None` (and had since 2026-02-10), so both `code_intel_full` and `code_intel_no_reranker` ran the identical retrieval path. The `BENCH_DISABLE_RERANKER=1` gate on the no_reranker arm never fired because the reranker object did not exist. The +0.20 judge delta (7.12 vs 6.92) is agent run-to-run variance, not a reranker effect: only 3 of 40 final answers were byte-identical between the two arms, and the metrics disagree on direction (judge favours no_reranker, mech 0.46 vs 0.43 and citation% 60 vs 50 favour full). The reranker has since been wired in (off by default, `RERANKER_ENABLED=1` for the full arm); a future round will measure its real effect.
2. **Descriptions help retrieval precision but do not move the judge.** Descriptions take `code_intel_full` from mech 0.43 to 0.46 and citation-hit from 50% to 60%, but the judges score the two arms the same on prose quality.
3. **Codegraph ties code_intel arms on judge but loses on precision and cost.** Judge 6.92 (same as code_intel), but mech 0.33 (worst), citation% 33% (worst), tools 7.0 (40% more than code_intel), tokens 219k (25% more). Polished prose, sloppy citations.
4. **Default trails MCP arms by -0.84 judge** but is competitive on mech (-0.05 to -0.02). The MCP path is real but the gap is not enormous on this question set.
5. **Judge disagreement** ranges from ±1.85 (default) to ±2.45 (code_intel_full). Higher disagreement around the MCP arms suggests judges have stronger opinions about what "good" looks like there.

### Caveats

- Initial run hit Claude Code subscription rate limits during judging (around call 240 in a 5-hour window). Recovered via a resume-aware rejudge script that batches calls and self-exits on 5 consecutive failures. Final judge file has 194 of 200 entries with non-zero scores; the 6 residual zeros are rate-limit casualties whose retries were not reattempted.
- `code_intel_no_descriptions` actually had 92% description coverage in `wolfmax/full` and 93% in `django/full` from prep (the description LLM stagnates on some symbols and the bench prep moves on after 2 minutes of no progress). The ablation comparison is therefore approximate at the edges.
- Codegraph version recorded in the round metadata is wrong (says "not installed"); it was actually 0.9.4. Cosmetic bug in `cmd_report`.

## R006 result (2026-05-29) — first real reranker measurement

After wiring the reranker into the daemon (it was `None` in `session.rs` through R005), R006 ran only `code_intel_full` with `RERANKER_ENABLED=1` against wolfmax + Django (40 questions), comparing against the R005 reranker-off rows. The daemon log confirms the cross-encoder loaded and was active per query.

| arm | judge (non-zero mean) | mech | citation% |
|---|---:|---:|---:|
| R006 `code_intel_full` (reranker **ON**) | 6.55 | 0.418 | 45% |
| R005 `code_intel_full` (off) | 6.92 | 0.462 | 60% |
| R005 `code_intel_no_reranker` (off) | 7.12 | 0.429 | 50% |

The reranker measured **net-negative**: judge -0.4 to -0.6 below both off baselines (after excluding 2 rate-limit judge zeros). The per-question judge split vs R005 no_reranker was 6 better / 20 worse / 12 tie — directional, unlike the symmetric 11/16/13 scatter of the (invalid) R005 full-vs-no_reranker noise. Mech was dead even (11/11 per question), so the reranker isn't changing citation correctness; it's reordering top hits in a way the judges score lower.

Caveat: cross-round, single-run (R006 today vs R005 yesterday, different daemon builds), so agent stochasticity is a confound and the -0.5 judge delta alone sits inside the ±2.2 band — but the 6/20 split is hard to dismiss. A same-round repeated A/B would settle it.

**Decision:** descriptions and the reranker both ship off by default (`DESCRIPTIONS_ENABLED` / `RERANKER_ENABLED`, both `false`). Neither moved the judge; each adds setup cost. Re-enable + re-bench if that changes.

## R007 result (2026-06-13) — post-foundation baseline

First cycle since R006, capturing the ~58 commits landed since (provenance-overlay foundation + evidence-pack quality layer + the `rrf.k` knob). Single arm: `code_intel_shipped` (renamed from `code_intel_no_descriptions`; production defaults = descriptions off, reranker off, the overlay schema present but inert because the bundled producers are stubs). Agent `claude-sonnet-4-6`; judges haiku-4-5 / sonnet-4-6 / **opus-4-8** (Opus bumped from the now-stale 4.7). wolfmax re-pinned `da34bb09`, django re-pinned `2d4add11`. 40/40 runs `end_turn`; 1 judge casualty (wolfmax-symbol-01) excluded from the means below.

| metric | R007 | R005 `no_descriptions` | Δ |
|---|---:|---:|---:|
| judge (casualties excl.) | 7.15 | 7.10 | +0.05 |
| mech | 0.465 | 0.426 | +0.039 |
| citation hit | 52% | 50% | +2pp |
| hallucinated | 30% | 35% | -5pp |

**No regression.** Judge is flat (well inside the ±2 noise band) with a small, consistent precision gain (mech +0.04, +2pp citation, -5pp hallucination) — the expected signature of the evidence-pack layer: better grounding, no prose-quality movement.

**Confounds (so this is a fresh baseline, not a clean A/B of the new code):** wolfmax content re-pinned `b56910e`→`da34bb09` (+85 commits), django `3e5887b`→`2d4add11`, judge Opus 4.7→4.8.

By task type, concept (+0.76 judge, mech 0.30→0.48), impact (+0.67), and multi_hop (+0.38) improved. symbol_lookup judge dropped (-1.14) **but retrieval was not the cause**: every symbol_lookup citation was correct with 0 hallucination (mech 0.67); the agent answered location-only and judges docked the missing rubric descriptions. multi_hop hallucination stays high (88%, ~on par with R005) — agents mis-cite line numbers in multi-file traces.

Says nothing about overlay retrieval quality (producers are stubs → zero external rows). The next external-overlay benchmark arm is `code_intel_external`: it keeps the R007 production defaults and enables explicit external producer execution only. Run it after the TypeScript and Python producer smoke tests import non-zero rows for wolfmax and Django.

### Operational notes from this run

- The default `exclude_patterns` entry `**/bench/state/repos/**` zeroed the django index (django is checked out under `bench/state/repos/`; every file matched the exclude → 0 symbols). Worked around via a `[repos.defaults] exclude_patterns` override in the variant `server.toml`. **Proper fix pending:** drop that entry from the daemon default, or move fixtures outside `bench/state/repos/`. wolfmax dodged it only because it is a symlink to an external path.
- The shallow-clone checkout does not honor the pinned SHA (a `git checkout <sha>` after `git fetch --depth=1` left HEAD unmoved). django was re-pinned to the actually-checked-out `2d4add11` to keep prep a no-op; revisit if an exact pinned SHA is required.

## R008 result (2026-06-14) — first external-overlay A/B

First round with real Tier-1 external producers (Python/TS/Rust/Go) merged from `codex/tier1-external-producers`. Arms: `code_intel_shipped` (no_desc, overlay inert) vs `code_intel_external` (same defaults + `EXTERNAL_INDEX_AUTO=true`, explicit producer execution). 80 runs, all `end_turn`.

**Headline aggregate is misleading** — it folds in an invalid django-external arm (see Caveats). The real signal is per-repo:

| arm / repo | n | judge | mech | citation | halluc | tools | tokens |
|---|---:|---:|---:|---:|---:|---:|---:|
| shipped / wolfmax | 20 | 6.90 | 0.45 | 55% | 35% | 4.2 | 178.6k |
| external / wolfmax | 20 | 6.65 | 0.45 | 60% | 35% | 5.2 | 179.8k |
| shipped / django | 20 | 6.85 | 0.45 | 50% | 45% | 5.3 | 201.1k |
| external / django | 20 | 7.25 | 0.40 | 35% | 25% | 6.0 | 255.3k |

**wolfmax is the only valid overlay test** (2,563 real `typescript_source` rows imported). There the external overlay is **judge -0.25** (inside the ±1.4–1.8 band), **mech flat**, **+5pp citation**, at **~1 extra tool call**. Same signature as descriptions/reranker in R005–R007: a marginal precision bump that does not move the judge. Does not justify enabling external indexing by default on this evidence.

### Caveats (why django-external is invalid)

- **Wrong producer for django.** `detect_producer_for_repo` returns a single producer and checks manifests in fixed priority with `package.json`/`tsconfig.json` first. Django's root has **both** `package.json` (JS tooling) and `pyproject.toml`, so TypeScript matched first and the Python producer never ran: django imported **60 stray-JS overlay rows instead of ~42k Python symbols**. The django-external arm is therefore ≈ the shipped arm; its +0.40 judge / -15pp citation / +54k tokens are run-to-run noise, not an overlay effect. **Fixed** in `detect_producers_for_repo` (auto-mode now runs every producer the repo's manifests indicate, not just the first by priority). **Still pending:** re-prep + re-run the django external arm for the decisive Python-overlay A/B.
- **Judging looked rate-limited but was a parser bug.** 57/80 judge rows first came back all-zero and were misread as a subscription limit. Real cause: `judge.py`'s `_JSON_BLOCK` regex (`\{[^{}]*"score"[^{}]*\}`) cannot parse a reply whose `justification` contains literal braces (e.g. "injects `{ user, session }`") or a ```` ```json ```` fence. Replaced with a string-aware balanced-brace extractor; added regression tests. Recovered the 57 rows with `python3 -m bench.rejudge R008` (resume-aware: re-judges only casualties, persists incrementally, self-exits after N consecutive real failures), then `report R008`.

## Resuming a partially-judged round

If judging stops part-way (real rate limit, or a parser/CLI failure), the casualty rows are all-zero with empty justifications. Re-judge only those without touching the 80 runs or the good rows:

```bash
python3 -m bench.rejudge R008 --dry-run   # list casualties, no API calls
python3 -m bench.rejudge R008             # re-judge casualties, persist incrementally
python3 -m bench.run report R008          # re-aggregate (report reads judge_median from scores.json)
```

## Daemon env contract

Both descriptions and the reranker ship **off by default** in production (neither moved the judge in R005/R006). Each is a real `StandaloneConfig` toggle (default `false`), opted into per arm/variant:

- **`RERANKER_ENABLED=1`** — `StandaloneConfig` loads it and the daemon builds a shared `DeferredReranker` that loads the bge-reranker-v2-m3 model in the background, then `src/session.rs::init_repo_state` passes it to every repo's `Retriever`. The `code_intel_full` arm sets it; `code_intel_no_reranker` leaves it unset so the reranker is never constructed.
- **`DESCRIPTIONS_ENABLED=1`** — gates the index-time description worker (`src/session.rs::init_repo_state`, via `should_spawn_description_worker`). Prep sets it when building the **full** index variant so that variant actually contains LLM descriptions; the **no_desc** variant build leaves it off.

The two bench-only ablation knobs (the literal `"1"` activates) still exist for forcing a clean ablation:

- **`BENCH_DISABLE_DESCRIPTIONS=1`** — `src/storage/tantivy.rs::upsert_symbol` forces the Tantivy `description` field empty AND skips the worker. Prep uses it for the `no_desc` variant so descriptions are absent at both write and query time.
- **`BENCH_DISABLE_RERANKER=1`** — `src/reranker/llamacpp.rs::LlamaCppReranker::rerank` returns a uniform passthrough score. Now redundant for the bench (the no_reranker arm just leaves `RERANKER_ENABLED` unset), but retained in production code.

`session.rs` re-introduces the description worker spawn that the v4 stdio refactor (`4736a0d`) deleted; the worker is now gated off by default and runs only when `DESCRIPTIONS_ENABLED=1`.

## Authoring fixtures

```bash
./bench authoring init <repo>      # scaffold an empty YAML
./bench validate bench/fixtures/<repo>.yaml --repo-root <path>
```

See `bench/fixtures/AUTHORING.md` for the rules.

## Open follow-ups

- Wire `cmd_arm`, `cmd_question`, `cmd_diff`, `cmd_clean` (stubs today).
- Fix `cmd_report` so codegraph version + daemon SHA in the round header come from real run metadata instead of the placeholder `"?"`.
- Investigate why the description worker stagnates around 90-95% on large repos. Either it is a real failure mode (some symbols never get described) or a timing issue with the 2-minute stagnant detection.
- Move fixture checkouts out of `bench/state/repos/` (kills the `**/bench/state/repos/**` exclude-pattern trap AND the ancestor-CLAUDE.md leak into agent runs; note this changes repo hashes and invalidates cached indexes).
- Content-addressed run reuse across rounds: key runs by (arm config, daemon SHA, question, repo SHA, agent model) so an unchanged baseline arm can be reused instead of re-run in A/B rounds.
- Curate a ~15-question iteration fixture from the most discriminative questions in R005-R008; keep the full 40 for release rounds.
- Re-prep and re-run the django external arm (the R008 django overlay used the wrong producer; the fix landed but the decisive Python-overlay A/B never ran).

## Re-running

Indexes are cached in `bench/state/home/{full,no_desc}/.code-intelligence/repos/<hash>/`. Cache freshness is keyed on `(daemon_sha, repo_upstream_sha, variant, schema_version)`. Touching any of those (e.g. by rebuilding the daemon or moving a fixture's pinned SHA) invalidates the cache and triggers a rebuild.

```bash
./bench prep --check          # dry run; print what would be rebuilt
./bench prep                  # build whatever is stale
./bench clean --indexes       # not yet implemented; for now: rm -rf bench/state/home
```

Full prep from cold against wolfmax + Django takes ~1.5-3 hours depending on how many descriptions the worker can generate before stagnating. Subsequent runs that only change daemon code re-index but skip the description backfill if the data dir is preserved (descriptions persist across reindexes for unchanged symbols).

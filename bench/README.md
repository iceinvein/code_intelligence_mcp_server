# Bench

The code-intelligence-mcp benchmark harness. Runs N arms against M repos with K questions each, scores each answer mechanically and via a 3-judge consensus, renders a markdown report.

R005 (2026-05-28) is the first cross-repo cycle: 5 arms × 40 questions across wolfmax and Django.

## Quick start

```bash
make -C bench install        # PyYAML + pytest (anthropic SDK is no longer required)
make -C bench test           # pytest
make -C bench validate-smoke # lint the smoke fixture
./bench full --arms default,code_intel_full --repos wolfmax,django
./bench report R005
```

The `./bench` wrapper is `python3 -m bench.run`. There is no shell-script wrapper at the repo root (it would collide with the `bench/` directory).

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
├── score.py          mech (citation_hit + file + facts) + citation verification + forbidden
├── runner.py         spawns `claude --print` per (arm, question) and parses the JSONL transcript
├── judge.py          multi-judge consensus via 3 `claude --print` calls (haiku + sonnet + opus); median + range
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
- The rejudge script (`/tmp/rejudge_R005_resume.py`) is one-off. Promote it to `bench/rejudge.py` plus a `./bench rejudge <round>` subcommand for repeatable rate-limit recovery.
- Consider raising `MAX_CONSECUTIVE_FAIL_BEFORE_EXIT` from 5 to 10 to ride out brief network blips.

## Re-running

Indexes are cached in `bench/state/home/{full,no_desc}/.code-intelligence/repos/<hash>/`. Cache freshness is keyed on `(daemon_sha, repo_upstream_sha, variant, schema_version)`. Touching any of those (e.g. by rebuilding the daemon or moving a fixture's pinned SHA) invalidates the cache and triggers a rebuild.

```bash
./bench prep --check          # dry run; print what would be rebuilt
./bench prep                  # build whatever is stale
./bench clean --indexes       # not yet implemented; for now: rm -rf bench/state/home
```

Full prep from cold against wolfmax + Django takes ~1.5-3 hours depending on how many descriptions the worker can generate before stagnating. Subsequent runs that only change daemon code re-index but skip the description backfill if the data dir is preserved (descriptions persist across reindexes for unchanged symbols).

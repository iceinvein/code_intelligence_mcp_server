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
| `code_intel_full` | yes | full | shipped code-intelligence (descriptions on, reranker on) |
| `code_intel_no_descriptions` | yes, BENCH_DISABLE_DESCRIPTIONS=1 | no_desc | descriptions ablated at write AND query time |
| `code_intel_no_reranker` | yes, BENCH_DISABLE_RERANKER=1 | full (reused) | reranker ablated at query time |
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

1. **Reranker is net-negative.** `code_intel_no_reranker` is the best arm overall (judge 7.12, +0.20 over `code_intel_full`). The reranker actively hurts judge scores while consuming GPU memory.
2. **Descriptions help retrieval precision but do not move the judge.** Descriptions take `code_intel_full` from mech 0.43 to 0.46 and citation-hit from 50% to 60%, but the judges score the two arms the same on prose quality.
3. **Codegraph ties code_intel arms on judge but loses on precision and cost.** Judge 6.92 (same as code_intel), but mech 0.33 (worst), citation% 33% (worst), tools 7.0 (40% more than code_intel), tokens 219k (25% more). Polished prose, sloppy citations.
4. **Default trails MCP arms by -0.84 judge** but is competitive on mech (-0.05 to -0.02). The MCP path is real but the gap is not enormous on this question set.
5. **Judge disagreement** ranges from ±1.85 (default) to ±2.45 (code_intel_full). Higher disagreement around the MCP arms suggests judges have stronger opinions about what "good" looks like there.

### Caveats

- Initial run hit Claude Code subscription rate limits during judging (around call 240 in a 5-hour window). Recovered via a resume-aware rejudge script that batches calls and self-exits on 5 consecutive failures. Final judge file has 194 of 200 entries with non-zero scores; the 6 residual zeros are rate-limit casualties whose retries were not reattempted.
- `code_intel_no_descriptions` actually had 92% description coverage in `wolfmax/full` and 93% in `django/full` from prep (the description LLM stagnates on some symbols and the bench prep moves on after 2 minutes of no progress). The ablation comparison is therefore approximate at the edges.
- Codegraph version recorded in the round metadata is wrong (says "not installed"); it was actually 0.9.4. Cosmetic bug in `cmd_report`.

## Daemon env contract

The bench adds two production-code env vars, both bench-only knobs (the literal value `"1"` activates; anything else is treated as off):

- **`BENCH_DISABLE_DESCRIPTIONS=1`** — `src/storage/tantivy.rs::upsert_symbol` forces the Tantivy `description` field to empty; `src/session.rs::init_repo_state` skips spawning the description worker.
- **`BENCH_DISABLE_RERANKER=1`** — `src/reranker/llamacpp.rs::LlamaCppReranker::rerank` returns uniform `FALLBACK_SCORE` for every document; the upstream blend formula `hit.score * 0.8 + reranker_score * 0.2 * 10` is order-preserving under constant reranker scores, so the agent observes the pre-rerank ordering.

`session.rs` also re-introduces the description worker spawn that the v4 stdio refactor (`4736a0d`) deleted; the worker runs by default in production and is gated only by the bench env.

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

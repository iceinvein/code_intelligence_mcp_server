# Benchmark R005 + R006: Code Intelligence vs Baselines

Full statistics from the `bench/` judge harness, combining round **R005** (2026-05-28, all five arms) and round **R006** (2026-05-29, the first real reranker measurement). Written up for reference and blog use.

## What this measures

Each arm is the **same Claude Code agent** answering the **same 40 questions** (20 against [wolfmax], 20 against Django 5.1.4), differing only in which tools it has and one paragraph of tool guidance. So we measure tool capability, not prompt engineering. Questions span symbol lookup, architecture, concepts, impact analysis, multi-hop tracing, and negative cases (things that don't exist).

Two independent scores per answer:

- **Mech (0.0-1.0):** mechanical accuracy. `0.5 * citation_hit + 0.25 * file_score + 0.25 * fact_score`, multiplied by a citation-verification factor (1.0 if every cited `file:line` exists, 0.5 if any is hallucinated, 0.0 if all are), minus `0.25` per forbidden-pattern hit. This is the objective "did it cite real, correct locations" score.
- **Judge (0-10):** three LLM judges (Haiku 4.5, Sonnet 4.6, Opus 4.7) independently score each answer; we take the median. This is the subjective "is this a good answer" score.

### A note on judge cleaning (important for reading the tables)

R005's judging hit Claude subscription rate limits, so some judge calls returned an empty response. The raw median treats a failed call as a `0`, which wrongly drags a question's median down (median of `[9, 0, 0]` = 0 even though the one judge that answered gave a 9).

Every table below reports **judge (clean)**: the median over only the judges whose justification is non-empty (a failed call is excluded, not counted as zero). We also report **judge (raw)** (failed calls = 0, as originally recorded), **clean_n** (questions with at least one valid judge), and **failed_q** (questions where all three judges failed). For R005 every question had at least one surviving judge (failed_q = 0); R006 lost one question entirely (wolfmax-symbol-04).

Where a question kept only one surviving judge, "median" is that single score, which is weaker than a true 3-judge median; raw-vs-clean divergence flags those.

## Caveats (read before quoting)

1. **The reranker was dead code in R005.** Through R005 the daemon built its retriever with `reranker = None`, so *every* R005 arm ran the identical retrieval path regardless of its label. The original R005 "reranker net-negative" claim was therefore measuring nothing. The reranker was wired in for R006; that is its first real measurement.
2. **R006 is cross-round, single-run.** R006 ran only `code_intel_full` with the reranker genuinely on, today, on a newer daemon build. Comparing it to R005 mixes run-to-run agent stochasticity (the agent is non-deterministic) with the reranker effect. Treat the R006 delta as directional, not definitive. A same-round A/B with repeats would be needed to fully isolate it.
3. **n = 40 per arm, single round.** Judge disagreement runs roughly ±2 points; sub-±0.5 differences between arms are inside the noise.

## Headline comparison

The three configurations that matter for "should I use this":

| | tool surface | judge (clean) | mech | citation accuracy | tokens | tool calls | wall |
|---|---|---:|---:|---:|---:|---:|---:|
| **default** | Read / Grep / Glob / Bash only | 6.30 | 0.409 | 52% | 164k | 5.2 | 52.4s |
| **code-intelligence** (shipped: descriptions + reranker off) | + 11 MCP tools | **7.12** | **0.426** | 50% | 177k | 5.1 | **37.3s** |
| **codegraph** (competitor MCP) | + codegraph tools | **7.15** | 0.331 | 32% | 219k | 7.0 | 46.9s |

> "code-intelligence" here is the R005 `code_intel_no_descriptions` arm. Because the reranker was dead in R005, that arm = descriptions off + reranker off = exactly the configuration that now ships by default.

**Reading it:**

- **vs a plain grep/read agent:** code-intelligence scores **+0.82 judge** (7.12 vs 6.30) and finishes **~29% faster** (37s vs 52s) for ~8% more tokens. The default agent's mean is dragged down by hard failures: it scored a clean 0 on four questions (architecture/concept/negative cases where grep finds nothing useful), whereas code-intelligence never bottomed out.
- **vs codegraph:** a near-tie on judge (7.12 vs 7.15, well inside noise) but code-intelligence is materially more **citation-accurate** (mech 0.426 vs 0.331; citation-hit 50% vs 32%) while using **~19% fewer tokens** (177k vs 219k) and **fewer tool calls** (5.1 vs 7.0). codegraph writes prose the judges like but cites less reliably and costs more. (Two of codegraph's questions scored a clean 0 on citation-verification failures: `django-symbol-01`, `django-concept-03`.)

The one-line story: **code-intelligence matches a polished competitor on answer quality while being cheaper and far more citation-accurate, and clearly beats a raw grep/read agent.**

## Full per-arm aggregates

All five R005 arms plus the R006 reranker-on arm.

| round | arm | n | judge (raw) | judge (clean) | clean_n | failed_q | mech | citation% | halluc% | forbidden | tool calls | in-tokens | out-tokens | wall |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| R005 | default | 40 | 6.08 | 6.30 | 40 | 0 | 0.409 | 52% | 32% | 5 | 5.2 | 164k | 1257 | 52.4s |
| R005 | code-intelligence (no_descriptions = shipped) | 40 | 6.92 | 7.12 | 40 | 0 | 0.426 | 50% | 35% | 6 | 5.1 | 177k | 1692 | 37.3s |
| R005 | code_intel_no_reranker (descriptions ON) | 40 | 7.12 | 7.12 | 40 | 0 | 0.429 | 50% | 30% | 5 | 4.6 | 172k | 1631 | 36.3s |
| R005 | code_intel_full (descriptions ON, reranker dead) | 40 | 6.92 | 6.95 | 40 | 0 | 0.462 | 60% | 35% | 5 | 5.0 | 176k | 1684 | 39.7s |
| R005 | codegraph | 40 | 6.92 | 7.15 | 40 | 0 | 0.331 | 32% | 22% | 5 | 7.0 | 219k | 1629 | 46.9s |
| R006 | code_intel_full (reranker **ON**, real) | 40 | 6.22 | 6.46 | 39 | 1 | 0.418 | 45% | 30% | 5 | 4.4 | 185k | 1587 | 35.5s |

Columns: **citation%** = fraction of answers whose cited location matched the canonical answer; **halluc%** = fraction citing a non-existent `file:line`; **forbidden** = count of answers hitting a forbidden pattern; tokens/tools/wall are per-question means.

## Per-repo split

Behaviour differs by codebase (wolfmax is a TypeScript/Elysia app; Django is large mature Python).

| round | arm | repo | n | judge (clean) | mech | citation% |
|---|---|---|---:|---:|---:|---:|
| R005 | default | wolfmax | 20 | 6.45 | 0.412 | 70% |
| R005 | default | django | 20 | 6.15 | 0.406 | 35% |
| R005 | code-intelligence (shipped) | wolfmax | 20 | 6.90 | 0.484 | 60% |
| R005 | code-intelligence (shipped) | django | 20 | 7.35 | 0.367 | 40% |
| R005 | code_intel_no_reranker | wolfmax | 20 | 6.95 | 0.443 | 45% |
| R005 | code_intel_no_reranker | django | 20 | 7.30 | 0.416 | 55% |
| R005 | code_intel_full | wolfmax | 20 | 6.55 | 0.451 | 70% |
| R005 | code_intel_full | django | 20 | 7.35 | 0.473 | 50% |
| R005 | codegraph | wolfmax | 20 | 7.50 | 0.382 | 45% |
| R005 | codegraph | django | 20 | 6.80 | 0.280 | 20% |
| R006 | code_intel_full (reranker ON) | wolfmax | 20 | 6.53 | 0.433 | 50% |
| R006 | code_intel_full (reranker ON) | django | 20 | 6.40 | 0.402 | 40% |

Notable: codegraph's citation accuracy on Django collapses to 20% (mech 0.28) while still scoring 6.80 on judge: maximal prose-vs-accuracy gap.

## Judge agreement (per-model means, clean)

How each judge model scored each arm. Opus is consistently the most generous; the three agree on ordering.

| round | arm | Haiku 4.5 | Sonnet 4.6 | Opus 4.7 |
|---|---|---:|---:|---:|
| R005 | default | 5.67 | 6.15 | 6.97 |
| R005 | code-intelligence (shipped) | 6.83 | 6.77 | 8.05 |
| R005 | code_intel_no_reranker | 6.40 | 6.85 | 7.95 |
| R005 | code_intel_full | 6.36 | 6.80 | 7.87 |
| R005 | codegraph | 6.56 | 6.64 | 7.85 |
| R006 | code_intel_full (reranker ON) | 6.13 | 6.26 | 7.32 |

All three judges rank reranker-ON (R006) below the lean shipped config, and all three rank the lean config above default.

## The reranker ablation (R006)

R006 is the first round where the reranker actually ran (daemon log confirms the cross-encoder loaded and scored per query). Turning it on, vs the lean shipped config:

- **Judge:** 6.46 vs 7.12 (**-0.66**, clean). All three judge models agree it's lower.
- **Mech:** 0.418 vs 0.426 (flat). The reranker is not changing citation correctness.
- **Per-question (reranker-ON vs lean-CI):** 6 better, 18 worse, 15 tie (of 39 comparable). Directional, unlike the symmetric scatter that pure noise produces.

Interpretation: the cross-encoder reorders the top hits in a way the judges consistently score lower, without improving citation accuracy. It surfaces slightly-less-relevant results to the top. **Shipped off.**

## The descriptions ablation (R005)

Descriptions-on (`code_intel_no_reranker`, descriptions ON) vs descriptions-off (`code_intel_no_descriptions`, the shipped config), both with the reranker dead, so this isolates descriptions:

- **Judge:** 7.12 vs 7.12 (**identical**).
- **Mech:** 0.429 vs 0.426 (flat).
- **Per-question:** descriptions-on better on 11, worse on 7, tie on 22. Inside noise given the identical means.

`code_intel_full` (also descriptions-on) reached the best raw mech (0.462) and citation% (60%), suggesting descriptions help retrieval *precision* at the margin, but it did not translate into a higher judge score. Given descriptions cost a multi-hour index-time backfill for no judge gain, **shipped off.**

## Per-question judge (clean) matrix

Every question, every arm. `-` = all judges failed for that cell.

| question | task | default | code-intelligence (shipped) | no_reranker | full | codegraph | R006 rerank-ON |
|---|---|---:|---:|---:|---:|---:|---:|
| django-arch-01 | architectural | 8 | 7 | 7 | 7 | 7 | 5 |
| django-arch-02 | architectural | 6 | 9 | 8 | 7 | 6 | 7 |
| django-arch-03 | architectural | 0 | 6 | 6 | 7 | 6 | 5 |
| django-concept-01 | concept | 0 | 6 | 5 | 7 | 7 | 6 |
| django-concept-02 | concept | 8 | 7 | 7 | 6 | 9 | 6 |
| django-concept-03 | concept | 8 | 8 | 7 | 7 | 0 | 7 |
| django-concept-04 | concept | 7 | 7 | 7 | 9 | 8 | 7 |
| django-impact-01 | impact | 6 | 8 | 5 | 7 | 7 | 4 |
| django-impact-02 | impact | 9 | 6 | 6 | 7 | 9 | 5 |
| django-impact-03 | impact | 7 | 5 | 6 | 9 | 6 | 7 |
| django-multi-hop-01 | multi_hop | 9 | 7 | 7 | 3 | 9 | 4 |
| django-multi-hop-02 | multi_hop | 8 | 8 | 9 | 10 | 8 | 8 |
| django-multi-hop-03 | multi_hop | 9 | 8 | 7 | 7 | 8 | 7 |
| django-multi-hop-04 | multi_hop | 0 | 5 | 7 | 6 | 7 | 6 |
| django-negative-01 | negative | 9 | 8 | 9 | 7 | 6 | 6 |
| django-negative-02 | negative | 0 | 9 | 9 | 9 | 9 | 9 |
| django-symbol-01 | symbol_lookup | 10 | 10 | 10 | 10 | 0 | 9 |
| django-symbol-02 | symbol_lookup | 7 | 7 | 8 | 7 | 9 | 7 |
| django-symbol-03 | symbol_lookup | 4 | 7 | 7 | 6 | 6 | 5 |
| django-symbol-04 | symbol_lookup | 8 | 9 | 9 | 9 | 9 | 8 |
| wolfmax-arch-01 | architectural | 6 | 8 | 9 | 6 | 8 | 7 |
| wolfmax-arch-02 | architectural | 8 | 9 | 9 | 7 | 8 | 9 |
| wolfmax-arch-03 | architectural | 8 | 5 | 6 | 5 | 8 | 8 |
| wolfmax-concept-01 | concept | 9 | 9 | 9 | 7 | 10 | 9 |
| wolfmax-concept-02 | concept | 7 | 9 | 9 | 8 | 10 | 9 |
| wolfmax-concept-03 | concept | 3 | 3 | 3 | 3 | 8 | 3 |
| wolfmax-concept-04 | concept | 3 | 4 | 4 | 4 | 3 | 5 |
| wolfmax-impact-01 | impact | 8 | 7 | 8 | 7 | 7 | 7 |
| wolfmax-impact-02 | impact | 6 | 5 | 5 | 5 | 5 | 5 |
| wolfmax-impact-03 | impact | 8 | 8 | 9 | 9 | 8 | 8 |
| wolfmax-multi-hop-01 | multi_hop | 9 | 9 | 9 | 7 | 7 | 6 |
| wolfmax-multi-hop-02 | multi_hop | 9 | 8 | 8 | 7 | 9 | 8 |
| wolfmax-multi-hop-03 | multi_hop | 6 | 5 | 5 | 7 | 7 | 3 |
| wolfmax-multi-hop-04 | multi_hop | 1 | 6 | 7 | 8 | 6 | 8 |
| wolfmax-negative-01 | negative | 4 | 6 | 6 | 6 | 8 | 6 |
| wolfmax-negative-02 | negative | 10 | 9 | 9 | 9 | 9 | 9 |
| wolfmax-symbol-01 | symbol_lookup | 9 | 8 | 7 | 9 | 9 | 3 |
| wolfmax-symbol-02 | symbol_lookup | 5 | 8 | 4 | 4 | 8 | 2 |
| wolfmax-symbol-03 | symbol_lookup | 5 | 6 | 6 | 7 | 6 | 9 |
| wolfmax-symbol-04 | symbol_lookup | 5 | 6 | 7 | 6 | 6 | - |

## Head-to-head (per-question judge clean)

| comparison | A better | B better | tie | comparable |
|---|---:|---:|---:|---:|
| code-intelligence (A) vs default (B) | 17 | 14 | 9 | 40 |
| code-intelligence (A) vs codegraph (B) | 9 | 16 | 15 | 40 |
| reranker-ON R006 (A) vs code-intelligence (B) | 6 | 18 | 15 | 39 |
| descriptions-ON (A) vs descriptions-off/shipped (B) | 11 | 7 | 22 | 40 |

The default head-to-head (17/14/9) looks closer than the means (7.12 vs 6.30) because default wins or ties most questions but loses *catastrophically* (clean 0) on a handful it can't handle at all; those drag its mean down. Code-intelligence's value is the floor it puts under the worst cases, not edging out grep on the easy ones.

## Conclusions

1. **Code-intelligence (lean) beats a raw grep/read agent** on quality (+0.82 judge) and speed (~29% faster), mainly by not failing hard on architecture/concept/negative questions.
2. **It ties the codegraph competitor on judge** while being meaningfully more citation-accurate and ~19% cheaper in tokens.
3. **The reranker is net-negative** (-0.66 judge, R006) and ships off.
4. **Descriptions are judge-neutral** (identical means) for a large index-time cost and ship off.
5. So the shipped configuration is the lean one: hybrid BM25 + vector retrieval with RRF and structural ranking, no cross-encoder reranker, no LLM descriptions.

## Reproducing

```bash
# R005 (all five arms) and R006 (reranker on) raw data live next to this file:
bench/results/R005/{runs,judge}.jsonl  bench/results/R005/scores.json
bench/results/R006/{runs,judge}.jsonl  bench/results/R006/scores.json

# Re-run the lean code-intelligence arm + baselines:
./bench full --arms default,code_intel_no_descriptions,codegraph --repos wolfmax,django

# Re-measure the reranker (opt-in):
./bench full --arms code_intel_full --repos wolfmax,django   # arm sets RERANKER_ENABLED=1
```

See `bench/README.md` for the harness architecture and the R005/R006 round notes.

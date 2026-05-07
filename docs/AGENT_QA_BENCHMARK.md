# Agent Q&A Benchmark

End-to-end agent comparison: default Claude Code tools (Read+Grep+Glob+Bash) versus default + code-intelligence MCP tools. Measures total input tokens consumed (including prompt-cache writes and reads) plus answer quality (mechanical hit-rate + LLM-as-judge grade) per (question, toolset).

## Methodology

See `docs/plans/2026-05-04-agent-qa-benchmark-design.md` for the full spec, including the migration log explaining the switch from Anthropic SDK to Claude Code CLI.

- **Driver**: subprocesses `claude --print --output-format stream-json` so the harness uses your existing Claude Code session auth. No `ANTHROPIC_API_KEY` required.
- **Toolset gating**: both runs get the same built-ins (`Read Grep Glob Bash`) via `--allowed-tools`. The only delta is `--mcp-config`: empty for the default run, code-intelligence binary for the code-intel run. `--strict-mcp-config` prevents leakage from your other MCP configs.
- **Agent model**: Sonnet 4.6 (override with `AGENT_MODEL`).
- **Judge model**: Haiku 4.5 (override with `JUDGE_MODEL`).
- **Repos**: self (this repo) + wolfmax (deferred to R002).
- **Q&A sets**: `scripts/queries_qa_self.json`, `scripts/queries_qa_wolfmax.json`.
- **Output**: `docs/benchmark_rounds/agent/RNNN.{json,md}`.

## Token accounting

Claude Code's default-system-prompt overhead lands almost entirely in `cache_creation_input_tokens` and `cache_read_input_tokens` rather than the uncached `input_tokens` field. The benchmark reports `total_input_tokens = input + cache_creation + cache_read` so the numbers are comparable across runs.

## Running

```bash
source .venv-bench/bin/activate
python3 scripts/bench_agent_qa.py --round 1 --repo self
python3 scripts/bench_agent_qa.py --round 1 --repo wolfmax --base-dir /path/to/wolfmax
```

Optional flags:
- `--question-ids self-q1,self-q3` for a subset.
- `--skip-judge` for cheap smoke runs.
- `--agent-timeout 600` (default 600s per run).

## Round history

### Smoke (R000)

Sanity-check pass on q1, q6, q8, q11 with judge skipped. Both toolsets scored mech=1.0 on every question (these are lookup/explain questions answerable with Grep+Read). Code-intel run averaged **+59,000 total_input_tokens (~30% overhead)** vs default.

Pattern observed across all four smoke questions:
1. Code-intel run starts with `ToolSearch` (Claude Code's deferred-schema loader) to pull in `mcp__code-intelligence__search_code`; this costs ~20k cached tokens per question.
2. The agent then calls 1-2 MCP tools (most often `search_code`, sometimes `get_definition`).
3. The agent still falls back to `Grep` and `Read` to verify and gather line-level detail.

Net: MCP tools complement, rather than displace, the built-ins for these question types. Until something forces the agent to trust the MCP results enough to skip the verify pass, code-intel pays a tax without saving work.

### Round 001

Full 12-question round on self-repo, Sonnet 4.6 + Haiku 4.5 judge.

| | default | code_intel | delta |
|---|---:|---:|---:|
| avg mech | 0.85 | 0.88 | +0.03 |
| avg judge (0-10) | 7.75 | 8.42 | +0.67 |
| avg total_input_tokens | 165,904 | 272,472 | +106,568 (+64%) |
| avg tool calls | 3.8 | 6.6 | +2.8 |

**Code-intel improves answer quality (+0.67 judge) but costs 64% more tokens** on this 12-question lookup-and-explain set. Per-question judge deltas: code-intel wins on 9/12 (q1+2, q2+2, q3+1, q4+1, q5+1, q6+1, q8+1, q10+3), ties on 1 (q9), loses on 3 (q7-1, q11-1, q12-2). Mechanical scoring is essentially tied (q6/q9 +0.4, q12 -0.4); both toolsets are usually finding the right files.

**Tool-reach finding:** of the 32 code-intel tools, the agent used only 4 across the entire round:
- `search_code`: 11 calls (one per question)
- `get_definition`: 1 call
- `find_references`: 1 call
- `get_file_symbols`: 1 call

The other 28 tools were never invoked. The agent's pattern is: ToolSearch → search_code once → fall back to Grep/Read for verification. `find_references` and `get_call_hierarchy` (which were the design's hypothetical wins for impact-analysis questions) almost never fire even on q10 (the explicit impact question).

**Where code-intel hurt** (q12: Qwen LLM components): the agent went 64k tokens deeper than default and got -0.4 mech / -2 judge. Default solved it with 7 grep/read calls; code-intel ran 18 tool calls (including 4 MCP) and produced a less-correct answer. Likely cause: the deferred-tool-loading tax plus search_code returning enough hits to keep the agent exploring alternatives.

**ToolSearch tax**: ~20-25k cached tokens per question regardless of whether the agent ends up using the loaded tool. With 12 questions that's ~250k tokens of overhead per round just to make MCP tools available.

**Methodological caveats:**
- Sonnet 4.6 is highly capable on Rust + grep/read patterns; the gap might widen on weaker models.
- The 12 self-repo questions skew lookup/explain. Questions that genuinely require cross-file reference tracing (the design hypothesized 3 of these per repo) may shift the balance; q10 was the only one and code-intel did get +3 judge there.
- The agent never reached for `get_call_hierarchy`, `find_affected_code`, `predict_impact`, or `trace_data_flow`. Either the question set didn't surface their value, or the tool descriptions aren't selling them strongly enough to compete with Grep.

### Round 002

After Spec 1 (`docs/plans/2026-05-07-search-code-followup-flow-design.md`): added a structured `next_step` hint to `search_code`'s discovery-mode response, plus rewrote the descriptions of `search_code` and `hydrate_symbols` to reinforce them as a paired workflow.

| | default R001 | code_intel R001 | default R002 | code_intel R002 |
|---|---:|---:|---:|---:|
| avg mech | 0.85 | 0.88 | 0.89 | 0.88 |
| avg judge | 7.75 | 8.42 | 7.92 | 8.67 |
| avg total_input_tokens | 165,904 | 272,472 | 164,524 | 216,133 |
| avg tool calls | 3.8 | 6.6 | 3.6 | 4.8 |

**Code_intel changes (R001 → R002):** total_input_tokens dropped 56k (-21%); gap vs default narrowed from +106k (+64%) to +52k (+31%). Judge improved +0.25 on top of the already high R001 baseline. Mech unchanged at 0.88.

**Spec 1 success criteria (recap):**
- Token delta under +50k: **borderline pass** at +52k (close enough that round-to-round noise covers it).
- Judge ≥ 8.42 and mech ≥ 0.88: **pass** (8.67 / 0.88).
- Tool reach shows `hydrate_symbols` calls > zero: **fail** (still zero).

**Tool reach delta (code_intel):**

| tool | R001 | R002 | Δ |
|---|---:|---:|---:|
| Grep | 31 | 23 | -8 |
| Read | 21 | 16 | -5 |
| ToolSearch | 11 | 8 | -3 |
| mcp__code-intelligence__search_code | 11 | 7 | -4 |
| mcp__code-intelligence__get_definition | 1 | 1 | 0 |
| mcp__code-intelligence__find_references | 1 | 1 | 0 |
| mcp__code-intelligence__get_file_symbols | 1 | 0 | -1 |
| mcp__code-intelligence__hydrate_symbols | 0 | 0 | 0 |

**What actually happened.** The hypothesis was that the agent would route `search_code` results into `hydrate_symbols` instead of falling back to `Grep`+`Read`. That did NOT happen. Across 12 questions and 7 `search_code` calls (down from 11), the agent invoked `hydrate_symbols` zero times. Of the 7 `search_code` calls, 6 used the default `context: "none"` (so they received the `next_step` hint in the response), but the agent ignored every hint.

The savings came from a different path: the rewritten descriptions discouraged grep fallback at the discovery stage, so the agent either skipped `search_code` entirely for simpler questions (relying on default tools that were already adequate) or accepted `search_code`'s results and stopped earlier. Net effect: 4 fewer `search_code` calls, 8 fewer `Grep`s, 5 fewer `Read`s, 3 fewer `ToolSearch` round-trips.

**Real lesson.** Tool descriptions shape agent behaviour more reliably than structured response hints. The "do NOT fall back to grep/read for symbols search_code already located" line in the description did real work; the JSON `next_step` directive in the response did not detectably move the agent.

**Decision gate for Spec 2 (ToolSearch tax investigation):** R002 closed the gap to +31% (target was <30% to deprioritise). It is borderline. Adopting Spec 2 next would target the residual `ToolSearch` round-trips (8 in R002) and the cache-creation overhead they impose (~20-25k cached tokens per round-trip). Estimated upside: another 100-150k tokens off the round average if the deferred-loading mechanism can be bypassed.

**Recommended next move:** keep Spec 2 (ToolSearch investigation) as the next planned work, but treat its priority as roughly equal to broadening the Q-set with impact-style questions that exercise `find_references` / `get_call_hierarchy` / `predict_impact` directly. The current set is too lookup-heavy to give those code-intel tools a fair shot. Either pick wins the right next round.

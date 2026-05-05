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
1. Code-intel run starts with `ToolSearch` (Claude Code's deferred-schema loader) to pull in `mcp__code-intelligence__search_code` — costs ~20k cached tokens per question.
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
- The 12 self-repo questions skew lookup/explain. Questions that genuinely require cross-file reference tracing (the design hypothesized 3 of these per repo) may shift the balance — q10 was the only one and code-intel did get +3 judge there.
- The agent never reached for `get_call_hierarchy`, `find_affected_code`, `predict_impact`, or `trace_data_flow`. Either the question set didn't surface their value, or the tool descriptions aren't selling them strongly enough to compete with Grep.

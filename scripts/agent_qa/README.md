# Agent Q&A Benchmark

Python harness that compares default Claude Code tools (Read/Grep/Glob/Bash) versus
code-intelligence MCP tools on hand-authored codebase questions. See the design doc at
`docs/plans/2026-05-04-agent-qa-benchmark-design.md`.

## Setup

The harness drives Claude Code's CLI (`claude --print`), so it reuses your
existing Claude Code session auth. No `ANTHROPIC_API_KEY` needed.

```bash
python3 -m venv .venv-bench
source .venv-bench/bin/activate
pip install -r scripts/requirements-bench.txt
cargo build --release   # the local code-intelligence MCP server
```

## Run

```bash
python3 scripts/bench_agent_qa.py --round 1 --repo self
python3 scripts/bench_agent_qa.py --round 1 --repo wolfmax --base-dir /path/to/wolfmax
```

## Test

```bash
python3 -m pytest scripts/agent_qa -v
```

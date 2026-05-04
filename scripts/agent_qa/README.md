# Agent Q&A Benchmark

Python harness that compares default Claude Code tools (Read/Grep/Glob/Bash) versus
code-intelligence MCP tools on hand-authored codebase questions. See the design doc at
`docs/plans/2026-05-04-agent-qa-benchmark-design.md`.

## Setup

```bash
python3 -m venv .venv-bench
source .venv-bench/bin/activate
pip install -r scripts/requirements-bench.txt
export ANTHROPIC_API_KEY=...
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

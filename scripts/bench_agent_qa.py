#!/usr/bin/env python3
"""Run the agent Q&A benchmark for one round.

Drives Claude Code's CLI (`claude --print`) twice per question (default tools
vs default + code-intelligence MCP), scores each answer mechanically and via
LLM-as-judge, and writes RNNN.{json,md} into docs/benchmark_rounds/agent/.

Usage:
    python3 scripts/bench_agent_qa.py --round 1 --repo self
    python3 scripts/bench_agent_qa.py --round 1 --repo wolfmax \\
        --base-dir /path/to/wolfmax \\
        --queries scripts/queries_qa_wolfmax.json

Env:
    CLAUDE_BINARY  override the `claude` executable (default: `claude`)
    AGENT_MODEL    override agent model (default: claude-sonnet-4-6)
    JUDGE_MODEL    override judge model (default: claude-haiku-4-5-20251001)
"""
from __future__ import annotations

import argparse
import json
import random
import sys
import time
from pathlib import Path
from typing import Any, Dict, List

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT))

from scripts.agent_qa.qa_schema import load_qa_set
from scripts.agent_qa.claude_cli import (
    ClaudeRunOptions,
    run_with_tools,
    run_one_shot,
)
from scripts.agent_qa.scoring import mech_score
from scripts.agent_qa.judge import judge_pair
from scripts.agent_qa.report import ScoredRun, aggregate_round, render_markdown


DEFAULT_BINARY = REPO_ROOT / "target" / "release" / "code-intelligence-mcp-server"
DEFAULT_QUERIES = {
    "self": REPO_ROOT / "scripts" / "queries_qa_self.json",
    "wolfmax": REPO_ROOT / "scripts" / "queries_qa_wolfmax.json",
}
RESULTS_DIR = REPO_ROOT / "docs" / "benchmark_rounds" / "agent"

DEFAULT_BUILTIN_TOOLS = ["Read", "Grep", "Glob", "Bash"]

AGENT_SYSTEM_PROMPT = (
    "You are an investigation agent answering a single question about a codebase. "
    "Use the provided tools to find evidence in the code. Cite specific file paths "
    "(and line numbers when possible) in your final answer. Stop searching once you "
    "have enough to answer. Produce a final answer as a normal text message (no tool "
    "calls) when ready. Be concise: a correct answer that names the right file and "
    "symbol with a one-paragraph explanation is better than a long survey."
)


def _build_default_mcp_config() -> Dict[str, Any]:
    return {"mcpServers": {}}


def _build_code_intel_mcp_config(binary: Path, base_dir: Path) -> Dict[str, Any]:
    return {
        "mcpServers": {
            "code-intelligence": {
                "command": str(binary),
                "args": [],
                "env": {"BASE_DIR": str(base_dir)},
            }
        }
    }


def _allowed_tools_for(toolset: str) -> List[str]:
    # We restrict built-in tools to the same 4 in both runs. MCP tools are
    # gated by --mcp-config (with --strict-mcp-config), so the only delta
    # between toolsets is whether the code-intelligence MCP server is wired up.
    return list(DEFAULT_BUILTIN_TOOLS)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--round", type=int, required=True)
    parser.add_argument("--repo", required=True, choices=["self", "wolfmax", "custom"])
    parser.add_argument("--base-dir", type=Path, default=None)
    parser.add_argument("--queries", type=Path, default=None)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY,
                        help="path to the code-intelligence-mcp-server binary")
    parser.add_argument("--output-dir", type=Path, default=RESULTS_DIR)
    parser.add_argument("--question-ids", default=None, help="comma-separated subset")
    parser.add_argument("--skip-judge", action="store_true")
    parser.add_argument("--agent-timeout", type=int, default=600,
                        help="per-run timeout in seconds (default 600)")
    args = parser.parse_args()

    base_dir = (args.base_dir or REPO_ROOT).resolve()
    queries_path = args.queries or DEFAULT_QUERIES.get(args.repo)
    if queries_path is None or not Path(queries_path).is_file():
        sys.exit(f"queries file not found: {queries_path}")
    if not args.binary.is_file():
        sys.exit(f"MCP binary not found: {args.binary}. Run `cargo build --release` first.")

    import os as _os
    agent_model = _os.environ.get("AGENT_MODEL", "claude-sonnet-4-6")
    judge_model = _os.environ.get("JUDGE_MODEL", "claude-haiku-4-5-20251001")

    qa_entries = load_qa_set(Path(queries_path))
    if args.question_ids:
        wanted = {s.strip() for s in args.question_ids.split(",")}
        qa_entries = [e for e in qa_entries if e.id in wanted]

    args.output_dir.mkdir(parents=True, exist_ok=True)

    raw_runs: List[dict] = []
    scored: List[ScoredRun] = []

    toolset_configs = {
        "default": _build_default_mcp_config(),
        "code_intel": _build_code_intel_mcp_config(args.binary, base_dir),
    }

    print(f"Bench round {args.round} on {args.repo} ({base_dir})", file=sys.stderr)
    print(f"Agent: {agent_model}  Judge: {judge_model}", file=sys.stderr)

    for entry in qa_entries:
        print(f"\n=== {entry.id} ===", file=sys.stderr)
        per_q_records: Dict[str, dict] = {}
        for toolset_name, mcp_cfg in toolset_configs.items():
            print(f"  running {toolset_name}...", file=sys.stderr, end=" ", flush=True)
            t0 = time.time()
            opts = ClaudeRunOptions(
                question=entry.question,
                system_prompt=AGENT_SYSTEM_PROMPT,
                allowed_tools=_allowed_tools_for(toolset_name),
                mcp_config=mcp_cfg,
                model=agent_model,
                cwd=base_dir,
                timeout_s=args.agent_timeout,
            )
            rec = run_with_tools(opts)
            rec.question_id = entry.id
            rec.toolset = toolset_name
            rec.repo = args.repo
            elapsed = int(time.time() - t0)
            print(
                f"{elapsed}s, total_tokens={rec.total_input_tokens}, "
                f"(in={rec.input_tokens}, cache_w={rec.cache_creation_input_tokens}, "
                f"cache_r={rec.cache_read_input_tokens}), tools={len(rec.tool_calls)}",
                file=sys.stderr,
            )
            d = rec.to_dict()
            raw_runs.append(d)
            per_q_records[toolset_name] = d

        default_rec = per_q_records["default"]
        ci_rec = per_q_records["code_intel"]

        mech_def = mech_score(entry, default_rec["final_answer"]).combined
        mech_ci = mech_score(entry, ci_rec["final_answer"]).combined

        if args.skip_judge:
            judge_def = judge_ci = 0
        else:
            seed = random.randint(0, 1)
            try:
                jr = judge_pair(
                    complete_fn=lambda system, user: run_one_shot(
                        prompt=user, system_prompt=system, model=judge_model
                    ),
                    question=entry.question,
                    rubric=entry.rubric,
                    default_answer=default_rec["final_answer"],
                    code_intel_answer=ci_rec["final_answer"],
                    seed=seed,
                )
                judge_def = jr.default_score
                judge_ci = jr.code_intel_score
                default_rec["judge_justification"] = jr.default_justification
                ci_rec["judge_justification"] = jr.code_intel_justification
            except Exception as e:
                print(f"  judge failed: {e}", file=sys.stderr)
                judge_def = judge_ci = 0

        default_rec["mech_score"] = mech_def
        default_rec["judge_score"] = judge_def
        ci_rec["mech_score"] = mech_ci
        ci_rec["judge_score"] = judge_ci

        for rec, mech_v, judge_v in (
            (default_rec, mech_def, judge_def),
            (ci_rec, mech_ci, judge_ci),
        ):
            # input_tokens here is the TOTAL the model saw (uncached + cache write + cache read)
            # because Claude Code's default-system-prompt overhead lands almost entirely in
            # cache_creation/read; comparing only the uncached fraction would be misleading.
            scored.append(
                ScoredRun(
                    question_id=rec["question_id"],
                    toolset=rec["toolset"],
                    repo=rec["repo"],
                    mech_score=mech_v,
                    judge_score=judge_v,
                    input_tokens=rec.get("total_input_tokens", rec["input_tokens"]),
                    output_tokens=rec["output_tokens"],
                    tool_calls=[tc["name"] for tc in rec["tool_calls"]],
                    wall_ms=rec["wall_ms"],
                    final_answer=rec["final_answer"],
                    stop_reason=rec["stop_reason"],
                )
            )

    aggregate = aggregate_round(scored)
    rnnn = f"R{args.round:03d}"
    json_path = args.output_dir / f"{rnnn}.json"
    md_path = args.output_dir / f"{rnnn}.md"
    json_path.write_text(json.dumps(
        {"runs": raw_runs, "round": args.round, "repo": args.repo,
         "agent_model": agent_model, "judge_model": judge_model},
        indent=2,
    ))
    md_path.write_text(render_markdown(round_id=args.round, repos=[args.repo], aggregate=aggregate))
    print(f"\nWrote {json_path} and {md_path}", file=sys.stderr)


if __name__ == "__main__":
    main()

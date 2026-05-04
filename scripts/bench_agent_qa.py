#!/usr/bin/env python3
"""Run the agent Q&A benchmark for one round.

Usage:
    python3 scripts/bench_agent_qa.py --round 1 --repo self
    python3 scripts/bench_agent_qa.py --round 1 --repo wolfmax \
        --base-dir /path/to/wolfmax \
        --queries scripts/queries_qa_wolfmax.json

Env:
    ANTHROPIC_API_KEY  required
    AGENT_MODEL        default: claude-sonnet-4-6
    JUDGE_MODEL        default: claude-haiku-4-5-20251001
"""
from __future__ import annotations

import argparse
import json
import os
import random
import sys
import time
from pathlib import Path
from typing import Any, Dict, List

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT))

from anthropic import Anthropic  # type: ignore

from scripts.agent_qa.qa_schema import load_qa_set
from scripts.agent_qa.tool_wrappers import (
    DEFAULT_TOOL_DEFS,
    DefaultToolset,
    dispatch_default,
)
from scripts.agent_qa.mcp_client import (
    CI_TOOL_PREFIX,
    McpStdioClient,
    to_anthropic_tool_defs,
)
from scripts.agent_qa.agent_loop import Toolbox, run_agent
from scripts.agent_qa.scoring import mech_score
from scripts.agent_qa.judge import judge_pair
from scripts.agent_qa.report import ScoredRun, aggregate_round, render_markdown


DEFAULT_BINARY = REPO_ROOT / "target" / "release" / "code-intelligence-mcp-server"
DEFAULT_QUERIES = {
    "self": REPO_ROOT / "scripts" / "queries_qa_self.json",
    "wolfmax": REPO_ROOT / "scripts" / "queries_qa_wolfmax.json",
}
RESULTS_DIR = REPO_ROOT / "docs" / "benchmark_rounds" / "agent"


def _build_default_toolbox(base_dir: Path) -> Toolbox:
    toolset = DefaultToolset(base_dir=base_dir)
    return Toolbox(
        tool_defs=list(DEFAULT_TOOL_DEFS),
        dispatch=lambda name, args: dispatch_default(name, args, toolset),
    )


def _build_code_intel_toolbox(base_dir: Path, mcp: McpStdioClient) -> Toolbox:
    server_tools = mcp.list_tools()
    ci_defs = to_anthropic_tool_defs(server_tools, prefix=CI_TOOL_PREFIX)
    default_toolset = DefaultToolset(base_dir=base_dir)
    all_defs = list(DEFAULT_TOOL_DEFS) + ci_defs

    def _dispatch(name: str, args: Dict[str, Any]) -> str:
        if name in {"read_file", "grep", "glob", "bash"}:
            return dispatch_default(name, args, default_toolset)
        if name.startswith(CI_TOOL_PREFIX):
            real_name = name[len(CI_TOOL_PREFIX):]
            res = mcp.call_tool(real_name, args)
            if res.is_error:
                return f"tool error: {res.text}"
            return res.text
        raise RuntimeError(f"unknown tool: {name}")

    return Toolbox(tool_defs=all_defs, dispatch=_dispatch)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--round", type=int, required=True)
    parser.add_argument("--repo", required=True, choices=["self", "wolfmax", "custom"])
    parser.add_argument("--base-dir", type=Path, default=None)
    parser.add_argument("--queries", type=Path, default=None)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output-dir", type=Path, default=RESULTS_DIR)
    parser.add_argument("--question-ids", default=None, help="comma-separated subset")
    parser.add_argument("--skip-judge", action="store_true")
    args = parser.parse_args()

    base_dir = (args.base_dir or REPO_ROOT).resolve()
    queries_path = args.queries or DEFAULT_QUERIES.get(args.repo)
    if queries_path is None or not Path(queries_path).is_file():
        sys.exit(f"queries file not found: {queries_path}")

    if not args.binary.is_file():
        sys.exit(f"MCP binary not found: {args.binary}. Run `cargo build --release` first.")
    if "ANTHROPIC_API_KEY" not in os.environ:
        sys.exit("ANTHROPIC_API_KEY not set")

    agent_model = os.environ.get("AGENT_MODEL", "claude-sonnet-4-6")
    judge_model = os.environ.get("JUDGE_MODEL", "claude-haiku-4-5-20251001")

    qa_entries = load_qa_set(Path(queries_path))
    if args.question_ids:
        wanted = set(s.strip() for s in args.question_ids.split(","))
        qa_entries = [e for e in qa_entries if e.id in wanted]

    args.output_dir.mkdir(parents=True, exist_ok=True)

    client = Anthropic()
    raw_runs: List[dict] = []
    scored: List[ScoredRun] = []

    print(f"Starting MCP server ({args.binary}) for {base_dir}", file=sys.stderr)
    mcp = McpStdioClient(binary=str(args.binary), base_dir=base_dir)
    try:
        mcp.initialize()
        # Trigger an indexing pass and wait briefly.
        try:
            mcp.call_tool("refresh_index", {})
        except Exception as e:
            print(f"warn: refresh_index failed: {e}", file=sys.stderr)
        time.sleep(2.0)

        default_box = _build_default_toolbox(base_dir)
        ci_box = _build_code_intel_toolbox(base_dir, mcp)

        for entry in qa_entries:
            print(f"\n=== {entry.id} ===", file=sys.stderr)
            for toolset_name, box in (("default", default_box), ("code_intel", ci_box)):
                print(f"  running {toolset_name}", file=sys.stderr)
                rec = run_agent(
                    client=client,
                    model=agent_model,
                    question=entry.question,
                    toolbox=box,
                )
                rec.question_id = entry.id
                rec.toolset = toolset_name
                rec.repo = args.repo
                raw_runs.append(rec.to_dict())

            default_rec = next(r for r in raw_runs if r["question_id"] == entry.id and r["toolset"] == "default")
            ci_rec = next(r for r in raw_runs if r["question_id"] == entry.id and r["toolset"] == "code_intel")

            mech_def = mech_score(entry, default_rec["final_answer"]).combined
            mech_ci = mech_score(entry, ci_rec["final_answer"]).combined

            if args.skip_judge:
                judge_def = judge_ci = 0
            else:
                seed = random.randint(0, 1)
                jr = judge_pair(
                    client=client,
                    model=judge_model,
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

            default_rec["mech_score"] = mech_def
            default_rec["judge_score"] = judge_def
            ci_rec["mech_score"] = mech_ci
            ci_rec["judge_score"] = judge_ci

            for rec, mech_v, judge_v in (
                (default_rec, mech_def, judge_def),
                (ci_rec, mech_ci, judge_ci),
            ):
                scored.append(
                    ScoredRun(
                        question_id=rec["question_id"],
                        toolset=rec["toolset"],
                        repo=rec["repo"],
                        mech_score=mech_v,
                        judge_score=judge_v,
                        input_tokens=rec["input_tokens"],
                        output_tokens=rec["output_tokens"],
                        tool_calls=[tc["name"] for tc in rec["tool_calls"]],
                        wall_ms=rec["wall_ms"],
                        final_answer=rec["final_answer"],
                        stop_reason=rec["stop_reason"],
                    )
                )
    finally:
        mcp.close()

    aggregate = aggregate_round(scored)
    rnnn = f"R{args.round:03d}"
    json_path = args.output_dir / f"{rnnn}.json"
    md_path = args.output_dir / f"{rnnn}.md"
    json_path.write_text(json.dumps({"runs": raw_runs, "round": args.round, "repo": args.repo}, indent=2))
    md_path.write_text(render_markdown(round_id=args.round, repos=[args.repo], aggregate=aggregate))
    print(f"\nWrote {json_path} and {md_path}", file=sys.stderr)


if __name__ == "__main__":
    main()

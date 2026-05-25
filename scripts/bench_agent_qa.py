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
import atexit
import json
import random
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Callable, Dict, List
from urllib.parse import urlencode

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


CompleteFn = Callable[[str, str], str]


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

CODE_INTEL_SYSTEM_PROMPT_EXTRA = (
    "This run has the code-intelligence MCP available and should answer from "
    "code-intelligence evidence rather than built-in file-reading tools. Start "
    "codebase investigations with `mcp__code-intelligence__ask_code`; use "
    "`mcp__code-intelligence__investigate` for follow-up graph questions. "
    "Call `mcp__code-intelligence__ask_code` at most once per question and "
    "`mcp__code-intelligence__investigate` at most once for a specific missing "
    "graph hop; do not repeat code-intelligence calls with paraphrased prompts. "
    "When `ask_code` or `investigate` returns `pack.rows`, synthesize from those "
    "rows and respect `pack.coverage.status` and `role=\"candidate\"`. If "
    "`pack.coverage.status` coverage is complete and `evidence[]` or `pack.rows` "
    "contains the needed line-level source, do not call Read, Grep, or Glob to "
    "re-check the same files. If the response has partial/no_hits coverage, "
    "candidate rows, or missing source bodies, report uncertainty from the "
    "evidence instead of falling back to file-reading tools."
)

CODE_GRAPH_SYSTEM_PROMPT_EXTRA = (
    "This run has the codegraph MCP available. Start codebase investigations "
    "with `mcp__codegraph__codegraph_search`, then use "
    "`mcp__codegraph__codegraph_context` or `mcp__codegraph__codegraph_callers` "
    "for graph follow-up before falling back to Read/Grep/Glob."
)

TOOLSET_SYSTEM_PROMPT_EXTRAS = {
    "code_intel": CODE_INTEL_SYSTEM_PROMPT_EXTRA,
    "code_graph": CODE_GRAPH_SYSTEM_PROMPT_EXTRA,
}

CODE_INTEL_MCP_TOOLS = [
    "mcp__code-intelligence__ask_code",
    "mcp__code-intelligence__investigate",
]

CODE_GRAPH_MCP_TOOLS = [
    "mcp__codegraph__codegraph_search",
    "mcp__codegraph__codegraph_context",
    "mcp__codegraph__codegraph_callers",
]

TOOLSET_MCP_TOOLS = {
    "code_intel": CODE_INTEL_MCP_TOOLS,
    "code_graph": CODE_GRAPH_MCP_TOOLS,
}


def _system_prompt_for(toolset: str) -> str:
    """Build the system prompt, optionally appending an env-var extra.

    AGENT_SYSTEM_PROMPT_EXTRA_<TOOLSET> (uppercase) overrides per-toolset.
    AGENT_SYSTEM_PROMPT_EXTRA applies to all toolsets if the specific one is unset.
    Use this to test instruction-layer changes (e.g., 'always prefer MCP tools').
    """
    import os as _os
    key_specific = f"AGENT_SYSTEM_PROMPT_EXTRA_{toolset.upper()}"
    extras = []
    toolset_extra = TOOLSET_SYSTEM_PROMPT_EXTRAS.get(toolset)
    if toolset_extra:
        extras.append(toolset_extra)
    env_extra = _os.environ.get(key_specific) or _os.environ.get("AGENT_SYSTEM_PROMPT_EXTRA", "")
    if env_extra:
        extras.append(env_extra.strip())
    if extras:
        return AGENT_SYSTEM_PROMPT + "\n\n" + "\n\n".join(extras)
    return AGENT_SYSTEM_PROMPT


def _build_default_mcp_config() -> Dict[str, Any]:
    return {"mcpServers": {}}


def _repo_mcp_url(mcp_url: str, base_dir: Path) -> str:
    sep = "&" if "?" in mcp_url else "?"
    return f"{mcp_url}{sep}{urlencode({'repo': str(base_dir)})}"


def _build_code_intel_mcp_config(mcp_url: str, base_dir: Path) -> Dict[str, Any]:
    return {
        "mcpServers": {
            "code-intelligence": {
                "type": "streamable-http",
                "url": _repo_mcp_url(mcp_url, base_dir),
            }
        }
    }


def _pick_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def _wait_for_port(port: int, proc: subprocess.Popen, timeout_s: float = 20.0) -> None:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"code-intelligence daemon exited early with code {proc.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.25):
                return
        except OSError:
            time.sleep(0.1)
    raise TimeoutError(f"code-intelligence daemon did not open port {port} within {timeout_s:.0f}s")


def _terminate_process(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)


def _start_code_intel_daemon(binary: Path, port: int) -> subprocess.Popen:
    proc = subprocess.Popen(
        [
            str(binary),
            "--port",
            str(port),
            "--discovery-port",
            str(port + 1),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    atexit.register(_terminate_process, proc)
    _wait_for_port(port, proc)
    return proc


def _build_code_graph_mcp_config(base_dir: Path) -> Dict[str, Any]:
    import os as _os
    binary = _os.environ.get("BENCH_CODE_GRAPH_BINARY", "codegraph")
    return {
        "mcpServers": {
            "codegraph": {
                "command": binary,
                "args": ["serve", "--mcp", "-p", str(base_dir), "--no-watch"],
                "env": {},
            }
        }
    }


def _allowed_tools_for(toolset: str) -> List[str]:
    # Keep built-in tools identical across runs. Add only the MCP tools that
    # belong to the selected toolset, so the benchmark measures the intended
    # tool layer instead of merely wiring an MCP server the agent may ignore.
    if toolset == "code_intel":
        return list(CODE_INTEL_MCP_TOOLS)
    return list(DEFAULT_BUILTIN_TOOLS) + list(TOOLSET_MCP_TOOLS.get(toolset, []))


def _extra_args_for(toolset: str) -> List[str]:
    if toolset == "code_intel":
        return ["--disallowed-tools", " ".join(DEFAULT_BUILTIN_TOOLS)]
    return []


def _score_question_records(
    entry: Any,
    per_q_records: Dict[str, dict],
    *,
    skip_judge: bool,
    complete_fn: CompleteFn,
) -> None:
    """Attach mechanical and judge scores to every toolset record for one question."""
    for rec in per_q_records.values():
        rec["mech_score"] = mech_score(entry, rec["final_answer"]).combined

    default_rec = per_q_records.get("default")
    candidate_toolsets = [ts for ts in per_q_records if ts != "default"]

    if default_rec is None or skip_judge or not candidate_toolsets:
        for rec in per_q_records.values():
            rec.setdefault("judge_score", 0)
        return

    default_pair_scores: Dict[str, int] = {}
    default_pair_justifications: Dict[str, str] = {}
    for toolset in candidate_toolsets:
        rec = per_q_records[toolset]
        seed = random.randint(0, 1)
        try:
            jr = judge_pair(
                complete_fn=complete_fn,
                question=entry.question,
                rubric=entry.rubric,
                default_answer=default_rec["final_answer"],
                code_intel_answer=rec["final_answer"],
                seed=seed,
            )
            default_pair_scores[toolset] = jr.default_score
            default_pair_justifications[toolset] = jr.default_justification
            rec["judge_baseline_score"] = jr.default_score
            rec["judge_baseline_justification"] = jr.default_justification
            rec["judge_justification"] = jr.code_intel_justification
            rec["judge_score"] = jr.code_intel_score
        except Exception as e:
            print(f"  judge failed for {toolset}: {e}", file=sys.stderr)
            rec["judge_baseline_score"] = 0
            rec["judge_score"] = 0

    if default_pair_scores:
        default_rec["judge_scores_by_pair"] = default_pair_scores
        default_rec["judge_justifications_by_pair"] = default_pair_justifications
        primary_toolset = (
            "code_intel" if "code_intel" in default_pair_scores else next(iter(default_pair_scores))
        )
        default_rec["judge_score"] = default_pair_scores[primary_toolset]
        default_rec["judge_justification"] = default_pair_justifications[primary_toolset]
    else:
        default_rec.setdefault("judge_score", 0)


def _scored_runs_from_records(per_q_records: Dict[str, dict]) -> List[ScoredRun]:
    scored: List[ScoredRun] = []
    for rec in per_q_records.values():
        if "mech_score" not in rec or "judge_score" not in rec:
            continue
        # input_tokens here is the TOTAL the model saw (uncached + cache write + cache read)
        # because Claude Code's default-system-prompt overhead lands almost entirely in
        # cache_creation/read; comparing only the uncached fraction would be misleading.
        scored.append(
            ScoredRun(
                question_id=rec["question_id"],
                toolset=rec["toolset"],
                repo=rec["repo"],
                mech_score=rec["mech_score"],
                judge_score=rec["judge_score"],
                input_tokens=rec.get("total_input_tokens", rec["input_tokens"]),
                output_tokens=rec["output_tokens"],
                tool_calls=[tc["name"] for tc in rec["tool_calls"]],
                wall_ms=rec["wall_ms"],
                final_answer=rec["final_answer"],
                stop_reason=rec["stop_reason"],
                judge_baseline_score=rec.get("judge_baseline_score"),
            )
        )
    return scored


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

    wanted_toolsets = _os.environ.get("BENCH_TOOLSETS")
    requested_toolsets = {"default", "code_intel", "code_graph"}
    if wanted_toolsets:
        known = requested_toolsets
        requested_toolsets = {s.strip() for s in wanted_toolsets.split(",")}
        requested_toolsets = {k for k in requested_toolsets if k in known}
        if not requested_toolsets:
            sys.exit(f"BENCH_TOOLSETS={wanted_toolsets} matched no known toolsets")

    code_intel_proc = None
    code_intel_url = _os.environ.get("BENCH_CODE_INTEL_URL")
    if "code_intel" in requested_toolsets and not code_intel_url:
        port = int(_os.environ.get("BENCH_CODE_INTEL_PORT") or _pick_free_port())
        code_intel_proc = _start_code_intel_daemon(args.binary, port)
        code_intel_url = f"http://127.0.0.1:{port}/mcp"
        print(f"Started code-intelligence daemon for benchmark at {code_intel_url}", file=sys.stderr)

    all_toolset_configs = {
        "default": _build_default_mcp_config(),
        "code_intel": _build_code_intel_mcp_config(code_intel_url or "", base_dir),
        "code_graph": _build_code_graph_mcp_config(base_dir),
    }
    toolset_configs = {
        k: all_toolset_configs[k]
        for k in all_toolset_configs
        if k in requested_toolsets
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
                system_prompt=_system_prompt_for(toolset_name),
                allowed_tools=_allowed_tools_for(toolset_name),
                mcp_config=mcp_cfg,
                model=agent_model,
                cwd=base_dir,
                timeout_s=args.agent_timeout,
                extra_args=_extra_args_for(toolset_name),
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

        _score_question_records(
            entry,
            per_q_records,
            skip_judge=args.skip_judge,
            complete_fn=lambda system, user: run_one_shot(
                prompt=user, system_prompt=system, model=judge_model
            ),
        )
        scored.extend(_scored_runs_from_records(per_q_records))

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

    if code_intel_proc is not None:
        _terminate_process(code_intel_proc)


if __name__ == "__main__":
    main()

"""Drive `claude --print` per question and parse the transcript."""
from __future__ import annotations

import json
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from bench import config
from bench.arms import Arm
from bench.fixtures_io import Question


@dataclass
class ToolCall:
    name: str
    input_summary: str = ""
    result_size: int = 0


@dataclass
class Run:
    arm: str
    question_id: str
    repo: str
    final_answer: str
    tool_calls: list[ToolCall] = field(default_factory=list)
    input_tokens: int = 0
    output_tokens: int = 0
    cache_read_tokens: int = 0
    cache_creation_tokens: int = 0
    wall_ms: int = 0
    stop_reason: str = ""
    model: str = ""
    daemon_sha: str | None = None
    raw_transcript_path: str | None = None


SYSTEM_PROMPT_TEMPLATE = """You are an investigation agent answering a single question about a codebase.
Use the provided tools to find evidence. Cite specific file paths and line numbers
in your final answer. Stop searching once you have enough to answer. Produce a
final answer as a normal text message (no tool calls) when ready. Be concise.

{tool_guidance}

Tools available: {tools}
"""


def build_system_prompt(arm: Arm) -> str:
    return SYSTEM_PROMPT_TEMPLATE.format(
        tool_guidance=arm.tool_guidance,
        tools=", ".join(arm.allowed_tools),
    )


def _summarize_input(input_obj: Any) -> str:
    if not isinstance(input_obj, dict):
        return ""
    parts = []
    for k, v in input_obj.items():
        vs = str(v)
        if len(vs) > 40:
            vs = vs[:37] + "..."
        parts.append(f"{k}={vs}")
    return ", ".join(parts)[:120]


def _parse_transcript(transcript_bytes: bytes) -> dict:
    """Parse claude --print JSONL output.

    Returns a dict with: final_answer, tool_calls (list[ToolCall]), usage (dict), stop_reason.
    """
    final_answer = ""
    tool_calls: list[ToolCall] = []
    usage = {
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_input_tokens": 0,
        "cache_creation_input_tokens": 0,
    }
    stop_reason = "unknown"

    for raw_line in transcript_bytes.decode("utf-8", errors="replace").splitlines():
        if not raw_line.strip():
            continue
        try:
            obj = json.loads(raw_line)
        except json.JSONDecodeError:
            continue

        msg_type = obj.get("type")
        if msg_type == "tool_use":
            tc = ToolCall(name=obj.get("name", ""), input_summary=_summarize_input(obj.get("input", {})))
            tool_calls.append(tc)
        elif msg_type == "tool_result":
            content = obj.get("content")
            size = len(content) if isinstance(content, str) else len(json.dumps(content) if content else "")
            if tool_calls:
                tool_calls[-1].result_size = size
        elif msg_type == "assistant":
            msg = obj.get("message", {})
            content = msg.get("content", [])
            for block in content:
                if isinstance(block, dict) and block.get("type") == "text":
                    final_answer = block.get("text", "")
            u = msg.get("usage", {})
            for k in usage:
                if k in u:
                    usage[k] = u[k]
        elif msg_type == "result":
            stop_reason = obj.get("stop_reason", "unknown")

    return {
        "final_answer": final_answer,
        "tool_calls": tool_calls,
        "usage": usage,
        "stop_reason": stop_reason,
    }


def run_question(
    arm: Arm,
    q: Question,
    daemon: Any | None,
    repo_path: Path,
    transcripts_dir: Path,
) -> Run:
    """Invoke claude --print for one (arm, question) pair and return a Run record."""
    system_prompt = build_system_prompt(arm)
    cmd = [
        config.CLAUDE_BINARY, "--print",
        "--model", config.AGENT_MODEL,
        "--system-prompt", system_prompt,
        "--allowed-tools", ",".join(arm.allowed_tools),
    ]
    if daemon is not None:
        mcp_config = daemon.build_mcp_config()
        if mcp_config:
            cmd.extend(["--mcp-config", json.dumps(mcp_config)])
    cmd.append(q.question)

    transcript_path = transcripts_dir / arm.name / f"{q.id}.jsonl"
    transcript_path.parent.mkdir(parents=True, exist_ok=True)

    start = time.monotonic()
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            timeout=config.PER_QUESTION_TIMEOUT_S,
            cwd=str(repo_path),
        )
        wall_ms = int((time.monotonic() - start) * 1000)
        transcript_path.write_bytes(result.stdout)
        parsed = _parse_transcript(result.stdout)
    except subprocess.TimeoutExpired:
        wall_ms = int((time.monotonic() - start) * 1000)
        return Run(
            arm=arm.name,
            question_id=q.id,
            repo=str(repo_path),
            final_answer="",
            wall_ms=wall_ms,
            stop_reason="timeout",
            model=config.AGENT_MODEL,
            raw_transcript_path=str(transcript_path),
        )

    return Run(
        arm=arm.name,
        question_id=q.id,
        repo=str(repo_path),
        final_answer=parsed["final_answer"],
        tool_calls=parsed["tool_calls"],
        input_tokens=parsed["usage"]["input_tokens"],
        output_tokens=parsed["usage"]["output_tokens"],
        cache_read_tokens=parsed["usage"]["cache_read_input_tokens"],
        cache_creation_tokens=parsed["usage"]["cache_creation_input_tokens"],
        wall_ms=wall_ms,
        stop_reason=parsed["stop_reason"],
        model=config.AGENT_MODEL,
        raw_transcript_path=str(transcript_path),
    )

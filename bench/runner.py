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
    run_error: str | None = None


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
    """Parse claude --print output.

    Handles two formats:
    - Single JSON object (--output-format json): has 'result', 'stop_reason', 'usage' keys.
    - JSONL stream (legacy / --output-format stream-json): one JSON object per line.

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

    text = transcript_bytes.decode("utf-8", errors="replace").strip()
    if not text:
        return {"final_answer": final_answer, "tool_calls": tool_calls,
                "usage": usage, "stop_reason": stop_reason}

    # Try single-JSON format first (--output-format json).
    try:
        obj = json.loads(text)
        if obj.get("type") == "result":
            final_answer = obj.get("result", "") or ""
            stop_reason = obj.get("stop_reason", "unknown") or "unknown"
            u = obj.get("usage", {}) or {}
            usage["input_tokens"] = u.get("input_tokens", 0) or 0
            usage["output_tokens"] = u.get("output_tokens", 0) or 0
            usage["cache_read_input_tokens"] = u.get("cache_read_input_tokens", 0) or 0
            usage["cache_creation_input_tokens"] = u.get("cache_creation_input_tokens", 0) or 0
            # Derive a synthetic ToolCall list from num_turns (no per-tool detail in this format).
            num_turns = obj.get("num_turns", 1) or 1
            for _ in range(max(0, num_turns - 1)):
                tool_calls.append(ToolCall(name="<unknown>", input_summary=""))
            return {"final_answer": final_answer, "tool_calls": tool_calls,
                    "usage": usage, "stop_reason": stop_reason}
    except (json.JSONDecodeError, AttributeError):
        pass

    # Fall back to JSONL stream format (stream-json --verbose or legacy format).
    for raw_line in text.splitlines():
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
                if isinstance(block, dict):
                    btype = block.get("type")
                    if btype == "text":
                        final_answer = block.get("text", "")
                    elif btype == "tool_use":
                        tc = ToolCall(
                            name=block.get("name", ""),
                            input_summary=_summarize_input(block.get("input", {})),
                        )
                        tool_calls.append(tc)
            u = msg.get("usage", {}) or {}
            for k in usage:
                if k in u:
                    usage[k] = u[k]
        elif msg_type == "result":
            stop_reason = obj.get("stop_reason", "unknown")
            if not final_answer:
                final_answer = obj.get("result", "") or ""

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
    # claude --print requires the prompt via stdin (positional arg form fails in --print mode).
    # --output-format json gives us a single JSON result object with 'result', 'stop_reason',
    # and 'usage' -- much easier to parse than the verbose stream-json hook flood.
    cmd = [
        config.CLAUDE_BINARY, "--print",
        "--output-format", "json",
        "--model", config.AGENT_MODEL,
        "--system-prompt", system_prompt,
        "--allowed-tools", ",".join(arm.allowed_tools),
        # Isolation: without these, globally-configured MCP servers (including the
        # production code-intelligence daemon) and user/project settings+hooks leak
        # into every arm, contaminating the comparison and inflating context tokens.
        "--strict-mcp-config",
        "--setting-sources", "",
    ]
    if daemon is not None:
        mcp_config = daemon.build_mcp_config()
        if mcp_config:
            cmd.extend(["--mcp-config", json.dumps(mcp_config)])

    transcript_path = transcripts_dir / arm.name / f"{q.id}.jsonl"
    transcript_path.parent.mkdir(parents=True, exist_ok=True)

    start = time.monotonic()
    run_error: str | None = None
    parsed: dict | None = None
    for _attempt in range(2):
        try:
            result = subprocess.run(
                cmd,
                input=q.question.encode("utf-8"),
                capture_output=True,
                timeout=config.PER_QUESTION_TIMEOUT_S,
                cwd=str(repo_path),
            )
        except subprocess.TimeoutExpired as e:
            # Don't retry timeouts (another PER_QUESTION_TIMEOUT_S wait for a run
            # that is likely to time out again); keep whatever partial output exists.
            if e.output:
                transcript_path.write_bytes(e.output)
            run_error = "timeout"
            break
        if result.returncode != 0:
            run_error = f"cli_exit_{result.returncode}"
            continue  # transient CLI failure: retry once
        run_error = None
        transcript_path.write_bytes(result.stdout)
        parsed = _parse_transcript(result.stdout)
        break
    wall_ms = int((time.monotonic() - start) * 1000)

    if parsed is None:
        return Run(
            arm=arm.name,
            question_id=q.id,
            repo=str(repo_path),
            final_answer="",
            wall_ms=wall_ms,
            stop_reason="timeout" if run_error == "timeout" else "cli_error",
            model=config.AGENT_MODEL,
            raw_transcript_path=str(transcript_path),
            run_error=run_error,
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
        run_error=None,
    )

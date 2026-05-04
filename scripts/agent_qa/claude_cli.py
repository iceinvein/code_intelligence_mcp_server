"""Subprocess wrapper around `claude --print` for the agent benchmark.

We drive Claude Code's CLI rather than the Anthropic SDK so the benchmark uses
the same authentication and runtime as a real Claude Code session. Each run
is launched with:

    claude --print
           --output-format stream-json
           --no-session-persistence
           --disable-slash-commands
           --strict-mcp-config
           --mcp-config <inline JSON>
           --allowed-tools <space-separated list>
           --system-prompt <prompt>
           --model <model>
           <question>

Stream-json output is one JSON object per line. We accumulate:
    - the assistant's final text (concatenated text blocks from the last
      assistant turn that did not also emit a tool_use)
    - each tool_use block (name + args)
    - input_tokens and output_tokens from the final usage record

Shapes vary slightly across CLI versions. The parser below is intentionally
permissive: it inspects every event for known keys and falls back to the JSON
single-result format when stream-json fields are not present.
"""
from __future__ import annotations

import json
import os
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional

from scripts.agent_qa.run_record import RunRecord, ToolCallRecord


CLAUDE_BINARY = os.environ.get("CLAUDE_BINARY", "claude")


@dataclass
class ClaudeRunOptions:
    question: str
    system_prompt: str
    allowed_tools: List[str]
    mcp_config: Dict[str, Any]  # full MCP config dict, will be JSON-encoded
    model: str
    cwd: Path
    timeout_s: int = 600
    extra_args: List[str] = field(default_factory=list)


def build_command(opts: ClaudeRunOptions) -> List[str]:
    cmd: List[str] = [
        CLAUDE_BINARY,
        "--print",
        "--output-format", "stream-json",
        "--verbose",  # stream-json requires --verbose
        "--no-session-persistence",
        "--disable-slash-commands",
        "--strict-mcp-config",
        "--mcp-config", json.dumps(opts.mcp_config),
        "--system-prompt", opts.system_prompt,
        "--model", opts.model,
    ]
    if opts.allowed_tools:
        cmd.extend(["--allowed-tools", " ".join(opts.allowed_tools)])
    cmd.extend(opts.extra_args)
    cmd.append(opts.question)
    return cmd


@dataclass
class ParsedStream:
    final_answer: str
    tool_calls: List[ToolCallRecord]
    input_tokens: int
    output_tokens: int
    stop_reason: str


def _walk(obj: Any) -> Iterable[Any]:
    """Yield obj and every nested dict/list child for permissive scanning."""
    yield obj
    if isinstance(obj, dict):
        for v in obj.values():
            yield from _walk(v)
    elif isinstance(obj, list):
        for v in obj:
            yield from _walk(v)


def _extract_usage(event: dict) -> Optional[Dict[str, int]]:
    for node in _walk(event):
        if isinstance(node, dict) and ("input_tokens" in node or "output_tokens" in node):
            return {
                "input_tokens": int(node.get("input_tokens") or 0),
                "output_tokens": int(node.get("output_tokens") or 0),
            }
    return None


def parse_stream(lines: Iterable[str]) -> ParsedStream:
    """Parse one JSON-per-line event stream from `claude --output-format stream-json`."""
    final_text_parts: List[str] = []
    tool_calls: List[ToolCallRecord] = []
    input_tokens = 0
    output_tokens = 0
    stop_reason = "unknown"

    for raw in lines:
        line = raw.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue

        # Tool calls: scan every nested block for content blocks with type "tool_use".
        for node in _walk(event):
            if not isinstance(node, dict):
                continue
            if node.get("type") == "tool_use" and "name" in node:
                tool_calls.append(
                    ToolCallRecord(
                        name=str(node.get("name", "")),
                        args=dict(node.get("input") or {}),
                        result_text="",  # tool result text comes in a sibling event we do not need
                        result_bytes=0,
                        duration_ms=0,
                        is_error=False,
                    )
                )

        # Final answer: accumulate any "type: text" blocks from assistant messages.
        # We take only the LAST assistant turn's text (the one without a following tool_use)
        # by resetting on each new assistant message.
        msg = event.get("message") if isinstance(event, dict) else None
        if isinstance(msg, dict) and msg.get("role") == "assistant":
            content = msg.get("content")
            if isinstance(content, list):
                # Reset; we want only the most recent assistant turn's text blocks.
                this_turn_text: List[str] = []
                this_turn_has_tool_use = False
                for block in content:
                    if not isinstance(block, dict):
                        continue
                    if block.get("type") == "text" and isinstance(block.get("text"), str):
                        this_turn_text.append(block["text"])
                    if block.get("type") == "tool_use":
                        this_turn_has_tool_use = True
                if not this_turn_has_tool_use and this_turn_text:
                    final_text_parts = this_turn_text  # latest non-tool turn wins
                stop = msg.get("stop_reason")
                if isinstance(stop, str):
                    stop_reason = stop

        # Top-level "result" event (single-shot json format) carries the final string.
        if event.get("type") == "result":
            res = event.get("result")
            if isinstance(res, str) and res.strip():
                final_text_parts = [res]
            usage = _extract_usage(event)
            if usage:
                input_tokens = max(input_tokens, usage["input_tokens"])
                output_tokens = max(output_tokens, usage["output_tokens"])

        # Any other event with a usage block: take the largest-seen totals.
        usage = _extract_usage(event)
        if usage:
            input_tokens = max(input_tokens, usage["input_tokens"])
            output_tokens = max(output_tokens, usage["output_tokens"])

    return ParsedStream(
        final_answer="".join(final_text_parts).strip(),
        tool_calls=tool_calls,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        stop_reason=stop_reason,
    )


def run_with_tools(opts: ClaudeRunOptions) -> RunRecord:
    """Drive a tool-using `claude --print` invocation and return a populated RunRecord."""
    cmd = build_command(opts)
    started = time.time()
    proc = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=opts.timeout_s,
        cwd=str(opts.cwd),
    )
    wall_ms = int((time.time() - started) * 1000)
    if proc.returncode != 0:
        # Surface stderr in the final answer so the caller can see what went wrong.
        return RunRecord(
            question_id="",
            toolset="",
            model=opts.model,
            repo="",
            final_answer=f"(claude CLI exit={proc.returncode})\nstderr:\n{proc.stderr.strip()}",
            input_tokens=0,
            output_tokens=0,
            wall_ms=wall_ms,
            stop_reason=f"cli_error_{proc.returncode}",
            tool_calls=[],
        )

    parsed = parse_stream(proc.stdout.splitlines())
    return RunRecord(
        question_id="",
        toolset="",
        model=opts.model,
        repo="",
        final_answer=parsed.final_answer,
        input_tokens=parsed.input_tokens,
        output_tokens=parsed.output_tokens,
        wall_ms=wall_ms,
        stop_reason=parsed.stop_reason,
        tool_calls=parsed.tool_calls,
    )


def run_one_shot(prompt: str, system_prompt: str, model: str, timeout_s: int = 120) -> str:
    """Run a no-tool one-shot completion (used by the LLM judge)."""
    cmd = [
        CLAUDE_BINARY,
        "--print",
        "--output-format", "json",
        "--no-session-persistence",
        "--disable-slash-commands",
        "--strict-mcp-config",
        "--mcp-config", '{"mcpServers":{}}',
        "--allowed-tools", "",
        "--system-prompt", system_prompt,
        "--model", model,
        prompt,
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout_s)
    if proc.returncode != 0:
        raise RuntimeError(f"claude CLI exit={proc.returncode}: {proc.stderr.strip()}")
    try:
        obj = json.loads(proc.stdout)
    except json.JSONDecodeError as e:
        raise RuntimeError(f"non-JSON output from claude CLI: {e}\n{proc.stdout[:500]}") from e
    res = obj.get("result")
    if isinstance(res, str):
        return res
    # Older shapes may put the text under message.content[0].text
    msg = obj.get("message") or {}
    content = msg.get("content") if isinstance(msg, dict) else None
    if isinstance(content, list):
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text" and isinstance(block.get("text"), str):
                return block["text"]
    return ""

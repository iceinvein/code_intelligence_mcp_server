"""Anthropic SDK tool-use loop wrapped around a pluggable Toolbox."""
from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Any, Callable, Dict, List

from scripts.agent_qa.run_record import RunRecord, ToolCallRecord


SYSTEM_PROMPT = """You are an investigation agent answering a single question about a codebase.

Use the provided tools to find evidence in the code. Cite specific file paths (and
line numbers when possible) in your final answer. Stop searching once you have enough
to answer. Produce a final answer as a normal text message (no tool calls) when ready.

Be concise. A correct answer that names the right file and symbol with a one-paragraph
explanation is better than a long survey.""".strip()


ToolDispatch = Callable[[str, Dict[str, Any]], str]


@dataclass
class Toolbox:
    tool_defs: List[dict]
    dispatch: ToolDispatch


def _flatten_text(blocks: List[Any]) -> str:
    parts: List[str] = []
    for b in blocks:
        if getattr(b, "type", None) == "text":
            parts.append(b.text)
    return "\n".join(parts).strip()


def run_agent(
    client: Any,
    model: str,
    question: str,
    toolbox: Toolbox,
    max_tokens: int = 4096,
) -> RunRecord:
    messages: List[dict] = [{"role": "user", "content": question}]
    input_tokens = 0
    output_tokens = 0
    tool_calls: List[ToolCallRecord] = []
    final_answer = ""
    stop_reason = "unknown"
    started = time.time()

    while True:
        msg = client.messages.create(
            model=model,
            max_tokens=max_tokens,
            system=SYSTEM_PROMPT,
            tools=toolbox.tool_defs,
            messages=messages,
        )
        usage = getattr(msg, "usage", None)
        if usage is not None:
            input_tokens += getattr(usage, "input_tokens", 0) or 0
            output_tokens += getattr(usage, "output_tokens", 0) or 0
        stop_reason = getattr(msg, "stop_reason", "unknown")

        # Append assistant turn to message history (preserve tool_use blocks for the API).
        assistant_blocks: List[dict] = []
        tool_uses: List[Any] = []
        for b in msg.content:
            t = getattr(b, "type", None)
            if t == "text":
                assistant_blocks.append({"type": "text", "text": b.text})
            elif t == "tool_use":
                assistant_blocks.append(
                    {
                        "type": "tool_use",
                        "id": b.id,
                        "name": b.name,
                        "input": b.input,
                    }
                )
                tool_uses.append(b)
        messages.append({"role": "assistant", "content": assistant_blocks})

        if not tool_uses:
            final_answer = _flatten_text(msg.content)
            break

        # Dispatch each tool_use, append a single user message with all tool_results.
        tool_results: List[dict] = []
        for tu in tool_uses:
            t0 = time.time()
            try:
                result_text = toolbox.dispatch(tu.name, tu.input or {})
                is_error = False
            except Exception as e:  # tool error: report it back to the model
                result_text = f"tool error: {e}"
                is_error = True
            dur = int((time.time() - t0) * 1000)
            tool_calls.append(
                ToolCallRecord(
                    name=tu.name,
                    args=dict(tu.input or {}),
                    result_text=result_text,
                    result_bytes=len(result_text.encode("utf-8")),
                    duration_ms=dur,
                    is_error=is_error,
                )
            )
            tool_results.append(
                {
                    "type": "tool_result",
                    "tool_use_id": tu.id,
                    "content": result_text,
                    "is_error": is_error,
                }
            )
        messages.append({"role": "user", "content": tool_results})

    wall_ms = int((time.time() - started) * 1000)
    return RunRecord(
        question_id="",  # filled in by caller
        toolset="",
        model=model,
        repo="",
        final_answer=final_answer,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        wall_ms=wall_ms,
        stop_reason=stop_reason,
        tool_calls=tool_calls,
    )

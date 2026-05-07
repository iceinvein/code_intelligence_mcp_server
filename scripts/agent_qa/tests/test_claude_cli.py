"""Tests for the claude CLI subprocess wrapper.

We do not invoke `claude` itself in these tests; the wrapper is exercised by
feeding fixture event lines through `parse_stream`, which is the only piece
that depends on the CLI's output shape.
"""
from __future__ import annotations

import json
from pathlib import Path

from scripts.agent_qa.claude_cli import (
    ClaudeRunOptions,
    build_command,
    parse_stream,
)


def test_build_command_includes_all_required_flags(tmp_path: Path):
    opts = ClaudeRunOptions(
        question="Where is X?",
        system_prompt="be terse",
        allowed_tools=["Read", "Grep"],
        mcp_config={"mcpServers": {}},
        model="claude-sonnet-4-6",
        cwd=tmp_path,
    )
    cmd = build_command(opts)
    assert cmd[0] == "claude"
    # Required flags
    for flag in (
        "--print",
        "--output-format", "stream-json",
        "--no-session-persistence",
        "--disable-slash-commands",
        "--strict-mcp-config",
        "--system-prompt",
    ):
        assert flag in cmd
    # Allowed tools collapsed to one space-separated string
    i = cmd.index("--allowed-tools")
    assert cmd[i + 1] == "Read Grep"
    # MCP config encoded as JSON
    j = cmd.index("--mcp-config")
    assert json.loads(cmd[j + 1]) == {"mcpServers": {}}
    # Question is NOT in argv; it is fed via stdin to avoid being slurped by
    # the variadic --allowed-tools option.
    assert "Where is X?" not in cmd


def test_parse_stream_extracts_final_answer_and_tool_calls():
    """A representative stream-json sequence: a tool_use turn, then a final text turn."""
    events = [
        # turn 1: assistant emits a tool_use
        {
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Searching..."},
                    {
                        "type": "tool_use",
                        "id": "tu_1",
                        "name": "Grep",
                        "input": {"pattern": "alpha"},
                    },
                ],
                "usage": {"input_tokens": 1000, "output_tokens": 50},
            },
        },
        # turn 2: assistant emits final text only
        {
            "type": "assistant",
            "message": {
                "role": "assistant",
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Found in src/foo.rs:10."}],
                "usage": {"input_tokens": 1500, "output_tokens": 30},
            },
        },
        # terminal aggregate
        {"type": "result", "usage": {"input_tokens": 1500, "output_tokens": 80}},
    ]
    lines = [json.dumps(e) for e in events]
    parsed = parse_stream(lines)
    assert parsed.final_answer == "Found in src/foo.rs:10."
    assert parsed.stop_reason == "end_turn"
    assert parsed.input_tokens == 1500
    assert parsed.output_tokens == 80
    assert len(parsed.tool_calls) == 1
    assert parsed.tool_calls[0].name == "Grep"
    assert parsed.tool_calls[0].args == {"pattern": "alpha"}


def test_parse_stream_handles_multiple_tool_uses():
    events = [
        {
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "1", "name": "Read", "input": {"path": "a"}},
                    {"type": "tool_use", "id": "2", "name": "Read", "input": {"path": "b"}},
                ],
            },
        },
        {
            "type": "assistant",
            "message": {
                "role": "assistant",
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "done"}],
            },
        },
    ]
    parsed = parse_stream([json.dumps(e) for e in events])
    assert [tc.name for tc in parsed.tool_calls] == ["Read", "Read"]
    assert parsed.final_answer == "done"


def test_parse_stream_falls_back_to_result_field():
    """If the CLI emits a single-shot 'result' event, use its text as the answer."""
    events = [
        {"type": "result", "result": "single-shot answer", "usage": {"input_tokens": 200, "output_tokens": 10}},
    ]
    parsed = parse_stream([json.dumps(e) for e in events])
    assert parsed.final_answer == "single-shot answer"
    assert parsed.input_tokens == 200
    assert parsed.output_tokens == 10


def test_parse_stream_ignores_blank_and_invalid_lines():
    events = [
        "",
        "not json at all",
        json.dumps({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn"}}),
    ]
    parsed = parse_stream(events)
    assert parsed.final_answer == "ok"

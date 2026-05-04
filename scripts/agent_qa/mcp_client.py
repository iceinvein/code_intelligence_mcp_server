"""Stdio JSON-RPC client for the code-intelligence MCP server."""
from __future__ import annotations

import json
import os
import queue
import subprocess
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional


CI_TOOL_PREFIX = "ci_"  # disambiguates ci_grep vs default grep, ci_search_code, etc.


@dataclass
class McpToolCallResult:
    text: str
    is_error: bool
    raw_bytes: int


class McpStdioClient:
    """Minimal JSON-RPC 2.0 client over stdio for the local MCP server."""

    def __init__(self, binary: str, base_dir: Path, env_overrides: Optional[Dict[str, str]] = None):
        env = dict(os.environ)
        env["BASE_DIR"] = str(base_dir)
        if env_overrides:
            env.update(env_overrides)
        self.proc = subprocess.Popen(
            [binary],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            bufsize=0,
        )
        self._next_id = 1
        self._responses: "queue.Queue[dict]" = queue.Queue()
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()

    def _read_loop(self) -> None:
        assert self.proc.stdout is not None
        for raw in self.proc.stdout:
            line = raw.decode("utf-8", errors="replace").strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "id" in msg:
                self._responses.put(msg)

    def _request(self, method: str, params: Optional[dict] = None, timeout: float = 60.0) -> dict:
        rid = self._next_id
        self._next_id += 1
        envelope = {"jsonrpc": "2.0", "id": rid, "method": method}
        if params is not None:
            envelope["params"] = params
        assert self.proc.stdin is not None
        self.proc.stdin.write((json.dumps(envelope) + "\n").encode("utf-8"))
        self.proc.stdin.flush()
        # Drain until we see our id (other ids buffered for later).
        held: List[dict] = []
        while True:
            msg = self._responses.get(timeout=timeout)
            if msg.get("id") == rid:
                for h in held:
                    self._responses.put(h)
                if "error" in msg:
                    raise RuntimeError(f"MCP error on {method}: {msg['error']}")
                return msg.get("result", {})
            held.append(msg)

    def initialize(self) -> None:
        self._request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": "agent-qa-bench", "version": "0.1.0"},
                "capabilities": {},
            },
        )
        # initialized notification (no id, no response expected)
        assert self.proc.stdin is not None
        self.proc.stdin.write(
            (json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n").encode("utf-8")
        )
        self.proc.stdin.flush()

    def list_tools(self) -> List[dict]:
        result = self._request("tools/list")
        return list(result.get("tools", []))

    def call_tool(self, name: str, arguments: dict, timeout: float = 120.0) -> McpToolCallResult:
        result = self._request("tools/call", {"name": name, "arguments": arguments}, timeout=timeout)
        content = result.get("content", [])
        text = ""
        for chunk in content:
            if chunk.get("type") == "text":
                text += chunk.get("text", "")
        return McpToolCallResult(
            text=text,
            is_error=bool(result.get("isError")),
            raw_bytes=len(text.encode("utf-8")),
        )

    def close(self) -> None:
        try:
            self.proc.terminate()
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def to_anthropic_tool_defs(mcp_tools: List[dict], prefix: str = CI_TOOL_PREFIX) -> List[dict]:
    """Convert MCP tool definitions into Anthropic tool-use shape with a name prefix."""
    out: List[dict] = []
    for t in mcp_tools:
        schema = t.get("inputSchema")
        if not isinstance(schema, dict):
            continue
        out.append(
            {
                "name": f"{prefix}{t['name']}",
                "description": t.get("description", ""),
                "input_schema": schema,
            }
        )
    return out

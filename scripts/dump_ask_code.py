#!/usr/bin/env python3
"""Spawn a fresh daemon, bind a repo via ?repo=, run ask_code/investigate
for a single question, and dump the full JSON-RPC response to stdout.

Useful for diagnosing what the agent actually sees server-side (which
ask_code does NOT include in the bench's tool_calls capture).

Usage:
  .venv-bench/bin/python scripts/dump_ask_code.py \
      --base-dir /path/to/repo --question "..." [--tool ask_code|investigate]
"""
from __future__ import annotations

import argparse
import atexit
import json
import socket
import subprocess
import sys
import time
from pathlib import Path

import httpx

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BINARY = REPO_ROOT / "target" / "release" / "code-intelligence-mcp-server"


def _pick_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def _wait_for_port(port: int, proc: subprocess.Popen, timeout_s: float = 30.0) -> None:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"daemon exited early with code {proc.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.25):
                return
        except OSError:
            time.sleep(0.1)
    raise TimeoutError(f"daemon did not open port {port} within {timeout_s:.0f}s")


def _terminate(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()


def _start_daemon(binary: Path, port: int) -> subprocess.Popen:
    proc = subprocess.Popen(
        [str(binary), "--port", str(port), "--discovery-port", str(port + 1)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    atexit.register(_terminate, proc)
    _wait_for_port(port, proc)
    return proc


def _parse_sse_payload(text: str) -> dict | None:
    """Extract the first JSON-RPC payload from an SSE stream."""
    for line in text.splitlines():
        if line.startswith("data:"):
            data = line[5:].strip()
            if not data:
                continue
            try:
                return json.loads(data)
            except json.JSONDecodeError:
                pass
    return None


def _post_jsonrpc(client: httpx.Client, url: str, payload: dict, session_id: str | None) -> tuple[dict, str | None]:
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if session_id:
        headers["mcp-session-id"] = session_id
    resp = client.post(url, json=payload, headers=headers, timeout=120.0)
    resp.raise_for_status()
    new_session = resp.headers.get("mcp-session-id") or session_id
    ctype = resp.headers.get("content-type", "")
    body = resp.text
    if not body.strip():
        # Notifications return 202 with empty body.
        return {}, new_session
    if "event-stream" in ctype:
        parsed = _parse_sse_payload(body)
        if parsed is None:
            raise RuntimeError(f"could not parse SSE response: {body[:500]}")
        return parsed, new_session
    return resp.json(), new_session


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-dir", type=Path, required=True)
    parser.add_argument("--question", required=True)
    parser.add_argument("--tool", default="ask_code", choices=["ask_code", "investigate"])
    parser.add_argument("--target", default=None)
    parser.add_argument("--file-path", default=None)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--port", type=int, default=None)
    args = parser.parse_args()

    if not args.binary.is_file():
        sys.exit(f"binary not found: {args.binary} -- run cargo build --release")

    port = args.port or _pick_free_port()
    print(f"Starting daemon on :{port}", file=sys.stderr)
    proc = _start_daemon(args.binary, port)

    base_dir = args.base_dir.resolve()
    url = f"http://127.0.0.1:{port}/mcp?repo={base_dir}"

    with httpx.Client() as client:
        session_id: str | None = None

        init_payload = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "dump_ask_code", "version": "1.0"},
            },
        }
        init_resp, session_id = _post_jsonrpc(client, url, init_payload, session_id)
        if "result" not in init_resp:
            sys.exit(f"initialize failed: {init_resp}")

        _post_jsonrpc(
            client,
            url,
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            session_id,
        )

        time.sleep(1.5)

        tool_args: dict = {"question": args.question}
        if args.target:
            tool_args["target"] = args.target
        if args.file_path:
            tool_args["file_path"] = args.file_path

        call_payload = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": args.tool, "arguments": tool_args},
        }
        t0 = time.time()
        resp, _ = _post_jsonrpc(client, url, call_payload, session_id)
        elapsed = time.time() - t0
        print(f"latency: {elapsed:.1f}s", file=sys.stderr)
        print(json.dumps(resp, indent=2))


if __name__ == "__main__":
    main()

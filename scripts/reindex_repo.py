#!/usr/bin/env python3
"""Spawn a fresh daemon, bind a repo, trigger a synchronous refresh_index,
and poll get_index_stats until indexing settles. Use after editing the
TS extractor or any code path that changes what gets stored in SQLite /
Tantivy / LanceDB.

Usage:
  .venv-bench/bin/python scripts/reindex_repo.py --base-dir /path/to/repo
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
        proc.wait(timeout=10)
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
    for line in text.splitlines():
        if line.startswith("data:"):
            data = line[5:].strip()
            if data:
                try:
                    return json.loads(data)
                except json.JSONDecodeError:
                    pass
    return None


def _post(client: httpx.Client, url: str, payload: dict, session_id: str | None) -> tuple[dict, str | None]:
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if session_id:
        headers["mcp-session-id"] = session_id
    resp = client.post(url, json=payload, headers=headers, timeout=600.0)
    resp.raise_for_status()
    new_session = resp.headers.get("mcp-session-id") or session_id
    body = resp.text
    if not body.strip():
        return {}, new_session
    if "event-stream" in resp.headers.get("content-type", ""):
        parsed = _parse_sse_payload(body)
        if parsed is None:
            raise RuntimeError(f"could not parse SSE response: {body[:500]}")
        return parsed, new_session
    return resp.json(), new_session


def _unwrap_tool(envelope: dict) -> dict:
    """Unwrap MCP tools/call result envelope into the inner JSON."""
    res = envelope.get("result", {})
    content = res.get("content", [])
    if content and isinstance(content[0], dict) and content[0].get("type") == "text":
        return json.loads(content[0]["text"])
    return res


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-dir", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--force", action="store_true", help="full re-extract")
    parser.add_argument("--port", type=int, default=None)
    parser.add_argument(
        "--max-wait", type=float, default=600.0, help="seconds to wait for indexing"
    )
    parser.add_argument(
        "--poll-interval", type=float, default=2.0, help="seconds between stat polls"
    )
    args = parser.parse_args()

    if not args.binary.is_file():
        sys.exit(f"binary not found: {args.binary} -- run cargo build --release")

    port = args.port or _pick_free_port()
    print(f"Starting daemon on :{port}", file=sys.stderr, flush=True)
    _start_daemon(args.binary, port)

    base_dir = args.base_dir.resolve()
    url = f"http://127.0.0.1:{port}/mcp?repo={base_dir}"

    with httpx.Client() as client:
        sid: str | None = None
        init, sid = _post(
            client,
            url,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "reindex_repo", "version": "1.0"},
                },
            },
            sid,
        )
        if "result" not in init:
            sys.exit(f"initialize failed: {init}")
        _post(
            client,
            url,
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            sid,
        )

        print("Triggering refresh_index...", file=sys.stderr, flush=True)
        t0 = time.time()
        refresh, sid = _post(
            client,
            url,
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "refresh_index",
                    "arguments": {"force": bool(args.force)},
                },
            },
            sid,
        )
        refresh_inner = _unwrap_tool(refresh)
        print(
            f"refresh_index returned in {time.time()-t0:.1f}s: {json.dumps(refresh_inner)[:200]}",
            file=sys.stderr,
            flush=True,
        )

        # Poll get_index_stats until symbol count stabilises.
        last_symbols = -1
        stable_polls = 0
        deadline = time.time() + args.max_wait
        while time.time() < deadline:
            stats, sid = _post(
                client,
                url,
                {
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {"name": "get_index_stats", "arguments": {}},
                },
                sid,
            )
            inner = _unwrap_tool(stats)
            symbols = inner.get("symbols") or inner.get("total_symbols") or 0
            if isinstance(symbols, dict):
                symbols = sum(symbols.values()) if symbols else 0
            print(
                f"  [{time.time()-t0:5.1f}s] symbols={symbols}, files={inner.get('files') or inner.get('total_files')}",
                file=sys.stderr,
                flush=True,
            )
            if symbols > 0 and symbols == last_symbols:
                stable_polls += 1
                if stable_polls >= 3:
                    print(
                        f"Index stable at {symbols} symbols after {time.time()-t0:.1f}s",
                        file=sys.stderr,
                    )
                    print(json.dumps(inner, indent=2))
                    return
            else:
                stable_polls = 0
            last_symbols = symbols
            time.sleep(args.poll_interval)

        print(
            f"WARNING: index did not stabilise within {args.max_wait}s -- continuing with current state",
            file=sys.stderr,
        )


if __name__ == "__main__":
    main()

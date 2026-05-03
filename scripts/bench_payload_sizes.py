#!/usr/bin/env python3
"""Benchmark MCP tool response sizes: published @iceinvein/code-intelligence-mcp
vs the local target/release binary.

Indexes this repository under both servers, sends identical tool requests,
and prints byte-size deltas per tool.

Usage:
    python3 scripts/bench_payload_sizes.py
"""
import json
import os
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time

REPO_ROOT = os.path.abspath(os.path.dirname(os.path.dirname(__file__)))
RELEASE_BINARY = os.path.join(REPO_ROOT, "target/release/code-intelligence-mcp-server")
DEBUG_BINARY = os.path.join(REPO_ROOT, "target/debug/code-intelligence-mcp-server")
LOCAL_BINARY = RELEASE_BINARY if os.path.isfile(RELEASE_BINARY) else DEBUG_BINARY
NPX_BEFORE = ["npx", "-y", "@iceinvein/code-intelligence-mcp"]


class McpClient:
    def __init__(self, label, cmd, env):
        self.label = label
        self.proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            bufsize=0,
        )
        self._raw_responses = {}  # id -> raw_text (the JSON-RPC envelope)
        self._tool_payloads = {}  # id -> raw_text inside content[0].text
        self._lock = threading.Lock()
        self._stderr_lines = []
        self._t_out = threading.Thread(target=self._read_stdout, daemon=True)
        self._t_err = threading.Thread(target=self._read_stderr, daemon=True)
        self._t_out.start()
        self._t_err.start()
        self._next_id = 1

    def _read_stdout(self):
        for raw in self.proc.stdout:
            line = raw.decode("utf-8", errors="replace").rstrip("\n")
            if not line.strip():
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            mid = msg.get("id")
            if mid is None:
                continue
            inner = ""
            result = msg.get("result")
            if isinstance(result, dict):
                content = result.get("content") or []
                if content and isinstance(content[0], dict):
                    inner = content[0].get("text", "")
            with self._lock:
                self._raw_responses[mid] = line
                self._tool_payloads[mid] = inner

    def _read_stderr(self):
        for raw in self.proc.stderr:
            self._stderr_lines.append(raw.decode("utf-8", errors="replace").rstrip("\n"))

    def initialize(self):
        self._send({
            "jsonrpc": "2.0",
            "id": self._claim_id(),
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "bench", "version": "1"},
            },
        })

    def _send(self, msg):
        self.proc.stdin.write((json.dumps(msg) + "\n").encode("utf-8"))
        self.proc.stdin.flush()

    def _claim_id(self):
        i = self._next_id
        self._next_id += 1
        return i

    def call_tool(self, name, arguments, timeout=120):
        mid = self._claim_id()
        self._send({
            "jsonrpc": "2.0",
            "id": mid,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        deadline = time.time() + timeout
        while time.time() < deadline:
            with self._lock:
                if mid in self._tool_payloads:
                    return self._raw_responses[mid], self._tool_payloads[mid]
            time.sleep(0.05)
        return None, None

    def close(self):
        try:
            self.proc.terminate()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


def make_env(data_dir):
    env = os.environ.copy()
    env["BASE_DIR"] = REPO_ROOT
    env["EMBEDDINGS_BACKEND"] = "hash"
    env["LLM_ENABLED"] = "false"
    env["WATCH_MODE"] = "true"
    env["DB_PATH"] = os.path.join(data_dir, "code-intelligence.db")
    env["VECTOR_DB_PATH"] = os.path.join(data_dir, "vectors")
    env["TANTIVY_INDEX_PATH"] = os.path.join(data_dir, "tantivy-index")
    env["RUST_LOG"] = "warn"
    return env


def wait_for_index(client, label, max_wait=300, stable=15):
    print(f"[{label}] waiting for indexing...", flush=True)
    last = -1
    stable_since = None
    start = time.time()
    while time.time() - start < max_wait:
        _, payload = client.call_tool("get_index_stats", {}, timeout=20)
        if payload:
            try:
                data = json.loads(payload)
                count = data.get("symbols", data.get("symbol_count", 0))
            except Exception:
                count = 0
        else:
            count = 0
        if count != last:
            print(f"[{label}]   symbols={count}", flush=True)
            last = count
            stable_since = time.time()
        elif stable_since and time.time() - stable_since >= stable and count > 0:
            print(f"[{label}] stable at {count} symbols", flush=True)
            return count
        time.sleep(2)
    print(f"[{label}] WARN: timeout at {last} symbols", flush=True)
    return last


CASES = [
    ("search_code (limit=5)", "search_code", {"query": "ranking and scoring", "limit": 5}),
    ("search_code (limit=20)", "search_code", {"query": "ranking and scoring", "limit": 20}),
    ("get_definition", "get_definition", {"symbol_name": "PathNormalizer"}),
    ("find_references (limit=200)", "find_references", {"symbol_name": "search", "limit": 200}),
    ("get_call_hierarchy (depth=3 limit=100)", "get_call_hierarchy",
     {"symbol_name": "build_dependency_graph", "depth": 3, "limit": 100}),
    ("get_type_graph (both, limit=100)", "get_type_graph",
     {"symbol_name": "SymbolRow", "depth": 3, "limit": 100, "direction": "both"}),
    ("explore_dependency_graph (limit=200)", "explore_dependency_graph",
     {"symbol_name": "Retriever", "depth": 2, "limit": 200}),
    ("find_dead_code (limit=100)", "find_dead_code", {"limit": 100}),
    ("find_affected_code (limit=100)", "find_affected_code",
     {"symbol_name": "search", "limit": 100, "depth": 3}),
    ("get_module_summary (grouped)", "get_module_summary",
     {"file_path": "src/handlers/navigation.rs", "group_by_kind": True}),
    ("get_module_summary (flat)", "get_module_summary",
     {"file_path": "src/handlers/navigation.rs", "group_by_kind": False}),
    ("get_file_symbols", "get_file_symbols", {"file_path": "src/handlers/navigation.rs"}),
    ("hydrate_symbols (default)", "hydrate_symbols", {"ids": []}),
]


def fmt_pct(before, after):
    if before == 0:
        return "n/a"
    return f"{(after - before) * 100.0 / before:+.1f}%"


def main():
    if not os.path.isfile(LOCAL_BINARY):
        sys.exit(f"local release binary missing: {LOCAL_BINARY}")

    before_dir = tempfile.mkdtemp(prefix="cimcp-before-")
    after_dir = tempfile.mkdtemp(prefix="cimcp-after-")
    print(f"before data: {before_dir}", flush=True)
    print(f"after  data: {after_dir}", flush=True)

    before = McpClient("before", NPX_BEFORE, make_env(before_dir))
    after = McpClient("after", [LOCAL_BINARY], make_env(after_dir))

    try:
        before.initialize()
        after.initialize()
        time.sleep(2)
        # Trigger a fresh index in each server (data dirs are tmp).
        print("[before] triggering refresh_index", flush=True)
        before.call_tool("refresh_index", {}, timeout=180)
        print("[after] triggering refresh_index", flush=True)
        after.call_tool("refresh_index", {}, timeout=180)
        wait_for_index(before, "before")
        wait_for_index(after, "after")

        print(flush=True)
        # Resolve a real symbol id once we know indexing is done, for hydrate_symbols.
        _, payload = after.call_tool("search_code", {"query": "PathNormalizer", "limit": 3})
        sample_ids = []
        try:
            data = json.loads(payload)
            sample_ids = [h.get("id") for h in data.get("hits", []) if h.get("id")]
        except Exception:
            pass
        if sample_ids:
            for case in CASES:
                if case[1] == "hydrate_symbols":
                    case[2]["ids"] = sample_ids[:3]

        rows = []
        for label, tool, args in CASES:
            print(f"[run] {label}", flush=True)
            b_envelope, b_payload = before.call_tool(tool, dict(args), timeout=120)
            a_envelope, a_payload = after.call_tool(tool, dict(args), timeout=120)
            b_size = len(b_payload) if b_payload else 0
            a_size = len(a_payload) if a_payload else 0
            rows.append((label, b_size, a_size))

        print()
        print(f"{'tool':<46} {'before':>10} {'after':>10} {'delta':>12} {'pct':>8}")
        print("-" * 90)
        total_before = 0
        total_after = 0
        for label, b, a in rows:
            total_before += b
            total_after += a
            print(f"{label:<46} {b:>10,} {a:>10,} {a - b:>+12,} {fmt_pct(b, a):>8}")
        print("-" * 90)
        print(f"{'TOTAL':<46} {total_before:>10,} {total_after:>10,} {total_after - total_before:>+12,} {fmt_pct(total_before, total_after):>8}")
    finally:
        before.close()
        after.close()
        shutil.rmtree(before_dir, ignore_errors=True)
        shutil.rmtree(after_dir, ignore_errors=True)


if __name__ == "__main__":
    main()

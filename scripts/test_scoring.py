#!/usr/bin/env python3
"""Test search scoring by spawning a temporary MCP server and querying it via stdio.

Usage:
    python3 scripts/test_scoring.py [query] [limit]

Requires: cargo build --release
Spawns a fresh server with isolated data, indexes the repo, then runs the query.
"""
import json
import subprocess
import sys
import os
import time
import threading
import queue
import tempfile
import shutil

# Force unbuffered output
sys.stdout = os.fdopen(sys.stdout.fileno(), 'w', buffering=1)

BINARY = "./target/release/code-intelligence-mcp-server"
BASE_DIR = os.getcwd()
QUERY = sys.argv[1] if len(sys.argv) > 1 else "PathNormalizer struct definition and methods"
LIMIT = int(sys.argv[2]) if len(sys.argv) > 2 else 10


class McpClient:
    """Simple MCP client over stdio with non-blocking reads."""

    def __init__(self, binary, env):
        self.proc = subprocess.Popen(
            [binary],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            bufsize=0,
        )
        self._response_queue = queue.Queue()
        self._stderr_lines = []
        self._stdout_thread = threading.Thread(target=self._read_stdout, daemon=True)
        self._stdout_thread.start()
        self._stderr_thread = threading.Thread(target=self._read_stderr, daemon=True)
        self._stderr_thread.start()

    def _read_stdout(self):
        try:
            while True:
                line = self.proc.stdout.readline()
                if not line:
                    break
                line = line.decode("utf-8", errors="replace").strip()
                if line:
                    try:
                        msg = json.loads(line)
                        self._response_queue.put(msg)
                    except json.JSONDecodeError:
                        pass
        except Exception:
            pass

    def _read_stderr(self):
        try:
            while True:
                line = self.proc.stderr.readline()
                if not line:
                    break
                line = line.decode("utf-8", errors="replace").strip()
                if line:
                    self._stderr_lines.append(line)
        except Exception:
            pass

    def send(self, msg_dict):
        data = json.dumps(msg_dict) + "\n"
        self.proc.stdin.write(data.encode("utf-8"))
        self.proc.stdin.flush()

    def recv(self, timeout=120, expected_id=None):
        deadline = time.time() + timeout
        orphans = []
        try:
            while time.time() < deadline:
                remaining = max(0.1, deadline - time.time())
                try:
                    msg = self._response_queue.get(timeout=remaining)
                except queue.Empty:
                    break
                if expected_id is not None and msg.get("id") != expected_id:
                    orphans.append(msg)
                    continue
                return msg
        finally:
            for o in orphans:
                self._response_queue.put(o)
        return None

    def get_stderr(self, max_lines=20):
        return self._stderr_lines[-max_lines:]

    def close(self):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def call_tool(client, tool_name, arguments, msg_id, timeout=120):
    """Call an MCP tool and return parsed result content."""
    client.send({
        "jsonrpc": "2.0", "id": msg_id,
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": arguments}
    })
    resp = client.recv(timeout=timeout, expected_id=msg_id)
    if not resp:
        print(f"  [{tool_name}] No response after {timeout}s", flush=True)
        return None
    result = resp.get("result")
    if result is None:
        error = resp.get("error", {})
        print(f"  [{tool_name}] Error: {json.dumps(error)[:500]}", flush=True)
        return None
    content = result.get("content", [])
    if not content:
        return {}
    text = content[0].get("text", "")
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return {"_raw_text": text}


def get_symbol_count(client, msg_id):
    stats = call_tool(client, "get_index_stats", {}, msg_id, timeout=10)
    if stats:
        return stats.get("symbols", 0)
    return 0


def wait_for_indexing(client, start_msg_id=100, max_wait=600, stable_seconds=20):
    msg_id = start_msg_id
    last_count = 0
    stable_since = None
    start = time.time()

    while time.time() - start < max_wait:
        msg_id += 1
        count = get_symbol_count(client, msg_id)
        elapsed = int(time.time() - start)

        if count != last_count:
            print(f"  [{elapsed}s] symbols={count} (was {last_count})", flush=True)
            last_count = count
            stable_since = time.time()
        elif stable_since and (time.time() - stable_since) >= stable_seconds:
            print(f"  [{elapsed}s] symbols={count} — stable for {stable_seconds}s, done", flush=True)
            return count

        time.sleep(2)

    print(f"  WARNING: Timed out after {max_wait}s with {last_count} symbols", flush=True)
    return last_count


def main():
    if not os.path.exists(BINARY):
        print("ERROR: Binary not found. Run: cargo build --release", flush=True)
        sys.exit(1)

    data_dir = tempfile.mkdtemp(prefix="cimcp_test_")
    print(f"Data dir: {data_dir}", flush=True)

    env = os.environ.copy()
    env["BASE_DIR"] = BASE_DIR
    env["WATCH_MODE"] = "false"
    env["EMBEDDINGS_BACKEND"] = "hash"
    env["METRICS_ENABLED"] = "false"
    env["DB_PATH"] = os.path.join(data_dir, "code-intelligence.db")
    env["VECTOR_DB_PATH"] = os.path.join(data_dir, "vectors")
    env["TANTIVY_INDEX_PATH"] = os.path.join(data_dir, "tantivy-index")

    print(f"Starting server (BASE_DIR={BASE_DIR})", flush=True)
    client = McpClient(BINARY, env)

    try:
        # Initialize MCP
        client.send({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"}
            }
        })
        resp = client.recv(timeout=60)
        if not resp:
            print("ERROR: No init response", flush=True)
            return
        print("Initialized OK", flush=True)

        client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        time.sleep(1)

        # Index
        print("Triggering refresh_index...", flush=True)
        call_tool(client, "refresh_index", {}, 50, timeout=30)
        print("Waiting for indexing...", flush=True)
        symbol_count = wait_for_indexing(client, start_msg_id=100, stable_seconds=15)
        if symbol_count < 100:
            print(f"ERROR: Only {symbol_count} symbols indexed", flush=True)
            return

        # Search
        print(f"\n{'='*70}", flush=True)
        print(f"Query: {QUERY}  (limit={LIMIT})", flush=True)
        print(f"{'='*70}", flush=True)

        data = call_tool(client, "explain_search",
                         {"query": QUERY, "limit": LIMIT, "verbose": True},
                         999, timeout=120)
        if not data:
            print("ERROR: No response from explain_search", flush=True)
            return

        # Handle both explain_search format (results) and search_code format (hits)
        results = data.get("results", [])
        if not results and "hits" in data:
            # Fallback: search_code response format
            results = [{"symbol_name": h.get("name", "?"),
                        "kind": h.get("kind", "?"),
                        "score": h.get("score", 0),
                        "file_path": h.get("file_path", "")}
                       for h in data["hits"]]

        if not results:
            print("No results found.", flush=True)
            print(f"  Response keys: {list(data.keys())}", flush=True)
        else:
            print(f"\n{'#':<4} {'Symbol':<45} {'Kind':<12} {'Score':>8} {'Intent':>7} {'TP':>7}  File", flush=True)
            print("-" * 110, flush=True)
            for i, r in enumerate(results):
                name = r.get("symbol_name", "?")
                kind = r.get("kind", "?")
                score = r.get("score", 0)
                bd = r.get("score_breakdown", {})
                intent = bd.get("intent_multiplier", "-")
                tp = bd.get("test_symbol_penalty", "-")
                fp = r.get("file_path", "")
                fp_short = fp.rsplit("/", 1)[-1] if "/" in fp else fp
                intent_s = f"{intent:.3f}" if isinstance(intent, (int, float)) else intent
                tp_s = f"{tp:.1f}" if isinstance(tp, (int, float)) else tp
                print(f"{i+1:<4} {name:<45} {kind:<12} {score:>8.2f} {intent_s:>7} {tp_s:>7}  {fp_short}", flush=True)

        # Context preview
        ctx = data.get("context", "")
        if ctx:
            ctx_lines = ctx.strip().split("\n")
            print(f"\nContext ({len(ctx_lines)} lines):", flush=True)
            for line in ctx_lines[:8]:
                print(f"  {line}", flush=True)
            if len(ctx_lines) > 8:
                print(f"  ... ({len(ctx_lines) - 8} more)", flush=True)

    finally:
        client.close()
        try:
            shutil.rmtree(data_dir)
            print(f"\nCleaned up {data_dir}", flush=True)
        except Exception:
            pass


if __name__ == "__main__":
    main()

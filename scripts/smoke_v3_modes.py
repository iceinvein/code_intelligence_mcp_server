#!/usr/bin/env python3
"""Smoke-test the v3 search_code context modes against a fresh local index.

For each realistic NL query, runs three search_code variants
(`context="none"`, `"snippets"`, `"full"`) and reports:

  - response bytes per mode
  - top-3 hit names + paths (sanity-check that ranking is identical)
  - the cost of a realistic agent loop:
      none + hydrate_symbols(top_3)    vs    full

Run:
    python3 scripts/smoke_v3_modes.py
"""
import json
import os
import queue
import shutil
import subprocess
import tempfile
import threading
import time

REPO = os.path.abspath(os.path.dirname(os.path.dirname(__file__)))
BINARY = os.path.join(REPO, "target/release/code-intelligence-mcp-server")

QUERIES = [
    "how does ranking and scoring work",
    "PathNormalizer struct definition",
    "error handling and graceful degradation",
    "vector embedding generation",
    "MCP tool dispatch",
]


class McpClient:
    def __init__(self, env):
        self.proc = subprocess.Popen(
            [BINARY],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            bufsize=0,
        )
        self._raw = {}
        self._payload = {}
        self._lock = threading.Lock()
        self._next = 1
        threading.Thread(target=self._reader, daemon=True).start()
        threading.Thread(target=self._stderr, daemon=True).start()

    def _reader(self):
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
            r = msg.get("result")
            if isinstance(r, dict):
                content = r.get("content") or []
                if content and isinstance(content[0], dict):
                    inner = content[0].get("text", "")
            with self._lock:
                self._raw[mid] = line
                self._payload[mid] = inner

    def _stderr(self):
        for _ in self.proc.stderr:
            pass

    def call(self, name, args, timeout=120):
        with self._lock:
            mid = self._next
            self._next += 1
        self.proc.stdin.write((json.dumps({
            "jsonrpc": "2.0", "id": mid,
            "method": "tools/call",
            "params": {"name": name, "arguments": args},
        }) + "\n").encode())
        self.proc.stdin.flush()
        deadline = time.time() + timeout
        while time.time() < deadline:
            with self._lock:
                if mid in self._payload:
                    return self._payload[mid]
            time.sleep(0.05)
        return None

    def initialize(self):
        with self._lock:
            mid = self._next
            self._next += 1
        self.proc.stdin.write((json.dumps({
            "jsonrpc": "2.0", "id": mid,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                       "clientInfo": {"name": "smoke", "version": "1"}},
        }) + "\n").encode())
        self.proc.stdin.flush()
        time.sleep(0.5)

    def close(self):
        try:
            self.proc.terminate()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


def main():
    if not os.path.isfile(BINARY):
        raise SystemExit(f"missing {BINARY}; run cargo build --release")

    data = tempfile.mkdtemp(prefix="smoke-v3-")
    env = os.environ.copy()
    env.update({
        "BASE_DIR": REPO,
        "EMBEDDINGS_BACKEND": "hash",
        "LLM_ENABLED": "false",
        "WATCH_MODE": "true",
        "DB_PATH": f"{data}/code-intelligence.db",
        "VECTOR_DB_PATH": f"{data}/vectors",
        "TANTIVY_INDEX_PATH": f"{data}/tantivy-index",
        "RUST_LOG": "warn",
    })

    client = McpClient(env)
    try:
        client.initialize()
        print("Triggering refresh_index...", flush=True)
        client.call("refresh_index", {}, timeout=180)
        # Wait for index to settle
        prev = -1
        stable_at = None
        for _ in range(60):
            stats_raw = client.call("get_index_stats", {}, timeout=20)
            try:
                stats = json.loads(stats_raw) if stats_raw else {}
                count = stats.get("symbols", 0)
            except Exception:
                count = 0
            if count != prev:
                prev = count
                stable_at = time.time()
            elif stable_at and time.time() - stable_at >= 10 and prev > 0:
                break
            time.sleep(1)
        print(f"Indexed {prev} symbols\n", flush=True)

        per_query_summary = []

        for query in QUERIES:
            print(f"=== {query!r} ===", flush=True)
            results_by_mode = {}
            for mode in ["none", "snippets", "full"]:
                args = {"query": query, "limit": 5}
                if mode != "none":
                    args["context"] = mode
                payload = client.call("search_code", args, timeout=120)
                size = len(payload) if payload else 0
                data_obj = {}
                try:
                    data_obj = json.loads(payload) if payload else {}
                except Exception:
                    pass
                hits = data_obj.get("hits", []) or []
                top3 = [(h.get("name", "?"), h.get("file_path", "?")) for h in hits[:3]]
                has_snippets = any("snippet" in h for h in hits)
                has_context = "context" in data_obj
                results_by_mode[mode] = {
                    "size": size,
                    "top3": top3,
                    "ids": [h.get("id") for h in hits[:3] if h.get("id")],
                    "has_snippets": has_snippets,
                    "has_context": has_context,
                }
                print(f"  mode={mode:<9}  bytes={size:>6}  has_context={has_context}  has_snippets={has_snippets}", flush=True)
                for n, p in top3:
                    print(f"    - {n}  ({p})", flush=True)

            # Ranking parity check
            top_none = [t[0] for t in results_by_mode["none"]["top3"]]
            top_snippets = [t[0] for t in results_by_mode["snippets"]["top3"]]
            top_full = [t[0] for t in results_by_mode["full"]["top3"]]
            parity = top_none == top_snippets == top_full
            print(f"  ranking parity (top3 names match across all modes): {parity}", flush=True)

            # Realistic agent loop cost
            none_size = results_by_mode["none"]["size"]
            full_size = results_by_mode["full"]["size"]
            snippets_size = results_by_mode["snippets"]["size"]
            ids = results_by_mode["none"]["ids"]
            hydrate_size = 0
            if ids:
                hydrate_payload = client.call(
                    "hydrate_symbols", {"ids": ids, "mode": "default"}, timeout=60
                )
                hydrate_size = len(hydrate_payload) if hydrate_payload else 0
            none_plus_hydrate = none_size + hydrate_size
            print(
                f"  agent loop cost: none+hydrate={none_size}+{hydrate_size}={none_plus_hydrate}  "
                f"snippets={snippets_size}  full={full_size}",
                flush=True,
            )
            per_query_summary.append({
                "query": query,
                "parity": parity,
                "none": none_size,
                "snippets": snippets_size,
                "full": full_size,
                "none_plus_hydrate": none_plus_hydrate,
            })
            print(flush=True)

        print("=" * 90)
        print(f"{'query':<45} {'none':>8} {'+hyd':>8} {'snip':>8} {'full':>8} {'parity':>8}")
        print("-" * 90)
        for r in per_query_summary:
            q = r["query"]
            q_short = q if len(q) <= 44 else q[:41] + "..."
            print(f"{q_short:<45} {r['none']:>8} {r['none_plus_hydrate']:>8} "
                  f"{r['snippets']:>8} {r['full']:>8} {str(r['parity']):>8}")
        print("-" * 90)
        totals = {k: sum(r[k] for r in per_query_summary)
                  for k in ["none", "none_plus_hydrate", "snippets", "full"]}
        print(f"{'TOTAL':<45} {totals['none']:>8} {totals['none_plus_hydrate']:>8} "
              f"{totals['snippets']:>8} {totals['full']:>8}")
    finally:
        client.close()
        shutil.rmtree(data, ignore_errors=True)


if __name__ == "__main__":
    main()

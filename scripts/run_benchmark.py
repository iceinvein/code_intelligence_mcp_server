#!/usr/bin/env python3
"""Run the full 15-query search quality benchmark against the MCP server.

Spawns a fresh server instance, indexes the codebase, runs all queries,
and writes structured results for agent evaluation.

Usage:
    python3 scripts/run_benchmark.py                    # Fresh mode (hash embeddings, temp dirs)
    python3 scripts/run_benchmark.py --live             # Live mode (real embeddings + LLM descriptions)
    python3 scripts/run_benchmark.py --round 46         # Set round number
    python3 scripts/run_benchmark.py --queries 1,3,9    # Run specific queries only
    python3 scripts/run_benchmark.py --output results.md # Custom output file

Modes:
    Fresh (default): Temp data dirs, hash embeddings, full re-index. Fast (~60s) but BM25-only.
    Live (--live):   Uses existing .cimcp/ data with real embeddings + LLM descriptions.
                     Comparable to agent-based benchmark rounds. Requires no running MCP server.

Output: docs/benchmark_rounds/round_N_results.md
"""
import json
import subprocess
import signal
import sys
import os
import time
import threading
import queue
import tempfile
import shutil
import argparse
from datetime import datetime

# Force unbuffered output
sys.stdout = os.fdopen(sys.stdout.fileno(), 'w', buffering=1)

BINARY = "./target/release/code-intelligence-mcp-server"
BASE_DIR = os.getcwd()

# The 15 standard benchmark queries (from docs/SEARCH_BENCHMARK.md)
QUERIES = [
    {"id": 1,  "query": "How does the ranking and scoring system work?",
     "expected": "retrieval/ranking/score.rs, retrieval/ranking/mod.rs, retrieval/ranking/diversify.rs, retrieval/ranking/rrf.rs"},
    {"id": 2,  "query": "How are embeddings generated and stored?",
     "expected": "storage/vector.rs, embedding backend files, storage/ layer"},
    {"id": 3,  "query": "How does tree-sitter parsing work in this codebase?",
     "expected": "indexer/parser.rs, indexer/extract/ language extractors"},
    {"id": 4,  "query": "Configuration from environment variables",
     "expected": "Config/settings module, main entry point with env var reads"},
    {"id": 5,  "query": "Indexing pipeline file scanning and symbol extraction",
     "expected": "indexer/mod.rs, indexer/extract/mod.rs, file scanner, symbol types"},
    {"id": 6,  "query": "How does the MCP server handle incoming tool requests?",
     "expected": "server/mod.rs, handlers/mod.rs, tool dispatch/routing logic"},
    {"id": 7,  "query": "How does the WebSocket handler work?",
     "expected": "WebSocket-related handler code, connection management"},
    {"id": 8,  "query": "SQLite database schema tables initialization",
     "expected": "storage/sqlite/ schema definitions, migration/init code"},
    {"id": 9,  "query": "Error handling and graceful degradation",
     "expected": "Error types, fallback logic across multiple modules"},
    {"id": 10, "query": "JSON serialization and response formatting",
     "expected": "Serde derive usage, response builders, MCP protocol formatting"},
    {"id": 11, "query": "Async concurrency and parallel processing",
     "expected": "Async mutex usage, parallel indexing, concurrent operations"},
    {"id": 12, "query": "Caching and cache invalidation",
     "expected": "retrieval/cache.rs, embedding cache, TTL/invalidation logic"},
    {"id": 13, "query": "PathNormalizer struct definition and methods",
     "expected": "path/mod.rs -- the struct and its impl block"},
    {"id": 14, "query": "EmbeddingCache get put cached embedding",
     "expected": "The cache struct and its get/put methods"},
    {"id": 15, "query": "File watcher debounce reindex on change",
     "expected": "Watcher module, debounce logic"},
]


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
        return None
    result = resp.get("result")
    if result is None:
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
            print(f"  [{elapsed}s] symbols={count} -- stable for {stable_seconds}s, done", flush=True)
            return count

        time.sleep(2)

    print(f"  WARNING: Timed out after {max_wait}s with {last_count} symbols", flush=True)
    return last_count


def run_query(client, query_text, msg_id, limit=10):
    """Run a single search query and return structured results."""
    data = call_tool(client, "explain_search",
                     {"query": query_text, "limit": limit, "verbose": True},
                     msg_id, timeout=120)
    if not data:
        return []

    results = data.get("results", [])
    if not results and "hits" in data:
        results = [{"symbol_name": h.get("name", "?"),
                     "kind": h.get("kind", "?"),
                     "score": h.get("score", 0),
                     "file_path": h.get("file_path", "")}
                    for h in data["hits"]]

    parsed = []
    for r in results[:limit]:
        fp = r.get("file_path", "")
        fp_short = fp.split("/src/", 1)[-1] if "/src/" in fp else fp.rsplit("/", 2)[-2:]
        if isinstance(fp_short, list):
            fp_short = "/".join(fp_short)
        bd = r.get("score_breakdown", {})
        parsed.append({
            "name": r.get("symbol_name", "?"),
            "kind": r.get("kind", "?"),
            "score": round(r.get("score", 0), 2),
            "file": fp_short,
            "file_full": fp,
            "keyword_score": bd.get("keyword_score"),
            "vector_score": bd.get("vector_score"),
            "base_score": bd.get("base_score"),
            "intent_mult": bd.get("intent_multiplier"),
            "test_penalty": bd.get("test_symbol_penalty"),
        })

    return parsed


def format_results_markdown(round_num, all_results, elapsed_total, symbol_count):
    """Format results as markdown for agent evaluation."""
    lines = []
    lines.append(f"# Round {round_num} - Raw Results")
    lines.append(f"")
    lines.append(f"Generated: {datetime.now().strftime('%Y-%m-%d %H:%M')}")
    lines.append(f"Symbols indexed: {symbol_count}")
    lines.append(f"Total time: {elapsed_total:.0f}s")
    lines.append(f"")
    lines.append(f"## Results Summary")
    lines.append(f"")

    for qr in all_results:
        q = qr["query"]
        results = qr["results"]
        lines.append(f"### Q{q['id']}: \"{q['query']}\"")
        lines.append(f"**Expected:** {q['expected']}")
        lines.append(f"")

        if not results:
            lines.append(f"*No results returned*")
        else:
            lines.append(f"| # | Symbol | Kind | Score | File |")
            lines.append(f"|---|--------|------|-------|------|")
            for i, r in enumerate(results[:5]):
                lines.append(f"| {i+1} | {r['name']} | {r['kind']} | {r['score']} | {r['file']} |")

        lines.append(f"")

    # Evaluation template for the agent
    lines.append(f"## Scoring Template")
    lines.append(f"")
    lines.append(f"Score each query's CI results on a 1-10 scale:")
    lines.append(f"- 9-10: Every result directly answers the query, spans relevant files")
    lines.append(f"- 7-8: Most results relevant, good diversity, top 3-5 strong")
    lines.append(f"- 5-6: ~Half relevant, some gaps, core code present but buried")
    lines.append(f"- 3-4: Few relevant, dominated by 1-2 files, test/re-export noise")
    lines.append(f"- 1-2: Mostly irrelevant, core implementation missing")
    lines.append(f"")
    lines.append(f"| # | Query | CI Score | Pattern |")
    lines.append(f"|---|-------|----------|---------|")
    for qr in all_results:
        q = qr["query"]
        short = q["query"][:50]
        lines.append(f"| {q['id']} | {short} | ___ | |")
    lines.append(f"")
    lines.append(f"**CI Average:** ___")

    return "\n".join(lines)


def check_tantivy_lock(tantivy_dir):
    """Check if another server holds the Tantivy writer lock."""
    lock_file = os.path.join(tantivy_dir, ".tantivy-writer.lock")
    if not os.path.exists(lock_file):
        return True
    # The lock file exists but our server deletes it on startup anyway.
    # Check if another MCP server process is running that might conflict.
    try:
        result = subprocess.run(
            ["pgrep", "-f", "code-intelligence-mcp-server"],
            capture_output=True, text=True, timeout=5
        )
        if result.stdout.strip():
            pids = result.stdout.strip().split("\n")
            print(f"  WARNING: Found {len(pids)} running MCP server process(es): {', '.join(pids)}", flush=True)
            print(f"  Live mode may conflict with a running server's Tantivy writer.", flush=True)
            print(f"  Consider stopping other servers first (or they may lose index writes).", flush=True)
            return False
    except Exception:
        pass
    return True


def main():
    parser = argparse.ArgumentParser(description="Run search quality benchmark")
    parser.add_argument("--round", type=int, default=0, help="Round number (0=auto)")
    parser.add_argument("--queries", type=str, default="", help="Comma-separated query IDs (default=all)")
    parser.add_argument("--output", type=str, default="", help="Output file path")
    parser.add_argument("--limit", type=int, default=5, help="Results per query (default=5)")
    parser.add_argument("--live", action="store_true",
                        help="Use existing .cimcp/ data (real embeddings + LLM descriptions)")
    args = parser.parse_args()

    if not os.path.exists(BINARY):
        print("ERROR: Binary not found. Run: cargo build --release", flush=True)
        sys.exit(1)

    # Determine round number
    round_num = args.round
    if round_num == 0:
        import glob
        existing = glob.glob("docs/benchmark_rounds/round_*_results.md")
        round_num = max([int(f.split("round_")[1].split("_")[0]) for f in existing] + [0]) + 1

    # Filter queries
    if args.queries:
        query_ids = set(int(q) for q in args.queries.split(","))
        queries = [q for q in QUERIES if q["id"] in query_ids]
    else:
        queries = QUERIES

    output_file = args.output or f"docs/benchmark_rounds/round_{round_num}_results.md"
    mode = "live" if args.live else "fresh"

    print(f"=== Benchmark Round {round_num} ({mode} mode) ===", flush=True)
    print(f"Queries: {len(queries)} ({', '.join(f'Q{q['id']}' for q in queries)})", flush=True)
    print(f"Output: {output_file}", flush=True)

    # Set up environment and data directories based on mode
    env = os.environ.copy()
    env["BASE_DIR"] = BASE_DIR
    env["WATCH_MODE"] = "false"
    env["METRICS_ENABLED"] = "false"
    data_dir = None  # Only set for fresh mode (temp dir to clean up)

    if args.live:
        # Live mode: use existing .cimcp/ data with real embeddings
        cimcp_dir = os.path.join(BASE_DIR, ".cimcp")
        if not os.path.isdir(cimcp_dir):
            print(f"ERROR: No .cimcp/ directory found at {cimcp_dir}", flush=True)
            print(f"  Run the MCP server once first to build the index.", flush=True)
            return 1

        db_path = os.path.join(cimcp_dir, "code-intelligence.db")
        tantivy_dir = os.path.join(cimcp_dir, "tantivy-index")
        vector_dir = os.path.join(cimcp_dir, "vectors")

        if not os.path.exists(db_path):
            print(f"ERROR: No database at {db_path}", flush=True)
            return 1

        check_tantivy_lock(tantivy_dir)

        env["DB_PATH"] = db_path
        env["TANTIVY_INDEX_PATH"] = tantivy_dir
        env["VECTOR_DB_PATH"] = vector_dir
        # Don't override EMBEDDINGS_BACKEND — use real fastembed
        # Don't set LLM_ENABLED=false — descriptions already in DB/index
        print(f"Data dir: {cimcp_dir} (live)", flush=True)
    else:
        # Fresh mode: temp dirs, hash embeddings, full re-index
        data_dir = tempfile.mkdtemp(prefix="cimcp_bench_")
        env["EMBEDDINGS_BACKEND"] = "hash"
        env["LLM_ENABLED"] = "false"
        env["DB_PATH"] = os.path.join(data_dir, "code-intelligence.db")
        env["VECTOR_DB_PATH"] = os.path.join(data_dir, "vectors")
        env["TANTIVY_INDEX_PATH"] = os.path.join(data_dir, "tantivy-index")
        print(f"Data dir: {data_dir} (temp)", flush=True)

    print(f"Starting server...", flush=True)
    client = McpClient(BINARY, env)
    start_time = time.time()

    try:
        # Initialize MCP
        client.send({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "benchmark", "version": "1.0"}
            }
        })
        resp = client.recv(timeout=60)
        if not resp:
            print("ERROR: No init response", flush=True)
            return 1
        print("Initialized OK", flush=True)

        client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        time.sleep(1)

        if args.live:
            # Live mode: index already exists, just verify symbol count
            print("Checking existing index...", flush=True)
            symbol_count = get_symbol_count(client, 50)
            if symbol_count < 100:
                print(f"WARNING: Only {symbol_count} symbols in index. Triggering refresh...", flush=True)
                call_tool(client, "refresh_index", {}, 51, timeout=30)
                symbol_count = wait_for_indexing(client, start_msg_id=100, stable_seconds=15)
            print(f"Index ready: {symbol_count} symbols", flush=True)
        else:
            # Fresh mode: trigger full indexing
            print("Triggering refresh_index...", flush=True)
            call_tool(client, "refresh_index", {}, 50, timeout=30)
            print("Waiting for indexing...", flush=True)
            symbol_count = wait_for_indexing(client, start_msg_id=100, stable_seconds=15)
            if symbol_count < 100:
                print(f"ERROR: Only {symbol_count} symbols indexed", flush=True)
                return 1

        index_time = time.time() - start_time
        print(f"\nReady in {index_time:.0f}s ({symbol_count} symbols)", flush=True)

        # Run all queries
        all_results = []
        msg_id = 1000
        for i, q in enumerate(queries):
            msg_id += 1
            print(f"\n[{i+1}/{len(queries)}] Q{q['id']}: {q['query'][:60]}...", flush=True)
            t0 = time.time()
            results = run_query(client, q["query"], msg_id, limit=args.limit)
            dt = time.time() - t0
            print(f"  -> {len(results)} results in {dt:.1f}s", flush=True)
            if results:
                for j, r in enumerate(results[:3]):
                    kw = r.get('keyword_score') or 0
                    vec = r.get('vector_score') or 0
                    print(f"     #{j+1} {r['name']} ({r['kind']}) [{r['score']}] kw={kw:.1f} vec={vec:.2f} {r['file']}", flush=True)
            all_results.append({"query": q, "results": results})

        elapsed_total = time.time() - start_time
        print(f"\n{'='*60}", flush=True)
        print(f"All queries complete in {elapsed_total:.0f}s", flush=True)

        # Write results
        os.makedirs(os.path.dirname(output_file), exist_ok=True)
        md = format_results_markdown(round_num, all_results, elapsed_total, symbol_count)
        with open(output_file, "w") as f:
            f.write(md)
        print(f"Results written to: {output_file}", flush=True)

        json_file = output_file.replace(".md", ".json")
        with open(json_file, "w") as f:
            json.dump({
                "round": round_num,
                "mode": mode,
                "timestamp": datetime.now().isoformat(),
                "symbol_count": symbol_count,
                "elapsed_seconds": elapsed_total,
                "results": all_results,
            }, f, indent=2)
        print(f"JSON data written to: {json_file}", flush=True)

        return 0

    finally:
        client.close()
        if data_dir:
            try:
                shutil.rmtree(data_dir)
                print(f"Cleaned up {data_dir}", flush=True)
            except Exception:
                pass


if __name__ == "__main__":
    sys.exit(main() or 0)

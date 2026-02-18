#!/usr/bin/env python3
"""Build a live index for a repo by spawning the MCP server and triggering indexing."""
import subprocess, json, time, threading, queue, os, sys

BINARY = "./target/release/code-intelligence-mcp-server"
BASE_DIR = sys.argv[1] if len(sys.argv) > 1 else os.getcwd()

env = os.environ.copy()
env["BASE_DIR"] = BASE_DIR
env["WATCH_MODE"] = "false"

proc = subprocess.Popen([BINARY], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, bufsize=0)

q = queue.Queue()

def read_stdout():
    while True:
        line = proc.stdout.readline()
        if not line:
            break
        line = line.decode("utf-8", errors="replace").strip()
        if line:
            try:
                q.put(json.loads(line))
            except json.JSONDecodeError:
                pass

threading.Thread(target=read_stdout, daemon=True).start()
# drain stderr
threading.Thread(target=lambda: [proc.stderr.readline() for _ in iter(int, 1)], daemon=True).start()

def send(msg):
    proc.stdin.write((json.dumps(msg) + "\n").encode())
    proc.stdin.flush()

def recv(timeout=120, eid=None):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            msg = q.get(timeout=max(0.1, deadline - time.time()))
            if eid is None or msg.get("id") == eid:
                return msg
        except queue.Empty:
            break
    return None

# Initialize
print(f"Indexing {BASE_DIR}...", flush=True)
send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
      "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                 "clientInfo": {"name": "indexer", "version": "1.0"}}})
r = recv(30, 1)
status = "ok" if r else "FAIL"
print(f"Init: {status}", flush=True)

# Trigger refresh_index
send({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
      "params": {"name": "refresh_index", "arguments": {}}})
r = recv(120, 2)
if r:
    content = r.get("result", {}).get("content", [{}])
    txt = content[0].get("text", "") if content else ""
    print(f"Refresh: {txt[:200]}", flush=True)
else:
    print("Refresh: FAIL", flush=True)

# Wait for indexing to stabilize
msg_id = 10
last_count = 0
stable_since = None
for _ in range(60):
    time.sleep(5)
    msg_id += 1
    send({"jsonrpc": "2.0", "id": msg_id, "method": "tools/call",
          "params": {"name": "get_index_stats", "arguments": {}}})
    r = recv(10, msg_id)
    if r:
        content = r.get("result", {}).get("content", [{}])
        txt = content[0].get("text", "") if content else ""
        try:
            stats = json.loads(txt)
            count = stats.get("symbols", 0)
            latest = stats.get("latest_index_run", "")
            print(f"  symbols={count} latest={latest}", flush=True)
            if count == last_count and count > 0:
                if stable_since is None:
                    stable_since = time.time()
                elif time.time() - stable_since > 20:
                    print(f"Stable at {count} symbols", flush=True)
                    break
            else:
                stable_since = None
                last_count = count
        except json.JSONDecodeError:
            pass

proc.terminate()
try:
    proc.wait(timeout=5)
except subprocess.TimeoutExpired:
    proc.kill()
print("Done - index built", flush=True)

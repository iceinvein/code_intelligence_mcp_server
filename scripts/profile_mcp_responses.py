#!/usr/bin/env python3
"""Replay the MCP calls the agent made in R9900 and dump the responses.

For each question (q12, q14, q15 -- the smoke set's mandate losses), we:

1. Pull the exact MCP tool-call arguments the agent used.
2. Replay them against a fresh MCP server (same repo, same index).
3. Dump the full response to docs/benchmark_rounds/agent/mcp_traces/.
4. Print a structural summary: size, top-level keys, presence of code bodies.
5. List what the agent Read/Grep'd after that call -- the content gap.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT))

from scripts.test_scoring import McpClient, call_tool  # type: ignore


BINARY = REPO_ROOT / "target" / "release" / "code-intelligence-mcp-server"
TRACE_DIR = REPO_ROOT / "docs" / "benchmark_rounds" / "agent" / "mcp_traces"


def _load_run(round_path: Path, qid: str, toolset: str = "code_intel") -> dict:
    d = json.loads(round_path.read_text())
    for r in d["runs"]:
        if r["question_id"] == qid and r["toolset"] == toolset:
            return r
    raise KeyError(f"{qid}/{toolset} not in {round_path}")


def _categorize_calls(tool_calls: list[dict]) -> list[dict]:
    """Walk the call sequence, attach 'follow_up_reads' to each MCP call."""
    out: list[dict] = []
    pending_followups: list[dict] = []
    for tc in tool_calls:
        name = tc["name"]
        if "mcp__code-intelligence__" in name:
            entry = {"name": name.replace("mcp__code-intelligence__", ""),
                     "args": tc.get("args", {}),
                     "followup_reads": []}
            out.append(entry)
            pending_followups = entry["followup_reads"]
        elif name in ("Grep", "Read", "Glob"):
            pending_followups.append({"name": name, "args": tc.get("args", {})})
        # ToolSearch ignored
    return out


def _response_summary(name: str, resp: dict | None) -> dict:
    """Lightweight structural summary."""
    if resp is None:
        return {"error": "no response"}
    raw = json.dumps(resp)
    summary: dict = {
        "bytes": len(raw),
        "top_keys": list(resp.keys()) if isinstance(resp, dict) else type(resp).__name__,
    }
    if isinstance(resp, dict):
        # Heuristics for body presence
        body_keys = ("body", "text", "code", "snippet", "snippets", "content")
        flat_text = raw.lower()
        summary["has_body_keys"] = [k for k in body_keys if k in flat_text]
        # Try common shapes
        if "answer" in resp:
            summary["answer_len"] = len(resp.get("answer") or "")
        if "evidence" in resp:
            ev = resp.get("evidence") or []
            summary["evidence_count"] = len(ev) if isinstance(ev, list) else None
            if ev and isinstance(ev[0], dict):
                summary["evidence_sample_keys"] = list(ev[0].keys())
        if "citations" in resp:
            cit = resp.get("citations") or []
            summary["citations_count"] = len(cit) if isinstance(cit, list) else None
        if "hits" in resp:
            hits = resp.get("hits") or []
            summary["hits_count"] = len(hits) if isinstance(hits, list) else None
            if hits and isinstance(hits[0], dict):
                summary["hit_sample_keys"] = list(hits[0].keys())
        if "callers" in resp or "callees" in resp:
            for k in ("callers", "callees"):
                v = resp.get(k) or []
                if v:
                    summary[f"{k}_count"] = len(v) if isinstance(v, list) else None
                    if isinstance(v[0], dict):
                        summary[f"{k}_sample_keys"] = list(v[0].keys())
    return summary


def main() -> None:
    if not BINARY.is_file():
        sys.exit(f"binary not found: {BINARY} -- run cargo build --release")

    TRACE_DIR.mkdir(parents=True, exist_ok=True)

    # Allow filtering via QID env var so we can drill in on a single question.
    only_qid = os.environ.get("PROFILE_QID")
    all_targets = [
        ("self-q12", REPO_ROOT / "docs/benchmark_rounds/agent/R9900.json"),
        ("self-q14", REPO_ROOT / "docs/benchmark_rounds/agent/R9900.json"),
        ("self-q15", REPO_ROOT / "docs/benchmark_rounds/agent/R9900.json"),
    ]
    targets = [t for t in all_targets if not only_qid or t[0] == only_qid]

    # Spawn fresh server with WATCH_MODE=false so it uses the existing index.
    env = os.environ.copy()
    env["BASE_DIR"] = str(REPO_ROOT)
    env["WATCH_MODE"] = "false"
    env["METRICS_ENABLED"] = "false"
    # Do NOT override db/vector paths: use the real shared index from ~/.code-intelligence/

    print(f"Starting MCP server (BASE_DIR={REPO_ROOT})", flush=True)
    client = McpClient(str(BINARY), env)
    msg_id = 0

    try:
        msg_id += 1
        client.send({
            "jsonrpc": "2.0", "id": msg_id, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "profile", "version": "1.0"},
            },
        })
        resp = client.recv(timeout=60, expected_id=msg_id)
        if not resp:
            sys.exit("no init response")
        client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        time.sleep(1.5)

        for qid, round_path in targets:
            run = _load_run(round_path, qid)
            calls = _categorize_calls(run["tool_calls"])
            print(f"\n=========== {qid} (mandate) ===========", flush=True)
            print(f"  agent answer len: {len(run['final_answer'])}", flush=True)
            print(f"  agent tool_calls: {len(run['tool_calls'])}", flush=True)
            print(f"  MCP calls:        {len(calls)}", flush=True)

            for i, entry in enumerate(calls):
                msg_id += 1
                print(f"\n  --- MCP call {i+1}/{len(calls)}: {entry['name']}", flush=True)
                args_short = json.dumps(entry['args'])[:120]
                print(f"      args: {args_short}", flush=True)
                t0 = time.time()
                resp = call_tool(client, entry['name'], entry['args'], msg_id, timeout=180)
                if resp is None:
                    print("      >>> recent stderr (last 80 lines):", flush=True)
                    for ln in client.get_stderr(80):
                        print(f"      | {ln}", flush=True)
                elapsed = time.time() - t0
                summary = _response_summary(entry['name'], resp)
                print(f"      latency: {elapsed:.1f}s", flush=True)
                for k, v in summary.items():
                    print(f"      {k}: {v}", flush=True)
                print(f"      agent followup reads: {len(entry['followup_reads'])}", flush=True)
                for fr in entry['followup_reads'][:6]:
                    fa = json.dumps(fr.get('args', {}))[:110]
                    print(f"         - {fr['name']:6s} {fa}", flush=True)

                out_path = TRACE_DIR / f"{qid}_{i+1:02d}_{entry['name']}.json"
                out_path.write_text(json.dumps({
                    "qid": qid, "call_index": i,
                    "tool": entry['name'], "args": entry['args'],
                    "response": resp, "summary": summary,
                    "followup_reads": entry['followup_reads'],
                    "latency_s": elapsed,
                }, indent=2))
                print(f"      saved -> {out_path.relative_to(REPO_ROOT)}", flush=True)
    finally:
        client.close()


if __name__ == "__main__":
    main()

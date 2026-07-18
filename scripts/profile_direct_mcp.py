#!/usr/bin/env python3
"""Measure direct MCP latency without an answering agent in the loop.

The target daemon and repository must already be installed/indexed. This tool
does not start a server, mutate an index, or invoke an LLM judge.
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
RECORDED_ENV = (
    "EMBEDDINGS_BACKEND",
    "EMBEDDINGS_DEVICE",
    "RERANKER_ENABLED",
    "DESCRIPTIONS_ENABLED",
    "HYBRID_ALPHA",
    "LEARNING_ENABLED",
)


class McpHttpClient:
    def __init__(self, base_url: str, repo: Path) -> None:
        query = urllib.parse.urlencode({"repo": str(repo)})
        self.endpoint = f"{base_url.rstrip('/')}/mcp?{query}"
        self.session_id: str | None = None
        self.protocol_version = "2025-03-26"
        self.next_id = 1

    def post(self, payload: dict[str, Any]) -> dict[str, Any] | None:
        headers = {
            "Accept": "application/json, text/event-stream",
            "Content-Type": "application/json",
        }
        if self.session_id:
            headers["mcp-session-id"] = self.session_id
            headers["MCP-Protocol-Version"] = self.protocol_version
        request = urllib.request.Request(
            self.endpoint,
            data=json.dumps(payload, separators=(",", ":")).encode(),
            headers=headers,
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                self.session_id = response.headers.get("mcp-session-id", self.session_id)
                body = response.read().decode()
                content_type = response.headers.get("content-type", "")
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode(errors="replace")
            raise RuntimeError(f"MCP HTTP {exc.code}: {detail}") from exc

        if not body.strip():
            return None
        if "text/event-stream" in content_type:
            data_lines = [
                line.removeprefix("data:").strip()
                for line in body.splitlines()
                if line.startswith("data:")
            ]
            if not data_lines:
                return None
            return json.loads(data_lines[-1])
        return json.loads(body)

    def initialize(self) -> None:
        response = self.post(
            {
                "jsonrpc": "2.0",
                "id": self.next_id,
                "method": "initialize",
                "params": {
                    "protocolVersion": self.protocol_version,
                    "capabilities": {},
                    "clientInfo": {"name": "direct-profiler", "version": "1"},
                },
            }
        )
        self.next_id += 1
        if not response or "error" in response:
            raise RuntimeError(f"MCP initialize failed: {response}")
        self.post({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def call_tool(
        self, name: str, arguments: dict[str, Any]
    ) -> tuple[float, int, dict[str, Any]]:
        request_id = self.next_id
        self.next_id += 1
        started = time.perf_counter()
        response = self.post(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }
        )
        elapsed_ms = (time.perf_counter() - started) * 1000.0
        if not response or "error" in response:
            raise RuntimeError(f"{name} failed: {response}")
        return elapsed_ms, len(json.dumps(response, separators=(",", ":"))), response


def tool_json(response: dict[str, Any]) -> dict[str, Any]:
    content = response.get("result", {}).get("content", [])
    if not content or "text" not in content[0]:
        raise RuntimeError(f"unexpected MCP tool response: {response}")
    return json.loads(content[0]["text"])


def percentile(values: list[float], p: float) -> float:
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    position = (len(ordered) - 1) * p
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    weight = position - lower
    return ordered[lower] * (1.0 - weight) + ordered[upper] * weight


def git_revision(repo: Path) -> str | None:
    try:
        return subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=repo,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def resolve_repo(fixture: dict[str, Any], fixture_path: Path) -> Path:
    if env_name := fixture.get("repo_env"):
        value = os.environ.get(env_name)
        if not value:
            raise RuntimeError(f"set {env_name} to the fixture repository path")
        return Path(value).expanduser().resolve()
    value = fixture.get("repo")
    if not value:
        raise RuntimeError("fixture must define repo or repo_env")
    path = Path(value).expanduser()
    if not path.is_absolute():
        path = (fixture_path.parent / path).resolve()
    return path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture", type=Path)
    parser.add_argument("--base-url", default="http://127.0.0.1:17800")
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--warmups", type=int, default=2)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.iterations < 1 or args.warmups < 0:
        parser.error("iterations must be >= 1 and warmups must be >= 0")

    fixture_path = args.fixture.resolve()
    fixture = json.loads(fixture_path.read_text())
    repo = resolve_repo(fixture, fixture_path)
    if not repo.is_dir():
        raise RuntimeError(f"repository does not exist: {repo}")

    client = McpHttpClient(args.base_url, repo)
    client.initialize()
    _, _, stats_response = client.call_tool("get_index_stats", {})
    index_stats = tool_json(stats_response)
    operations: list[dict[str, Any]] = []
    gate_failures: list[str] = []

    for scenario in fixture["operations"]:
        tool = scenario["tool"]
        arguments = scenario["arguments"]
        cold_ms, cold_bytes, _ = client.call_tool(tool, arguments)
        for _ in range(args.warmups):
            client.call_tool(tool, arguments)
        warm_samples = [client.call_tool(tool, arguments)[0] for _ in range(args.iterations)]
        summary = {
            "name": scenario["name"],
            "tool": tool,
            "arguments": arguments,
            "cold_ms": round(cold_ms, 3),
            "cold_response_bytes": cold_bytes,
            "warm": {
                "samples": len(warm_samples),
                "mean_ms": round(statistics.fmean(warm_samples), 3),
                "p50_ms": round(percentile(warm_samples, 0.50), 3),
                "p95_ms": round(percentile(warm_samples, 0.95), 3),
                "p99_ms": round(percentile(warm_samples, 0.99), 3),
            },
        }
        operations.append(summary)
        max_p95 = scenario.get("max_warm_p95_ms")
        if max_p95 is not None and summary["warm"]["p95_ms"] > max_p95:
            gate_failures.append(
                f"{scenario['name']}: p95 {summary['warm']['p95_ms']}ms > {max_p95}ms"
            )

    report = {
        "schema_version": 1,
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "fixture": fixture.get("name", fixture_path.stem),
        "size_class": fixture.get("size_class"),
        "fixture_file": str(fixture_path.relative_to(ROOT))
        if fixture_path.is_relative_to(ROOT)
        else str(fixture_path),
        "repo": str(repo),
        "repo_revision": git_revision(repo),
        "server_revision": git_revision(ROOT),
        "server_url": args.base_url,
        "configuration": index_stats.get("performance_config", {}),
        "caller_environment": {name: os.environ.get(name) for name in RECORDED_ENV},
        "index_stats": {
            "symbols": index_stats.get("symbols"),
            "edges": index_stats.get("edges"),
            "descriptions": index_stats.get("descriptions"),
            "latest_index_run": index_stats.get("latest_index_run"),
        },
        "warmups": args.warmups,
        "iterations": args.iterations,
        "cold_definition": "first invocation for this exact operation argument set in the existing daemon",
        "operations": operations,
        "gate_failures": gate_failures,
    }
    output = args.output or ROOT / "bench" / "results" / "direct-mcp-latest.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    print(f"wrote {output}", file=sys.stderr)
    return 1 if gate_failures else 0


if __name__ == "__main__":
    raise SystemExit(main())

"""Bench-wide configuration: model IDs, timeouts, paths.

All defaults are overridable via BENCH_* environment variables. The module
is import-time evaluated; tests using monkeypatch.setenv must `importlib.reload`
the module to pick up changes.
"""
from __future__ import annotations

import os
from pathlib import Path


def _env(key: str, default: str) -> str:
    return os.environ.get(key, default)


def _env_int(key: str, default: int) -> int:
    raw = os.environ.get(key)
    return int(raw) if raw is not None else default


# Models
AGENT_MODEL = _env("BENCH_AGENT_MODEL", "claude-sonnet-4-6")
JUDGE_HAIKU = _env("BENCH_JUDGE_HAIKU", "claude-haiku-4-5")
JUDGE_SONNET = _env("BENCH_JUDGE_SONNET", "claude-sonnet-4-6")
JUDGE_OPUS = _env("BENCH_JUDGE_OPUS", "claude-opus-4-7")

# Binaries
CLAUDE_BINARY = _env("BENCH_CLAUDE_BINARY", "claude")
CODEGRAPH_BINARY = _env("BENCH_CODEGRAPH_BINARY", "codegraph")

# Daemon binary built by `cargo build --release`
REPO_ROOT = Path(__file__).resolve().parent.parent
DAEMON_BINARY = REPO_ROOT / "target" / "release" / "code-intelligence-mcp-server"

# Timeouts
PER_QUESTION_TIMEOUT_S = _env_int("BENCH_TIMEOUT_S", 180)
DAEMON_HEALTH_TIMEOUT_S = _env_int("BENCH_DAEMON_HEALTH_TIMEOUT_S", 30)

# Paths
BENCH_DIR = REPO_ROOT / "bench"
STATE_DIR = BENCH_DIR / "state"
BENCH_HOME = STATE_DIR / "home"
BENCH_INDEXES_DIR = BENCH_HOME / ".code-intelligence" / "repos"
BENCH_REPOS_DIR = STATE_DIR / "repos"
BENCH_CODEGRAPH_DIR = STATE_DIR / "codegraph"
RESULTS_DIR = BENCH_DIR / "results"
FIXTURES_DIR = BENCH_DIR / "fixtures"

# Progress
PROGRESS_MODE = _env("BENCH_PROGRESS", "rich")  # rich | plain

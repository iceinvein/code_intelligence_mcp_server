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


def _env_bool(key: str, default: bool) -> bool:
    raw = os.environ.get(key)
    if raw is None:
        return default
    return raw.strip().lower() in ("1", "true", "yes", "on")


# Models
AGENT_MODEL = _env("BENCH_AGENT_MODEL", "claude-sonnet-4-6")
JUDGE_HAIKU = _env("BENCH_JUDGE_HAIKU", "claude-haiku-4-5")
JUDGE_SONNET = _env("BENCH_JUDGE_SONNET", "claude-sonnet-4-6")
JUDGE_OPUS = _env("BENCH_JUDGE_OPUS", "claude-opus-4-8")

# Binaries
CLAUDE_BINARY = _env("BENCH_CLAUDE_BINARY", "claude")
CODEGRAPH_BINARY = _env("BENCH_CODEGRAPH_BINARY", "codegraph")

# Daemon binary built by `cargo build --release`
REPO_ROOT = Path(__file__).resolve().parent.parent
DAEMON_BINARY = REPO_ROOT / "target" / "release" / "code-intelligence-mcp-server"

# Timeouts
PER_QUESTION_TIMEOUT_S = _env_int("BENCH_TIMEOUT_S", 180)
DAEMON_HEALTH_TIMEOUT_S = _env_int("BENCH_DAEMON_HEALTH_TIMEOUT_S", 30)

# Agent turn cap per question. Typical runs use 5-6 turns; the pathological tail
# (30 turns / ~880k tokens in R008) burns tokens quadratically via cache re-reads
# without improving answers. 16 leaves headroom for the measured 2-3 turn
# deferred-MCP-schema discovery tax (12 truncated deep multi_hop traces to
# judge-0 fragments in R010-R012). 0 disables the cap.
MAX_TURNS = _env_int("BENCH_MAX_TURNS", 16)

# Paths
BENCH_DIR = REPO_ROOT / "bench"
# Mutable bench state (fixture checkouts, per-variant daemon homes, indexes)
# lives OUTSIDE the repo tree. Keeping it under bench/ required a bench-specific
# "**/bench/state/repos/**" entry in the daemon's default exclude patterns (plus
# a hand-edited server.toml override in every variant home to un-exclude the
# fixtures' own files) and put ~27G of indexes inside the working copy. Fixture
# definitions (bench/fixtures/) and results (bench/results/) stay in-repo.
# NOTE: changing this path changes repo hashes, so all index caches invalidate
# and `prep` rebuilds from scratch.
STATE_DIR = Path(
    _env("BENCH_STATE_DIR", str(Path.home() / ".code-intelligence-bench"))
).expanduser()
BENCH_HOME = STATE_DIR / "home"
BENCH_INDEXES_DIR = BENCH_HOME / ".code-intelligence" / "repos"
BENCH_REPOS_DIR = STATE_DIR / "repos"
BENCH_CODEGRAPH_DIR = STATE_DIR / "codegraph"
RESULTS_DIR = BENCH_DIR / "results"
FIXTURES_DIR = BENCH_DIR / "fixtures"


def bench_home_for_variant(variant: str) -> Path:
    """Return the isolated HOME path for the given index variant.

    Each variant gets its own .code-intelligence/ tree so benchmark index
    indexes can coexist without overwriting each other.
    Arms with index_variant=None (default, codegraph) use the legacy BENCH_HOME.
    """
    return STATE_DIR / "home" / variant

# Concurrency. Agent runs are independent (the daemon serves concurrent
# sessions); judge calls are independent CLI spawns. Both were fully serial,
# which made a 2-arm round ~2h wall time.
RUN_CONCURRENCY = _env_int("BENCH_RUN_CONCURRENCY", 4)
JUDGE_CONCURRENCY = _env_int("BENCH_JUDGE_CONCURRENCY", 3)

# Circuit breaker: abort the cycle after this many consecutive failed agent runs
# or judge calls (the signature of subscription quota exhaustion). Everything
# completed so far is persisted; `full --round <N>` resumes, re-running failures.
MAX_CONSECUTIVE_FAILURES = _env_int("BENCH_MAX_CONSECUTIVE_FAILURES", 5)

# Content-addressed run reuse: before running a (arm, question, rep) slot, look
# for a prior-round record with an identical run_key (arm config, daemon binary
# hash, question text, repo pin, agent model + CLI, turn cap) and adopt it
# instead of re-running. Unchanged baseline arms in A/B rounds become free.
# Judging is never reused. See bench/reuse.py.
RUN_REUSE = _env_bool("BENCH_RUN_REUSE", True)

# Tiered judging: haiku scores first; the sonnet+opus panel only runs when the
# haiku score is mid-band (3-8) or errored. Extremes are stable across judges,
# and the ~240-calls/5h subscription window is the real judging constraint.
JUDGE_TIERED = _env_bool("BENCH_JUDGE_TIERED", True)

# Progress
PROGRESS_MODE = _env("BENCH_PROGRESS", "rich")  # rich | plain

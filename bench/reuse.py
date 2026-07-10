"""Content-addressed run reuse across rounds.

An agent run is a (costly) sample from a distribution determined entirely by:
the arm definition (system prompt, allowed tools, daemon env, index variant),
the daemon binary that built/serves the index, the question text, the repo pin,
the agent model and CLI, and the turn cap. When none of those changed since a
prior round, re-running the arm buys a fresh sample of the same distribution --
which A/B rounds do not need for the UNCHANGED side. Keying each run by a hash
of those inputs lets a new round adopt prior-round records instead of re-running
them (typically the baseline arm), spending quota only on the changed arm.

Reused rows are copied into the new round's runs.jsonl with `reused_from` set,
so the round stays self-contained (rescore/report never chase other rounds).
Judging is NOT reused: judge scores are re-derived per round so judge-side
changes (models, tiering, rubric edits) apply uniformly to all rows.

The per-question timeout is deliberately NOT in the key: it only decides
whether a run completes, and only completed runs are reusable. Rounds recorded
before this feature landed carry no run_key and are never matched.

Opt out with BENCH_RUN_REUSE=0.
"""
from __future__ import annotations

import hashlib
import json
import subprocess
from functools import lru_cache
from pathlib import Path

from bench import config
from bench.arms import Arm

KEY_SCHEMA_VERSION = 1


@lru_cache(maxsize=None)
def binary_version(binary: str) -> str:
    """`<binary> --version` output, cached per process. 'unknown' on failure."""
    try:
        out = subprocess.run(
            [binary, "--version"], capture_output=True, timeout=15
        ).stdout.decode().strip()
        return out or "unknown"
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"


def run_key(
    arm: Arm,
    question_id: str,
    question_text: str,
    repo_sha: str,
    *,
    model: str,
    max_turns: int,
    daemon_bin: str | None,
    cli_version: str,
) -> str:
    """Content hash of everything that determines a run's behavior.

    `daemon_bin` is the daemon binary hash for daemon arms (None otherwise);
    the index is a pure function of (binary, repo pin, variant env), so hashing
    the binary covers index content changes without hashing the index itself.
    """
    material = {
        "v": KEY_SCHEMA_VERSION,
        "arm": {
            "name": arm.name,
            "index_variant": arm.index_variant,
            "daemon_env": dict(sorted(arm.daemon_env.items())),
            "allowed_tools": list(arm.allowed_tools),
            "tool_guidance": arm.tool_guidance,
            "needs_daemon": arm.needs_daemon,
            "is_codegraph": arm.is_codegraph,
        },
        "daemon_bin": daemon_bin,
        "codegraph": binary_version(config.CODEGRAPH_BINARY) if arm.is_codegraph else None,
        "question": {"id": question_id, "text": question_text},
        "repo_sha": repo_sha,
        "model": model,
        "max_turns": max_turns,
        "cli": cli_version,
    }
    canonical = json.dumps(material, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode()).hexdigest()


def _reusable(rec: dict) -> bool:
    return (
        not rec.get("run_error")
        and bool(str(rec.get("final_answer", "")).strip())
        and bool(rec.get("run_key"))
    )


def find_reusable(
    results_root: Path,
    current_round: Path,
    wanted: dict[tuple[str, str, int], str],
) -> dict[tuple[str, str, int], dict]:
    """Match pending (arm, question_id, rep) slots against prior rounds.

    `wanted` maps each pending slot to its run_key. Rounds are scanned newest
    first; the first valid record whose (arm, question_id, rep, run_key) all
    match claims the slot. Matching rep-for-rep keeps a slot from adopting two
    copies of the same prior sample when repeats > 1.
    """
    found: dict[tuple[str, str, int], dict] = {}
    if not wanted or not results_root.exists():
        return found
    rounds = sorted(
        (p for p in results_root.iterdir()
         if p.is_dir() and p.name.startswith("R") and p != current_round),
        key=lambda p: p.name,
        reverse=True,
    )
    for round_dir in rounds:
        if len(found) == len(wanted):
            break
        runs_path = round_dir / "runs.jsonl"
        if not runs_path.exists():
            continue
        by_slot: dict[tuple[str, str, int], dict] = {}
        for line in runs_path.read_text().splitlines():
            if not line.strip():
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            # Last record per slot wins, mirroring the orchestrator's dedupe.
            by_slot[(rec.get("arm"), rec.get("question_id"), rec.get("rep", 0))] = rec
        for slot, key in wanted.items():
            if slot in found:
                continue
            rec = by_slot.get(slot)
            if rec and _reusable(rec) and rec["run_key"] == key:
                copied = dict(rec)
                copied["reused_from"] = round_dir.name
                found[slot] = copied
    return found

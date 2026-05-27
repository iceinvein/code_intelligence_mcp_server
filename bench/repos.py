"""Per-repo checkout and index variant management."""
from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path

from bench import config


def repo_hash(repo_path: str) -> str:
    """16-char hex hash matching the daemon's repo_id."""
    return hashlib.sha256(repo_path.encode()).hexdigest()[:16]


def variant_data_dir(repo_hash_str: str, variant: str) -> Path:
    return config.BENCH_INDEXES_DIR / f"{repo_hash_str}_{variant}"


def index_is_fresh(data_dir: Path, current_meta: dict) -> bool:
    cache = data_dir / "bench-cache.json"
    if not cache.exists():
        return False
    try:
        cached = json.loads(cache.read_text())
    except json.JSONDecodeError:
        return False
    return cached == current_meta


def write_cache(data_dir: Path, meta: dict) -> None:
    data_dir.mkdir(parents=True, exist_ok=True)
    (data_dir / "bench-cache.json").write_text(json.dumps(meta, indent=2))


def ensure_repo_checkout(*, name: str, upstream_url: str, upstream_sha: str, target_dir: Path) -> None:
    """Clone the repo if missing, then check out the pinned SHA."""
    if not target_dir.exists():
        target_dir.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["git", "clone", upstream_url, str(target_dir)],
            check=True,
            capture_output=True,
        )

    head = subprocess.run(
        ["git", "-C", str(target_dir), "rev-parse", "HEAD"],
        check=True, capture_output=True,
    ).stdout.decode().strip()
    if head == upstream_sha:
        return

    subprocess.run(
        ["git", "-C", str(target_dir), "fetch", "--depth=1", "origin", upstream_sha],
        check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "-C", str(target_dir), "checkout", upstream_sha],
        check=True, capture_output=True,
    )


def current_daemon_sha() -> str:
    """Git SHA of code-intelligence-mcp-server HEAD."""
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        check=True, capture_output=True, cwd=str(config.REPO_ROOT),
    ).stdout.decode().strip()

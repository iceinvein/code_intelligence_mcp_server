"""Zero-token re-scoring of a stored round after scoring-logic changes.

Usage:
    python3 -m bench.rescore R008 [R007 ...]

Mech scoring is deterministic and free, so any change to bench/score.py can be
validated against history without re-running agents or judges. Citations are
verified against the fixture's pinned SHA via `git show` (the working tree may
have drifted, especially for symlinked local repos); the working tree is the
fallback when the pin is not present.

Writes a fresh scores.json (backing up the old one to scores.json.pre-rescore
the first time) and prints an old-vs-new aggregate comparison.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

from bench import config, fixtures_io
from bench.orchestrator import _dedupe_last, _read_jsonl, _score_record, _write_jsonl


def git_line_reader(repo_path: Path, sha: str):
    """Line reader over a pinned git tree. Returns None for missing/binary files."""
    cache: dict[str, list[str] | None] = {}

    def read(file: str) -> list[str] | None:
        if file in cache:
            return cache[file]
        proc = subprocess.run(
            ["git", "-C", str(repo_path), "show", f"{sha}:{file}"],
            capture_output=True,
        )
        if proc.returncode != 0:
            cache[file] = None
            return None
        try:
            lines: list[str] | None = proc.stdout.decode().splitlines()
        except UnicodeDecodeError:
            lines = None
        cache[file] = lines
        return lines

    return read


def git_file_lister(repo_path: Path, sha: str):
    """File lister over a pinned git tree (for shortened-path resolution)."""
    cache: dict[str, list[str]] = {}

    def list_files() -> list[str]:
        if "files" not in cache:
            proc = subprocess.run(
                ["git", "-C", str(repo_path), "ls-tree", "-r", "--name-only", sha],
                capture_output=True,
            )
            cache["files"] = proc.stdout.decode().splitlines() if proc.returncode == 0 else []
        return cache["files"]

    return list_files


def _sha_available(repo_path: Path, sha: str) -> bool:
    proc = subprocess.run(
        ["git", "-C", str(repo_path), "cat-file", "-e", f"{sha}^{{commit}}"],
        capture_output=True,
    )
    return proc.returncode == 0


def _resolve_repo_path(name: str) -> Path:
    if name == "smoke":
        return config.REPO_ROOT
    return config.BENCH_REPOS_DIR / name


def _legacy_casualty(jrec: dict) -> bool:
    """Casualty detection for judge rows written before the casualty field existed."""
    if "casualty" in jrec:
        return bool(jrec["casualty"])
    scores = jrec.get("scores", {})
    justs = jrec.get("justifications", {})
    return bool(scores) and all(v == 0 for v in scores.values()) \
        and not any(str(j).strip() for j in justs.values())


def _aggregate(rows: list[dict]) -> dict:
    n = len(rows)
    if n == 0:
        return {}
    judged = [r["judge_median"] for r in rows if r.get("judge_median") is not None]
    # Product scope: rows where the agent actually called code-intelligence.
    # Zero-MCP rows (agent answered via Grep/Read alone) score the agent, not
    # the product, so they are excluded here while staying in the headline
    # numbers. Rows without the field predate it and count as product-scope.
    product = [r for r in rows if r.get("mcp_tool_calls", 1) > 0]
    pjudged = [r["judge_median"] for r in product if r.get("judge_median") is not None]
    return {
        "n": n,
        "mech": sum(r.get("mech", 0) for r in rows) / n,
        "citation_hit": sum(1 for r in rows if r.get("citation_hit")) / n,
        "hallucinated": sum(1 for r in rows if r.get("hallucinated")) / n,
        "forbidden_hit": sum(1 for r in rows if r.get("forbidden_hit")) / n,
        "judge": sum(judged) / len(judged) if judged else None,
        "imprecise": sum(1 for r in rows if r.get("imprecise_citations", 0) > 0) / n,
        "n_product": len(product),
        "mech_product": (
            sum(r.get("mech", 0) for r in product) / len(product) if product else None
        ),
        "judge_product": sum(pjudged) / len(pjudged) if pjudged else None,
    }


def rescore_round(round_dir: Path) -> dict:
    round_dir = Path(round_dir)
    runs = _dedupe_last(_read_jsonl(round_dir / "runs.jsonl"))
    judges = _dedupe_last(_read_jsonl(round_dir / "judge.jsonl"))
    if not runs:
        raise SystemExit(f"no runs.jsonl in {round_dir}")

    old_scores = _read_jsonl(round_dir / "scores.json")

    repo_names = {r.get("repo") or r["question_id"].split("-")[0] for r in runs}
    qmap: dict[tuple[str, str], object] = {}
    repo_ctx: dict[str, tuple[Path, object]] = {}
    for name in repo_names:
        fixture = fixtures_io.load_fixture(config.FIXTURES_DIR / f"{name}.yaml")
        repo_path = _resolve_repo_path(name)
        sha = fixture.meta.upstream_sha
        if _sha_available(repo_path, sha):
            reader = git_line_reader(repo_path, sha)
            lister = git_file_lister(repo_path, sha)
        else:
            reader = lister = None
            print(f"warning: {name}: pinned {sha[:12]} not available; verifying "
                  f"against working tree", file=sys.stderr)
        repo_ctx[name] = (repo_path, reader, lister)
        for q in fixture.questions:
            qmap[(name, q.id)] = q

    new_scores: list[dict] = []
    score_by_key: dict[tuple[str, str, int], dict] = {}
    for rec in runs:
        repo_name = rec.get("repo") or rec["question_id"].split("-")[0]
        q = qmap.get((repo_name, rec["question_id"]))
        if q is None:
            print(f"warning: no fixture question for {rec['question_id']}; skipping",
                  file=sys.stderr)
            continue
        repo_path, reader, lister = repo_ctx[repo_name]
        rec = {**rec, "repo": repo_name}
        s = _score_record(q, rec, repo_path, read_lines=reader, list_files=lister)
        new_scores.append(s)
        score_by_key[(rec["arm"], rec["question_id"], rec.get("rep", 0))] = s

    for jrec in judges:
        s = score_by_key.get((jrec["arm"], jrec["question_id"], jrec.get("rep", 0)))
        if s is None:
            continue
        if _legacy_casualty(jrec):
            s["judge_casualty"] = True
        else:
            s["judge_median"] = jrec["median"]
            s["judge_range"] = jrec["range"]

    scores_path = round_dir / "scores.json"
    backup = round_dir / "scores.json.pre-rescore"
    if scores_path.exists() and not backup.exists():
        backup.write_text(scores_path.read_text())
    _write_jsonl(scores_path, new_scores)

    return {
        "round": round_dir.name,
        "n_rescored": len(new_scores),
        "old": _aggregate(old_scores),
        "new": _aggregate(new_scores),
    }


def main(argv: list[str] | None = None) -> int:
    argv = argv if argv is not None else sys.argv[1:]
    if not argv:
        print("usage: python3 -m bench.rescore R<NNN> [R<NNN> ...]", file=sys.stderr)
        return 2
    for round_id in argv:
        summary = rescore_round(config.RESULTS_DIR / round_id)
        old, new = summary["old"], summary["new"]
        print(f"\n{summary['round']}: rescored {summary['n_rescored']} rows")
        if old:
            for k in (
                "mech",
                "mech_product",
                "citation_hit",
                "hallucinated",
                "imprecise",
                "forbidden_hit",
                "judge",
                "judge_product",
            ):
                o, nv = old.get(k), new.get(k)
                fmt = lambda v: "n/a" if v is None else f"{v:.3f}"
                delta = "" if o is None or nv is None else f"  ({nv - o:+.3f})"
                print(f"  {k:>14}: {fmt(o)} -> {fmt(nv)}{delta}")
        else:
            for k, v in new.items():
                print(f"  {k:>14}: {v}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

"""End-to-end cycle: prep -> per-arm question runs -> judging -> report.

Crash-safety contract: runs.jsonl and judge.jsonl are appended record-by-record
as work completes, so a crash mid-cycle loses at most the in-flight record.
scores.json is derived data (mech scoring is deterministic and free) and is
rebuilt at the end of every cycle from runs.jsonl + judge.jsonl. Re-invoking a
cycle with the same results_dir resumes: completed (arm, question) runs and
judged rows are skipped.
"""
from __future__ import annotations

import json
from pathlib import Path

from bench import arms as arms_mod
from bench import config as config_mod
from bench import daemon as daemon_mod
from bench import judge as judge_mod
from bench import runner, score
from bench.fixtures_io import Fixture, Question


def _append_jsonl(path: Path, record: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as f:
        f.write(json.dumps(record, default=str) + "\n")


def _read_jsonl(path: Path) -> list[dict]:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def _write_jsonl(path: Path, records: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        for r in records:
            f.write(json.dumps(r, default=str) + "\n")


def _run_record(run: runner.Run, repo_name: str) -> dict:
    return {
        "arm": run.arm,
        "question_id": run.question_id,
        "repo": repo_name,
        "final_answer": run.final_answer,
        "tool_calls": [{"name": t.name, "input_summary": t.input_summary} for t in run.tool_calls],
        "input_tokens": run.input_tokens,
        "output_tokens": run.output_tokens,
        "cache_read_tokens": run.cache_read_tokens,
        "cache_creation_tokens": run.cache_creation_tokens,
        "wall_ms": run.wall_ms,
        "stop_reason": run.stop_reason,
        "model": run.model,
        "run_error": run.run_error,
    }


def _score_record(q: Question, rec: dict, repo_path: Path) -> dict:
    answer = rec.get("final_answer", "")
    multiplier, verifications = score.compute_citation_multiplier(answer, repo_path)
    final_mech = score.final_mech(q, answer, multiplier)
    raw = score.mech_score(q, answer)
    forbidden = score.forbidden_hits(q, answer)
    return {
        "question_id": q.id,
        "task_type": q.task_type,
        "arm": rec["arm"],
        "repo": rec["repo"],
        "mech": final_mech,
        "mech_raw": raw["raw"],
        "citation_hit": raw["citation_hit"],
        "citation_multiplier": multiplier,
        "hallucinated": any(not v.ok for v in verifications),
        "forbidden_hit": bool(forbidden),
        "tool_calls": len(rec.get("tool_calls", [])),
        "input_tokens": rec.get("input_tokens", 0) + rec.get("cache_read_tokens", 0),
        "wall_ms": rec.get("wall_ms", 0),
        "run_error": rec.get("run_error"),
        "judge_median": None,
        "judge_range": None,
        "judge_casualty": False,
    }


def _is_run_casualty(rec: dict) -> bool:
    return bool(rec.get("run_error")) or not str(rec.get("final_answer", "")).strip()


def run_cycle(
    *,
    arms_to_run: list[arms_mod.Arm],
    repos: list[tuple[Fixture, Path]],
    results_dir: Path,
    judge_enabled: bool = False,
    resume: bool = True,
) -> dict:
    """Run one bench cycle. Returns a summary dict; writes results to results_dir."""
    results_dir.mkdir(parents=True, exist_ok=True)
    transcripts_dir = results_dir / "transcripts"
    runs_path = results_dir / "runs.jsonl"
    judge_path = results_dir / "judge.jsonl"

    # (repo_name, question_id) -> (Question, repo_path); used to re-score resumed runs.
    qmap: dict[tuple[str, str], tuple[Question, Path]] = {
        (fixture.meta.repo, q.id): (q, repo_path)
        for fixture, repo_path in repos
        for q in fixture.questions
    }

    all_runs: list[dict] = _read_jsonl(runs_path) if resume else []
    done = {(r["arm"], r["question_id"]) for r in all_runs}

    for arm in arms_to_run:
        pending = [
            (fixture, repo_path, q)
            for fixture, repo_path in repos
            for q in fixture.questions
            if (arm.name, q.id) not in done
        ]
        if not pending:
            continue

        port = daemon_mod.pick_free_port() if arm.needs_daemon else 0
        if arm.needs_daemon:
            home = config_mod.bench_home_for_variant(arm.index_variant) if arm.index_variant else None
            daemon = daemon_mod.maybe_start_daemon(arm, port=port, home=home)
        else:
            daemon = None

        try:
            for fixture, repo_path, q in pending:
                run = runner.run_question(
                    arm=arm, q=q, daemon=daemon,
                    repo_path=repo_path, transcripts_dir=transcripts_dir,
                )
                rec = _run_record(run, fixture.meta.repo)
                _append_jsonl(runs_path, rec)
                all_runs.append(rec)
        finally:
            if daemon is not None:
                daemon.stop()

    # Scoring is derived: recomputed for every run (including resumed ones).
    all_scores: list[dict] = []
    score_by_key: dict[tuple[str, str], dict] = {}
    for rec in all_runs:
        entry = qmap.get((rec["repo"], rec["question_id"]))
        if entry is None:
            continue
        q, repo_path = entry
        s = _score_record(q, rec, repo_path)
        all_scores.append(s)
        score_by_key[(rec["arm"], rec["question_id"])] = s

    # Judging (skipped if judge_enabled is False). Casualty runs (empty answer or
    # run_error) are never judged: 3 judge calls to confirm a known-zero is waste.
    judge_records: list[dict] = _read_jsonl(judge_path) if resume else []
    judged = {(j["arm"], j["question_id"]) for j in judge_records}
    if judge_enabled:
        for rec in all_runs:
            key = (rec["arm"], rec["question_id"])
            if key in judged or key not in score_by_key:
                continue
            if _is_run_casualty(rec):
                score_by_key[key]["judge_casualty"] = True
                continue
            entry = qmap[(rec["repo"], rec["question_id"])]
            q, _repo_path = entry
            s = score_by_key[key]
            citations = [{"file": c.file, "line_range": list(c.line_range), "symbol": c.symbol}
                         for c in q.expected.citations]
            mech_context = {
                "citation_hit": s["citation_hit"],
                "hallucinated_citation": s["hallucinated"],
                "forbidden_hit": s["forbidden_hit"],
            }
            agg = judge_mod.judge_all(
                question_id=q.id,
                question=q.question,
                rubric=q.rubric,
                citations=citations,
                mech_context=mech_context,
                answer=rec["final_answer"],
            )
            jrec = {
                "arm": rec["arm"],
                "question_id": q.id,
                "repo": rec["repo"],
                "scores": agg.scores,
                "justifications": agg.justifications,
                "median": agg.median,
                "range": agg.range,
                "errors": agg.errors,
                "n_valid": agg.n_valid,
                "casualty": agg.casualty,
            }
            _append_jsonl(judge_path, jrec)
            judge_records.append(jrec)

    # Apply judge results (fresh + resumed) to score rows. Casualty rows keep
    # judge_median=None so aggregates skip them instead of averaging zeros.
    for jrec in judge_records:
        s = score_by_key.get((jrec["arm"], jrec["question_id"]))
        if s is None:
            continue
        if jrec.get("casualty"):
            s["judge_casualty"] = True
        else:
            s["judge_median"] = jrec["median"]
            s["judge_range"] = jrec["range"]

    _write_jsonl(results_dir / "scores.json", all_scores)

    return {
        "n_runs": len(all_runs),
        "n_judged": len(judge_records),
        "scores": all_scores,
    }

"""End-to-end cycle: prep -> per-arm question runs -> judging -> report."""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from bench import arms as arms_mod
from bench import daemon as daemon_mod
from bench import judge as judge_mod
from bench import runner, score
from bench.fixtures_io import Fixture


def _write_jsonl(path: Path, records: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        for r in records:
            f.write(json.dumps(r, default=str) + "\n")


def run_cycle(
    *,
    arms_to_run: list[arms_mod.Arm],
    repos: list[tuple[Fixture, Path]],
    results_dir: Path,
    judge_client: Any | None,
) -> dict:
    """Run one bench cycle. Returns a summary dict; writes results to results_dir."""
    results_dir.mkdir(parents=True, exist_ok=True)
    transcripts_dir = results_dir / "transcripts"

    all_runs: list[dict] = []
    all_scores: list[dict] = []

    for arm in arms_to_run:
        port = daemon_mod.pick_free_port() if arm.needs_daemon else 0
        daemon = daemon_mod.maybe_start_daemon(arm, port=port) if arm.needs_daemon else None

        try:
            for fixture, repo_path in repos:
                for q in fixture.questions:
                    run = runner.run_question(
                        arm=arm, q=q, daemon=daemon,
                        repo_path=repo_path, transcripts_dir=transcripts_dir,
                    )

                    multiplier, verifications = score.compute_citation_multiplier(
                        run.final_answer, repo_path,
                    )
                    final_mech = score.final_mech(q, run.final_answer, multiplier)
                    raw = score.mech_score(q, run.final_answer)
                    forbidden = score.forbidden_hits(q, run.final_answer)

                    score_record = {
                        "question_id": q.id,
                        "task_type": q.task_type,
                        "arm": arm.name,
                        "repo": fixture.meta.repo,
                        "mech": final_mech,
                        "mech_raw": raw["raw"],
                        "citation_hit": raw["citation_hit"],
                        "citation_multiplier": multiplier,
                        "hallucinated": any(not v.ok for v in verifications),
                        "forbidden_hit": bool(forbidden),
                        "tool_calls": len(run.tool_calls),
                        "input_tokens": run.input_tokens + run.cache_read_tokens,
                        "wall_ms": run.wall_ms,
                    }
                    all_runs.append({
                        "arm": run.arm,
                        "question_id": run.question_id,
                        "repo": fixture.meta.repo,
                        "final_answer": run.final_answer,
                        "tool_calls": [{"name": t.name, "input_summary": t.input_summary} for t in run.tool_calls],
                        "input_tokens": run.input_tokens,
                        "output_tokens": run.output_tokens,
                        "cache_read_tokens": run.cache_read_tokens,
                        "cache_creation_tokens": run.cache_creation_tokens,
                        "wall_ms": run.wall_ms,
                        "stop_reason": run.stop_reason,
                        "model": run.model,
                    })
                    all_scores.append(score_record)
        finally:
            if daemon is not None:
                daemon.stop()

    # Judging (skipped if no client provided).
    judge_records: list[dict] = []
    if judge_client is not None:
        for run in all_runs:
            for fixture, repo_path in repos:
                q = next((qq for qq in fixture.questions if qq.id == run["question_id"]), None)
                if q is None:
                    continue
                score_rec = next(s for s in all_scores
                                 if s["question_id"] == run["question_id"] and s["arm"] == run["arm"])
                citations = [{"file": c.file, "line_range": list(c.line_range), "symbol": c.symbol}
                             for c in q.expected.citations]
                mech_context = {
                    "citation_hit": score_rec["citation_hit"],
                    "hallucinated_citation": score_rec["hallucinated"],
                    "forbidden_hit": score_rec["forbidden_hit"],
                }
                agg = judge_mod.judge_all(
                    client=judge_client,
                    question_id=q.id,
                    question=q.question,
                    rubric=q.rubric,
                    citations=citations,
                    mech_context=mech_context,
                    answer=run["final_answer"],
                )
                judge_records.append({
                    "arm": run["arm"],
                    "question_id": q.id,
                    "scores": agg.scores,
                    "justifications": agg.justifications,
                    "median": agg.median,
                    "range": agg.range,
                })
                score_rec["judge_median"] = agg.median
                score_rec["judge_range"] = agg.range

    _write_jsonl(results_dir / "runs.jsonl", all_runs)
    _write_jsonl(results_dir / "judge.jsonl", judge_records)
    _write_jsonl(results_dir / "scores.json", all_scores)

    return {
        "n_runs": len(all_runs),
        "n_judged": len(judge_records),
        "scores": all_scores,
    }

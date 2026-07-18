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
import subprocess
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from bench import arms as arms_mod
from bench import config as config_mod
from bench import daemon as daemon_mod
from bench import judge as judge_mod
from bench import repos as repos_mod
from bench import reuse as reuse_mod
from bench import runner, score
from bench.fixtures_io import Fixture, Question


META_SCHEMA_VERSION = 1


def _append_jsonl(path: Path, record: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as f:
        f.write(json.dumps(record, default=str) + "\n")


def _read_jsonl(path: Path) -> list[dict]:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def _key(rec: dict) -> tuple[str, str, int]:
    return (rec["arm"], rec["question_id"], rec.get("rep", 0))


def _dedupe_last(records: list[dict]) -> list[dict]:
    """Keep the last record per (arm, question, rep). Appended re-runs and
    re-judgements come later in the file, so last-wins is retry-wins."""
    by_key: dict[tuple[str, str, int], dict] = {}
    for r in records:
        by_key[_key(r)] = r
    return list(by_key.values())


def _write_jsonl(path: Path, records: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        for r in records:
            f.write(json.dumps(r, default=str) + "\n")


def _run_record(run: runner.Run, repo_name: str, rep: int = 0) -> dict:
    return {
        "arm": run.arm,
        "question_id": run.question_id,
        "repo": repo_name,
        "rep": rep,
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


def _safe_daemon_git_sha() -> str | None:
    try:
        return repos_mod.current_daemon_sha()
    except (OSError, subprocess.CalledProcessError):
        return None


def _safe_daemon_binary_sha256() -> str | None:
    try:
        return repos_mod.daemon_binary_sha256()
    except OSError:
        return None


def _comparator(arms_to_run: list[arms_mod.Arm]) -> dict:
    arm_names = [arm.name for arm in arms_to_run]
    baseline = "default" if "default" in arm_names else (arm_names[0] if arm_names else None)
    return {
        "baseline_arm": baseline,
        "candidate_arms": [name for name in arm_names if name != baseline],
    }


def _cycle_metadata(
    arms_to_run: list[arms_mod.Arm],
    repos: list[tuple[Fixture, Path]],
    *,
    judge_enabled: bool,
    repeats: int,
) -> dict:
    daemon_git_sha = _safe_daemon_git_sha()
    daemon_binary_sha256 = _safe_daemon_binary_sha256()
    return {
        "schema_version": META_SCHEMA_VERSION,
        "daemon": {
            "git_sha": daemon_git_sha,
            "binary_sha256": daemon_binary_sha256,
        },
        "fixtures": [
            {
                "repo": fixture.meta.repo,
                "upstream_url": fixture.meta.upstream_url,
                "upstream_sha": fixture.meta.upstream_sha,
                "fixture_sha256": fixture.meta.fixture_sha256,
                "authored_at": fixture.meta.authored_at,
                "authored_against_schema_version": (
                    fixture.meta.authored_against_schema_version
                ),
                "question_ids": sorted(q.id for q in fixture.questions),
            }
            for fixture, _repo_path in sorted(repos, key=lambda entry: entry[0].meta.repo)
        ],
        "configuration": {
            "arms": [
                {
                    "name": arm.name,
                    "needs_daemon": arm.needs_daemon,
                    "is_codegraph": arm.is_codegraph,
                    "index_variant": arm.index_variant,
                    "daemon_env": dict(sorted(arm.daemon_env.items())),
                    "allowed_tools": list(arm.allowed_tools),
                    "tool_guidance": arm.tool_guidance,
                }
                for arm in arms_to_run
            ],
            "repeats": max(1, repeats),
            "judge_enabled": judge_enabled,
            "max_turns": config_mod.MAX_TURNS,
            "per_question_timeout_s": config_mod.PER_QUESTION_TIMEOUT_S,
            "run_concurrency": config_mod.RUN_CONCURRENCY,
            "judge_concurrency": config_mod.JUDGE_CONCURRENCY,
            "run_reuse": config_mod.RUN_REUSE,
            "judge_tiered": config_mod.JUDGE_TIERED,
        },
        "models": {
            "agent": config_mod.AGENT_MODEL,
            "judges": {
                "haiku": config_mod.JUDGE_HAIKU,
                "sonnet": config_mod.JUDGE_SONNET,
                "opus": config_mod.JUDGE_OPUS,
            },
        },
        "binaries": {
            "agent_cli": reuse_mod.binary_version(config_mod.CLAUDE_BINARY),
            "codegraph": (
                reuse_mod.binary_version(config_mod.CODEGRAPH_BINARY)
                if any(arm.is_codegraph for arm in arms_to_run)
                else None
            ),
        },
        "comparator": _comparator(arms_to_run),
    }


def _persist_cycle_metadata(results_dir: Path, metadata: dict, *, resume: bool) -> None:
    """Write immutable round provenance before executing the first sample."""
    path = results_dir / "meta.json"
    if path.exists() and resume:
        existing = json.loads(path.read_text())
        if existing != metadata:
            raise ValueError(
                "benchmark round metadata differs from the persisted provenance; "
                "start a new round instead of mixing revisions or configurations"
            )
        return
    path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n")


def _score_record(
    q: Question, rec: dict, repo_path: Path, read_lines=None, list_files=None
) -> dict:
    answer = rec.get("final_answer", "")
    multiplier, verifications = score.compute_citation_multiplier(
        answer, repo_path, read_lines, list_files
    )
    final_mech = score.final_mech(q, answer, multiplier)
    raw = score.mech_score(q, answer)
    forbidden = score.forbidden_hits(q, answer)
    return {
        "question_id": q.id,
        "task_type": q.task_type,
        "arm": rec["arm"],
        "repo": rec["repo"],
        "rep": rec.get("rep", 0),
        "mech": final_mech,
        "mech_raw": raw["raw"],
        "citation_hit": raw["citation_hit"],
        "citation_multiplier": multiplier,
        "hallucinated": any(not v.ok for v in verifications),
        "imprecise_citations": sum(1 for v in verifications if v.imprecise),
        "forbidden_hit": bool(forbidden),
        "tool_calls": len(rec.get("tool_calls", [])),
        # True product usage: rows with 0 answered the question without ever
        # calling code-intelligence (greppable questions), so their score
        # cannot reflect product changes. Gate aggregates report both scopes.
        "mcp_tool_calls": sum(
            1
            for t in rec.get("tool_calls", [])
            if str(t.get("name", "")).startswith("mcp__code-intelligence")
        ),
        "input_tokens": rec.get("input_tokens", 0) + rec.get("cache_read_tokens", 0),
        "wall_ms": rec.get("wall_ms", 0),
        "run_error": rec.get("run_error"),
        "hit_turn_cap": rec.get("stop_reason") == "max_turns",
        "judge_median": None,
        "judge_range": None,
        "judge_casualty": False,
    }


def _is_run_casualty(rec: dict) -> bool:
    return bool(rec.get("run_error")) or not str(rec.get("final_answer", "")).strip()


def _compute_run_keys(
    arms_to_run: list[arms_mod.Arm],
    repos: list[tuple[Fixture, Path]],
    repeats: int,
) -> dict[tuple[str, str, int], str | None]:
    """run_key per (arm, question_id, rep) slot; None where reuse is unsound.

    None cases: the smoke fixture (runs against this repo's live working tree,
    so the pinned SHA does not describe the content) and daemon arms when the
    daemon binary is missing (index provenance cannot be attested; the run
    itself would fail at daemon start anyway).
    """
    try:
        daemon_bin: str | None = repos_mod.daemon_binary_hash()
    except OSError:
        daemon_bin = None
    cli_version = reuse_mod.binary_version(config_mod.CLAUDE_BINARY)

    keys: dict[tuple[str, str, int], str | None] = {}
    for arm in arms_to_run:
        for fixture, repo_path in repos:
            unsound = (
                repo_path.resolve() == config_mod.REPO_ROOT.resolve()
                or (arm.needs_daemon and daemon_bin is None)
            )
            for q in fixture.questions:
                key = None if unsound else reuse_mod.run_key(
                    arm,
                    q.id,
                    q.question,
                    fixture.meta.upstream_sha,
                    model=config_mod.AGENT_MODEL,
                    max_turns=config_mod.MAX_TURNS,
                    daemon_bin=daemon_bin if arm.needs_daemon else None,
                    cli_version=cli_version,
                )
                for rep in range(max(1, repeats)):
                    keys[(arm.name, q.id, rep)] = key
    return keys


def run_cycle(
    *,
    arms_to_run: list[arms_mod.Arm],
    repos: list[tuple[Fixture, Path]],
    results_dir: Path,
    judge_enabled: bool = False,
    resume: bool = True,
    repeats: int = 1,
) -> dict:
    """Run one bench cycle. Returns a summary dict; writes results to results_dir."""
    results_dir.mkdir(parents=True, exist_ok=True)
    cycle_metadata = _cycle_metadata(
        arms_to_run,
        repos,
        judge_enabled=judge_enabled,
        repeats=repeats,
    )
    _persist_cycle_metadata(results_dir, cycle_metadata, resume=resume)
    transcripts_dir = results_dir / "transcripts"
    runs_path = results_dir / "runs.jsonl"
    judge_path = results_dir / "judge.jsonl"

    # (repo_name, question_id) -> (Question, repo_path); used to re-score resumed runs.
    qmap: dict[tuple[str, str], tuple[Question, Path]] = {
        (fixture.meta.repo, q.id): (q, repo_path)
        for fixture, repo_path in repos
        for q in fixture.questions
    }

    all_runs: list[dict] = _dedupe_last(_read_jsonl(runs_path)) if resume else []
    # Errored runs are NOT done: a resume after quota exhaustion re-runs them.
    done = {_key(r) for r in all_runs if not r.get("run_error")}

    # Content-addressed keys for every slot this cycle could run. Also used to
    # stamp run_key on fresh records so FUTURE rounds can adopt them.
    run_keys = _compute_run_keys(arms_to_run, repos, repeats)

    # Adopt prior-round records for slots whose key matches (see bench/reuse.py).
    n_reused = 0
    if config_mod.RUN_REUSE:
        wanted = {
            slot: key for slot, key in run_keys.items()
            if key is not None and slot not in done
        }
        for slot, rec in sorted(
            reuse_mod.find_reusable(results_dir.parent, results_dir, wanted).items()
        ):
            _append_jsonl(runs_path, rec)
            all_runs.append(rec)
            done.add(slot)
            n_reused += 1

    consecutive_failures = 0
    abort = threading.Event()

    for arm in arms_to_run:
        if abort.is_set():
            break
        pending = [
            (fixture, repo_path, q, rep)
            for fixture, repo_path in repos
            for q in fixture.questions
            for rep in range(max(1, repeats))
            if (arm.name, q.id, rep) not in done
        ]
        if not pending:
            continue

        port = daemon_mod.pick_free_port() if arm.needs_daemon else 0
        if arm.needs_daemon:
            home = config_mod.bench_home_for_variant(arm.index_variant) if arm.index_variant else None
            daemon = daemon_mod.maybe_start_daemon(arm, port=port, home=home)
        else:
            daemon = None

        write_lock = threading.Lock()

        def _execute(item, _arm=arm, _daemon=daemon):
            nonlocal consecutive_failures
            if abort.is_set():
                return
            fixture, repo_path, q, rep = item
            # rep 0 keeps the historical transcript layout; higher reps get
            # their own subtree so transcripts are never overwritten.
            tdir = transcripts_dir if rep == 0 else transcripts_dir / f"rep{rep}"
            run = runner.run_question(
                arm=_arm, q=q, daemon=_daemon,
                repo_path=repo_path, transcripts_dir=tdir,
            )
            rec = _run_record(run, fixture.meta.repo, rep=rep)
            rec["run_key"] = run_keys.get((_arm.name, q.id, rep))
            rec["reused_from"] = None
            rec["fixture_sha256"] = fixture.meta.fixture_sha256
            rec["fixture_upstream_sha"] = fixture.meta.upstream_sha
            rec["daemon_sha"] = (
                cycle_metadata["daemon"]["git_sha"] if _arm.needs_daemon else None
            )
            rec["daemon_binary_sha256"] = (
                cycle_metadata["daemon"]["binary_sha256"]
                if _arm.needs_daemon
                else None
            )
            with write_lock:
                if run.run_error:
                    consecutive_failures += 1
                    if consecutive_failures >= config_mod.MAX_CONSECUTIVE_FAILURES:
                        abort.set()
                else:
                    consecutive_failures = 0
                _append_jsonl(runs_path, rec)
                all_runs.append(rec)

        try:
            workers = max(1, config_mod.RUN_CONCURRENCY)
            with ThreadPoolExecutor(max_workers=workers) as pool:
                list(pool.map(_execute, pending))
        finally:
            if daemon is not None:
                daemon.stop()

    # Scoring is derived: recomputed for every run (including resumed ones).
    # Dedupe keeps the freshest attempt per (arm, question, rep).
    all_runs = _dedupe_last(all_runs)
    all_scores: list[dict] = []
    score_by_key: dict[tuple[str, str, int], dict] = {}
    for rec in all_runs:
        entry = qmap.get((rec["repo"], rec["question_id"]))
        if entry is None:
            continue
        q, repo_path = entry
        s = _score_record(q, rec, repo_path)
        all_scores.append(s)
        score_by_key[(rec["arm"], rec["question_id"], rec.get("rep", 0))] = s

    # Judging (skipped if judge_enabled is False). Casualty runs (empty answer or
    # run_error) are never judged: 3 judge calls to confirm a known-zero is waste.
    judge_records: list[dict] = _dedupe_last(_read_jsonl(judge_path)) if resume else []
    # Rows judged cleanly are done; casualties caused by judge errors (quota
    # exhaustion) are re-judged on resume.
    judged = {_key(j) for j in judge_records
              if not (j.get("casualty") and j.get("errors"))}
    judge_consecutive = 0
    judge_abort = threading.Event()
    if judge_enabled:
        pending_judge: list[dict] = []
        for rec in all_runs:
            key = _key(rec)
            if key in judged or key not in score_by_key:
                continue
            if _is_run_casualty(rec):
                score_by_key[key]["judge_casualty"] = True
                continue
            pending_judge.append(rec)

        judge_lock = threading.Lock()

        def _judge(rec):
            nonlocal judge_consecutive
            if judge_abort.is_set():
                return
            key = _key(rec)
            q, _repo_path = qmap[(rec["repo"], rec["question_id"])]
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
                "rep": rec.get("rep", 0),
                "scores": agg.scores,
                "justifications": agg.justifications,
                "median": agg.median,
                "range": agg.range,
                "errors": agg.errors,
                "n_valid": agg.n_valid,
                "casualty": agg.casualty,
                "tier": agg.tier,
            }
            with judge_lock:
                if agg.n_valid == 0:
                    judge_consecutive += 1
                    if judge_consecutive >= config_mod.MAX_CONSECUTIVE_FAILURES:
                        judge_abort.set()
                else:
                    judge_consecutive = 0
                _append_jsonl(judge_path, jrec)
                judge_records.append(jrec)

        if pending_judge:
            workers = max(1, config_mod.JUDGE_CONCURRENCY)
            with ThreadPoolExecutor(max_workers=workers) as pool:
                list(pool.map(_judge, pending_judge))

    # Apply judge results (fresh + resumed) to score rows. Casualty rows keep
    # judge_median=None so aggregates skip them instead of averaging zeros.
    # Dedupe last-wins so a fresh re-judgement replaces an earlier casualty.
    for jrec in _dedupe_last(judge_records):
        s = score_by_key.get(_key(jrec))
        if s is None:
            continue
        if jrec.get("casualty"):
            s["judge_casualty"] = True
        else:
            s["judge_median"] = jrec["median"]
            s["judge_range"] = jrec["range"]
            s["judge_casualty"] = False

    _write_jsonl(results_dir / "scores.json", all_scores)

    return {
        "n_runs": len(all_runs),
        "n_reused": n_reused,
        "n_judged": len(judge_records),
        "scores": all_scores,
        "aborted": abort.is_set(),
        "judge_aborted": judge_abort.is_set(),
    }

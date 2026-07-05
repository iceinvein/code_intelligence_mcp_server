"""Tests for bench/orchestrator.py - the end-to-end cycle (mocked)."""
import json
from pathlib import Path
from unittest.mock import MagicMock

import pytest

from bench import arms, fixtures_io, judge, orchestrator, runner


def _smoke_fixture():
    fixture_path = Path(__file__).resolve().parents[1] / "fixtures" / "smoke.yaml"
    return fixtures_io.load_fixture(fixture_path)


def test_orchestrator_runs_all_arms_in_order(monkeypatch, tmp_path):
    arms_ordered = ["default", "code_intel_full", "codegraph"]
    arm_calls = []

    def fake_run_question(arm, q, daemon, repo_path, transcripts_dir):
        arm_calls.append((arm.name, q.id))
        return runner.Run(
            arm=arm.name,
            question_id=q.id,
            repo=str(repo_path),
            final_answer="src/indexer/pipeline/mod.rs:85 IndexPipeline struct",
            stop_reason="end_turn",
            model="x",
        )
    monkeypatch.setattr(runner, "run_question", fake_run_question)

    monkeypatch.setattr(
        "bench.daemon.maybe_start_daemon",
        lambda arm, port, home=None: None if not arm.needs_daemon else MagicMock(stop=lambda: None, build_mcp_config=lambda: {}),
    )

    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]
    results_dir = tmp_path / "R001"

    summary = orchestrator.run_cycle(
        arms_to_run=[arms.ARMS[n] for n in arms_ordered],
        repos=[(fixture, repo_path)],
        results_dir=results_dir,
        judge_enabled=False,
    )

    expected_pairs = [(a, q.id) for a in arms_ordered for q in fixture.questions]
    assert set(arm_calls) == set(expected_pairs)
    assert (results_dir / "runs.jsonl").exists()


def _fake_run(arm, q, repo_path, answer="src/indexer/pipeline/mod.rs:85 IndexPipeline struct",
              run_error=None):
    return runner.Run(
        arm=arm.name, question_id=q.id, repo=str(repo_path),
        final_answer=answer, stop_reason="end_turn", model="x", run_error=run_error,
    )


def _no_daemon(monkeypatch):
    monkeypatch.setattr(
        "bench.daemon.maybe_start_daemon",
        lambda arm, port, home=None: None,
    )


def test_orchestrator_persists_runs_incrementally_on_crash(monkeypatch, tmp_path):
    """A crash mid-cycle must not lose completed runs (the R009 failure)."""
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]
    calls = {"n": 0}

    def fake_run_question(arm, q, daemon, repo_path, transcripts_dir):
        calls["n"] += 1
        if calls["n"] == 3:
            raise RuntimeError("boom")
        return _fake_run(arm, q, repo_path)

    monkeypatch.setattr(runner, "run_question", fake_run_question)
    _no_daemon(monkeypatch)

    results_dir = tmp_path / "R001"
    with pytest.raises(RuntimeError):
        orchestrator.run_cycle(
            arms_to_run=[arms.ARMS["default"]],
            repos=[(fixture, repo_path)],
            results_dir=results_dir,
            judge_enabled=False,
        )
    lines = (results_dir / "runs.jsonl").read_text().splitlines()
    assert len(lines) == 2  # the two completed runs survived


def test_orchestrator_resume_skips_completed_runs(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]
    first_q = fixture.questions[0]

    results_dir = tmp_path / "R001"
    results_dir.mkdir(parents=True)
    prior = {
        "arm": "default", "question_id": first_q.id, "repo": fixture.meta.repo,
        "final_answer": "prior answer src/indexer/pipeline/mod.rs:85",
        "tool_calls": [], "input_tokens": 1, "output_tokens": 1,
        "cache_read_tokens": 0, "cache_creation_tokens": 0,
        "wall_ms": 5, "stop_reason": "end_turn", "model": "x", "run_error": None,
    }
    (results_dir / "runs.jsonl").write_text(json.dumps(prior) + "\n")

    ran = []

    def fake_run_question(arm, q, daemon, repo_path, transcripts_dir):
        ran.append(q.id)
        return _fake_run(arm, q, repo_path)

    monkeypatch.setattr(runner, "run_question", fake_run_question)
    _no_daemon(monkeypatch)

    summary = orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"]],
        repos=[(fixture, repo_path)],
        results_dir=results_dir,
        judge_enabled=False,
    )
    assert first_q.id not in ran  # resumed, not re-run
    assert len(ran) == len(fixture.questions) - 1
    # the resumed run still gets a score row
    scored_ids = {s["question_id"] for s in summary["scores"]}
    assert first_q.id in scored_ids


def test_orchestrator_skips_judging_empty_and_errored_answers(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]
    bad_q = fixture.questions[0].id

    def fake_run_question(arm, q, daemon, repo_path, transcripts_dir):
        if q.id == bad_q:
            return _fake_run(arm, q, repo_path, answer="", run_error="timeout")
        return _fake_run(arm, q, repo_path)

    monkeypatch.setattr(runner, "run_question", fake_run_question)
    _no_daemon(monkeypatch)

    judged = []

    def fake_judge_all(**kwargs):
        judged.append(kwargs["question_id"])
        return judge.JudgeAggregate(
            question_id=kwargs["question_id"], scores={"haiku": 7},
            justifications={}, median=7.0, range=0, errors={}, n_valid=3,
        )

    monkeypatch.setattr(orchestrator.judge_mod, "judge_all", fake_judge_all)

    summary = orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"]],
        repos=[(fixture, repo_path)],
        results_dir=tmp_path / "R001",
        judge_enabled=True,
    )
    assert bad_q not in judged
    bad_score = next(s for s in summary["scores"] if s["question_id"] == bad_q)
    assert bad_score["judge_median"] is None
    assert bad_score["judge_casualty"] is True
    assert bad_score["run_error"] == "timeout"


def test_orchestrator_runs_questions_concurrently(monkeypatch, tmp_path):
    import threading
    import time as _time

    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]

    state = {"active": 0, "max_active": 0}
    lock = threading.Lock()

    def fake_run_question(arm, q, daemon, repo_path, transcripts_dir):
        with lock:
            state["active"] += 1
            state["max_active"] = max(state["max_active"], state["active"])
        _time.sleep(0.05)
        with lock:
            state["active"] -= 1
        return _fake_run(arm, q, repo_path)

    monkeypatch.setattr(runner, "run_question", fake_run_question)
    monkeypatch.setattr(orchestrator.config_mod, "RUN_CONCURRENCY", 3)
    _no_daemon(monkeypatch)

    results_dir = tmp_path / "R001"
    summary = orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"]],
        repos=[(fixture, repo_path)],
        results_dir=results_dir,
        judge_enabled=False,
    )
    assert state["max_active"] > 1  # questions overlapped
    lines = (results_dir / "runs.jsonl").read_text().splitlines()
    assert len(lines) == len(fixture.questions)  # thread-safe appends, no loss
    assert summary["n_runs"] == len(fixture.questions)


def test_orchestrator_judges_concurrently(monkeypatch, tmp_path):
    import threading
    import time as _time

    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]

    monkeypatch.setattr(
        runner, "run_question",
        lambda arm, q, daemon, repo_path, transcripts_dir: _fake_run(arm, q, repo_path),
    )
    _no_daemon(monkeypatch)
    monkeypatch.setattr(orchestrator.config_mod, "JUDGE_CONCURRENCY", 3)

    state = {"active": 0, "max_active": 0}
    lock = threading.Lock()

    def fake_judge_all(**kwargs):
        with lock:
            state["active"] += 1
            state["max_active"] = max(state["max_active"], state["active"])
        _time.sleep(0.05)
        with lock:
            state["active"] -= 1
        return judge.JudgeAggregate(
            question_id=kwargs["question_id"], scores={"haiku": 7},
            justifications={}, median=7.0, range=0, errors={}, n_valid=3,
        )

    monkeypatch.setattr(orchestrator.judge_mod, "judge_all", fake_judge_all)

    results_dir = tmp_path / "R001"
    summary = orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"]],
        repos=[(fixture, repo_path)],
        results_dir=results_dir,
        judge_enabled=True,
    )
    assert state["max_active"] > 1
    assert summary["n_judged"] == len(fixture.questions)
    jlines = (results_dir / "judge.jsonl").read_text().splitlines()
    assert len(jlines) == len(fixture.questions)


def test_orchestrator_repeats_run_each_question_n_times(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]
    ran = []

    def fake_run_question(arm, q, daemon, repo_path, transcripts_dir):
        ran.append(q.id)
        return _fake_run(arm, q, repo_path)

    monkeypatch.setattr(runner, "run_question", fake_run_question)
    _no_daemon(monkeypatch)

    results_dir = tmp_path / "R001"
    summary = orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"]],
        repos=[(fixture, repo_path)],
        results_dir=results_dir,
        judge_enabled=False,
        repeats=2,
    )
    nq = len(fixture.questions)
    assert len(ran) == nq * 2
    rows = [json.loads(l) for l in (results_dir / "runs.jsonl").read_text().splitlines()]
    assert len(rows) == nq * 2
    reps = {(r["question_id"], r["rep"]) for r in rows}
    assert len(reps) == nq * 2  # each (question, rep) pair distinct
    assert summary["n_runs"] == nq * 2
    assert len(summary["scores"]) == nq * 2


def test_orchestrator_repeats_resume_skips_completed_reps(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]
    first_q = fixture.questions[0]

    results_dir = tmp_path / "R001"
    results_dir.mkdir(parents=True)
    prior = {
        "arm": "default", "question_id": first_q.id, "repo": fixture.meta.repo,
        "rep": 0,
        "final_answer": "prior", "tool_calls": [], "input_tokens": 1,
        "output_tokens": 1, "cache_read_tokens": 0, "cache_creation_tokens": 0,
        "wall_ms": 5, "stop_reason": "end_turn", "model": "x", "run_error": None,
    }
    (results_dir / "runs.jsonl").write_text(json.dumps(prior) + "\n")

    ran = []

    def fake_run_question(arm, q, daemon, repo_path, transcripts_dir):
        ran.append(q.id)
        return _fake_run(arm, q, repo_path)

    monkeypatch.setattr(runner, "run_question", fake_run_question)
    _no_daemon(monkeypatch)

    orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"]],
        repos=[(fixture, repo_path)],
        results_dir=results_dir,
        judge_enabled=False,
        repeats=2,
    )
    # first_q rep0 resumed; rep1 plus both reps of the others run fresh
    assert ran.count(first_q.id) == 1


def test_orchestrator_persists_judge_errors_and_casualties(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]

    monkeypatch.setattr(
        runner, "run_question",
        lambda arm, q, daemon, repo_path, transcripts_dir: _fake_run(arm, q, repo_path),
    )
    _no_daemon(monkeypatch)

    def fake_judge_all(**kwargs):
        return judge.JudgeAggregate(
            question_id=kwargs["question_id"], scores={"haiku": 7, "sonnet": 0, "opus": 0},
            justifications={"haiku": "ok"}, median=7.0, range=0,
            errors={"sonnet": "timeout", "opus": "parse_failed"}, n_valid=1, casualty=True,
        )

    monkeypatch.setattr(orchestrator.judge_mod, "judge_all", fake_judge_all)

    results_dir = tmp_path / "R001"
    summary = orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"]],
        repos=[(fixture, repo_path)],
        results_dir=results_dir,
        judge_enabled=True,
    )
    jrec = json.loads((results_dir / "judge.jsonl").read_text().splitlines()[0])
    assert jrec["errors"] == {"sonnet": "timeout", "opus": "parse_failed"}
    assert jrec["n_valid"] == 1
    assert jrec["casualty"] is True
    # casualty judge results must not pollute score aggregates
    for s in summary["scores"]:
        assert s["judge_median"] is None
        assert s["judge_casualty"] is True

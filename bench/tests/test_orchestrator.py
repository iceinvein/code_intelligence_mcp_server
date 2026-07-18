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


def test_orchestrator_persists_exact_round_provenance(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]
    monkeypatch.setattr(
        runner,
        "run_question",
        lambda arm, q, daemon, repo_path, transcripts_dir: _fake_run(
            arm, q, repo_path
        ),
    )
    _no_daemon(monkeypatch)
    monkeypatch.setattr(orchestrator.repos_mod, "current_daemon_sha", lambda: "git123")
    monkeypatch.setattr(orchestrator.repos_mod, "daemon_binary_sha256", lambda: "bin123")
    monkeypatch.setattr(
        orchestrator.reuse_mod, "binary_version", lambda binary: f"{binary}-v1"
    )

    results_dir = tmp_path / "R001"
    orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"], arms.ARMS["code_intel_shipped"]],
        repos=[(fixture, repo_path)],
        results_dir=results_dir,
        judge_enabled=False,
        repeats=2,
    )

    meta = json.loads((results_dir / "meta.json").read_text())
    assert meta["daemon"] == {"git_sha": "git123", "binary_sha256": "bin123"}
    assert meta["fixtures"][0]["fixture_sha256"] == fixture.meta.fixture_sha256
    assert meta["configuration"]["repeats"] == 2
    assert meta["models"]["agent"] == orchestrator.config_mod.AGENT_MODEL
    assert meta["comparator"] == {
        "baseline_arm": "default",
        "candidate_arms": ["code_intel_shipped"],
    }
    run_records = [
        json.loads(line) for line in (results_dir / "runs.jsonl").read_text().splitlines()
    ]
    shipped = next(rec for rec in run_records if rec["arm"] == "code_intel_shipped")
    assert shipped["daemon_sha"] == "git123"
    assert shipped["daemon_binary_sha256"] == "bin123"
    assert shipped["fixture_sha256"] == fixture.meta.fixture_sha256


def test_orchestrator_rejects_resume_with_changed_provenance(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    results_dir = tmp_path / "R001"
    results_dir.mkdir()
    (results_dir / "meta.json").write_text("{}")
    monkeypatch.setattr(orchestrator.repos_mod, "current_daemon_sha", lambda: "git123")
    monkeypatch.setattr(orchestrator.repos_mod, "daemon_binary_sha256", lambda: "bin123")

    with pytest.raises(ValueError, match="metadata differs"):
        orchestrator.run_cycle(
            arms_to_run=[arms.ARMS["default"]],
            repos=[(fixture, Path(__file__).resolve().parents[2])],
            results_dir=results_dir,
            judge_enabled=False,
        )


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


def test_resume_reruns_errored_records(monkeypatch, tmp_path):
    """A run that failed (run_error set) must be re-run on resume, and the fresh
    record must win in scoring (quota-exhaustion recovery)."""
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]
    first_q = fixture.questions[0]

    results_dir = tmp_path / "R001"
    results_dir.mkdir(parents=True)
    failed = {
        "arm": "default", "question_id": first_q.id, "repo": fixture.meta.repo,
        "rep": 0, "final_answer": "", "tool_calls": [], "input_tokens": 0,
        "output_tokens": 0, "cache_read_tokens": 0, "cache_creation_tokens": 0,
        "wall_ms": 5, "stop_reason": "cli_error", "model": "x",
        "run_error": "cli_exit_1",
    }
    (results_dir / "runs.jsonl").write_text(json.dumps(failed) + "\n")

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
    assert first_q.id in ran  # errored record re-run
    # one score row per (arm, question, rep), and it reflects the fresh run
    rows = [s for s in summary["scores"] if s["question_id"] == first_q.id]
    assert len(rows) == 1
    assert rows[0]["run_error"] is None


def test_circuit_breaker_aborts_after_consecutive_run_failures(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]
    ran = []

    def fake_run_question(arm, q, daemon, repo_path, transcripts_dir):
        ran.append((arm.name, q.id))
        return _fake_run(arm, q, repo_path, answer="", run_error="cli_exit_1")

    monkeypatch.setattr(runner, "run_question", fake_run_question)
    monkeypatch.setattr(orchestrator.config_mod, "RUN_CONCURRENCY", 1)
    monkeypatch.setattr(orchestrator.config_mod, "MAX_CONSECUTIVE_FAILURES", 3)
    _no_daemon(monkeypatch)

    summary = orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"], arms.ARMS["codegraph"]],
        repos=[(fixture, repo_path)],
        results_dir=tmp_path / "R001",
        judge_enabled=False,
    )
    assert summary["aborted"] is True
    assert len(ran) == 3  # stopped at the threshold, second arm never started


def test_judge_circuit_breaker_aborts_after_consecutive_failures(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]

    monkeypatch.setattr(
        runner, "run_question",
        lambda arm, q, daemon, repo_path, transcripts_dir: _fake_run(arm, q, repo_path),
    )
    _no_daemon(monkeypatch)
    monkeypatch.setattr(orchestrator.config_mod, "JUDGE_CONCURRENCY", 1)
    monkeypatch.setattr(orchestrator.config_mod, "MAX_CONSECUTIVE_FAILURES", 2)

    judged = []

    def fake_judge_all(**kwargs):
        judged.append(kwargs["question_id"])
        return judge.JudgeAggregate(
            question_id=kwargs["question_id"], scores={},
            justifications={}, median=0.0, range=0,
            errors={"haiku": "timeout", "sonnet": "timeout", "opus": "timeout"},
            n_valid=0, casualty=True,
        )

    monkeypatch.setattr(orchestrator.judge_mod, "judge_all", fake_judge_all)

    summary = orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"]],
        repos=[(fixture, repo_path)],
        results_dir=tmp_path / "R001",
        judge_enabled=True,
    )
    assert summary["judge_aborted"] is True
    assert len(judged) == 2  # stopped at the threshold


def test_resume_rejudges_error_casualties(monkeypatch, tmp_path):
    """Judge rows that were casualties due to judge errors must be re-judged on
    resume; cleanly judged rows must not be."""
    fixture = _smoke_fixture()
    repo_path = Path(__file__).resolve().parents[2]
    q0, q1 = fixture.questions[0], fixture.questions[1]

    results_dir = tmp_path / "R001"
    results_dir.mkdir(parents=True)
    runs = [
        {"arm": "default", "question_id": q.id, "repo": fixture.meta.repo, "rep": 0,
         "final_answer": "src/indexer/pipeline/mod.rs:85", "tool_calls": [],
         "input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 0,
         "cache_creation_tokens": 0, "wall_ms": 5, "stop_reason": "end_turn",
         "model": "x", "run_error": None}
        for q in fixture.questions
    ]
    (results_dir / "runs.jsonl").write_text("".join(json.dumps(r) + "\n" for r in runs))
    judge_rows = [
        {"arm": "default", "question_id": q0.id, "repo": fixture.meta.repo, "rep": 0,
         "scores": {"haiku": 8}, "justifications": {}, "median": 8.0, "range": 0,
         "errors": {}, "n_valid": 1, "casualty": False},
        {"arm": "default", "question_id": q1.id, "repo": fixture.meta.repo, "rep": 0,
         "scores": {}, "justifications": {}, "median": 0.0, "range": 0,
         "errors": {"haiku": "timeout"}, "n_valid": 0, "casualty": True},
    ]
    (results_dir / "judge.jsonl").write_text("".join(json.dumps(j) + "\n" for j in judge_rows))

    monkeypatch.setattr(
        runner, "run_question",
        lambda arm, q, daemon, repo_path, transcripts_dir: _fake_run(arm, q, repo_path),
    )
    _no_daemon(monkeypatch)

    judged = []

    def fake_judge_all(**kwargs):
        judged.append(kwargs["question_id"])
        return judge.JudgeAggregate(
            question_id=kwargs["question_id"], scores={"haiku": 7},
            justifications={}, median=7.0, range=0, errors={}, n_valid=1,
        )

    monkeypatch.setattr(orchestrator.judge_mod, "judge_all", fake_judge_all)

    summary = orchestrator.run_cycle(
        arms_to_run=[arms.ARMS["default"]],
        repos=[(fixture, repo_path)],
        results_dir=results_dir,
        judge_enabled=True,
    )
    assert q0.id not in judged  # cleanly judged: skipped
    assert q1.id in judged      # error casualty: re-judged
    s1 = next(s for s in summary["scores"] if s["question_id"] == q1.id)
    assert s1["judge_median"] == 7.0  # fresh judgement wins over the casualty row


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


def test_score_record_counts_mcp_tool_calls(tmp_path):
    """scores.json rows carry true code-intelligence MCP usage per run.

    scores.json `tool_calls` counts every tool (Grep, ToolSearch, ...); a
    product gate needs to know whether the product was exercised at all,
    so `mcp_tool_calls` counts only code-intelligence MCP calls.
    """
    q = fixtures_io.Question(
        id="t-1",
        task_type="symbol_lookup",
        difficulty="easy",
        question="?",
        rubric="r",
        expected=fixtures_io.Expected(
            citations=[], files=[], facts=["bar"], forbidden=[],
            forbidden_strict=False,
        ),
    )
    rec = {
        "arm": "code_intel_shipped", "repo": "smoke", "rep": 0,
        "final_answer": "bar",
        "tool_calls": [
            {"name": "ToolSearch", "input_summary": ""},
            {"name": "Grep", "input_summary": ""},
            {"name": "mcp__code-intelligence__ask_code", "input_summary": ""},
            {"name": "mcp__code-intelligence__search_code", "input_summary": ""},
        ],
    }
    s = orchestrator._score_record(q, rec, tmp_path)
    assert s["tool_calls"] == 4
    assert s["mcp_tool_calls"] == 2

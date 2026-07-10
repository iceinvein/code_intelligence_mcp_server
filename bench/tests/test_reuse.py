"""Tests for bench/reuse.py and its orchestrator wiring."""
import json
from pathlib import Path

from bench import arms, config, fixtures_io, orchestrator, reuse, runner


def _key(arm_name="default", qid="q1", text="Where is X?", sha="abc123", **over):
    kwargs = dict(model="m1", max_turns=16, daemon_bin=None, cli_version="1.0.0")
    kwargs.update(over)
    return reuse.run_key(arms.ARMS[arm_name], qid, text, sha, **kwargs)


def test_run_key_deterministic():
    assert _key() == _key()


def test_run_key_sensitive_to_each_material_input():
    base = _key()
    assert _key(qid="q2") != base
    assert _key(text="Where is Y?") != base
    assert _key(sha="def456") != base
    assert _key(model="m2") != base
    assert _key(max_turns=12) != base
    assert _key(daemon_bin="deadbeef") != base
    assert _key(cli_version="2.0.0") != base
    assert _key(arm_name="code_intel_shipped", daemon_bin="deadbeef") != _key(
        arm_name="code_intel_full", daemon_bin="deadbeef"
    )


def _write_round(root: Path, name: str, records: list[dict]) -> Path:
    round_dir = root / name
    round_dir.mkdir(parents=True)
    with (round_dir / "runs.jsonl").open("w") as f:
        for r in records:
            f.write(json.dumps(r) + "\n")
    return round_dir


def _record(arm="default", qid="q1", rep=0, key="k1", answer="src/a.rs:1 found",
            run_error=None, **extra):
    return {
        "arm": arm, "question_id": qid, "rep": rep, "run_key": key,
        "final_answer": answer, "run_error": run_error, "repo": "smoke",
        "reused_from": None, **extra,
    }


def test_find_reusable_matches_key_and_slot(tmp_path):
    _write_round(tmp_path, "R001", [_record(key="k1"), _record(qid="q2", key="k2")])
    current = tmp_path / "R002"
    found = reuse.find_reusable(tmp_path, current, {("default", "q1", 0): "k1"})
    assert set(found) == {("default", "q1", 0)}
    assert found[("default", "q1", 0)]["reused_from"] == "R001"
    assert found[("default", "q1", 0)]["final_answer"] == "src/a.rs:1 found"


def test_find_reusable_rejects_stale_key_errors_and_empty_answers(tmp_path):
    _write_round(tmp_path, "R001", [
        _record(qid="q1", key="OLD"),
        _record(qid="q2", key="k2", run_error="timeout"),
        _record(qid="q3", key="k3", answer="   "),
        _record(qid="q4", key=None),
    ])
    wanted = {("default", f"q{i}", 0): f"k{i}" for i in range(1, 5)}
    assert reuse.find_reusable(tmp_path, tmp_path / "R002", wanted) == {}


def test_find_reusable_prefers_newest_round_and_skips_current(tmp_path):
    _write_round(tmp_path, "R001", [_record(key="k1", answer="old answer")])
    _write_round(tmp_path, "R002", [_record(key="k1", answer="new answer")])
    current = _write_round(tmp_path, "R003", [_record(key="k1", answer="current")])
    found = reuse.find_reusable(tmp_path, current, {("default", "q1", 0): "k1"})
    assert found[("default", "q1", 0)]["final_answer"] == "new answer"
    assert found[("default", "q1", 0)]["reused_from"] == "R002"


def test_find_reusable_matches_rep_for_rep(tmp_path):
    _write_round(tmp_path, "R001", [_record(rep=0, key="k1", answer="rep0 sample")])
    wanted = {("default", "q1", 0): "k1", ("default", "q1", 1): "k1"}
    found = reuse.find_reusable(tmp_path, tmp_path / "R002", wanted)
    # Only rep 0 exists upstream: rep 1 must run fresh, not adopt a second
    # copy of the same prior sample.
    assert set(found) == {("default", "q1", 0)}


def _smoke_fixture():
    fixture_path = Path(__file__).resolve().parents[1] / "fixtures" / "smoke.yaml"
    return fixtures_io.load_fixture(fixture_path)


def _no_daemon(monkeypatch):
    monkeypatch.setattr(
        "bench.daemon.maybe_start_daemon", lambda arm, port, home=None: None
    )


def test_orchestrator_adopts_reusable_runs_instead_of_running(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    repo_path = tmp_path / "checkout"  # not REPO_ROOT, so keys are computed
    repo_path.mkdir()
    _no_daemon(monkeypatch)
    monkeypatch.setattr(config, "RUN_REUSE", True)
    monkeypatch.setattr(orchestrator.config_mod, "RUN_REUSE", True)

    calls: list[str] = []

    def fake_run_question(arm, q, daemon, repo_path, transcripts_dir):
        calls.append(q.id)
        return runner.Run(
            arm=arm.name, question_id=q.id, repo=str(repo_path),
            final_answer="fresh src/a.rs:1", stop_reason="end_turn", model="x",
        )

    monkeypatch.setattr(runner, "run_question", fake_run_question)

    arm = arms.ARMS["default"]
    results_root = tmp_path / "results"

    # Round 1: everything fresh; records get run_key stamped.
    s1 = orchestrator.run_cycle(
        arms_to_run=[arm], repos=[(fixture, repo_path)],
        results_dir=results_root / "R001", judge_enabled=False,
    )
    assert s1["n_reused"] == 0
    n_questions = len(fixture.questions)
    assert len(calls) == n_questions
    r1_runs = [json.loads(l) for l in (results_root / "R001" / "runs.jsonl").read_text().splitlines()]
    assert all(r["run_key"] for r in r1_runs)

    # Round 2: identical inputs; every slot is adopted, zero fresh runs.
    calls.clear()
    s2 = orchestrator.run_cycle(
        arms_to_run=[arm], repos=[(fixture, repo_path)],
        results_dir=results_root / "R002", judge_enabled=False,
    )
    assert calls == []
    assert s2["n_reused"] == n_questions
    r2_runs = [json.loads(l) for l in (results_root / "R002" / "runs.jsonl").read_text().splitlines()]
    assert {r["reused_from"] for r in r2_runs} == {"R001"}


def test_orchestrator_reuse_opt_out(monkeypatch, tmp_path):
    fixture = _smoke_fixture()
    repo_path = tmp_path / "checkout"
    repo_path.mkdir()
    _no_daemon(monkeypatch)

    def fake_run_question(arm, q, daemon, repo_path, transcripts_dir):
        return runner.Run(
            arm=arm.name, question_id=q.id, repo=str(repo_path),
            final_answer="fresh src/a.rs:1", stop_reason="end_turn", model="x",
        )

    monkeypatch.setattr(runner, "run_question", fake_run_question)
    arm = arms.ARMS["default"]
    results_root = tmp_path / "results"

    orchestrator.run_cycle(
        arms_to_run=[arm], repos=[(fixture, repo_path)],
        results_dir=results_root / "R001", judge_enabled=False,
    )
    monkeypatch.setattr(orchestrator.config_mod, "RUN_REUSE", False)
    s2 = orchestrator.run_cycle(
        arms_to_run=[arm], repos=[(fixture, repo_path)],
        results_dir=results_root / "R002", judge_enabled=False,
    )
    assert s2["n_reused"] == 0
    r2_runs = [json.loads(l) for l in (results_root / "R002" / "runs.jsonl").read_text().splitlines()]
    assert all(r["reused_from"] is None for r in r2_runs)


def test_smoke_fixture_slots_are_never_keyed(monkeypatch, tmp_path):
    # The smoke fixture runs against this repo's live working tree; its pinned
    # SHA does not describe the content, so reuse must not fire.
    fixture = _smoke_fixture()
    keys = orchestrator._compute_run_keys([arms.ARMS["default"]], [(fixture, config.REPO_ROOT)], 1)
    assert set(keys.values()) == {None}

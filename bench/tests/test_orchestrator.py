"""Tests for bench/orchestrator.py - the end-to-end cycle (mocked)."""
from pathlib import Path
from unittest.mock import MagicMock

import pytest

from bench import arms, fixtures_io, orchestrator, runner


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
        lambda arm, port: None if not arm.needs_daemon else MagicMock(stop=lambda: None, build_mcp_config=lambda: {}),
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

"""Tests for bench/runner.py.

These tests mock subprocess so no real claude --print is invoked.
"""
import json
import subprocess
from dataclasses import dataclass

import pytest

from bench import runner
from bench.arms import ARMS
from bench.fixtures_io import Question, Expected


@dataclass
class FakeCompleted:
    stdout: bytes
    stderr: bytes = b""
    returncode: int = 0


def _q():
    return Question(
        id="q1",
        task_type="symbol_lookup",
        difficulty="easy",
        question="Where is foo?",
        rubric="r",
        expected=Expected(citations=[], files=[], facts=[], forbidden=[], forbidden_strict=False),
    )


def _fake_transcript(answer: str, tool_calls: list[dict] | None = None) -> bytes:
    """Emit a JSONL stream resembling claude --print output."""
    lines = []
    if tool_calls:
        for tc in tool_calls:
            lines.append(json.dumps({"type": "tool_use", "name": tc["name"], "input": tc.get("input", {})}))
            lines.append(json.dumps({"type": "tool_result", "tool_use_id": tc.get("id", "x"), "content": tc.get("result", "")}))
    lines.append(json.dumps({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": answer}],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 200,
                "cache_creation_input_tokens": 50,
            },
        },
    }))
    lines.append(json.dumps({"type": "result", "stop_reason": "end_turn"}))
    return ("\n".join(lines) + "\n").encode()


def test_runner_parses_transcript(monkeypatch, tmp_path):
    arm = ARMS["default"]
    q = _q()
    monkeypatch.setattr(
        subprocess,
        "run",
        lambda *a, **k: FakeCompleted(stdout=_fake_transcript(
            "The foo function is at src/foo.rs:42",
            tool_calls=[{"name": "Grep", "input": {"pattern": "foo"}, "result": "match"}],
        )),
    )
    run = runner.run_question(arm, q, daemon=None, repo_path=tmp_path, transcripts_dir=tmp_path)
    assert run.arm == "default"
    assert run.question_id == "q1"
    assert "src/foo.rs:42" in run.final_answer
    assert run.stop_reason == "end_turn"
    assert len(run.tool_calls) == 1
    assert run.tool_calls[0].name == "Grep"
    assert run.input_tokens == 100
    assert run.cache_read_tokens == 200


def test_runner_handles_timeout(monkeypatch, tmp_path):
    arm = ARMS["default"]
    q = _q()
    def fake_run(*a, **k):
        raise subprocess.TimeoutExpired(cmd="claude", timeout=1)
    monkeypatch.setattr(subprocess, "run", fake_run)
    run = runner.run_question(arm, q, daemon=None, repo_path=tmp_path, transcripts_dir=tmp_path)
    assert run.stop_reason == "timeout"
    assert run.final_answer == ""


def test_runner_retries_on_nonzero_exit_then_succeeds(monkeypatch, tmp_path):
    arm = ARMS["default"]
    q = _q()
    calls = {"n": 0}

    def fake_run(*a, **k):
        calls["n"] += 1
        if calls["n"] == 1:
            return FakeCompleted(stdout=b"", stderr=b"boom", returncode=1)
        return FakeCompleted(stdout=_fake_transcript("recovered answer"))

    monkeypatch.setattr(subprocess, "run", fake_run)
    run = runner.run_question(arm, q, daemon=None, repo_path=tmp_path, transcripts_dir=tmp_path)
    assert calls["n"] == 2
    assert run.run_error is None
    assert run.final_answer == "recovered answer"


def test_runner_records_error_after_persistent_cli_failure(monkeypatch, tmp_path):
    arm = ARMS["default"]
    q = _q()
    monkeypatch.setattr(
        subprocess, "run",
        lambda *a, **k: FakeCompleted(stdout=b"", stderr=b"fatal", returncode=2),
    )
    run = runner.run_question(arm, q, daemon=None, repo_path=tmp_path, transcripts_dir=tmp_path)
    assert run.run_error == "cli_exit_2"
    assert run.final_answer == ""


def test_runner_timeout_records_error_and_keeps_partial_transcript(monkeypatch, tmp_path):
    arm = ARMS["default"]
    q = _q()

    def fake_run(*a, **k):
        raise subprocess.TimeoutExpired(cmd="claude", timeout=1, output=b'{"partial": true}')

    monkeypatch.setattr(subprocess, "run", fake_run)
    run = runner.run_question(arm, q, daemon=None, repo_path=tmp_path, transcripts_dir=tmp_path)
    assert run.run_error == "timeout"
    assert (tmp_path / "default" / "q1.jsonl").read_bytes() == b'{"partial": true}'


def test_runner_isolates_cli_from_user_config(monkeypatch, tmp_path):
    arm = ARMS["default"]
    q = _q()
    captured = {}

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return FakeCompleted(stdout=_fake_transcript("a"))

    monkeypatch.setattr(subprocess, "run", fake_run)
    runner.run_question(arm, q, daemon=None, repo_path=tmp_path, transcripts_dir=tmp_path)
    assert "--strict-mcp-config" in captured["cmd"]
    idx = captured["cmd"].index("--setting-sources")
    assert captured["cmd"][idx + 1] == ""


def test_runner_writes_transcript_to_disk(monkeypatch, tmp_path):
    arm = ARMS["default"]
    q = _q()
    transcript_bytes = _fake_transcript("answer")
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: FakeCompleted(stdout=transcript_bytes))
    run = runner.run_question(arm, q, daemon=None, repo_path=tmp_path, transcripts_dir=tmp_path)
    transcript_path = tmp_path / "default" / "q1.jsonl"
    assert transcript_path.exists()
    assert transcript_path.read_bytes() == transcript_bytes

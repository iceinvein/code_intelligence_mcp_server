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


def _stream_json_transcript() -> bytes:
    """Realistic claude --print --output-format stream-json --verbose output."""
    lines = [
        {"type": "system", "subtype": "init", "tools": ["Grep", "Read"]},
        {"type": "assistant", "message": {
            "content": [{"type": "tool_use", "id": "tu1", "name": "Grep",
                         "input": {"pattern": "foo"}}],
            "usage": {"input_tokens": 10, "output_tokens": 5,
                      "cache_read_input_tokens": 0, "cache_creation_input_tokens": 30},
        }},
        {"type": "user", "message": {
            "content": [{"type": "tool_result", "tool_use_id": "tu1",
                         "content": [{"type": "text", "text": "x" * 500}]}],
        }},
        {"type": "assistant", "message": {
            "content": [{"type": "text", "text": "The answer is src/foo.rs:42"}],
            "usage": {"input_tokens": 12, "output_tokens": 40,
                      "cache_read_input_tokens": 600, "cache_creation_input_tokens": 0},
        }},
        {"type": "result", "subtype": "success", "result": "The answer is src/foo.rs:42",
         "stop_reason": "end_turn", "num_turns": 2,
         "usage": {"input_tokens": 22, "output_tokens": 45,
                   "cache_read_input_tokens": 600, "cache_creation_input_tokens": 30}},
    ]
    return ("\n".join(json.dumps(l) for l in lines) + "\n").encode()


def test_runner_caps_turns(monkeypatch, tmp_path):
    arm = ARMS["default"]
    q = _q()
    captured = {}

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return FakeCompleted(stdout=_stream_json_transcript())

    monkeypatch.setattr(subprocess, "run", fake_run)
    runner.run_question(arm, q, daemon=None, repo_path=tmp_path, transcripts_dir=tmp_path)
    idx = captured["cmd"].index("--max-turns")
    assert captured["cmd"][idx + 1] == "12"  # BENCH_MAX_TURNS default


def test_runner_requests_stream_json(monkeypatch, tmp_path):
    arm = ARMS["default"]
    q = _q()
    captured = {}

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        return FakeCompleted(stdout=_stream_json_transcript())

    monkeypatch.setattr(subprocess, "run", fake_run)
    runner.run_question(arm, q, daemon=None, repo_path=tmp_path, transcripts_dir=tmp_path)
    idx = captured["cmd"].index("--output-format")
    assert captured["cmd"][idx + 1] == "stream-json"
    assert "--verbose" in captured["cmd"]


def test_parser_extracts_real_tool_calls_and_result_sizes():
    parsed = runner._parse_transcript(_stream_json_transcript())
    assert [t.name for t in parsed["tool_calls"]] == ["Grep"]
    assert parsed["tool_calls"][0].result_size == 500
    assert parsed["final_answer"] == "The answer is src/foo.rs:42"


def test_parser_takes_cumulative_usage_from_result_line():
    parsed = runner._parse_transcript(_stream_json_transcript())
    assert parsed["usage"]["input_tokens"] == 22
    assert parsed["usage"]["cache_read_input_tokens"] == 600
    assert parsed["usage"]["cache_creation_input_tokens"] == 30


def test_parser_flags_max_turns_stop():
    lines = [
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "partial"}]}},
        {"type": "result", "subtype": "error_max_turns", "result": "partial", "num_turns": 12},
    ]
    raw = ("\n".join(json.dumps(l) for l in lines) + "\n").encode()
    parsed = runner._parse_transcript(raw)
    assert parsed["stop_reason"] == "max_turns"


def test_runner_accepts_max_turns_exit_1_without_retry(monkeypatch, tmp_path):
    """claude -p exits 1 when --max-turns is hit but stdout still carries the
    full transcript (subtype error_max_turns). That is a capped run, not a CLI
    failure: keep the transcript, no retry, no run_error."""
    arm = ARMS["default"]
    q = _q()
    lines = [
        {"type": "assistant", "message": {
            "content": [{"type": "text", "text": "partial exploration"}],
            "usage": {"input_tokens": 5, "output_tokens": 9,
                      "cache_read_input_tokens": 10, "cache_creation_input_tokens": 2},
        }},
        {"type": "result", "subtype": "error_max_turns", "is_error": True,
         "num_turns": 12,
         "usage": {"input_tokens": 5, "output_tokens": 9,
                   "cache_read_input_tokens": 10, "cache_creation_input_tokens": 2}},
    ]
    stdout = ("\n".join(json.dumps(l) for l in lines) + "\n").encode()
    calls = {"n": 0}

    def fake_run(*a, **k):
        calls["n"] += 1
        return FakeCompleted(stdout=stdout, returncode=1)

    monkeypatch.setattr(subprocess, "run", fake_run)
    run = runner.run_question(arm, q, daemon=None, repo_path=tmp_path, transcripts_dir=tmp_path)
    assert calls["n"] == 1  # not retried
    assert run.run_error is None
    assert run.stop_reason == "max_turns"
    assert run.final_answer == "partial exploration"
    assert (tmp_path / "default" / "q1.jsonl").read_bytes() == stdout


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

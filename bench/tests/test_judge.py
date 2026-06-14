"""Tests for bench/judge.py."""
import json
import subprocess
from dataclasses import dataclass

import pytest

from bench import judge


@dataclass
class FakeCompleted:
    stdout: bytes
    returncode: int = 0
    stderr: bytes = b""


def make_response(score: int, model: str) -> bytes:
    """Wrap a judge score in the canonical claude --print --output-format json envelope."""
    inner = f'{{"score": {score}, "justification": "fake-{model}"}}'
    cli_output = json.dumps({"type": "result", "subtype": "success", "result": inner})
    return cli_output.encode()


def test_judge_one_parses_json_response(monkeypatch):
    monkeypatch.setattr(
        subprocess, "run",
        lambda *a, **k: FakeCompleted(stdout=make_response(7, "claude-haiku-4-5")),
    )
    result = judge.judge_one(
        model="claude-haiku-4-5",
        question_id="q1",
        question="What?",
        rubric="r",
        citations=[],
        mech_context={},
        answer="a",
    )
    assert result.score == 7
    assert "fake" in result.justification


def test_judge_all_returns_median_and_range(monkeypatch):
    scores = {
        "claude-haiku-4-5": 4,
        "claude-sonnet-4-6": 7,
        "claude-opus-4-8": 9,
    }

    def fake_run(cmd, **kwargs):
        model = cmd[cmd.index("--model") + 1]
        return FakeCompleted(stdout=make_response(scores[model], model))

    monkeypatch.setattr(subprocess, "run", fake_run)

    agg = judge.judge_all(
        question_id="q1",
        question="?",
        rubric="r",
        citations=[],
        mech_context={},
        answer="a",
    )
    assert agg.median == 7
    assert agg.range == 5
    assert agg.scores == {"haiku": 4, "sonnet": 7, "opus": 9}


def test_judge_one_retries_on_malformed_json(monkeypatch):
    attempts = {"count": 0}

    def fake_run(cmd, **kwargs):
        attempts["count"] += 1
        if attempts["count"] == 1:
            # First call: malformed output, no valid JSON
            cli_output = json.dumps({"type": "result", "result": "not json at all"})
        else:
            cli_output = json.dumps({"type": "result", "result": '{"score": 5, "justification": "ok"}'})
        return FakeCompleted(stdout=cli_output.encode())

    monkeypatch.setattr(subprocess, "run", fake_run)

    result = judge.judge_one(
        model="claude-haiku-4-5",
        question_id="q1",
        question="?",
        rubric="r",
        citations=[],
        mech_context={},
        answer="a",
        max_retries=1,
    )
    assert attempts["count"] == 2  # one retry
    assert result.score == 5


def test_parse_response_handles_braces_inside_justification():
    # Regression (R008): the old brace-free regex bailed at the literal `{` in the
    # justification text, dropping a perfectly valid judgement to a 0 casualty.
    raw = (
        '{"score": 6, "justification": "Correct, but omits that authPlugin injects '
        '{ user, session } into route context."}'
    )
    score, just = judge._parse_response(raw)
    assert score == 6
    assert "{ user, session }" in just


def test_parse_response_handles_markdown_fence():
    raw = '```json\n{"score": 8, "justification": "good answer"}\n```'
    score, just = judge._parse_response(raw)
    assert score == 8
    assert just == "good answer"


def test_parse_response_skips_leading_prose_brace():
    # A stray `{` in prose before the real object must not abort extraction.
    raw = 'Here { is my verdict: {"score": 3, "justification": "weak {x} here"}'
    score, just = judge._parse_response(raw)
    assert score == 3
    assert "weak {x} here" == just


def test_judge_one_parses_justification_with_braces(monkeypatch):
    inner = '{"score": 6, "justification": "injects { user, session } into context"}'
    cli_output = json.dumps({"type": "result", "subtype": "success", "result": inner})
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: FakeCompleted(stdout=cli_output.encode()))
    result = judge.judge_one(
        model="claude-opus-4-8",
        question_id="q1",
        question="?",
        rubric="r",
        citations=[],
        mech_context={},
        answer="a",
    )
    assert result.error is None
    assert result.score == 6
    assert "{ user, session }" in result.justification

"""Tests for bench/judge.py."""
import pytest

from bench import judge


class FakeClient:
    """Mock anthropic client. Returns canned scores per (model, question) tuple."""
    def __init__(self, scores):
        self.scores = scores
        self.calls = []
        self.messages = self

    def create(self, *, model, max_tokens, system, messages, **kwargs):
        user_content = messages[0]["content"]
        qid = "q1" if "QUESTION_ID: q1" in user_content else "unknown"
        self.calls.append((model, qid))
        score = self.scores.get((model, qid), 5)
        class FakeContent:
            text = f'{{"score": {score}, "justification": "fake-{model}-{qid}"}}'
        class FakeResponse:
            content = [FakeContent()]
            usage = type("U", (), {"input_tokens": 100, "output_tokens": 20})()
        return FakeResponse()


def test_judge_one_parses_json_response():
    client = FakeClient({("claude-haiku-4-5", "q1"): 7})
    result = judge.judge_one(
        client=client,
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


def test_judge_all_returns_median_and_range():
    client = FakeClient({
        ("claude-haiku-4-5", "q1"): 4,
        ("claude-sonnet-4-6", "q1"): 7,
        ("claude-opus-4-7", "q1"): 9,
    })
    agg = judge.judge_all(
        client=client,
        question_id="q1",
        question="What?",
        rubric="r",
        citations=[],
        mech_context={},
        answer="a",
    )
    assert agg.median == 7
    assert agg.range == 5
    assert agg.scores == {"haiku": 4, "sonnet": 7, "opus": 9}


def test_judge_one_retries_on_malformed_json():
    class MalformedClient:
        def __init__(self):
            self.attempts = 0
            self.messages = self
        def create(self, **kwargs):
            self.attempts += 1
            text = "not json at all" if self.attempts == 1 else '{"score": 5, "justification": "ok"}'
            class C: pass
            c = C()
            c.text = text
            class R:
                content = [c]
                usage = type("U", (), {"input_tokens": 50, "output_tokens": 10})()
            return R()

    client = MalformedClient()
    result = judge.judge_one(
        client=client,
        model="claude-haiku-4-5",
        question_id="q1",
        question="?",
        rubric="r",
        citations=[],
        mech_context={},
        answer="a",
    )
    assert client.attempts == 2  # one retry
    assert result.score == 5

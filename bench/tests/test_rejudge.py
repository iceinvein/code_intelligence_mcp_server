"""Tests for bench/rejudge.py casualty repair."""
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from bench import config, rejudge


def _write_jsonl(path: Path, rows: list[dict]) -> None:
    path.write_text("".join(json.dumps(r) + "\n" for r in rows))


@pytest.fixture
def round_dir(tmp_path, monkeypatch):
    rd = tmp_path / "R999"
    rd.mkdir()
    monkeypatch.setattr(config, "RESULTS_DIR", tmp_path)
    return rd


def _casualty_row(rep: int) -> dict:
    return {
        "arm": "a", "question_id": "wolfmax-arch-03", "repo": "wolfmax",
        "rep": rep, "tier": "t", "casualty": True,
        "scores": {"haiku": 0, "sonnet": 0, "opus": 0},
        "justifications": {"haiku": "", "sonnet": "", "opus": ""},
        "median": 0, "range": 0, "errors": {}, "n_valid": 0,
    }


def test_rejudge_keys_by_rep_and_clears_casualty_flag(round_dir, monkeypatch):
    # Two reps: rep 0 is fine, rep 1 is a casualty. The repair must judge
    # rep 1's OWN answer (not rep 0's, the pre-fix behaviour) and flip the
    # explicit casualty flag so rescore keeps the repaired median.
    judge_rows = [
        {**_casualty_row(0), "casualty": False,
         "scores": {"haiku": 8, "sonnet": 8, "opus": 8},
         "justifications": {"haiku": "ok", "sonnet": "ok", "opus": "ok"},
         "median": 8, "range": 0},
        _casualty_row(1),
    ]
    runs = [
        {"arm": "a", "question_id": "wolfmax-arch-03", "rep": 0,
         "final_answer": "answer for rep zero"},
        {"arm": "a", "question_id": "wolfmax-arch-03", "rep": 1,
         "final_answer": "answer for rep one"},
    ]
    scores = [
        {"arm": "a", "question_id": "wolfmax-arch-03", "rep": 0,
         "citation_hit": True, "hallucinated": False, "forbidden_hit": False},
        {"arm": "a", "question_id": "wolfmax-arch-03", "rep": 1,
         "citation_hit": True, "hallucinated": False, "forbidden_hit": False},
    ]
    _write_jsonl(round_dir / "judge.jsonl", judge_rows)
    _write_jsonl(round_dir / "runs.jsonl", runs)
    _write_jsonl(round_dir / "scores.json", scores)

    q = SimpleNamespace(
        id="wolfmax-arch-03", question="?", rubric="r",
        expected=SimpleNamespace(citations=[]),
    )
    monkeypatch.setattr(rejudge, "_load_fixture_questions", lambda: {q.id: q})

    judged_answers: list[str] = []

    def fake_judge_one(*, answer, **kwargs):
        judged_answers.append(answer)
        return SimpleNamespace(score=7, justification="looks fine", error=None)

    monkeypatch.setattr(rejudge.judge_mod, "judge_one", fake_judge_one)
    monkeypatch.setattr(rejudge.time, "sleep", lambda _s: None)
    monkeypatch.setattr(rejudge.sys, "argv", ["rejudge", "R999"])

    assert rejudge.main() == 0

    assert judged_answers, "casualty must be rejudged"
    assert all(a == "answer for rep one" for a in judged_answers), (
        "rejudge must use the casualty rep's own answer"
    )
    fixed = [json.loads(l) for l in (round_dir / "judge.jsonl").read_text().splitlines()]
    repaired = next(r for r in fixed if r["rep"] == 1)
    assert repaired["casualty"] is False
    assert repaired["median"] == 7
    new_scores = [json.loads(l) for l in (round_dir / "scores.json").read_text().splitlines()]
    assert next(s for s in new_scores if s["rep"] == 1)["judge_median"] == 7

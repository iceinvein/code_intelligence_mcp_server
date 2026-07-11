"""Tests for bench/rescore.py - zero-token re-scoring of stored rounds."""
import json
from pathlib import Path

import pytest

from bench import rescore


REPO_ROOT = Path(__file__).resolve().parents[2]


def _write_round(tmp_path, runs, judges=None, scores=None):
    round_dir = tmp_path / "R900"
    round_dir.mkdir(parents=True)
    (round_dir / "runs.jsonl").write_text(
        "".join(json.dumps(r) + "\n" for r in runs))
    if judges is not None:
        (round_dir / "judge.jsonl").write_text(
            "".join(json.dumps(j) + "\n" for j in judges))
    if scores is not None:
        (round_dir / "scores.json").write_text(
            "".join(json.dumps(s) + "\n" for s in scores))
    return round_dir


def _smoke_run(qid="smoke-symbol-01", answer="see src/indexer/mod.rs:10", arm="default"):
    return {
        "arm": arm, "question_id": qid, "repo": "smoke",
        "final_answer": answer, "tool_calls": [], "input_tokens": 10,
        "output_tokens": 5, "cache_read_tokens": 0, "cache_creation_tokens": 0,
        "wall_ms": 100, "stop_reason": "end_turn", "model": "x",
    }


def _smoke_qid():
    from bench import fixtures_io, config
    fixture = fixtures_io.load_fixture(config.FIXTURES_DIR / "smoke.yaml")
    return fixture.questions[0]


def test_rescore_recomputes_mech_and_preserves_judge(tmp_path):
    q = _smoke_qid()
    runs = [_smoke_run(qid=q.id)]
    judges = [{"arm": "default", "question_id": q.id, "repo": "smoke",
               "scores": {"haiku": 6, "sonnet": 7, "opus": 8},
               "justifications": {}, "median": 7.0, "range": 2}]
    round_dir = _write_round(tmp_path, runs, judges=judges)

    summary = rescore.rescore_round(round_dir)

    rows = [json.loads(l) for l in (round_dir / "scores.json").read_text().splitlines()]
    assert len(rows) == 1
    assert rows[0]["question_id"] == q.id
    assert rows[0]["judge_median"] == 7.0
    assert "mech" in rows[0]
    assert summary["n_rescored"] == 1


def test_rescore_preserves_judge_on_every_repeat(tmp_path):
    # R022/R023 regression: with 8 repeats, the judge merge keyed on
    # (arm, question_id) only, so the last rep's score record absorbed every
    # judge row and reps 0-6 lost their judge medians.
    q = _smoke_qid()
    runs = [{**_smoke_run(qid=q.id), "rep": rep} for rep in range(3)]
    judges = [{"arm": "default", "question_id": q.id, "repo": "smoke", "rep": rep,
               "scores": {"haiku": 5 + rep}, "justifications": {},
               "median": float(5 + rep), "range": 0}
              for rep in range(3)]
    round_dir = _write_round(tmp_path, runs, judges=judges)

    rescore.rescore_round(round_dir)

    rows = [json.loads(l) for l in (round_dir / "scores.json").read_text().splitlines()]
    medians = {r["rep"]: r["judge_median"] for r in rows}
    assert medians == {0: 5.0, 1: 6.0, 2: 7.0}


def test_rescore_backs_up_previous_scores(tmp_path):
    q = _smoke_qid()
    old_scores = [{"question_id": q.id, "arm": "default", "mech": 0.1}]
    round_dir = _write_round(tmp_path, [_smoke_run(qid=q.id)], scores=old_scores)

    rescore.rescore_round(round_dir)

    backup = round_dir / "scores.json.pre-rescore"
    assert backup.exists()
    assert json.loads(backup.read_text().splitlines()[0])["mech"] == 0.1


def test_rescore_treats_legacy_all_zero_judge_rows_as_casualties(tmp_path):
    q = _smoke_qid()
    judges = [{"arm": "default", "question_id": q.id, "repo": "smoke",
               "scores": {"haiku": 0, "sonnet": 0, "opus": 0},
               "justifications": {"haiku": "", "sonnet": "", "opus": ""},
               "median": 0.0, "range": 0}]
    round_dir = _write_round(tmp_path, [_smoke_run(qid=q.id)], judges=judges)

    rescore.rescore_round(round_dir)

    rows = [json.loads(l) for l in (round_dir / "scores.json").read_text().splitlines()]
    assert rows[0]["judge_median"] is None
    assert rows[0]["judge_casualty"] is True


def test_git_line_reader_reads_pinned_tree():
    sha = "4c2b5ae"  # a known commit in this repo
    reader = rescore.git_line_reader(REPO_ROOT, sha)
    lines = reader("bench/README.md")
    assert lines and lines[0].startswith("# Bench")
    assert reader("does/not/exist.rs") is None


def test_git_file_lister_lists_pinned_tree():
    sha = "4c2b5ae"
    lister = rescore.git_file_lister(REPO_ROOT, sha)
    files = lister()
    assert "bench/README.md" in files
    assert lister() is files  # memoized


def test_rescore_dedupes_superseded_retry_records(tmp_path):
    q = _smoke_qid()
    failed = _smoke_run(qid=q.id, answer="")
    failed["run_error"] = "timeout"
    retry = _smoke_run(qid=q.id, answer="see src/indexer/mod.rs:10")
    retry["run_error"] = None
    round_dir = _write_round(tmp_path, [failed, retry])

    summary = rescore.rescore_round(round_dir)

    assert summary["n_rescored"] == 1  # one row per (arm, question, rep)
    rows = [json.loads(l) for l in (round_dir / "scores.json").read_text().splitlines()]
    assert len(rows) == 1
    assert rows[0]["run_error"] is None  # the later retry wins

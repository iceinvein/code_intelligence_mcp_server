"""Tests for bench/fixtures_io.py."""
from pathlib import Path

import pytest

from bench import fixtures_io


SMOKE_PATH = Path(__file__).resolve().parents[1] / "fixtures" / "smoke.yaml"


def test_load_smoke_fixture():
    fixture = fixtures_io.load_fixture(SMOKE_PATH)
    assert fixture.meta.repo == "smoke"
    assert fixture.meta.upstream_sha == "HEAD"
    assert len(fixture.questions) == 3


def test_question_fields_parse():
    fixture = fixtures_io.load_fixture(SMOKE_PATH)
    q = next(q for q in fixture.questions if q.id == "smoke-symbol-01")
    assert q.task_type == "symbol_lookup"
    assert q.difficulty == "easy"
    assert len(q.expected.citations) == 1
    cite = q.expected.citations[0]
    assert cite.file == "src/indexer/pipeline/mod.rs"
    assert cite.line_range == (74, 94)
    assert cite.symbol == "IndexPipeline"
    assert "IndexPipeline" in q.expected.facts
    # OR-alternatives parse as a list inside the facts list
    facts_with_or = [f for f in q.expected.facts if isinstance(f, list)]
    assert ["struct", "Struct"] in facts_with_or


def test_forbidden_strict_default_false():
    fixture = fixtures_io.load_fixture(SMOKE_PATH)
    q = next(q for q in fixture.questions if q.id == "smoke-symbol-01")
    assert q.expected.forbidden_strict is False
    q3 = next(q for q in fixture.questions if q.id == "smoke-negative-01")
    assert q3.expected.forbidden_strict is True


def test_validate_succeeds_for_smoke():
    # smoke.yaml's citations point at this repo; validate should succeed when
    # repo_path is the repo root.
    repo_root = Path(__file__).resolve().parents[2]
    errs = fixtures_io.validate_fixture(SMOKE_PATH, repo_root)
    assert errs == [], f"unexpected validation errors: {errs}"


def test_validate_flags_missing_citation_file(tmp_path):
    bad = tmp_path / "bad.yaml"
    bad.write_text(
        """meta:
  repo: bad
  upstream_url: "."
  upstream_sha: "HEAD"
  authored_at: "2026-05-27"
  authored_against_schema_version: 22
questions:
  - id: bad-q1
    task_type: symbol_lookup
    difficulty: easy
    question: "Where?"
    rubric: "n/a"
    expected:
      citations:
        - { file: "this/does/not/exist.rs", line_range: [1, 5], symbol: "Nope" }
      files: []
      facts: []
      forbidden: []
"""
    )
    errs = fixtures_io.validate_fixture(bad, tmp_path)
    assert any("this/does/not/exist.rs" in e for e in errs)


def test_validate_flags_duplicate_ids(tmp_path):
    dup = tmp_path / "dup.yaml"
    dup.write_text(
        """meta:
  repo: dup
  upstream_url: "."
  upstream_sha: "HEAD"
  authored_at: "2026-05-27"
  authored_against_schema_version: 22
questions:
  - id: dup-1
    task_type: symbol_lookup
    difficulty: easy
    question: "A?"
    rubric: "n/a"
    expected: {citations: [], files: [], facts: [], forbidden: []}
  - id: dup-1
    task_type: symbol_lookup
    difficulty: easy
    question: "B?"
    rubric: "n/a"
    expected: {citations: [], files: [], facts: [], forbidden: []}
"""
    )
    errs = fixtures_io.validate_fixture(dup, tmp_path)
    assert any("duplicate" in e.lower() for e in errs)

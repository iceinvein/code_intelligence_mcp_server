"""Tests for bench/score.py."""
from pathlib import Path

import pytest

from bench import fixtures_io, score


def _q(forbidden=None, forbidden_strict=False, facts=None, files=None, citations=None):
    return fixtures_io.Question(
        id="t-1",
        task_type="symbol_lookup",
        difficulty="easy",
        question="?",
        rubric="r",
        expected=fixtures_io.Expected(
            citations=citations if citations is not None else [
                fixtures_io.Citation(file="src/foo.rs", line_range=(10, 30), symbol="bar")
            ],
            files=files if files is not None else ["src/foo.rs"],
            facts=facts if facts is not None else ["bar"],
            forbidden=forbidden if forbidden is not None else [],
            forbidden_strict=forbidden_strict,
        ),
    )


def test_perfect_answer_scores_1():
    q = _q()
    answer = "The bar function is defined in src/foo.rs:10-30 on line 15."
    result = score.mech_score(q, answer)
    assert result["citation_hit"] is True
    assert result["file_score"] == 1.0
    assert result["fact_score"] == 1.0
    assert result["raw"] == pytest.approx(1.0)


def test_answer_with_no_citation_caps_at_half():
    q = _q()
    answer = "The bar function exists in src/foo.rs and does things."
    result = score.mech_score(q, answer)
    assert result["citation_hit"] is False
    # 0.5 * 0 + 0.25 * 1.0 + 0.25 * 1.0 = 0.5
    assert result["raw"] == pytest.approx(0.5)


def test_fact_or_alternatives_satisfied_by_either():
    q = _q(facts=[["create session", "createSession"]])
    answer = "src/foo.rs:10 contains createSession."
    result = score.mech_score(q, answer)
    assert result["fact_score"] == 1.0


def test_missing_files_and_facts_drops_score():
    q = _q(files=["src/a.rs", "src/b.rs"], facts=["alpha", "beta"], citations=[])
    answer = "alpha is somewhere."
    result = score.mech_score(q, answer)
    # citation_hit is True when citations is empty (vacuously satisfied).
    assert result["citation_hit"] is True
    assert result["file_score"] == 0.0
    assert result["fact_score"] == 0.5


def test_empty_citations_treats_as_vacuous_for_negative_question():
    q = _q(citations=[], files=[], facts=["does not exist"])
    answer = "Searched the repo; the symbol does not exist anywhere."
    result = score.mech_score(q, answer)
    assert result["citation_hit"] is True
    assert result["file_score"] == 1.0
    assert result["fact_score"] == 1.0
    assert result["raw"] == pytest.approx(1.0)


def test_empty_answer_scores_zero():
    q = _q()
    result = score.mech_score(q, "")
    assert result["raw"] == 0.0


def test_citation_verification_passes_for_existing_file(tmp_path):
    f = tmp_path / "src" / "foo.rs"
    f.parent.mkdir(parents=True)
    f.write_text("\n".join(f"line {i}" for i in range(1, 100)))

    answer = "Defined at src/foo.rs:50 line 50"
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert multiplier == 1.0
    assert all(r.ok for r in results)


def test_citation_verification_flags_missing_file(tmp_path):
    answer = "See src/nonexistent.rs:10-20 for details."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert multiplier == 0.0  # all (1 of 1) citations are hallucinated
    assert all(not r.ok for r in results)


def test_citation_verification_partial_hallucination(tmp_path):
    f = tmp_path / "src" / "real.rs"
    f.parent.mkdir(parents=True)
    f.write_text("real content\n")
    answer = "Defined at src/real.rs:1 and also src/fake.rs:5"
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert multiplier == 0.5  # at least one hallucination, not all


def test_forbidden_soft_penalty_subtracts_quarter_per_hit():
    q = _q(forbidden=["src/banned.rs", "fake_helper"])
    answer = "src/foo.rs:10-30 bar - but also see src/banned.rs and fake_helper."
    final = score.final_mech(q, answer, citation_multiplier=1.0)
    # raw=1.0, multiplier=1.0, penalty = 2 * 0.25 = 0.5 => 0.5
    assert final == pytest.approx(0.5)


def test_forbidden_strict_zeros_on_any_hit():
    q = _q(forbidden=["src/banned.rs"], forbidden_strict=True)
    answer = "src/foo.rs:10 bar (do not confuse with src/banned.rs)."
    final = score.final_mech(q, answer, citation_multiplier=1.0)
    assert final == 0.0


def test_combined_pipeline_clamps_to_zero():
    q = _q(forbidden=["a", "b", "c", "d", "e"])  # 5 hits = -1.25 penalty
    answer = "src/foo.rs:10-30 bar - a b c d e."
    final = score.final_mech(q, answer, citation_multiplier=1.0)
    assert final == 0.0

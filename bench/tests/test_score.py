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


def test_file_score_accepts_multi_segment_suffix_mention():
    # R030: the server's `cite` field pushes agents toward short unique path
    # forms ("lib/receipt-signer.ts"); those must count as file coverage for
    # the full expected path.
    q = _q(
        files=["packages/backend/src/lib/receipt-signer.ts"],
        facts=["signer"],
        citations=[],
    )
    answer = "Receipts are signed in lib/receipt-signer.ts:170 by the signer."
    result = score.mech_score(q, answer)
    assert result["file_score"] == 1.0


def test_file_score_rejects_bare_basename_mention():
    # A bare basename is not path evidence: "operations.py" matches a dozen
    # files in django and must not earn file coverage on its own.
    q = _q(
        files=["django/db/backends/base/operations.py"],
        facts=["ops"],
        citations=[],
    )
    answer = "See operations.py for the ops."
    result = score.mech_score(q, answer)
    assert result["file_score"] == 0.0


def test_file_score_rejects_non_suffix_path():
    q = _q(files=["django/db/backends/base/operations.py"], facts=["ops"], citations=[])
    answer = "See mysql/operations.py for the ops."
    result = score.mech_score(q, answer)
    assert result["file_score"] == 0.0


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


def test_citation_hit_survives_markdown_emphasis():
    # R032: agents write "**`django/.../__init__.py`**, line **183**"; the
    # bold markers around the digits broke both citation patterns and cost
    # citation_hit on otherwise correct answers.
    q = _q(
        citations=[fixtures_io.Citation(
            file="django/contrib/auth/__init__.py", line_range=(183, 197), symbol="gum",
        )],
        files=["django/contrib/auth/__init__.py"],
        facts=["get_user_model"],
    )
    answer = (
        "`get_user_model` is defined in **`django/contrib/auth/__init__.py`**, "
        "line **183**."
    )
    result = score.mech_score(q, answer)
    assert result["citation_hit"] is True


def test_extract_citations_strips_markdown_emphasis(tmp_path):
    _monorepo(tmp_path)
    answer = "See **`packages/backend/src/api/upgrade-helper.ts`**, line **15**."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert len(results) == 1
    assert results[0].file == "packages/backend/src/api/upgrade-helper.ts"
    assert results[0].start_line == 15
    assert results[0].ok


def test_extract_citations_parses_en_dash_ranges(tmp_path):
    _monorepo(tmp_path)
    answer = "See packages/backend/src/api/upgrade-helper.ts, lines 12–15."
    _multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert len(results) == 1
    assert (results[0].start_line, results[0].end_line) == (12, 15)


def test_citation_hit_matches_plural_lines():
    q = _q()
    answer = "The bar function is defined in src/foo.rs, lines 12-28."
    result = score.mech_score(q, answer)
    assert result["citation_hit"] is True


def test_citation_hit_matches_l_notation():
    q = _q()
    answer = "See src/foo.rs (L15) for the bar definition."
    result = score.mech_score(q, answer)
    assert result["citation_hit"] is True


def test_citation_hit_still_requires_overlapping_range():
    q = _q()  # canonical range 10-30
    answer = "The bar function is defined in src/foo.rs, lines 200-220."
    result = score.mech_score(q, answer)
    assert result["citation_hit"] is False


def test_ip_and_port_is_not_a_citation(tmp_path):
    answer = "The daemon listens on 127.0.0.1:17800 and the API on 127.0.0.1:17802."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert results == []
    assert multiplier == 1.0


def test_url_with_port_is_not_a_citation(tmp_path):
    answer = "Docs are served at localhost.example.com:8080 during development."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert results == []
    assert multiplier == 1.0


def test_scheme_url_with_port_is_not_a_citation(tmp_path):
    # R022/R023: answers describing the desktop OAuth handoff mention the
    # loopback callback URL; the regex captured "//127.0.0.1" as a file and
    # the port as a line number, flagging a true fact as a hallucination.
    answer = (
        "The web app redirects to http://127.0.0.1:47831/callback, "
        "see https://tauri.app/v1/guides:8080 for background."
    )
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert results == []
    assert multiplier == 1.0


def test_version_string_is_not_a_citation(tmp_path):
    answer = "This changed in release 4.5.0:2020 of the toolchain."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert results == []
    assert multiplier == 1.0


def test_bare_filename_with_code_extension_is_a_citation(tmp_path):
    (tmp_path / "models.py").write_text("x = 1\n" * 50)
    answer = "The field is declared in models.py:12."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert len(results) == 1
    assert results[0].ok
    assert multiplier == 1.0


def _monorepo(tmp_path):
    f = tmp_path / "packages" / "backend" / "src" / "api" / "upgrade-helper.ts"
    f.parent.mkdir(parents=True)
    f.write_text("line\n" * 100)
    return tmp_path


def test_shortened_path_resolves_by_unique_suffix(tmp_path):
    _monorepo(tmp_path)
    answer = "The helper is defined in upgrade-helper.ts:15."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert multiplier == 1.0
    assert results[0].ok
    assert results[0].imprecise  # shortened, but resolvable: not a fabrication
    assert results[0].resolved_file == "packages/backend/src/api/upgrade-helper.ts"


def test_shortened_path_with_partial_directory_resolves(tmp_path):
    _monorepo(tmp_path)
    answer = "See api/upgrade-helper.ts:15 for details."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert multiplier == 1.0
    assert results[0].ok
    assert results[0].imprecise


def test_ambiguous_suffix_resolves_when_only_one_has_the_line(tmp_path):
    _monorepo(tmp_path)
    other = tmp_path / "packages" / "frontend" / "upgrade-helper.ts"
    other.parent.mkdir(parents=True)
    other.write_text("short\n")  # 1 line: cited line 15 cannot be here
    answer = "See upgrade-helper.ts:15."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert multiplier == 1.0
    assert results[0].ok
    assert results[0].resolved_file == "packages/backend/src/api/upgrade-helper.ts"


def test_ambiguous_suffix_with_multiple_candidates_is_style_not_hallucination(tmp_path):
    # The cited file exists (twice) with the line in range: that is citation
    # style, not fabrication. Tracked as imprecise, multiplier untouched.
    _monorepo(tmp_path)
    other = tmp_path / "packages" / "frontend" / "upgrade-helper.ts"
    other.parent.mkdir(parents=True)
    other.write_text("line\n" * 100)  # both candidates contain line 15
    answer = "See upgrade-helper.ts:15."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert multiplier == 1.0
    assert results[0].ok
    assert results[0].imprecise
    assert results[0].resolved_file is None
    assert results[0].reason == "ambiguous_suffix"


def test_ambiguous_suffix_resolves_via_answer_context(tmp_path):
    # R030: django answers name django/db/backends/base/operations.py in
    # full once, then repeat "base/operations.py:18" in prose. The repeat is
    # ambiguous against the contrib/gis shadow tree in isolation, but a
    # reader of the whole answer resolves it; the scorer must too.
    _monorepo(tmp_path)
    other = tmp_path / "packages" / "frontend" / "upgrade-helper.ts"
    other.parent.mkdir(parents=True)
    other.write_text("line\n" * 100)  # both candidates contain line 15
    answer = (
        "The helper lives in packages/backend/src/api/upgrade-helper.ts. "
        "Rotation happens at upgrade-helper.ts:15."
    )
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert multiplier == 1.0
    bad = [r for r in results if not r.ok]
    assert not bad
    resolved = [r for r in results if r.imprecise and r.resolved_file]
    assert any(
        r.resolved_file == "packages/backend/src/api/upgrade-helper.ts" for r in resolved
    )


def test_ambiguous_suffix_with_both_candidates_mentioned_stays_unresolved(tmp_path):
    # Context resolution must not pick a side when the answer names both
    # candidates; the cite stays imprecise with no resolved_file.
    _monorepo(tmp_path)
    other = tmp_path / "packages" / "frontend" / "upgrade-helper.ts"
    other.parent.mkdir(parents=True)
    other.write_text("line\n" * 100)
    answer = (
        "Both packages/backend/src/api/upgrade-helper.ts and "
        "packages/frontend/upgrade-helper.ts exist; see upgrade-helper.ts:15."
    )
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert multiplier == 1.0
    assert results[-1].ok
    assert results[-1].imprecise
    assert results[-1].resolved_file is None
    assert results[-1].reason == "ambiguous_suffix"


def test_fabricated_path_stays_hallucinated(tmp_path):
    _monorepo(tmp_path)
    answer = "See totally/made-up.ts:15."
    multiplier, results = score.compute_citation_multiplier(answer, tmp_path)
    assert multiplier == 0.0
    assert not results[0].ok


def test_citation_hit_accepts_suffix_cited_expected_file():
    q = _q()  # expected: src/foo.rs lines 10-30
    answer = "The bar function is defined in foo.rs:15."
    result = score.mech_score(q, answer)
    assert result["citation_hit"] is True


def test_citation_hit_suffix_still_requires_range_overlap():
    q = _q()
    answer = "The bar function is defined in foo.rs:200."
    result = score.mech_score(q, answer)
    assert result["citation_hit"] is False


def test_citation_hit_suffix_does_not_match_different_file():
    q = _q()  # expected src/foo.rs
    answer = "Defined in oo.rs:15 and also barfoo.rs:15."
    result = score.mech_score(q, answer)
    assert result["citation_hit"] is False


def test_forbidden_soft_penalty_subtracts_quarter_per_hit():
    q = _q(forbidden=["src/banned.rs", "fake_helper"])
    answer = "src/foo.rs:10-30 bar - but also see src/banned.rs and fake_helper."
    final = score.final_mech(q, answer, citation_multiplier=1.0)
    # raw=1.0, multiplier=1.0, penalty = 2 * 0.25 = 0.5 => 0.5
    assert final == pytest.approx(0.5)


def test_forbidden_strict_zeros_on_any_hit():
    q = _q(forbidden=["src/banned.rs"], forbidden_strict=True)
    answer = "src/foo.rs:10 bar (also implemented in src/banned.rs)."
    final = score.final_mech(q, answer, citation_multiplier=1.0)
    assert final == 0.0


def test_forbidden_negated_mention_not_counted():
    q = _q(forbidden=["RedisCache", "ioredis"])
    answer = (
        "No, there is no RedisCache class in the codebase. "
        "The project does not import ioredis anywhere; rate limiting uses an in-memory Map."
    )
    assert score.forbidden_hits(q, answer) == []


def test_forbidden_denial_phrasing_not_counted():
    q = _q(forbidden=["LegacyAuthMiddleware"])
    answer = "There is no LegacyAuthMiddleware; authentication is handled by authPlugin in macro.ts."
    assert score.forbidden_hits(q, answer) == []


def test_forbidden_affirmative_mention_still_counted():
    q = _q(forbidden=["RedisCache"])
    answer = "Caching is implemented by the RedisCache class in cache.py."
    assert score.forbidden_hits(q, answer) == ["RedisCache"]


def test_forbidden_negation_does_not_leak_across_sentences():
    q = _q(forbidden=["RedisCache"])
    answer = "There is no LRU layer. The RedisCache class handles all caching."
    assert score.forbidden_hits(q, answer) == ["RedisCache"]


def test_forbidden_mixed_negated_and_affirmative_counts():
    q = _q(forbidden=["RedisCache"])
    answer = "There is no RedisCache in src/. However, RedisCache is defined in vendor/cache.py."
    assert score.forbidden_hits(q, answer) == ["RedisCache"]


def test_combined_pipeline_clamps_to_zero():
    q = _q(forbidden=["a", "b", "c", "d", "e"])  # 5 hits = -1.25 penalty
    answer = "src/foo.rs:10-30 bar - a b c d e."
    final = score.final_mech(q, answer, citation_multiplier=1.0)
    assert final == 0.0

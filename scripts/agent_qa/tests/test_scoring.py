from scripts.agent_qa.qa_schema import QAEntry, Expected
from scripts.agent_qa.scoring import mech_score


def _entry(files=None, symbols=None, facts=None):
    return QAEntry(
        id="x",
        question="q",
        expected=Expected(
            files=files or [],
            symbols=symbols or [],
            facts=facts or [],
        ),
        rubric="r",
    )


def test_perfect_match_scores_one():
    entry = _entry(files=["src/foo.rs"], symbols=["BAR"], facts=["0.4"])
    answer = "See src/foo.rs where BAR is set to 0.4."
    s = mech_score(entry, answer)
    assert s.combined == 1.0
    assert s.files_hit == 1.0
    assert s.symbols_hit == 1.0
    assert s.facts_hit == 1.0


def test_case_insensitive_substring():
    entry = _entry(files=["src/Foo.rs"], symbols=["bar"])
    answer = "look in SRC/foo.rs at BAR."
    s = mech_score(entry, answer)
    assert s.files_hit == 1.0
    assert s.symbols_hit == 1.0


def test_partial_match():
    entry = _entry(files=["a.rs", "b.rs"], symbols=["X", "Y"])
    answer = "a.rs has X."
    s = mech_score(entry, answer)
    assert s.files_hit == 0.5
    assert s.symbols_hit == 0.5
    # No facts -> facts_hit is 1.0 (vacuous), but combined ignores empty buckets
    assert 0.4 < s.combined < 0.6


def test_or_group_in_facts():
    entry = _entry(facts=[["0.4", "0.40"], "ratio"])
    # "0.4" satisfies the OR group; "ratio" matches.
    s = mech_score(entry, "the 0.40 ratio is the gate")
    assert s.facts_hit == 1.0


def test_or_group_partial():
    entry = _entry(facts=[["0.4", "0.40"], "ratio"])
    s = mech_score(entry, "the 0.4 number")  # "ratio" missing
    assert s.facts_hit == 0.5


def test_empty_buckets_are_vacuous_not_zero():
    entry = _entry(files=["foo.rs"])  # no symbols, no facts
    s = mech_score(entry, "foo.rs is here")
    assert s.combined == 1.0


def test_combined_weights_when_all_buckets_present():
    entry = _entry(files=["a"], symbols=["B"], facts=["c"])
    # only files hit
    s = mech_score(entry, "a is here")
    # weights: files 0.4, symbols 0.4, facts 0.2 -> 0.4
    assert abs(s.combined - 0.4) < 1e-6

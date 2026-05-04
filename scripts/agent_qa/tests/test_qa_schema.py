import pytest
from scripts.agent_qa.qa_schema import (
    QAEntry,
    SchemaError,
    validate_qa_set,
    load_qa_set,
)


def test_valid_entry_parses():
    raw = {
        "id": "self-q1",
        "question": "Where is X?",
        "expected": {
            "files": ["src/foo.rs"],
            "symbols": ["BAR"],
            "facts": [["0.4", "0.40"], "ratio threshold"],
        },
        "rubric": "Names src/foo.rs and the 0.4 value.",
    }
    entry = QAEntry.from_dict(raw)
    assert entry.id == "self-q1"
    assert entry.expected.files == ["src/foo.rs"]
    assert entry.expected.symbols == ["BAR"]
    assert entry.expected.facts == [["0.4", "0.40"], "ratio threshold"]
    assert entry.rubric.startswith("Names")


def test_missing_field_raises():
    with pytest.raises(SchemaError) as exc:
        QAEntry.from_dict({"id": "x", "question": "y"})
    assert "expected" in str(exc.value)


def test_validate_qa_set_rejects_duplicate_ids():
    entries = [
        {
            "id": "a",
            "question": "q1",
            "expected": {"files": ["f.rs"], "symbols": [], "facts": []},
            "rubric": "r",
        },
        {
            "id": "a",
            "question": "q2",
            "expected": {"files": ["g.rs"], "symbols": [], "facts": []},
            "rubric": "r",
        },
    ]
    with pytest.raises(SchemaError) as exc:
        validate_qa_set(entries)
    assert "duplicate" in str(exc.value).lower()


def test_validate_qa_set_requires_at_least_one_expected():
    bad = [
        {
            "id": "a",
            "question": "q",
            "expected": {"files": [], "symbols": [], "facts": []},
            "rubric": "r",
        }
    ]
    with pytest.raises(SchemaError) as exc:
        validate_qa_set(bad)
    assert "expected" in str(exc.value).lower()


def test_load_qa_set_round_trips(tmp_path):
    p = tmp_path / "qa.json"
    p.write_text(
        '[{"id":"a","question":"q","expected":{"files":["f.rs"],"symbols":[],"facts":[]},"rubric":"r"}]'
    )
    entries = load_qa_set(p)
    assert len(entries) == 1
    assert entries[0].id == "a"

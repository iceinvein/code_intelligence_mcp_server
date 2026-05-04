"""Q&A entry schema, validation, and JSON loader for the agent benchmark."""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, List, Union

FactGroup = Union[str, List[str]]


class SchemaError(ValueError):
    pass


@dataclass
class Expected:
    files: List[str]
    symbols: List[str]
    facts: List[FactGroup]

    @staticmethod
    def from_dict(raw: dict) -> "Expected":
        for key in ("files", "symbols", "facts"):
            if key not in raw:
                raise SchemaError(f"expected.{key} missing")
            if not isinstance(raw[key], list):
                raise SchemaError(f"expected.{key} must be a list")
        return Expected(
            files=list(raw["files"]),
            symbols=list(raw["symbols"]),
            facts=list(raw["facts"]),
        )


@dataclass
class QAEntry:
    id: str
    question: str
    expected: Expected
    rubric: str

    @staticmethod
    def from_dict(raw: dict) -> "QAEntry":
        for key in ("id", "question", "expected", "rubric"):
            if key not in raw:
                raise SchemaError(f"missing required field: {key}")
        if not isinstance(raw["expected"], dict):
            raise SchemaError("expected must be an object")
        return QAEntry(
            id=str(raw["id"]),
            question=str(raw["question"]),
            expected=Expected.from_dict(raw["expected"]),
            rubric=str(raw["rubric"]),
        )


def validate_qa_set(entries: List[dict]) -> List[QAEntry]:
    if not isinstance(entries, list):
        raise SchemaError("Q&A set must be a JSON array")
    seen_ids: set[str] = set()
    parsed: List[QAEntry] = []
    for i, raw in enumerate(entries):
        try:
            entry = QAEntry.from_dict(raw)
        except SchemaError as e:
            raise SchemaError(f"entry[{i}]: {e}") from e
        if entry.id in seen_ids:
            raise SchemaError(f"duplicate id: {entry.id}")
        seen_ids.add(entry.id)
        if not (entry.expected.files or entry.expected.symbols or entry.expected.facts):
            raise SchemaError(
                f"entry {entry.id}: expected must have at least one of files/symbols/facts"
            )
        parsed.append(entry)
    return parsed


def load_qa_set(path: Path) -> List[QAEntry]:
    raw = json.loads(Path(path).read_text())
    return validate_qa_set(raw)

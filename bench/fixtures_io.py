"""Load and validate YAML fixture files."""
from __future__ import annotations

import hashlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml


@dataclass(frozen=True)
class Citation:
    file: str
    line_range: tuple[int, int]
    symbol: str


@dataclass(frozen=True)
class Expected:
    citations: list[Citation]
    files: list[str]
    facts: list[Any]  # each entry: str | list[str]
    forbidden: list[str]
    forbidden_strict: bool


@dataclass(frozen=True)
class Question:
    id: str
    task_type: str
    difficulty: str
    question: str
    rubric: str
    expected: Expected


@dataclass(frozen=True)
class FixtureMeta:
    repo: str
    upstream_url: str
    upstream_sha: str
    fixture_sha256: str
    authored_at: str
    authored_against_schema_version: int


@dataclass(frozen=True)
class Fixture:
    meta: FixtureMeta
    questions: list[Question]


VALID_TASK_TYPES = {
    "symbol_lookup", "concept", "multi_hop", "impact", "architectural", "negative"
}


def _parse_citation(raw: dict) -> Citation:
    return Citation(
        file=raw["file"],
        line_range=tuple(raw["line_range"]),
        symbol=raw.get("symbol", ""),
    )


def _parse_expected(raw: dict) -> Expected:
    return Expected(
        citations=[_parse_citation(c) for c in raw.get("citations", [])],
        files=list(raw.get("files", [])),
        facts=list(raw.get("facts", [])),
        forbidden=list(raw.get("forbidden", [])),
        forbidden_strict=bool(raw.get("forbidden_strict", False)),
    )


def _parse_question(raw: dict) -> Question:
    return Question(
        id=raw["id"],
        task_type=raw["task_type"],
        difficulty=raw.get("difficulty", "medium"),
        question=raw["question"],
        rubric=raw["rubric"],
        expected=_parse_expected(raw["expected"]),
    )


def load_fixture(path: Path) -> Fixture:
    fixture_bytes = path.read_bytes()
    data = yaml.safe_load(fixture_bytes)
    meta = data["meta"]
    return Fixture(
        meta=FixtureMeta(
            repo=meta["repo"],
            upstream_url=meta["upstream_url"],
            upstream_sha=meta["upstream_sha"],
            fixture_sha256=hashlib.sha256(fixture_bytes).hexdigest(),
            authored_at=meta["authored_at"],
            authored_against_schema_version=int(meta["authored_against_schema_version"]),
        ),
        questions=[_parse_question(q) for q in data["questions"]],
    )


def validate_fixture(path: Path, repo_path: Path) -> list[str]:
    """Lint a fixture file. Returns a list of error strings; empty = valid.

    repo_path: filesystem root the fixture's citations should resolve against.
    Caller is responsible for checking out the repo at the fixture's upstream_sha
    before validating.
    """
    errors: list[str] = []
    try:
        fixture = load_fixture(path)
    except Exception as e:
        return [f"failed to load fixture: {e}"]

    seen_ids: set[str] = set()
    for q in fixture.questions:
        if q.id in seen_ids:
            errors.append(f"duplicate question id: {q.id}")
        seen_ids.add(q.id)

        if q.task_type not in VALID_TASK_TYPES:
            errors.append(f"{q.id}: invalid task_type {q.task_type!r}")

        # Citation files must exist in the repo at the pinned SHA.
        for cite in q.expected.citations:
            cite_path = repo_path / cite.file
            if not cite_path.exists():
                errors.append(f"{q.id}: citation file does not exist: {cite.file}")
                continue
            try:
                lines = cite_path.read_text().splitlines()
                if cite.line_range[1] > len(lines):
                    errors.append(
                        f"{q.id}: citation line_range {cite.line_range} exceeds "
                        f"file length {len(lines)} for {cite.file}"
                    )
            except UnicodeDecodeError:
                errors.append(f"{q.id}: citation file is not text: {cite.file}")

        # forbidden_strict requires a non-empty forbidden list.
        if q.expected.forbidden_strict and not q.expected.forbidden:
            errors.append(f"{q.id}: forbidden_strict=true requires non-empty forbidden list")

    return errors

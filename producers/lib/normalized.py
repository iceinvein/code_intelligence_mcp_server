from __future__ import annotations

from dataclasses import asdict, dataclass
import json
from pathlib import Path


SKIP_DIRS = {
    ".git",
    ".hg",
    ".svn",
    ".mypy_cache",
    ".pytest_cache",
    "__pycache__",
    "node_modules",
    "dist",
    "build",
    "target",
    "vendor",
    ".venv",
    "venv",
}


@dataclass(frozen=True)
class Symbol:
    external_symbol: str
    display_name: str
    kind: str
    file_path: str | None
    start_line: int | None
    end_line: int | None
    start_byte: int | None
    end_byte: int | None


@dataclass(frozen=True)
class Reference:
    from_external_symbol: str | None
    to_external_symbol: str | None
    relationship: str
    file_path: str
    line: int
    column: int | None
    end_line: int | None
    end_column: int | None
    confidence: float | None
    provenance: str | None


@dataclass(frozen=True)
class Artifact:
    source_kind: str
    producer: str
    language: str
    root_path: str
    symbols: list[Symbol]
    references: list[Reference]


def stable_symbol_id(
    language: str,
    file_path: str,
    kind: str,
    qualified_name: str,
    start_line: int | None,
    start_byte: int | None,
) -> str:
    return f"{language}:{file_path}:{kind}:{qualified_name}:{start_line or 0}:{start_byte or 0}"


def line_index(source: str) -> list[int]:
    starts = [0]
    for index, char in enumerate(source):
        if char == "\n":
            starts.append(index + 1)
    return starts


def position_for_offset(starts: list[int], offset: int) -> tuple[int, int]:
    line = 1
    for index, start in enumerate(starts):
        if start > offset:
            break
        line = index + 1
    column = offset - starts[line - 1] + 1
    return line, column


def discover_files(root: Path, extensions: set[str]) -> list[Path]:
    found: list[Path] = []
    for path in root.rglob("*"):
        rel_parts = path.relative_to(root).parts
        if any(part in SKIP_DIRS for part in rel_parts):
            continue
        if path.is_file() and path.suffix in extensions:
            found.append(path.relative_to(root))
    return sorted(found, key=lambda item: item.as_posix())


def write_artifact(output: Path, artifact: Artifact) -> None:
    symbols = sorted(artifact.symbols, key=lambda item: (item.file_path or "", item.start_line or 0, item.external_symbol))
    references = sorted(
        artifact.references,
        key=lambda item: (item.file_path, item.line, item.column or 0, item.relationship, item.to_external_symbol or ""),
    )
    payload = {
        "source_kind": artifact.source_kind,
        "producer": artifact.producer,
        "language": artifact.language,
        "root_path": artifact.root_path,
        "symbols": [asdict(item) for item in symbols],
        "references": [asdict(item) for item in references],
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")

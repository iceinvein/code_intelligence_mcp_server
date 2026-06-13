from __future__ import annotations

from bisect import bisect_right
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
from typing import Sequence


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
    symbols: Sequence[Symbol]
    references: Sequence[Reference]


def _escape_id_field(value: str) -> str:
    return value.replace("%", "%25").replace(":", "%3A")


def _encode_optional_int(value: int | None) -> str:
    if value is None:
        return "~"
    return str(value)


def stable_symbol_id(
    language: str,
    file_path: str,
    kind: str,
    qualified_name: str,
    start_line: int | None,
    start_byte: int | None,
) -> str:
    fields = [
        _escape_id_field(language),
        _escape_id_field(file_path),
        _escape_id_field(kind),
        _escape_id_field(qualified_name),
        _encode_optional_int(start_line),
        _encode_optional_int(start_byte),
    ]
    return ":".join(fields)


def line_index(source: str) -> list[int]:
    """Return UTF-8 byte line starts plus an EOF sentinel byte offset."""
    encoded = source.encode("utf-8")
    starts = [0]
    for index, byte in enumerate(encoded):
        if byte == 0x0A:
            starts.append(index + 1)
    if starts[-1] != len(encoded):
        starts.append(len(encoded))
    return starts


def position_for_offset(starts: list[int], offset: int) -> tuple[int, int]:
    """Convert a UTF-8 byte offset to a one-based line and byte column."""
    if not starts:
        raise ValueError("line index cannot be empty")
    if offset < 0:
        raise ValueError(f"offset must be non-negative: {offset}")
    if offset >= starts[-1]:
        raise ValueError(f"offset {offset} is outside indexed source length {starts[-1]}")

    line = bisect_right(starts, offset)
    column = offset - starts[line - 1] + 1
    return line, column


def discover_files(root: Path, extensions: set[str]) -> list[Path]:
    found: list[Path] = []
    root = Path(root)
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(dirname for dirname in dirnames if dirname not in SKIP_DIRS)
        current = Path(dirpath)
        for filename in filenames:
            path = current / filename
            if path.suffix in extensions:
                found.append(path.relative_to(root))
    return sorted(found, key=lambda item: item.as_posix())


def _canonical_json(value: object) -> str:
    return json.dumps(asdict(value), sort_keys=True, separators=(",", ":"))


def write_artifact(output: Path, artifact: Artifact) -> None:
    symbols = sorted(artifact.symbols, key=_canonical_json)
    references = sorted(artifact.references, key=_canonical_json)
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

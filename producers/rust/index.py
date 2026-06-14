#!/usr/bin/env python3
from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import sys


BUNDLE_ROOT = Path(__file__).resolve().parents[2]
if str(BUNDLE_ROOT) not in sys.path:
    sys.path.insert(0, str(BUNDLE_ROOT))

from producers.lib.normalized import (  # noqa: E402
    Artifact,
    Reference,
    Symbol,
    discover_files,
    line_index,
    position_for_offset,
    stable_symbol_id,
    write_artifact,
)


PRODUCER = "code-intelligence-external-rust"
SOURCE_KIND = "rust_source"
LANGUAGE = "rust"
CONFIDENCE = 0.65

IDENTIFIER = r"[A-Za-z_][A-Za-z0-9_]*"
TYPE_RE = re.compile(
    rf"\b(?:pub(?:\([^)]*\))?\s+)?(struct|enum|trait)\s+({IDENTIFIER})"
)
FN_RE = re.compile(
    rf"\b(?:pub(?:\([^)]*\))?\s+)?"
    rf"(?:(?:async|const|unsafe)\s+)*"
    rf"(?:extern\s+(?:\"[^\"]+\"\s+)?)?"
    rf"fn\s+({IDENTIFIER})"
)
IMPL_RE = re.compile(r"\bimpl\b")
DIRECT_CALL_RE = re.compile(rf"(?<![\w.])({IDENTIFIER})\s*(?=\()")
MEMBER_CALL_RE = re.compile(rf"\b({IDENTIFIER})\s*\.\s*({IDENTIFIER})\s*(?=\()")
LET_EXPLICIT_TYPE_RE = re.compile(
    rf"\blet\s+(?:mut\s+)?({IDENTIFIER})\s*:\s*([A-Za-z_][A-Za-z0-9_:<>]*)"
)
LET_FUNCTION_CALL_RE = re.compile(
    rf"\blet\s+(?:mut\s+)?({IDENTIFIER})\s*(?::[^=;]+)?=\s*({IDENTIFIER})\s*\("
)
RETURN_TYPE_RE = re.compile(r"->\s*([A-Za-z_][A-Za-z0-9_:<>]*)")


@dataclass(frozen=True)
class LocalSymbol:
    symbol: Symbol
    body_start: int | None
    body_end: int | None
    return_type: str | None = None


@dataclass(frozen=True)
class ImplRange:
    type_name: str
    open_brace: int
    close_brace: int


@dataclass(frozen=True)
class ItemRange:
    kind: str
    name: str
    open_brace: int
    close_brace: int


def char_byte_offsets(source: str) -> list[int]:
    offsets = [0]
    total = 0
    for char in source:
        total += len(char.encode("utf-8"))
        offsets.append(total)
    return offsets


def mask_range(buffer: list[str], start: int, end: int) -> None:
    for index in range(start, min(end, len(buffer))):
        if buffer[index] != "\n":
            buffer[index] = " "


def raw_string_end(source: str, start: int) -> int | None:
    if source.startswith("br", start):
        start += 2
    elif source.startswith("r", start):
        start += 1
    else:
        return None

    hashes_start = start
    while start < len(source) and source[start] == "#":
        start += 1
    if start >= len(source) or source[start] != '"':
        return None

    hashes = source[hashes_start:start]
    terminator = '"' + hashes
    found = source.find(terminator, start + 1)
    if found == -1:
        return len(source)
    return found + len(terminator)


def mask_non_code(source: str) -> str:
    masked = list(source)
    index = 0
    while index < len(source):
        raw_end = raw_string_end(source, index)
        if raw_end is not None:
            mask_range(masked, index, raw_end)
            index = raw_end
            continue

        two = source[index : index + 2]
        if two == "//":
            end = source.find("\n", index)
            if end == -1:
                end = len(source)
            mask_range(masked, index, end)
            index = end
            continue

        if two == "/*":
            depth = 1
            cursor = index + 2
            while cursor < len(source) and depth:
                if source.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif source.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            mask_range(masked, index, cursor)
            index = cursor
            continue

        if source[index] == '"':
            cursor = index + 1
            escaped = False
            while cursor < len(source):
                char = source[cursor]
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    cursor += 1
                    break
                cursor += 1
            mask_range(masked, index, cursor)
            index = cursor
            continue

        if source[index] == "'" and index + 1 < len(source) and not source[index + 1].isalpha():
            cursor = index + 1
            escaped = False
            while cursor < len(source):
                char = source[cursor]
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == "'":
                    cursor += 1
                    break
                elif char == "\n":
                    break
                cursor += 1
            mask_range(masked, index, cursor)
            index = cursor
            continue

        index += 1
    return "".join(masked)


def matching_brace(masked: str, open_brace: int) -> int | None:
    depth = 0
    for index in range(open_brace, len(masked)):
        char = masked[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index
    return None


def find_item_end(masked: str, start: int) -> int:
    open_brace = masked.find("{", start)
    semicolon = masked.find(";", start)
    if open_brace != -1 and (semicolon == -1 or open_brace < semicolon):
        close_brace = matching_brace(masked, open_brace)
        if close_brace is not None:
            return close_brace + 1
    if semicolon != -1:
        return semicolon + 1
    newline = masked.find("\n", start)
    return len(masked) if newline == -1 else newline


def find_body_range(masked: str, start: int, end: int) -> tuple[int | None, int | None]:
    open_brace = masked.find("{", start, end)
    if open_brace == -1:
        return None, None
    close_brace = matching_brace(masked, open_brace)
    if close_brace is None:
        return None, None
    return open_brace + 1, close_brace


def parse_impl_type(header: str) -> str | None:
    rest = header.strip()
    if not rest.startswith("impl"):
        return None
    rest = rest[len("impl") :].strip()
    if rest.startswith("<"):
        depth = 0
        for index, char in enumerate(rest):
            if char == "<":
                depth += 1
            elif char == ">":
                depth -= 1
                if depth == 0:
                    rest = rest[index + 1 :].strip()
                    break
    if " for " in rest:
        rest = rest.rsplit(" for ", 1)[1].strip()
    match = re.search(IDENTIFIER, rest.rsplit("::", 1)[-1])
    return match.group(0) if match else None


def normalize_type_name(type_text: str) -> str | None:
    text = type_text.strip().lstrip("&").strip()
    if text.startswith("mut "):
        text = text[len("mut ") :].strip()
    prefix = text.split("<", 1)[0]
    match = re.search(IDENTIFIER, prefix.rsplit("::", 1)[-1])
    return match.group(0) if match else None


def parse_return_type(masked: str, start: int, end: int) -> str | None:
    open_brace = masked.find("{", start, end)
    semicolon = masked.find(";", start, end)
    signature_end = end
    if open_brace != -1:
        signature_end = min(signature_end, open_brace)
    if semicolon != -1:
        signature_end = min(signature_end, semicolon)
    match = RETURN_TYPE_RE.search(masked, start, signature_end)
    if match is None:
        return None
    return normalize_type_name(match.group(1))


def collect_impl_ranges(masked: str) -> list[ImplRange]:
    ranges: list[ImplRange] = []
    for match in IMPL_RE.finditer(masked):
        open_brace = masked.find("{", match.end())
        if open_brace == -1:
            continue
        line_end = masked.find("\n", match.end())
        if line_end != -1 and line_end < open_brace:
            continue
        close_brace = matching_brace(masked, open_brace)
        if close_brace is None:
            continue
        type_name = parse_impl_type(masked[match.start() : open_brace])
        if type_name:
            ranges.append(ImplRange(type_name, open_brace, close_brace))
    return sorted(ranges, key=lambda item: (item.open_brace, item.close_brace, item.type_name))


def containing_impl(offset: int, ranges: list[ImplRange]) -> ImplRange | None:
    candidates = [
        item for item in ranges if item.open_brace < offset < item.close_brace
    ]
    if not candidates:
        return None
    return min(candidates, key=lambda item: item.close_brace - item.open_brace)


def containing_item(offset: int, ranges: list[ItemRange], kind: str) -> ItemRange | None:
    candidates = [
        item
        for item in ranges
        if item.kind == kind and item.open_brace < offset < item.close_brace
    ]
    if not candidates:
        return None
    return min(candidates, key=lambda item: item.close_brace - item.open_brace)


def symbol_for_match(
    rel_path: str,
    offsets: list[int],
    starts: list[int],
    kind: str,
    display_name: str,
    qualified_name: str,
    start: int,
    end: int,
) -> Symbol:
    start_byte = offsets[start]
    end_byte = offsets[end]
    start_line, _ = position_for_offset(starts, start_byte)
    end_line, _ = position_for_offset(starts, max(start_byte, end_byte - 1))
    return Symbol(
        stable_symbol_id(LANGUAGE, rel_path, kind, qualified_name, start_line, start_byte),
        display_name,
        kind,
        rel_path,
        start_line,
        end_line,
        start_byte,
        end_byte,
    )


def collect_file_symbols(root: Path, rel: Path) -> list[LocalSymbol]:
    source = (root / rel).read_text(encoding="utf-8")
    rel_path = rel.as_posix()
    masked = mask_non_code(source)
    offsets = char_byte_offsets(source)
    starts = line_index(source)
    impl_ranges = collect_impl_ranges(masked)

    symbols: list[LocalSymbol] = []
    item_ranges: list[ItemRange] = []
    for match in TYPE_RE.finditer(masked):
        kind, name = match.groups()
        end = find_item_end(masked, match.end())
        body_start, body_end = find_body_range(masked, match.end(), end)
        symbol = symbol_for_match(
            rel_path,
            offsets,
            starts,
            kind,
            name,
            name,
            match.start(),
            end,
        )
        symbols.append(LocalSymbol(symbol, body_start, body_end))
        if body_start is not None and body_end is not None:
            item_ranges.append(ItemRange(kind, name, body_start - 1, body_end))

    for match in FN_RE.finditer(masked):
        name = match.group(1)
        impl = containing_impl(match.start(), impl_ranges)
        if impl is None and containing_item(match.start(), item_ranges, "trait") is not None:
            continue

        item_end = find_item_end(masked, match.end())
        body_start, body_end = find_body_range(masked, match.end(), item_end)
        if impl is not None:
            kind = "method"
            display_name = f"{impl.type_name}.{name}"
        else:
            kind = "function"
            display_name = name
        symbol = symbol_for_match(
            rel_path,
            offsets,
            starts,
            kind,
            display_name,
            display_name,
            match.start(),
            item_end,
        )
        return_type = parse_return_type(masked, match.end(), item_end)
        symbols.append(LocalSymbol(symbol, body_start, body_end, return_type))

    return sorted(
        symbols,
        key=lambda item: (
            item.symbol.file_path or "",
            item.symbol.start_byte if item.symbol.start_byte is not None else -1,
            item.symbol.display_name,
        ),
    )


def reference_for_call(
    rel_path: str,
    offsets: list[int],
    starts: list[int],
    from_external: str,
    to_external: str,
    start: int,
    end: int,
) -> Reference:
    start_byte = offsets[start]
    end_byte = offsets[end]
    line, column = position_for_offset(starts, start_byte)
    end_line, end_column = position_for_offset(starts, max(start_byte, end_byte - 1))
    return Reference(
        from_external,
        to_external,
        "call",
        rel_path,
        line,
        column,
        end_line,
        end_column,
        CONFIDENCE,
        SOURCE_KIND,
    )


def receiver_type_for_local(local: LocalSymbol) -> str | None:
    if local.symbol.kind != "method" or "." not in local.symbol.display_name:
        return None
    return local.symbol.display_name.rsplit(".", 1)[0]


def infer_receiver_types(
    body: str,
    local: LocalSymbol,
    free_function_return_types: dict[str, str],
) -> dict[str, str]:
    receiver_types: dict[str, str] = {}
    self_type = receiver_type_for_local(local)
    if self_type is not None:
        receiver_types["self"] = self_type

    for match in LET_EXPLICIT_TYPE_RE.finditer(body):
        type_name = normalize_type_name(match.group(2))
        if type_name is not None:
            receiver_types[match.group(1)] = type_name

    for match in LET_FUNCTION_CALL_RE.finditer(body):
        variable_name, function_name = match.groups()
        return_type = free_function_return_types.get(function_name)
        if return_type is not None:
            receiver_types.setdefault(variable_name, return_type)

    return receiver_types


def collect_file_references(
    root: Path,
    rel: Path,
    local_symbols: list[LocalSymbol],
    free_functions: dict[str, str],
    free_function_return_types: dict[str, str],
    methods_by_type_and_name: dict[tuple[str, str], str],
) -> list[Reference]:
    source = (root / rel).read_text(encoding="utf-8")
    masked = mask_non_code(source)
    offsets = char_byte_offsets(source)
    starts = line_index(source)
    references: list[Reference] = []
    rel_path = rel.as_posix()

    for local in local_symbols:
        if local.body_start is None or local.body_end is None:
            continue
        from_external = local.symbol.external_symbol
        body = masked[local.body_start : local.body_end]
        receiver_types = infer_receiver_types(body, local, free_function_return_types)

        for match in MEMBER_CALL_RE.finditer(body):
            receiver_name, name = match.groups()
            receiver_type = receiver_types.get(receiver_name)
            if receiver_type is None:
                continue
            target = methods_by_type_and_name.get((receiver_type, name))
            if target is None:
                continue
            start = local.body_start + match.start(2)
            end = local.body_start + match.end(2)
            references.append(
                reference_for_call(
                    rel_path,
                    offsets,
                    starts,
                    from_external,
                    target,
                    start,
                    end,
                )
            )

        for match in DIRECT_CALL_RE.finditer(body):
            name = match.group(1)
            target = free_functions.get(name)
            if target is None:
                continue
            start = local.body_start + match.start(1)
            end = local.body_start + match.end(1)
            references.append(
                reference_for_call(
                    rel_path,
                    offsets,
                    starts,
                    from_external,
                    target,
                    start,
                    end,
                )
            )

    return references


def discover_rust_files(root: Path) -> list[Path]:
    files = discover_files(root, {".rs"})
    preferred = [rel for rel in files if "src" in rel.parts]
    return preferred or files


def collect(root: Path) -> Artifact:
    files = discover_rust_files(root)
    symbols_by_file: dict[Path, list[LocalSymbol]] = {}
    symbols: list[Symbol] = []
    for rel in files:
        local = collect_file_symbols(root, rel)
        symbols_by_file[rel] = local
        symbols.extend(item.symbol for item in local)

    free_functions: dict[str, str] = {}
    function_candidates: dict[str, list[Symbol]] = {}
    function_return_type_candidates: dict[str, list[str]] = {}
    free_function_return_types: dict[str, str] = {}
    method_candidates: dict[tuple[str, str], list[Symbol]] = {}
    methods_by_type_and_name: dict[tuple[str, str], str] = {}
    symbols_by_external = {
        local.symbol.external_symbol: local
        for local_symbols in symbols_by_file.values()
        for local in local_symbols
    }
    for symbol in sorted(
        symbols,
        key=lambda item: (
            item.display_name,
            item.file_path or "",
            item.start_byte if item.start_byte is not None else -1,
        ),
    ):
        if symbol.kind == "function":
            function_candidates.setdefault(symbol.display_name, []).append(symbol)
            local = symbols_by_external.get(symbol.external_symbol)
            if local is not None and local.return_type is not None:
                function_return_type_candidates.setdefault(symbol.display_name, []).append(
                    local.return_type
                )
        elif symbol.kind == "method":
            type_name, method_name = symbol.display_name.rsplit(".", 1)
            method_candidates.setdefault((type_name, method_name), []).append(symbol)

    for name, candidates in function_candidates.items():
        if len(candidates) == 1:
            free_functions[name] = candidates[0].external_symbol

    for name, candidates in function_return_type_candidates.items():
        unique_return_types = sorted(set(candidates))
        if len(function_candidates.get(name, [])) == 1 and len(unique_return_types) == 1:
            free_function_return_types[name] = unique_return_types[0]

    for key, candidates in method_candidates.items():
        if len(candidates) == 1:
            methods_by_type_and_name[key] = candidates[0].external_symbol

    references: list[Reference] = []
    for rel in files:
        references.extend(
            collect_file_references(
                root,
                rel,
                symbols_by_file[rel],
                free_functions,
                free_function_return_types,
                methods_by_type_and_name,
            )
        )

    return Artifact(SOURCE_KIND, PRODUCER, LANGUAGE, str(root), symbols, references)


class UsageArgumentParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        self.print_usage(sys.stderr)
        self.exit(64, f"{self.prog}: error: {message}\n")


def main(argv: list[str]) -> int:
    parser = UsageArgumentParser(prog=PRODUCER)
    parser.add_argument("command", choices=["index"])
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)

    root = Path.cwd()
    files = discover_rust_files(root)
    if not files and not (root / "Cargo.toml").exists():
        print("no Rust project or source files found", file=sys.stderr)
        return 69

    try:
        write_artifact(Path(args.output), collect(root))
    except OSError as exc:
        print(f"failed to write output: {exc}", file=sys.stderr)
        return 64
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

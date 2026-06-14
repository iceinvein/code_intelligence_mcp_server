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


PRODUCER = "code-intelligence-external-go"
SOURCE_KIND = "go_source"
LANGUAGE = "go"
CONFIDENCE = 0.65

IDENTIFIER = r"[A-Za-z_][A-Za-z0-9_]*"
PACKAGE_RE = re.compile(rf"(?m)^\s*package\s+({IDENTIFIER})\b")
TYPE_RE = re.compile(rf"\btype\s+({IDENTIFIER})\b")
FUNC_RE = re.compile(
    rf"\bfunc\s*"
    rf"(?:\(\s*(?:(?P<recv_name>{IDENTIFIER})\s+)?"
    rf"(?P<recv>\*?\s*{IDENTIFIER}(?:\s*\[[^\]]+\])?)\s*\)\s*)?"
    rf"(?P<name>{IDENTIFIER})(?:\s*\[[^\]]+\])?\s*\("
)
IMPORT_RE = re.compile(r"\bimport\b")
IMPORT_SPEC_RE = re.compile(
    rf"(?m)^\s*(?:(?P<alias>\.|_|{IDENTIFIER})\s+)?\"(?P<path>[^\"]+)\""
)
SELECTOR_CALL_RE = re.compile(rf"\b({IDENTIFIER})\s*\.\s*({IDENTIFIER})\s*(?=\()")
DIRECT_CALL_RE = re.compile(rf"(?<![\w.])({IDENTIFIER})\s*(?=\()")
SHORT_DECL_RE = re.compile(rf"\b({IDENTIFIER})\s*:=")
VAR_DECL_RE = re.compile(rf"\bvar\s+({IDENTIFIER})(?:\s+([^=\n;]+))?")
ASSIGNMENT_RE = re.compile(rf"(?<![:\w.])({IDENTIFIER})\s*=(?!=)")
MODULE_RE = re.compile(r"(?m)^\s*module\s+(\S+)\s*$")
RESERVED_CALLS = {
    "append",
    "cap",
    "close",
    "complex",
    "copy",
    "delete",
    "imag",
    "len",
    "make",
    "new",
    "panic",
    "print",
    "println",
    "real",
    "recover",
}


@dataclass(frozen=True)
class ReceiverType:
    package_dir: str
    type_name: str


@dataclass(frozen=True)
class LocalSymbol:
    symbol: Symbol
    package_name: str
    package_dir: str
    body_start: int | None
    body_end: int | None
    return_type: ReceiverType | None = None
    param_names: tuple[str, ...] = ()
    receiver_name: str | None = None


@dataclass(frozen=True)
class ImportSpec:
    alias: str | None
    import_path: str
    start: int
    end: int
    target_package_dir: str | None


@dataclass(frozen=True)
class BindingEvent:
    offset: int
    name: str
    receiver_type: ReceiverType | None
    introduces_name: bool


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


def mask_non_code(source: str) -> str:
    masked = list(source)
    index = 0
    while index < len(source):
        two = source[index : index + 2]
        if two == "//":
            end = source.find("\n", index)
            if end == -1:
                end = len(source)
            mask_range(masked, index, end)
            index = end
            continue

        if two == "/*":
            cursor = index + 2
            while cursor < len(source) and not source.startswith("*/", cursor):
                cursor += 1
            cursor = min(len(source), cursor + 2)
            mask_range(masked, index, cursor)
            index = cursor
            continue

        if source[index] == "`":
            cursor = source.find("`", index + 1)
            cursor = len(source) if cursor == -1 else cursor + 1
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

        if source[index] == "'":
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


def matching_delimiter(
    masked: str, open_index: int, open_char: str, close_char: str
) -> int | None:
    depth = 0
    for index in range(open_index, len(masked)):
        char = masked[index]
        if char == open_char:
            depth += 1
        elif char == close_char:
            depth -= 1
            if depth == 0:
                return index
    return None


def matching_brace(masked: str, open_brace: int) -> int | None:
    return matching_delimiter(masked, open_brace, "{", "}")


def find_line_end(source: str, start: int) -> int:
    newline = source.find("\n", start)
    return len(source) if newline == -1 else newline


def find_function_end(masked: str, start: int) -> tuple[int, int | None, int | None]:
    open_paren = masked.find("(", start)
    close_paren = (
        matching_delimiter(masked, open_paren, "(", ")") if open_paren != -1 else None
    )
    search_start = start if close_paren is None else close_paren + 1
    open_brace = masked.find("{", search_start)
    line_end = find_line_end(masked, search_start)
    if open_brace == -1 or open_brace > line_end:
        return line_end, None, None
    close_brace = matching_brace(masked, open_brace)
    if close_brace is None:
        return len(masked), open_brace + 1, len(masked)
    return close_brace + 1, open_brace + 1, close_brace


def symbol_for_range(
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


def reference_for_range(
    relationship: str,
    rel_path: str,
    offsets: list[int],
    starts: list[int],
    from_external: str | None,
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
        relationship,
        rel_path,
        line,
        column,
        end_line,
        end_column,
        CONFIDENCE,
        SOURCE_KIND,
    )


def package_dir_for_file(rel: Path) -> str:
    parent = rel.parent.as_posix()
    return "" if parent == "." else parent


def parse_package(masked: str) -> tuple[str | None, re.Match[str] | None]:
    match = PACKAGE_RE.search(masked)
    if match is None:
        return None, None
    return match.group(1), match


def split_top_level_commas(text: str) -> list[str]:
    parts: list[str] = []
    start = 0
    depths = {"(": 0, "[": 0, "{": 0}
    closing = {")": "(", "]": "[", "}": "{"}
    for index, char in enumerate(text):
        if char in depths:
            depths[char] += 1
        elif char in closing:
            opener = closing[char]
            if depths[opener] > 0:
                depths[opener] -= 1
        elif char == "," and all(depth == 0 for depth in depths.values()):
            parts.append(text[start:index])
            start = index + 1
    parts.append(text[start:])
    return parts


def parse_param_names(signature: str) -> tuple[str, ...]:
    match = FUNC_RE.search(signature)
    if match is None:
        return ()
    open_paren = signature.find("(", match.end("name"))
    if open_paren == -1:
        return ()
    close_paren = matching_delimiter(signature, open_paren, "(", ")")
    if close_paren is None:
        return ()
    params = signature[open_paren + 1 : close_paren]
    names: list[str] = []
    for part in split_top_level_commas(params):
        part = part.strip()
        if not part or part == "...":
            continue
        if part.startswith(("func(", "chan ", "<-chan ")):
            continue
        tokens = part.split()
        if len(tokens) < 2:
            continue
        binding_part = tokens[0]
        for name in binding_part.split(","):
            if re.fullmatch(IDENTIFIER, name):
                names.append(name)
    return tuple(names)


def normalize_type_name(type_text: str) -> str | None:
    text = type_text.strip()
    while text.startswith(("*", "[]")):
        text = text[1:].strip() if text.startswith("*") else text[2:].strip()
    if text.startswith("..."):
        text = text[3:].strip()
    text = text.split("[", 1)[0].strip()
    match = re.search(rf"(?:({IDENTIFIER})\s*\.\s*)?({IDENTIFIER})", text)
    if match is None:
        return None
    return match.group(2)


def parse_return_type(
    package_dir: str,
    signature: str,
    import_aliases: dict[str, str],
) -> ReceiverType | None:
    open_paren = signature.find("(")
    if open_paren == -1:
        return None
    close_paren = matching_delimiter(signature, open_paren, "(", ")")
    if close_paren is None:
        return None
    rest = signature[close_paren + 1 :].strip()
    if not rest:
        return None
    if rest.startswith("("):
        close_returns = matching_delimiter(rest, 0, "(", ")")
        if close_returns is None:
            return None
        returns = split_top_level_commas(rest[1:close_returns])
        if len(returns) != 1:
            return None
        rest = returns[0].strip()
        tokens = rest.split()
        if len(tokens) == 2 and re.fullmatch(IDENTIFIER, tokens[0]):
            rest = tokens[1]
    type_name = normalize_type_name(rest)
    if type_name is None:
        return None
    package_match = re.search(rf"({IDENTIFIER})\s*\.\s*{type_name}", rest)
    if package_match is not None:
        target_dir = import_aliases.get(package_match.group(1))
        if target_dir is None:
            return None
        return ReceiverType(target_dir, type_name)
    return ReceiverType(package_dir, type_name)


def discover_go_files(root: Path) -> list[Path]:
    return [
        rel
        for rel in discover_files(root, {".go"})
        if not rel.name.endswith("_test.go")
    ]


def has_go_project_or_files(root: Path) -> bool:
    return (root / "go.mod").exists() or bool(discover_files(root, {".go"}))


def read_module_path(root: Path) -> str | None:
    go_mod = root / "go.mod"
    if not go_mod.exists():
        return None
    match = MODULE_RE.search(go_mod.read_text(encoding="utf-8", errors="ignore"))
    return match.group(1) if match is not None else None


def build_import_path_map(
    module_path: str | None,
    packages: dict[str, tuple[str, str]],
) -> dict[str, str]:
    mapping: dict[str, str] = {}
    for package_dir, (package_name, _external) in packages.items():
        if package_dir:
            mapping[package_dir] = package_dir
            mapping[package_name] = package_dir
            if module_path is not None:
                mapping[f"{module_path}/{package_dir}"] = package_dir
        elif module_path is not None:
            mapping[module_path] = package_dir
    return mapping


def resolve_import_path(import_path: str, import_path_map: dict[str, str]) -> str | None:
    return import_path_map.get(import_path)


def parse_imports(
    source: str,
    masked: str,
    import_path_map: dict[str, str],
) -> list[ImportSpec]:
    specs: list[ImportSpec] = []
    for match in IMPORT_RE.finditer(masked):
        cursor = match.end()
        while cursor < len(masked) and masked[cursor].isspace() and masked[cursor] != "\n":
            cursor += 1
        if cursor < len(masked) and masked[cursor] == "(":
            close = matching_delimiter(masked, cursor, "(", ")")
            if close is None:
                continue
            segment_start = cursor + 1
            segment_end = close
        else:
            segment_start = match.end()
            segment_end = find_line_end(masked, match.end())

        segment = source[segment_start:segment_end]
        for spec_match in IMPORT_SPEC_RE.finditer(segment):
            import_path = spec_match.group("path")
            alias = spec_match.group("alias")
            start = segment_start + spec_match.start("path")
            end = segment_start + spec_match.end("path")
            specs.append(
                ImportSpec(
                    alias,
                    import_path,
                    start,
                    end,
                    resolve_import_path(import_path, import_path_map),
                )
            )
    return sorted(specs, key=lambda item: (item.start, item.import_path, item.alias or ""))


def type_declaration_ranges(masked: str) -> list[tuple[str, int, int]]:
    declarations: list[tuple[str, int, int]] = []
    for match in TYPE_RE.finditer(masked):
        declarations.append(type_declaration_range(masked, match.start(1), match.end(1)))

    for group_match in re.finditer(r"\btype\s*\(", masked):
        close = matching_delimiter(masked, group_match.end() - 1, "(", ")")
        if close is None:
            continue
        declarations.extend(grouped_type_declaration_ranges(masked, group_match.end(), close))

    unique: dict[tuple[str, int], tuple[str, int, int]] = {}
    for name, start, end in declarations:
        unique[(name, start)] = (name, start, end)
    return sorted(unique.values(), key=lambda item: (item[1], item[0]))


def type_declaration_range(
    masked: str,
    name_start: int,
    name_end: int,
    declaration_end: int | None = None,
) -> tuple[str, int, int]:
    name = masked[name_start:name_end]
    line_end = find_line_end(masked, name_end)
    end_limit = line_end if declaration_end is None else min(line_end, declaration_end)
    open_brace = masked.find("{", name_end, end_limit)
    if open_brace != -1:
        close_brace = matching_brace(masked, open_brace)
        end = close_brace + 1 if close_brace is not None else end_limit
    else:
        end = end_limit
    return name, name_start, end


def grouped_type_declaration_ranges(
    masked: str,
    start: int,
    end: int,
) -> list[tuple[str, int, int]]:
    declarations: list[tuple[str, int, int]] = []
    cursor = start
    while cursor < end:
        while cursor < end and (masked[cursor].isspace() or masked[cursor] == ";"):
            cursor += 1
        name_match = re.match(rf"{IDENTIFIER}\b", masked[cursor:end])
        if name_match is None:
            cursor += 1
            continue

        name_start = cursor
        name_end = cursor + name_match.end()
        cursor = name_end
        depths = {"(": 0, "[": 0, "{": 0}
        closing = {")": "(", "]": "[", "}": "{"}
        while cursor < end:
            char = masked[cursor]
            if char in depths:
                depths[char] += 1
            elif char in closing:
                opener = closing[char]
                if depths[opener] > 0:
                    depths[opener] -= 1
            elif char in {";", "\n"} and all(depth == 0 for depth in depths.values()):
                break
            cursor += 1
        declarations.append(
            type_declaration_range(masked, name_start, name_end, cursor)
        )
        cursor += 1
    return declarations


def alias_for_import(spec: ImportSpec, package_name: str | None) -> str | None:
    if spec.alias in {".", "_"}:
        return None
    if spec.alias:
        return spec.alias
    if package_name:
        return package_name
    return spec.import_path.rsplit("/", 1)[-1]


def collect_package_symbols(
    root: Path,
    files: list[Path],
) -> dict[str, tuple[str, str]]:
    packages: dict[str, tuple[str, str]] = {}
    for rel in files:
        source = (root / rel).read_text(encoding="utf-8")
        masked = mask_non_code(source)
        package_name, match = parse_package(masked)
        if package_name is None or match is None:
            continue
        package_dir = package_dir_for_file(rel)
        if package_dir in packages:
            continue
        offsets = char_byte_offsets(source)
        starts = line_index(source)
        rel_path = rel.as_posix()
        symbol = symbol_for_range(
            rel_path,
            offsets,
            starts,
            "package",
            package_name,
            package_dir or package_name,
            match.start(),
            match.end(1),
        )
        packages[package_dir] = (package_name, symbol.external_symbol)
    return packages


def collect_file_symbols(
    root: Path,
    rel: Path,
    import_path_map: dict[str, str],
    packages: dict[str, tuple[str, str]],
) -> tuple[list[LocalSymbol], list[ImportSpec]]:
    source = (root / rel).read_text(encoding="utf-8")
    masked = mask_non_code(source)
    package_name, package_match = parse_package(masked)
    package_dir = package_dir_for_file(rel)
    if package_name is None or package_match is None:
        return [], []

    imports = parse_imports(source, masked, import_path_map)
    import_aliases: dict[str, str] = {}
    for spec in imports:
        if spec.target_package_dir is None:
            continue
        target_package_name = packages.get(spec.target_package_dir, (None, ""))[0]
        alias = alias_for_import(spec, target_package_name)
        if alias is not None:
            import_aliases[alias] = spec.target_package_dir

    offsets = char_byte_offsets(source)
    starts = line_index(source)
    rel_path = rel.as_posix()
    symbols: list[LocalSymbol] = []

    package_symbol = symbol_for_range(
        rel_path,
        offsets,
        starts,
        "package",
        package_name,
        package_dir or package_name,
        package_match.start(),
        package_match.end(1),
    )
    if packages.get(package_dir, (None, None))[1] == package_symbol.external_symbol:
        symbols.append(
            LocalSymbol(package_symbol, package_name, package_dir, None, None)
        )

    for name, start, end in type_declaration_ranges(masked):
        symbol = symbol_for_range(
            rel_path,
            offsets,
            starts,
            "type",
            name,
            f"{package_dir}:{name}",
            start,
            end,
        )
        symbols.append(LocalSymbol(symbol, package_name, package_dir, None, None))

    for match in FUNC_RE.finditer(masked):
        name = match.group("name")
        end, body_start, body_end = find_function_end(masked, match.end("name"))
        receiver = match.group("recv")
        if receiver is not None:
            receiver_name = normalize_type_name(receiver)
            if receiver_name is None:
                continue
            kind = "method"
            display_name = f"{receiver_name}.{name}"
            qualified = f"{package_dir}:{receiver_name}.{name}"
        else:
            kind = "function"
            display_name = name
            qualified = f"{package_dir}:{name}"
        signature = masked[match.start() : body_start - 1 if body_start else end]
        symbol = symbol_for_range(
            rel_path,
            offsets,
            starts,
            kind,
            display_name,
            qualified,
            match.start(),
            end,
        )
        return_type = (
            None
            if receiver is not None
            else parse_return_type(package_dir, signature, import_aliases)
        )
        param_names = parse_param_names(signature)
        symbols.append(
            LocalSymbol(
                symbol,
                package_name,
                package_dir,
                body_start,
                body_end,
                return_type,
                param_names,
                match.group("recv_name"),
            )
        )

    return sorted(
        symbols,
        key=lambda item: (
            item.symbol.file_path or "",
            item.symbol.start_byte if item.symbol.start_byte is not None else -1,
            item.symbol.display_name,
        ),
    ), imports


def call_expression_receiver_type(
    expression: str,
    import_aliases: dict[str, str],
    free_functions_by_package_and_name: dict[tuple[str, str], LocalSymbol],
) -> ReceiverType | None:
    stripped = expression.strip()
    selector = re.fullmatch(rf"({IDENTIFIER})\s*\.\s*({IDENTIFIER})\s*\(\s*\)", stripped)
    if selector is not None:
        package_dir = import_aliases.get(selector.group(1))
        if package_dir is None:
            return None
        target = free_functions_by_package_and_name.get((package_dir, selector.group(2)))
        return target.return_type if target is not None else None

    direct = re.fullmatch(rf"({IDENTIFIER})\s*\(\s*\)", stripped)
    if direct is not None:
        candidates = [
            local
            for (package_dir, name), local in free_functions_by_package_and_name.items()
            if name == direct.group(1)
        ]
        if len(candidates) == 1:
            return candidates[0].return_type
    return None


def explicit_type_receiver(
    type_text: str | None,
    current_package_dir: str,
    import_aliases: dict[str, str],
) -> ReceiverType | None:
    if not type_text:
        return None
    type_text = type_text.strip()
    package_match = re.search(rf"({IDENTIFIER})\s*\.\s*({IDENTIFIER})", type_text)
    if package_match is not None:
        package_dir = import_aliases.get(package_match.group(1))
        if package_dir is None:
            return None
        return ReceiverType(package_dir, package_match.group(2))
    type_name = normalize_type_name(type_text)
    if type_name is None:
        return None
    return ReceiverType(current_package_dir, type_name)


def collect_binding_events(
    body: str,
    current_package_dir: str,
    import_aliases: dict[str, str],
    free_functions_by_package_and_name: dict[tuple[str, str], LocalSymbol],
) -> list[BindingEvent]:
    events: list[BindingEvent] = []
    for match in SHORT_DECL_RE.finditer(body):
        line_end = find_line_end(body, match.end())
        expression = body[match.end() : line_end]
        events.append(
            BindingEvent(
                line_end,
                match.group(1),
                call_expression_receiver_type(
                    expression,
                    import_aliases,
                    free_functions_by_package_and_name,
                ),
                True,
            )
        )

    for match in VAR_DECL_RE.finditer(body):
        line_end = find_line_end(body, match.end())
        type_text = match.group(2)
        events.append(
            BindingEvent(
                line_end,
                match.group(1),
                explicit_type_receiver(type_text, current_package_dir, import_aliases),
                True,
            )
        )

    for match in ASSIGNMENT_RE.finditer(body):
        line_end = find_line_end(body, match.end())
        expression = body[match.end() : line_end]
        events.append(
            BindingEvent(
                line_end,
                match.group(1),
                call_expression_receiver_type(
                    expression,
                    import_aliases,
                    free_functions_by_package_and_name,
                ),
                False,
            )
        )

    return sorted(events, key=lambda item: (item.offset, item.name))


def collect_file_references(
    root: Path,
    rel: Path,
    local_symbols: list[LocalSymbol],
    imports: list[ImportSpec],
    packages: dict[str, tuple[str, str]],
    free_functions_by_package_and_name: dict[tuple[str, str], LocalSymbol],
    methods_by_type_and_name: dict[tuple[str, str, str], str],
) -> list[Reference]:
    source = (root / rel).read_text(encoding="utf-8")
    masked = mask_non_code(source)
    offsets = char_byte_offsets(source)
    starts = line_index(source)
    rel_path = rel.as_posix()
    package_dir = package_dir_for_file(rel)
    current_package_external = packages.get(package_dir, (None, None))[1]
    import_aliases: dict[str, str] = {}
    for spec in imports:
        if spec.target_package_dir is None:
            continue
        target_package_name = packages.get(spec.target_package_dir, (None, ""))[0]
        alias = alias_for_import(spec, target_package_name)
        if alias is not None:
            import_aliases[alias] = spec.target_package_dir

    references: list[Reference] = []
    for spec in imports:
        if spec.target_package_dir is None:
            continue
        to_external = packages.get(spec.target_package_dir, (None, None))[1]
        if to_external is None:
            continue
        references.append(
            reference_for_range(
                "import",
                rel_path,
                offsets,
                starts,
                current_package_external,
                to_external,
                spec.start,
                spec.end,
            )
        )

    for local in local_symbols:
        if local.body_start is None or local.body_end is None:
            continue
        from_external = local.symbol.external_symbol
        body = masked[local.body_start : local.body_end]
        receiver_types: dict[str, ReceiverType] = {}
        shadowed_names = set(local.param_names)
        if local.symbol.kind == "method" and "." in local.symbol.display_name:
            receiver_type = local.symbol.display_name.rsplit(".", 1)[0]
            receiver_value = ReceiverType(local.package_dir, receiver_type)
            receiver_types["self"] = receiver_value
            if local.receiver_name is not None:
                receiver_types[local.receiver_name] = receiver_value

        events: list[tuple[int, int, str, object]] = []
        for binding in collect_binding_events(
            body,
            local.package_dir,
            import_aliases,
            free_functions_by_package_and_name,
        ):
            events.append((binding.offset, 0, "binding", binding))
        for match in SELECTOR_CALL_RE.finditer(body):
            events.append((match.start(), 1, "selector", match))
        for match in DIRECT_CALL_RE.finditer(body):
            events.append((match.start(1), 1, "direct", match))

        for _, _, event_kind, event in sorted(events, key=lambda item: (item[0], item[1])):
            if event_kind == "binding":
                binding = event
                assert isinstance(binding, BindingEvent)
                if binding.introduces_name:
                    shadowed_names.add(binding.name)
                if binding.receiver_type is None:
                    receiver_types.pop(binding.name, None)
                else:
                    receiver_types[binding.name] = binding.receiver_type
                continue

            if event_kind == "selector":
                match = event
                assert isinstance(match, re.Match)
                receiver_name, name = match.groups()
                if receiver_name in shadowed_names and receiver_name in import_aliases:
                    continue
                target: str | None = None
                package_dir_for_alias = import_aliases.get(receiver_name)
                if package_dir_for_alias is not None:
                    function = free_functions_by_package_and_name.get(
                        (package_dir_for_alias, name)
                    )
                    target = function.symbol.external_symbol if function else None
                else:
                    receiver_type = receiver_types.get(receiver_name)
                    if receiver_type is not None:
                        target = methods_by_type_and_name.get(
                            (
                                receiver_type.package_dir,
                                receiver_type.type_name,
                                name,
                            )
                        )
                if target is None:
                    continue
                start = local.body_start + match.start(2)
                end = local.body_start + match.end(2)
                references.append(
                    reference_for_range(
                        "call",
                        rel_path,
                        offsets,
                        starts,
                        from_external,
                        target,
                        start,
                        end,
                    )
                )
                continue

            match = event
            assert isinstance(match, re.Match)
            name = match.group(1)
            if name in RESERVED_CALLS or name in shadowed_names:
                continue
            target_local = free_functions_by_package_and_name.get((local.package_dir, name))
            if target_local is None:
                continue
            start = local.body_start + match.start(1)
            end = local.body_start + match.end(1)
            references.append(
                reference_for_range(
                    "call",
                    rel_path,
                    offsets,
                    starts,
                    from_external,
                    target_local.symbol.external_symbol,
                    start,
                    end,
                )
            )

    return references


def collect(root: Path) -> Artifact:
    files = discover_go_files(root)
    module_path = read_module_path(root)
    packages = collect_package_symbols(root, files)
    import_path_map = build_import_path_map(module_path, packages)

    symbols_by_file: dict[Path, list[LocalSymbol]] = {}
    imports_by_file: dict[Path, list[ImportSpec]] = {}
    symbols: list[Symbol] = []
    for rel in files:
        local_symbols, imports = collect_file_symbols(root, rel, import_path_map, packages)
        symbols_by_file[rel] = local_symbols
        imports_by_file[rel] = imports
        symbols.extend(item.symbol for item in local_symbols)

    free_function_candidates: dict[tuple[str, str], list[LocalSymbol]] = {}
    method_candidates: dict[tuple[str, str, str], list[LocalSymbol]] = {}
    for local_symbols in symbols_by_file.values():
        for local in local_symbols:
            symbol = local.symbol
            if symbol.kind == "function":
                free_function_candidates.setdefault(
                    (local.package_dir, symbol.display_name), []
                ).append(local)
            elif symbol.kind == "method" and "." in symbol.display_name:
                type_name, method_name = symbol.display_name.rsplit(".", 1)
                method_candidates.setdefault(
                    (local.package_dir, type_name, method_name), []
                ).append(local)

    free_functions_by_package_and_name = {
        key: candidates[0]
        for key, candidates in free_function_candidates.items()
        if len(candidates) == 1
    }
    methods_by_type_and_name = {
        key: candidates[0].symbol.external_symbol
        for key, candidates in method_candidates.items()
        if len(candidates) == 1
    }

    references: list[Reference] = []
    for rel in files:
        references.extend(
            collect_file_references(
                root,
                rel,
                symbols_by_file[rel],
                imports_by_file[rel],
                packages,
                free_functions_by_package_and_name,
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
    if not has_go_project_or_files(root):
        print("no Go project or source files found", file=sys.stderr)
        return 69

    try:
        write_artifact(Path(args.output), collect(root))
    except OSError as exc:
        print(f"failed to write output: {exc}", file=sys.stderr)
        return 64
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

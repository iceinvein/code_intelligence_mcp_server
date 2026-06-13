#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
from pathlib import Path
import sys


BUNDLE_ROOT = Path(__file__).resolve().parents[2]
if str(BUNDLE_ROOT) not in sys.path:
    sys.path.insert(0, str(BUNDLE_ROOT))

from producers.lib.normalized import (  # noqa: E402
    Artifact,
    Reference,
    Symbol,
    discover_files,
    stable_symbol_id,
    write_artifact,
)


PRODUCER = "code-intelligence-external-python"


def module_name_for_path(path: Path) -> str:
    parts = list(path.with_suffix("").parts)
    if parts and parts[0] == "src":
        parts = parts[1:]
    if parts and parts[-1] == "__init__":
        parts = parts[:-1]
    return ".".join(parts)


def display_name(scope: list[str], name: str) -> str:
    return ".".join([*scope, name]) if scope else name


def stable_python_id(
    rel_path: str,
    kind: str,
    qualified: str,
    node: ast.AST,
) -> str:
    return stable_symbol_id(
        "python",
        rel_path,
        kind,
        qualified,
        getattr(node, "lineno", None),
        getattr(node, "col_offset", None),
    )


def dotted_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        parent = dotted_name(node.value)
        if parent is not None:
            return f"{parent}.{node.attr}"
    return None


def import_module_name(current_module: str, rel_path: Path, node: ast.ImportFrom) -> str:
    if node.level == 0:
        return node.module or ""

    if rel_path.name == "__init__.py":
        package_parts = current_module.split(".")
    else:
        package_parts = current_module.split(".")[:-1]

    if node.level > 1:
        package_parts = package_parts[: -(node.level - 1)]

    parts = [*package_parts]
    if node.module:
        parts.extend(node.module.split("."))
    return ".".join(part for part in parts if part)


def register_definitions(
    tree: ast.Module,
    module_name: str,
    rel_path: str,
    known: dict[str, str],
    qualified_by_symbol: dict[str, str],
    class_qualified: set[str],
) -> None:
    def visit_body(body: list[ast.stmt], scope: list[str], class_depth: int) -> None:
        for node in body:
            if isinstance(node, ast.ClassDef):
                qualified = ".".join([module_name, *scope, node.name])
                display = display_name(scope, node.name)
                external = stable_python_id(rel_path, "class", qualified, node)
                known[qualified] = external
                known[display] = external
                known[node.name] = external
                qualified_by_symbol[external] = qualified
                class_qualified.add(qualified)
                visit_body(node.body, [*scope, node.name], class_depth + 1)
            elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                qualified = ".".join([module_name, *scope, node.name])
                display = display_name(scope, node.name)
                kind = "method" if class_depth else "function"
                external = stable_python_id(rel_path, kind, qualified, node)
                known[qualified] = external
                known[display] = external
                known[node.name] = external
                qualified_by_symbol[external] = qualified
                visit_body(node.body, [*scope, node.name], class_depth)

    visit_body(tree.body, [], 0)


def register_module(
    module_name: str,
    rel_path: str,
    known: dict[str, str],
    qualified_by_symbol: dict[str, str],
) -> None:
    external = stable_symbol_id("python", rel_path, "module", module_name, 1, 0)
    known[module_name] = external
    qualified_by_symbol[external] = module_name


def infer_function_returns(
    tree: ast.Module,
    module_name: str,
    known: dict[str, str],
    class_qualified: set[str],
) -> dict[str, str]:
    returns: dict[str, str] = {}

    def visit_body(body: list[ast.stmt], scope: list[str], class_depth: int) -> None:
        for node in body:
            if isinstance(node, ast.ClassDef):
                visit_body(node.body, [*scope, node.name], class_depth + 1)
                continue
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue

            qualified = ".".join([module_name, *scope, node.name])
            for child in ast.walk(node):
                if (
                    isinstance(child, ast.Return)
                    and isinstance(child.value, ast.Call)
                    and isinstance(child.value.func, ast.Name)
                ):
                    class_name = child.value.func.id
                    candidates = [f"{module_name}.{class_name}"]
                    for candidate in candidates:
                        target = known.get(candidate)
                        if target is not None:
                            target_qualified = next(
                                (
                                    item
                                    for item in class_qualified
                                    if known.get(item) == target
                                ),
                                None,
                            )
                            if target_qualified is not None:
                                returns[qualified] = target_qualified
                                break
                if qualified in returns:
                    break
            visit_body(node.body, [*scope, node.name], class_depth)

    visit_body(tree.body, [], 0)
    return returns


class PythonCollector(ast.NodeVisitor):
    def __init__(
        self,
        rel_path: str,
        module_name: str,
        source: str,
        known: dict[str, str],
        qualified_by_symbol: dict[str, str],
        class_qualified: set[str],
        function_returns: dict[str, str],
    ):
        self.rel_path = rel_path
        self.module_name = module_name
        self.source = source
        self.known = known
        self.qualified_by_symbol = qualified_by_symbol
        self.class_qualified = class_qualified
        self.function_returns = function_returns
        self.symbols: list[Symbol] = []
        self.references: list[Reference] = []
        self.scope: list[str] = []
        self.class_stack: list[str] = []
        self.class_depth = 0
        self.import_scopes: list[dict[str, str]] = [{}]
        self.assigned_type_scopes: list[dict[str, str]] = [{}]
        self.enclosing_symbols: list[str] = []

    def symbol_id(self, kind: str, qualified: str, node: ast.AST) -> str:
        return stable_python_id(self.rel_path, kind, qualified, node)

    def add_symbol(
        self,
        kind: str,
        display: str,
        qualified: str,
        node: ast.AST,
    ) -> str:
        start_line = getattr(node, "lineno", None)
        end_line = getattr(node, "end_lineno", start_line)
        start_byte = getattr(node, "col_offset", None)
        end_byte = getattr(node, "end_col_offset", start_byte)
        external = self.symbol_id(kind, qualified, node)
        self.symbols.append(
            Symbol(
                external,
                display,
                kind,
                self.rel_path,
                start_line,
                end_line,
                start_byte,
                end_byte,
            )
        )
        self.known[qualified] = external
        self.known[display] = external
        self.known[display.rsplit(".", 1)[-1]] = external
        self.qualified_by_symbol[external] = qualified
        if kind == "class":
            self.class_qualified.add(qualified)
        return external

    def add_reference(
        self,
        relationship: str,
        target: str | None,
        node: ast.AST,
        confidence: float = 0.9,
    ) -> None:
        if target is None:
            return
        self.references.append(
            Reference(
                self.current_from_symbol(),
                target,
                relationship,
                self.rel_path,
                getattr(node, "lineno", 1),
                getattr(node, "col_offset", None),
                getattr(node, "end_lineno", None),
                getattr(node, "end_col_offset", None),
                confidence,
                "python_ast",
            )
        )

    def current_from_symbol(self) -> str | None:
        if self.enclosing_symbols:
            return self.enclosing_symbols[-1]
        return self.known.get(self.module_name)

    def bind_import(self, local: str, qualified: str) -> None:
        self.import_scopes[-1][local] = qualified

    def lookup_import(self, local: str) -> str | None:
        for scope in reversed(self.import_scopes):
            if local in scope:
                return scope[local]
        return None

    def bind_assigned_type(self, local: str, qualified_class: str) -> None:
        self.assigned_type_scopes[-1][local] = qualified_class

    def lookup_assigned_type(self, local: str) -> str | None:
        for scope in reversed(self.assigned_type_scopes):
            if local in scope:
                return scope[local]
        return None

    def visit_Module(self, node: ast.Module) -> None:
        module_symbol = stable_symbol_id(
            "python",
            self.rel_path,
            "module",
            self.module_name,
            1,
            0,
        )
        self.symbols.append(
            Symbol(module_symbol, self.module_name, "module", self.rel_path, 1, 1, 0, 0)
        )
        self.known[self.module_name] = module_symbol
        self.qualified_by_symbol[module_symbol] = self.module_name
        self.enclosing_symbols.append(module_symbol)
        self.generic_visit(node)
        self.enclosing_symbols.pop()

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            if alias.name in self.known:
                local = alias.asname or alias.name
                self.bind_import(local, alias.name)
                if alias.asname:
                    self.bind_import(alias.asname, alias.name)
                else:
                    root = alias.name.split(".", 1)[0]
                    if root in self.known:
                        self.bind_import(root, root)
                self.add_reference("imports", self.known[alias.name], node)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        module = import_module_name(self.module_name, Path(self.rel_path), node)
        for alias in node.names:
            qualified = f"{module}.{alias.name}" if module else alias.name
            if qualified in self.known:
                local = alias.asname or alias.name
                self.bind_import(local, qualified)
                self.add_reference("imports", self.known[qualified], node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        qualified = ".".join([self.module_name, *self.scope, node.name])
        display = display_name(self.scope, node.name)
        external = self.add_symbol("class", display, qualified, node)
        self.scope.append(node.name)
        self.class_stack.append(node.name)
        self.class_depth += 1
        self.enclosing_symbols.append(external)
        self.generic_visit(node)
        self.enclosing_symbols.pop()
        self.class_depth -= 1
        self.class_stack.pop()
        self.scope.pop()

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._visit_function(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self._visit_function(node)

    def _visit_function(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> None:
        qualified = ".".join([self.module_name, *self.scope, node.name])
        display = display_name(self.scope, node.name)
        kind = "method" if self.class_depth else "function"
        external = self.add_symbol(kind, display, qualified, node)
        self.scope.append(node.name)
        self.import_scopes.append({})
        self.assigned_type_scopes.append({})
        self.enclosing_symbols.append(external)
        self.generic_visit(node)
        self.enclosing_symbols.pop()
        self.assigned_type_scopes.pop()
        self.import_scopes.pop()
        self.scope.pop()

    def visit_Assign(self, node: ast.Assign) -> None:
        if isinstance(node.value, ast.Call):
            class_name = self.resolve_call_return_type(node.value)
            if class_name is not None:
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        self.bind_assigned_type(target.id, class_name)
        self.generic_visit(node)

    def visit_Call(self, node: ast.Call) -> None:
        self.add_reference("calls", self.resolve_call(node), node, 0.85)
        self.generic_visit(node)

    def resolve_call_key(self, node: ast.Call) -> str | None:
        func = node.func
        if isinstance(func, ast.Name):
            return (
                self.lookup_import(func.id)
                or (
                    f"{self.module_name}.{func.id}"
                    if f"{self.module_name}.{func.id}" in self.known
                    else None
                )
            )
        if isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name):
            owner = self.lookup_assigned_type(func.value.id)
            if owner is None and func.value.id in {"self", "cls"} and self.class_stack:
                owner = ".".join([self.module_name, *self.class_stack])
            if owner:
                key = f"{owner}.{func.attr}"
                if key in self.known:
                    return key
        if isinstance(func, ast.Attribute):
            resolved = self.resolve_dotted_call(dotted_name(func))
            if resolved is not None:
                return resolved
        return None

    def resolve_dotted_call(self, name: str | None) -> str | None:
        if name is None:
            return None
        parts = name.split(".")
        for index in range(len(parts) - 1, 0, -1):
            prefix = ".".join(parts[:index])
            imported = self.lookup_import(prefix)
            if imported is None:
                continue
            candidate = ".".join([imported, *parts[index:]])
            if candidate in self.known:
                return candidate
        return None

    def resolve_call(self, node: ast.Call) -> str | None:
        key = self.resolve_call_key(node)
        if key is None:
            return None
        return self.known.get(key)

    def resolve_call_return_type(self, node: ast.Call) -> str | None:
        key = self.resolve_call_key(node)
        if key is None:
            return None
        if key in self.class_qualified:
            return key
        return self.function_returns.get(key)


def collect(root: Path) -> Artifact:
    files = discover_files(root, {".py"})
    known: dict[str, str] = {}
    qualified_by_symbol: dict[str, str] = {}
    class_qualified: set[str] = set()
    modules: list[tuple[Path, str, ast.Module]] = []

    for rel in files:
        source = (root / rel).read_text(encoding="utf-8")
        modules.append((rel, source, ast.parse(source, filename=rel.as_posix())))

    for rel, _source, _tree in modules:
        register_module(
            module_name_for_path(rel),
            rel.as_posix(),
            known,
            qualified_by_symbol,
        )

    for rel, _source, tree in modules:
        register_definitions(
            tree,
            module_name_for_path(rel),
            rel.as_posix(),
            known,
            qualified_by_symbol,
            class_qualified,
        )

    function_returns: dict[str, str] = {}
    for rel, _source, tree in modules:
        function_returns.update(
            infer_function_returns(tree, module_name_for_path(rel), known, class_qualified)
        )

    symbols: list[Symbol] = []
    references: list[Reference] = []
    for rel, source, tree in modules:
        collector = PythonCollector(
            rel.as_posix(),
            module_name_for_path(rel),
            source,
            known,
            qualified_by_symbol,
            class_qualified,
            function_returns,
        )
        collector.visit(tree)
        symbols.extend(collector.symbols)
        references.extend(collector.references)

    return Artifact("python_ast", PRODUCER, "python", str(root), symbols, references)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog=PRODUCER)
    parser.add_argument("command", choices=["index"])
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)

    root = Path.cwd()
    if not discover_files(root, {".py"}):
        print("no Python files found", file=sys.stderr)
        return 69

    write_artifact(Path(args.output), collect(root))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

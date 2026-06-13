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
    line_index,
    stable_symbol_id,
    write_artifact,
)


PRODUCER = "code-intelligence-external-python"
COMMON_SOURCE_ROOTS = {"lib", "src"}


def module_name_for_path(path: Path, source_roots: set[str] | None = None) -> str:
    parts = list(path.with_suffix("").parts)
    if parts and source_roots and parts[0] in source_roots:
        parts = parts[1:]
    if parts and parts[-1] == "__init__":
        parts = parts[:-1]
    return ".".join(parts)


def detect_source_roots(root: Path, files: list[Path]) -> set[str]:
    roots: set[str] = set()
    project_markers = ("pyproject.toml", "setup.cfg", "setup.py")
    has_project_marker = any((root / marker).exists() for marker in project_markers)
    for candidate in COMMON_SOURCE_ROOTS:
        has_python_files = any(
            len(rel.parts) > 1 and rel.parts[0] == candidate for rel in files
        )
        if has_python_files and (candidate == "src" or has_project_marker):
            roots.add(candidate)
    return roots


def display_name(scope: list[str], name: str) -> str:
    return ".".join([*scope, name]) if scope else name


def absolute_byte(
    starts: list[int],
    line: int | None,
    column: int | None,
) -> int | None:
    if line is None or column is None:
        return None
    if line <= 0 or line > len(starts):
        return None
    return starts[line - 1] + column


def one_based_column(column: int | None) -> int | None:
    if column is None:
        return None
    return column + 1


def stable_python_id(
    rel_path: str,
    kind: str,
    qualified: str,
    node: ast.AST,
    starts: list[int] | None = None,
) -> str:
    start_byte = getattr(node, "col_offset", None)
    if starts is not None:
        start_byte = absolute_byte(starts, getattr(node, "lineno", None), start_byte)
    return stable_symbol_id(
        "python",
        rel_path,
        kind,
        qualified,
        getattr(node, "lineno", None),
        start_byte,
    )


def dotted_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        parent = dotted_name(node.value)
        if parent is not None:
            return f"{parent}.{node.attr}"
    return None


def target_names(target: ast.AST) -> list[str]:
    if isinstance(target, ast.Name):
        return [target.id]
    if isinstance(target, (ast.Tuple, ast.List)):
        names: list[str] = []
        for item in target.elts:
            names.extend(target_names(item))
        return names
    if isinstance(target, ast.Starred):
        return target_names(target.value)
    return []


def argument_names(arguments: ast.arguments) -> list[str]:
    names: list[str] = []
    for arg in [
        *arguments.posonlyargs,
        *arguments.args,
        *arguments.kwonlyargs,
    ]:
        names.append(arg.arg)
    if arguments.vararg is not None:
        names.append(arguments.vararg.arg)
    if arguments.kwarg is not None:
        names.append(arguments.kwarg.arg)
    return names


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
    source: str,
    known: dict[str, str],
    qualified_by_symbol: dict[str, str],
    class_qualified: set[str],
) -> None:
    starts = line_index(source)

    def visit_body(body: list[ast.stmt], scope: list[str], class_depth: int) -> None:
        for node in body:
            if isinstance(node, ast.ClassDef):
                qualified = ".".join([module_name, *scope, node.name])
                display = display_name(scope, node.name)
                external = stable_python_id(rel_path, "class", qualified, node, starts)
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
                external = stable_python_id(rel_path, kind, qualified, node, starts)
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


def return_values_without_nested_defs(body: list[ast.stmt]):
    for statement in body:
        yield from return_values_in_statement(statement)


def return_values_in_statement(statement: ast.stmt):
    if isinstance(statement, ast.Return):
        yield statement.value
        return
    if isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
        return
    for child in ast.iter_child_nodes(statement):
        if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            continue
        if isinstance(child, ast.Return):
            yield child.value
        elif isinstance(child, ast.stmt):
            yield from return_values_in_statement(child)


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
            for value in return_values_without_nested_defs(node.body):
                if (
                    isinstance(value, ast.Call)
                    and isinstance(value.func, ast.Name)
                ):
                    class_name = value.func.id
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
        module_names: set[str],
        qualified_by_symbol: dict[str, str],
        class_qualified: set[str],
        function_returns: dict[str, str],
    ):
        self.rel_path = rel_path
        self.module_name = module_name
        self.source = source
        self.line_starts = line_index(source)
        self.known = known
        self.module_names = module_names
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
        self.local_binding_scopes: list[set[str]] = [set()]
        self.enclosing_symbols: list[str] = []

    def symbol_id(self, kind: str, qualified: str, node: ast.AST) -> str:
        return stable_python_id(self.rel_path, kind, qualified, node, self.line_starts)

    def add_symbol(
        self,
        kind: str,
        display: str,
        qualified: str,
        node: ast.AST,
    ) -> str:
        start_line = getattr(node, "lineno", None)
        end_line = getattr(node, "end_lineno", start_line)
        start_byte = absolute_byte(
            self.line_starts,
            start_line,
            getattr(node, "col_offset", None),
        )
        end_byte = absolute_byte(
            self.line_starts,
            end_line,
            getattr(node, "end_col_offset", None),
        )
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
                one_based_column(getattr(node, "col_offset", None)),
                getattr(node, "end_lineno", None),
                one_based_column(getattr(node, "end_col_offset", None)),
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
        for index in range(len(self.import_scopes) - 1, -1, -1):
            if local in self.import_scopes[index]:
                return self.import_scopes[index][local]
            if local in self.local_binding_scopes[index]:
                return None
        return None

    def bind_local(self, local: str) -> None:
        self.local_binding_scopes[-1].add(local)

    def bind_assigned_type(self, local: str, qualified_class: str) -> None:
        self.import_scopes[-1].pop(local, None)
        self.bind_local(local)
        self.assigned_type_scopes[-1][local] = qualified_class

    def clear_assigned_type(self, local: str) -> None:
        self.import_scopes[-1].pop(local, None)
        self.bind_local(local)
        self.assigned_type_scopes[-1].pop(local, None)

    def lookup_assigned_type(self, local: str) -> str | None:
        for index in range(len(self.assigned_type_scopes) - 1, -1, -1):
            if local in self.assigned_type_scopes[index]:
                return self.assigned_type_scopes[index][local]
            if local in self.local_binding_scopes[index]:
                return None
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
            local = alias.asname or alias.name
            if alias.name in self.module_names:
                self.bind_import(local, alias.name)
                if alias.asname:
                    self.bind_import(alias.asname, alias.name)
                else:
                    root = alias.name.split(".", 1)[0]
                    if root in self.module_names:
                        self.bind_import(root, root)
                self.add_reference("import", self.known[alias.name], node)
            else:
                self.bind_local(alias.asname or alias.name.split(".", 1)[0])

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        module = import_module_name(self.module_name, Path(self.rel_path), node)
        for alias in node.names:
            qualified = f"{module}.{alias.name}" if module else alias.name
            local = alias.asname or alias.name
            if qualified in self.known:
                self.bind_import(local, qualified)
                self.add_reference("import", self.known[qualified], node)
            else:
                self.bind_local(local)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        if self.scope:
            self.bind_local(node.name)
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
        if self.scope:
            self.bind_local(node.name)
        self._visit_function(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        if self.scope:
            self.bind_local(node.name)
        self._visit_function(node)

    def _visit_function(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> None:
        qualified = ".".join([self.module_name, *self.scope, node.name])
        display = display_name(self.scope, node.name)
        kind = "method" if self.class_depth else "function"
        external = self.add_symbol(kind, display, qualified, node)
        self.scope.append(node.name)
        self.import_scopes.append({})
        self.assigned_type_scopes.append({})
        self.local_binding_scopes.append(set(argument_names(node.args)))
        self.enclosing_symbols.append(external)
        self.generic_visit(node)
        self.enclosing_symbols.pop()
        self.local_binding_scopes.pop()
        self.assigned_type_scopes.pop()
        self.import_scopes.pop()
        self.scope.pop()

    def visit_Assign(self, node: ast.Assign) -> None:
        class_name = None
        if isinstance(node.value, ast.Call):
            class_name = self.resolve_call_return_type(node.value)
        for target in node.targets:
            for name in target_names(target):
                if class_name is not None:
                    self.bind_assigned_type(name, class_name)
                else:
                    self.clear_assigned_type(name)
        self.generic_visit(node)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        class_name = None
        if isinstance(node.value, ast.Call):
            class_name = self.resolve_call_return_type(node.value)
        for name in target_names(node.target):
            if class_name is not None:
                self.bind_assigned_type(name, class_name)
            else:
                self.clear_assigned_type(name)
        self.generic_visit(node)

    def visit_AugAssign(self, node: ast.AugAssign) -> None:
        for name in target_names(node.target):
            self.clear_assigned_type(name)
        self.generic_visit(node)

    def visit_For(self, node: ast.For) -> None:
        for name in target_names(node.target):
            self.clear_assigned_type(name)
        self.generic_visit(node)

    def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
        self.visit_For(node)

    def visit_With(self, node: ast.With) -> None:
        for item in node.items:
            if item.optional_vars is not None:
                for name in target_names(item.optional_vars):
                    self.clear_assigned_type(name)
        self.generic_visit(node)

    def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
        self.visit_With(node)

    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
        if node.name is not None:
            self.clear_assigned_type(node.name)
        self.generic_visit(node)

    def visit_Call(self, node: ast.Call) -> None:
        self.add_reference("call", self.resolve_call(node), node, 0.85)
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
    source_roots = detect_source_roots(root, files)
    known: dict[str, str] = {}
    module_names: set[str] = set()
    qualified_by_symbol: dict[str, str] = {}
    class_qualified: set[str] = set()
    modules: list[tuple[Path, str, ast.Module]] = []

    for rel in files:
        source = (root / rel).read_text(encoding="utf-8")
        modules.append((rel, source, ast.parse(source, filename=rel.as_posix())))

    for rel, _source, _tree in modules:
        module_names.add(module_name_for_path(rel, source_roots))
        register_module(
            module_name_for_path(rel, source_roots),
            rel.as_posix(),
            known,
            qualified_by_symbol,
        )

    for rel, source, tree in modules:
        register_definitions(
            tree,
            module_name_for_path(rel, source_roots),
            rel.as_posix(),
            source,
            known,
            qualified_by_symbol,
            class_qualified,
        )

    function_returns: dict[str, str] = {}
    for rel, _source, tree in modules:
        function_returns.update(
            infer_function_returns(
                tree,
                module_name_for_path(rel, source_roots),
                known,
                class_qualified,
            )
        )

    symbols: list[Symbol] = []
    references: list[Reference] = []
    for rel, source, tree in modules:
        collector = PythonCollector(
            rel.as_posix(),
            module_name_for_path(rel, source_roots),
            source,
            known,
            module_names,
            qualified_by_symbol,
            class_qualified,
            function_returns,
        )
        collector.visit(tree)
        symbols.extend(collector.symbols)
        references.extend(collector.references)

    return Artifact("python_ast", PRODUCER, "python", str(root), symbols, references)


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
    if not discover_files(root, {".py"}):
        print("no Python files found", file=sys.stderr)
        return 69

    write_artifact(Path(args.output), collect(root))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

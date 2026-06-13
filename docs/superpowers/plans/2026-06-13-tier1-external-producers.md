# Tier 1 External Producers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Tier 1 bundled external producer stubs with deterministic normalized artifact generators for TypeScript/JavaScript, Python, Rust, and Go, then prepare the benchmark external arm.

**Architecture:** Keep the Rust daemon/importer unchanged and implement producer behavior behind the existing `code-intelligence-external-<language> index --output <artifact>` wrapper contract. Add a small shared Python producer library for normalized artifact writing, deterministic file discovery, span math, and source-level symbol/reference helpers; use it for Python, Rust, and Go. Use a Node producer for TypeScript/JavaScript with a dependency-free source scanner first and project-local `typescript` package detection inside the same CLI boundary.

**Tech Stack:** Rust 2021 daemon, existing normalized external index JSON schema, Python 3 stdlib, Node.js stdlib, shell wrappers, `unittest`, `node:test`, Cargo integration tests, npm bundle validation, bench Python harness.

---

## File Structure

- Create `producers/lib/normalized.py`: shared artifact dataclasses, stable id helpers, repository file walking, UTF-8 path handling, line/column/span helpers, deterministic JSON writer.
- Create `producers/python/index.py`: Python AST producer.
- Create `producers/rust/index.py`: Rust source-level producer.
- Create `producers/go/index.py`: Go source-level producer with `go.mod`/`.go` discovery.
- Create `producers/typescript/index.js`: TypeScript/JavaScript source-level producer with optional project-local TypeScript module detection.
- Create `producers/tests/fixtures/{python,typescript,rust,go}/`: tiny fixture repositories.
- Create `producers/tests/test_normalized.py`, `test_python_producer.py`, `test_rust_producer.py`, `test_go_producer.py`, and `test_typescript_producer.js`: producer unit/contract/determinism tests.
- Modify `producers/bin/code-intelligence-external-{python,typescript,rust,go}`: invoke real producers while preserving usage exit `64` and supported-but-not-configured exit `69`.
- Modify `npm/bundle.js` and `npm/bundle.test.js`: validate manifest producer support files, not only executable wrappers.
- Modify `bench/arms.py` and `bench/tests/test_arms.py`: add the opt-in `code_intel_external` arm after TS/Python smoke is green.
- Add optional Rust integration fixture files under `tests/fixtures/external_index/generated-tier1/` only if Cargo tests need persisted generated artifacts.

---

### Task 1: Shared Normalized Artifact Library

**Files:**
- Create: `producers/lib/__init__.py`
- Create: `producers/lib/normalized.py`
- Create: `producers/tests/test_normalized.py`

- [ ] **Step 1: Write the failing shared-library tests**

Create `producers/tests/test_normalized.py` with these tests:

```python
import json
import tempfile
import unittest
from pathlib import Path

from producers.lib.normalized import (
    Artifact,
    Reference,
    Symbol,
    discover_files,
    line_index,
    position_for_offset,
    stable_symbol_id,
    write_artifact,
)


class NormalizedArtifactTests(unittest.TestCase):
    def test_stable_symbol_id_is_deterministic_and_path_scoped(self):
        left = stable_symbol_id("python", "pkg/app.py", "function", "pkg.app.run", 8, 21)
        right = stable_symbol_id("python", "pkg/app.py", "function", "pkg.app.run", 8, 21)
        other = stable_symbol_id("python", "pkg/other.py", "function", "pkg.app.run", 8, 21)
        self.assertEqual(left, right)
        self.assertNotEqual(left, other)
        self.assertEqual(left, "python:pkg/app.py:function:pkg.app.run:8:21")

    def test_line_index_and_position_for_offset_are_one_based(self):
        source = "alpha\\n  beta()\\n"
        starts = line_index(source)
        self.assertEqual(starts, [0, 6, 15])
        self.assertEqual(position_for_offset(starts, 8), (2, 3))

    def test_discover_files_skips_vendor_and_orders_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "src").mkdir()
            (root / "src" / "b.py").write_text("def b(): pass\\n", encoding="utf-8")
            (root / "src" / "a.py").write_text("def a(): pass\\n", encoding="utf-8")
            (root / ".git").mkdir()
            (root / ".git" / "ignored.py").write_text("def ignored(): pass\\n", encoding="utf-8")
            (root / "node_modules").mkdir()
            (root / "node_modules" / "ignored.py").write_text("def ignored(): pass\\n", encoding="utf-8")

            found = [path.as_posix() for path in discover_files(root, {".py"})]

        self.assertEqual(found, ["src/a.py", "src/b.py"])

    def test_write_artifact_sorts_symbols_and_references(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "artifact.json"
            artifact = Artifact(
                source_kind="python_ast",
                producer="code-intelligence-external-python",
                language="python",
                root_path=tmp,
                symbols=[
                    Symbol("python:b.py:function:b:1:1", "b", "function", "b.py", 1, 1, 0, 1),
                    Symbol("python:a.py:function:a:1:1", "a", "function", "a.py", 1, 1, 0, 1),
                ],
                references=[
                    Reference(None, "python:b.py:function:b:1:1", "calls", "a.py", 3, 4, 3, 5, 0.7, "python_ast"),
                    Reference(None, "python:a.py:function:a:1:1", "calls", "a.py", 2, 4, 2, 5, 0.7, "python_ast"),
                ],
            )

            write_artifact(output, artifact)
            payload = json.loads(output.read_text(encoding="utf-8"))

        self.assertEqual([item["display_name"] for item in payload["symbols"]], ["a", "b"])
        self.assertEqual([item["to_external_symbol"] for item in payload["references"]], [
            "python:a.py:function:a:1:1",
            "python:b.py:function:b:1:1",
        ])


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run the shared-library tests to verify they fail**

Run:

```bash
python3 -m unittest producers.tests.test_normalized
```

Expected: fail with `ModuleNotFoundError` for `producers.lib.normalized`.

- [ ] **Step 3: Add the shared normalized artifact implementation**

Create `producers/lib/__init__.py` as an empty package marker.

Create `producers/lib/normalized.py` with these public shapes and behavior:

```python
from __future__ import annotations

from dataclasses import asdict, dataclass
import json
from pathlib import Path
from typing import Iterable


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


def stable_symbol_id(language: str, file_path: str, kind: str, qualified_name: str, start_line: int | None, start_byte: int | None) -> str:
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
```

- [ ] **Step 4: Run shared-library tests to verify they pass**

Run:

```bash
python3 -m unittest producers.tests.test_normalized
```

Expected: `OK`.

- [ ] **Step 5: Commit shared producer support**

Run:

```bash
git add producers/lib/__init__.py producers/lib/normalized.py producers/tests/test_normalized.py
git commit -m "feat(producers): add normalized artifact support"
```

---

### Task 2: Python Producer for Django

**Files:**
- Create: `producers/python/index.py`
- Create: `producers/tests/fixtures/python/pyproject.toml`
- Create: `producers/tests/fixtures/python/pkg/__init__.py`
- Create: `producers/tests/fixtures/python/pkg/services.py`
- Create: `producers/tests/fixtures/python/pkg/views.py`
- Create: `producers/tests/test_python_producer.py`
- Modify: `producers/bin/code-intelligence-external-python`

- [ ] **Step 1: Add the Python fixture and failing producer tests**

Create fixture files:

```toml
# producers/tests/fixtures/python/pyproject.toml
[project]
name = "producer-python-fixture"
```

```python
# producers/tests/fixtures/python/pkg/__init__.py
from .services import UserService
```

```python
# producers/tests/fixtures/python/pkg/services.py
class UserService:
    def load(self, user_id):
        return {"id": user_id}


def make_service():
    return UserService()
```

```python
# producers/tests/fixtures/python/pkg/views.py
from pkg.services import UserService, make_service


def render_user(user_id):
    service = make_service()
    return service.load(user_id)
```

Create `producers/tests/test_python_producer.py`:

```python
import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "producers" / "tests" / "fixtures" / "python"
PRODUCER = ROOT / "producers" / "python" / "index.py"


class PythonProducerTests(unittest.TestCase):
    def run_producer(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "python-normalized.json"
            result = subprocess.run(
                ["python3", str(PRODUCER), "index", "--output", str(output)],
                cwd=FIXTURE,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(output.read_text(encoding="utf-8"))

    def test_emits_symbols_imports_and_calls(self):
        payload = self.run_producer()
        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertEqual(payload["source_kind"], "python_ast")
        self.assertEqual(payload["language"], "python")
        self.assertIn("UserService", symbols)
        self.assertIn("UserService.load", symbols)
        self.assertIn("make_service", symbols)
        self.assertIn("render_user", symbols)
        relationships = {(item["relationship"], item["to_external_symbol"]) for item in payload["references"]}
        target_ids = {item["display_name"]: item["external_symbol"] for item in payload["symbols"]}
        self.assertIn(("imports", target_ids["make_service"]), relationships)
        self.assertIn(("calls", target_ids["make_service"]), relationships)
        self.assertIn(("calls", target_ids["UserService.load"]), relationships)

    def test_output_is_deterministic(self):
        self.assertEqual(self.run_producer(), self.run_producer())


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run Python producer tests to verify they fail**

Run:

```bash
python3 -m unittest producers.tests.test_python_producer
```

Expected: fail because `producers/python/index.py` does not exist.

- [ ] **Step 3: Implement the Python AST producer**

Create `producers/python/index.py` with this behavior:

```python
#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
from pathlib import Path
import sys

from producers.lib.normalized import Artifact, Reference, Symbol, discover_files, stable_symbol_id, write_artifact


PRODUCER = "code-intelligence-external-python"


def module_name_for_path(path: Path) -> str:
    parts = list(path.with_suffix("").parts)
    if parts[-1] == "__init__":
        parts = parts[:-1]
    return ".".join(parts)


class PythonCollector(ast.NodeVisitor):
    def __init__(self, rel_path: str, module_name: str, source: str, known: dict[str, str]):
        self.rel_path = rel_path
        self.module_name = module_name
        self.source = source
        self.known = known
        self.symbols: list[Symbol] = []
        self.references: list[Reference] = []
        self.scope: list[str] = []
        self.imported: dict[str, str] = {}
        self.assigned_types: dict[str, str] = {}

    def symbol_id(self, kind: str, qualified: str, node: ast.AST) -> str:
        return stable_symbol_id("python", self.rel_path, kind, qualified, getattr(node, "lineno", None), getattr(node, "col_offset", None))

    def add_symbol(self, kind: str, display: str, qualified: str, node: ast.AST) -> str:
        start_line = getattr(node, "lineno", None)
        end_line = getattr(node, "end_lineno", start_line)
        start_byte = getattr(node, "col_offset", None)
        end_byte = getattr(node, "end_col_offset", start_byte)
        external = self.symbol_id(kind, qualified, node)
        self.symbols.append(Symbol(external, display, kind, self.rel_path, start_line, end_line, start_byte, end_byte))
        self.known[qualified] = external
        self.known[display] = external
        return external

    def add_reference(self, relationship: str, target: str | None, node: ast.AST, confidence: float = 0.9) -> None:
        if target is None:
            return
        self.references.append(Reference(None, target, relationship, self.rel_path, getattr(node, "lineno", 1), getattr(node, "col_offset", None), getattr(node, "end_lineno", None), getattr(node, "end_col_offset", None), confidence, "python_ast"))

    def visit_Module(self, node: ast.Module):
        module_symbol = stable_symbol_id("python", self.rel_path, "module", self.module_name, 1, 0)
        self.symbols.append(Symbol(module_symbol, self.module_name, "module", self.rel_path, 1, 1, 0, 0))
        self.known[self.module_name] = module_symbol
        self.generic_visit(node)

    def visit_ImportFrom(self, node: ast.ImportFrom):
        module = node.module or ""
        for alias in node.names:
            qualified = f"{module}.{alias.name}" if module else alias.name
            if qualified in self.known:
                local = alias.asname or alias.name
                self.imported[local] = self.known[qualified]
                self.add_reference("imports", self.known[qualified], node)

    def visit_ClassDef(self, node: ast.ClassDef):
        qualified = ".".join([self.module_name, *self.scope, node.name])
        display = ".".join([*self.scope, node.name]) if self.scope else node.name
        self.add_symbol("class", display, qualified, node)
        self.scope.append(node.name)
        self.generic_visit(node)
        self.scope.pop()

    def visit_FunctionDef(self, node: ast.FunctionDef):
        self._visit_function(node, "function")

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef):
        self._visit_function(node, "function")

    def _visit_function(self, node: ast.AST, kind: str):
        name = getattr(node, "name")
        qualified = ".".join([self.module_name, *self.scope, name])
        display = ".".join([*self.scope, name]) if self.scope else name
        self.add_symbol("method" if self.scope else kind, display, qualified, node)
        self.scope.append(name)
        self.generic_visit(node)
        self.scope.pop()

    def visit_Assign(self, node: ast.Assign):
        if isinstance(node.value, ast.Call):
            target_id = self.resolve_call(node.value)
            if target_id:
                class_name = self.display_for_symbol(target_id)
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        self.assigned_types[target.id] = class_name
        self.generic_visit(node)

    def visit_Call(self, node: ast.Call):
        self.add_reference("calls", self.resolve_call(node), node, 0.85)
        self.generic_visit(node)

    def resolve_call(self, node: ast.Call) -> str | None:
        func = node.func
        if isinstance(func, ast.Name):
            return self.imported.get(func.id) or self.known.get(func.id) or self.known.get(f"{self.module_name}.{func.id}")
        if isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name):
            owner = self.assigned_types.get(func.value.id)
            if owner:
                return self.known.get(f"{owner}.{func.attr}") or self.known.get(func.attr)
        return None

    def display_for_symbol(self, symbol_id: str) -> str:
        for name, candidate in self.known.items():
            if candidate == symbol_id:
                return name.split(".")[-1]
        return symbol_id


def collect(root: Path) -> Artifact:
    files = discover_files(root, {".py"})
    known: dict[str, str] = {}
    modules: list[tuple[Path, str, ast.Module]] = []
    for rel in files:
        source = (root / rel).read_text(encoding="utf-8")
        modules.append((rel, source, ast.parse(source, filename=rel.as_posix())))

    for rel, _source, tree in modules:
        module = module_name_for_path(rel)
        for node in ast.walk(tree):
            if isinstance(node, ast.ClassDef):
                known[f"{module}.{node.name}"] = stable_symbol_id("python", rel.as_posix(), "class", f"{module}.{node.name}", node.lineno, node.col_offset)
                known[node.name] = known[f"{module}.{node.name}"]
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                known[f"{module}.{node.name}"] = stable_symbol_id("python", rel.as_posix(), "function", f"{module}.{node.name}", node.lineno, node.col_offset)
                known[node.name] = known[f"{module}.{node.name}"]

    symbols: list[Symbol] = []
    references: list[Reference] = []
    for rel, source, tree in modules:
        collector = PythonCollector(rel.as_posix(), module_name_for_path(rel), source, known)
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
```

- [ ] **Step 4: Wire the Python wrapper**

Replace `producers/bin/code-intelligence-external-python` with:

```sh
#!/bin/sh
set -eu

name=$(basename "$0")

if [ "${1:-}" != "index" ]; then
  echo "usage: $name index --output <normalized-json>" >&2
  exit 64
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
producer_root=$(dirname "$script_dir")
exec python3 "$producer_root/python/index.py" "$@"
```

Run:

```bash
chmod +x producers/bin/code-intelligence-external-python producers/python/index.py
```

- [ ] **Step 5: Run Python producer tests through both script and wrapper**

Run:

```bash
python3 -m unittest producers.tests.test_python_producer
(cd producers/tests/fixtures/python && ../../../bin/code-intelligence-external-python index --output /tmp/python-normalized.json)
python3 -m json.tool /tmp/python-normalized.json >/dev/null
```

Expected: unittest `OK`, wrapper exits `0`, JSON validates.

- [ ] **Step 6: Commit Python producer**

Run:

```bash
git add producers/python/index.py producers/bin/code-intelligence-external-python producers/tests/fixtures/python producers/tests/test_python_producer.py
git commit -m "feat(producers): add python external index producer"
```

---

### Task 3: TypeScript and JavaScript Producer for Wolfmax

**Files:**
- Create: `producers/typescript/index.js`
- Create: `producers/tests/fixtures/typescript/package.json`
- Create: `producers/tests/fixtures/typescript/src/service.ts`
- Create: `producers/tests/fixtures/typescript/src/app.ts`
- Create: `producers/tests/test_typescript_producer.js`
- Modify: `producers/bin/code-intelligence-external-typescript`

- [ ] **Step 1: Add the TS fixture and failing Node tests**

Create fixture files:

```json
// producers/tests/fixtures/typescript/package.json
{"name":"producer-typescript-fixture","type":"module"}
```

```ts
// producers/tests/fixtures/typescript/src/service.ts
export class UserService {
  load(id: string) {
    return { id };
  }
}

export function makeService() {
  return new UserService();
}
```

```ts
// producers/tests/fixtures/typescript/src/app.ts
import { makeService } from "./service";

export function renderUser(id: string) {
  const service = makeService();
  return service.load(id);
}
```

Create `producers/tests/test_typescript_producer.js`:

```javascript
const assert = require("node:assert");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const test = require("node:test");

const root = path.resolve(__dirname, "..", "..");
const fixture = path.join(root, "producers", "tests", "fixtures", "typescript");
const producer = path.join(root, "producers", "typescript", "index.js");

function runProducer() {
	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ts-producer-"));
	try {
		const output = path.join(dir, "typescript-normalized.json");
		const result = spawnSync(process.execPath, [producer, "index", "--output", output], {
			cwd: fixture,
			encoding: "utf8",
		});
		assert.equal(result.status, 0, result.stderr);
		return JSON.parse(fs.readFileSync(output, "utf8"));
	} finally {
		fs.rmSync(dir, { recursive: true, force: true });
	}
}

test("typescript producer emits symbols imports and calls", () => {
	const payload = runProducer();
	assert.equal(payload.source_kind, "typescript_source");
	assert.equal(payload.language, "typescript");
	const byName = new Map(payload.symbols.map((symbol) => [symbol.display_name, symbol]));
	assert.ok(byName.has("UserService"));
	assert.ok(byName.has("UserService.load"));
	assert.ok(byName.has("makeService"));
	assert.ok(byName.has("renderUser"));
	const relationships = new Set(payload.references.map((ref) => `${ref.relationship}:${ref.to_external_symbol}`));
	assert.ok(relationships.has(`imports:${byName.get("makeService").external_symbol}`));
	assert.ok(relationships.has(`calls:${byName.get("makeService").external_symbol}`));
	assert.ok(relationships.has(`calls:${byName.get("UserService.load").external_symbol}`));
});

test("typescript producer output is deterministic", () => {
	assert.deepEqual(runProducer(), runProducer());
});
```

- [ ] **Step 2: Run TypeScript producer tests to verify they fail**

Run:

```bash
node --test producers/tests/test_typescript_producer.js
```

Expected: fail because `producers/typescript/index.js` does not exist.

- [ ] **Step 3: Implement the TypeScript/JavaScript source producer**

Create `producers/typescript/index.js` as an executable CommonJS script. It must:

```javascript
#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const path = require("node:path");

const PRODUCER = "code-intelligence-external-typescript";
const EXTENSIONS = new Set([".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"]);
const SKIP_DIRS = new Set([".git", "node_modules", "dist", "build", "target", "vendor", ".next", "coverage"]);

function loadProjectTypescript(root) {
	try {
		return require(require.resolve("typescript", { paths: [root] }));
	} catch {
		return null;
	}
}

function usage() {
	console.error(`usage: ${PRODUCER} index --output <normalized-json>`);
	return 64;
}

function stableId(language, filePath, kind, qualifiedName, startLine, startByte) {
	return `${language}:${filePath}:${kind}:${qualifiedName}:${startLine || 0}:${startByte || 0}`;
}

function discoverFiles(root) {
	const found = [];
	function walk(dir) {
		for (const name of fs.readdirSync(dir).sort()) {
			if (SKIP_DIRS.has(name)) continue;
			const full = path.join(dir, name);
			const stat = fs.statSync(full);
			if (stat.isDirectory()) walk(full);
			if (stat.isFile() && EXTENSIONS.has(path.extname(name))) found.push(path.relative(root, full).split(path.sep).join("/"));
		}
	}
	walk(root);
	return found;
}

function lineStarts(source) {
	const starts = [0];
	for (let index = 0; index < source.length; index += 1) {
		if (source[index] === "\n") starts.push(index + 1);
	}
	return starts;
}

function positionForOffset(starts, offset) {
	let line = 1;
	for (let index = 0; index < starts.length; index += 1) {
		if (starts[index] > offset) break;
		line = index + 1;
	}
	return { line, column: offset - starts[line - 1] + 1 };
}
```

Then add scanner functions with these exact rules:

- Match class declarations with `/\\bexport\\s+)?class\\s+([A-Za-z_$][\\w$]*)/g`.
- Match exported and non-exported functions with `/\\b(?:export\\s+)?(?:async\\s+)?function\\s+([A-Za-z_$][\\w$]*)\\s*\\(/g`.
- Match methods inside class bodies with `/\\b([A-Za-z_$][\\w$]*)\\s*\\([^)]*\\)\\s*\\{/g`, skipping `if`, `for`, `while`, `switch`, `function`, and `constructor`.
- Match named imports with `/import\\s+\\{([^}]+)\\}\\s+from\\s+["']([^"']+)["']/g` and resolve relative modules to known symbols by basename.
- Match calls with `/\\b([A-Za-z_$][\\w$]*)\\s*\\(/g`.
- Match member calls with `/\\b([A-Za-z_$][\\w$]*)\\.([A-Za-z_$][\\w$]*)\\s*\\(/g` and resolve by method display name suffix.

Emit normalized JSON:

```javascript
{
	source_kind: "typescript_source",
	producer: PRODUCER,
	language: "typescript",
	root_path: root,
	symbols,
	references
}
```

Sort `symbols` by `file_path`, `start_line`, `external_symbol`. Sort `references` by `file_path`, `line`, `column`, `relationship`, `to_external_symbol`. Use `confidence: 0.75` and `provenance: "typescript_source"` for source-level references.

The script must return `69` with `no TypeScript or JavaScript files found` on stderr when discovery finds no files.

Call `loadProjectTypescript(root)` during startup and use it to read `tsconfig.json` file lists when both the package and config exist. If either is absent, use `discoverFiles(root)`. The first phase still emits `source_kind: "typescript_source"` because symbol/reference binding is source-level.

- [ ] **Step 4: Wire the TypeScript wrapper**

Replace `producers/bin/code-intelligence-external-typescript` with:

```sh
#!/bin/sh
set -eu

name=$(basename "$0")

if [ "${1:-}" != "index" ]; then
  echo "usage: $name index --output <normalized-json>" >&2
  exit 64
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
producer_root=$(dirname "$script_dir")
exec node "$producer_root/typescript/index.js" "$@"
```

Run:

```bash
chmod +x producers/bin/code-intelligence-external-typescript producers/typescript/index.js
```

- [ ] **Step 5: Run TypeScript producer tests through script and wrapper**

Run:

```bash
node --test producers/tests/test_typescript_producer.js
(cd producers/tests/fixtures/typescript && ../../../bin/code-intelligence-external-typescript index --output /tmp/typescript-normalized.json)
python3 -m json.tool /tmp/typescript-normalized.json >/dev/null
```

Expected: Node tests pass, wrapper exits `0`, JSON validates.

- [ ] **Step 6: Commit TypeScript producer**

Run:

```bash
git add producers/typescript/index.js producers/bin/code-intelligence-external-typescript producers/tests/fixtures/typescript producers/tests/test_typescript_producer.js
git commit -m "feat(producers): add typescript external index producer"
```

---

### Task 4: Rust Source-Level Producer

**Files:**
- Create: `producers/rust/index.py`
- Create: `producers/tests/fixtures/rust/Cargo.toml`
- Create: `producers/tests/fixtures/rust/src/lib.rs`
- Create: `producers/tests/test_rust_producer.py`
- Modify: `producers/bin/code-intelligence-external-rust`

- [ ] **Step 1: Add Rust fixture and failing tests**

Create fixture files:

```toml
# producers/tests/fixtures/rust/Cargo.toml
[package]
name = "producer-rust-fixture"
version = "0.1.0"
edition = "2021"
```

```rust
// producers/tests/fixtures/rust/src/lib.rs
pub struct UserService;

impl UserService {
    pub fn load(&self, id: &str) -> String {
        id.to_string()
    }
}

pub fn make_service() -> UserService {
    UserService
}

pub fn render_user(id: &str) -> String {
    let service = make_service();
    service.load(id)
}
```

Create `producers/tests/test_rust_producer.py`:

```python
import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "producers" / "tests" / "fixtures" / "rust"
PRODUCER = ROOT / "producers" / "rust" / "index.py"


class RustProducerTests(unittest.TestCase):
    def run_producer(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "rust-normalized.json"
            result = subprocess.run(["python3", str(PRODUCER), "index", "--output", str(output)], cwd=FIXTURE, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(output.read_text(encoding="utf-8"))

    def test_emits_rust_symbols_and_calls(self):
        payload = self.run_producer()
        by_name = {item["display_name"]: item for item in payload["symbols"]}
        self.assertEqual(payload["source_kind"], "rust_source")
        self.assertIn("UserService", by_name)
        self.assertIn("UserService.load", by_name)
        self.assertIn("make_service", by_name)
        self.assertIn("render_user", by_name)
        refs = {(item["relationship"], item["to_external_symbol"]) for item in payload["references"]}
        self.assertIn(("calls", by_name["make_service"]["external_symbol"]), refs)
        self.assertIn(("calls", by_name["UserService.load"]["external_symbol"]), refs)

    def test_output_is_deterministic(self):
        self.assertEqual(self.run_producer(), self.run_producer())


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run Rust producer tests to verify they fail**

Run:

```bash
python3 -m unittest producers.tests.test_rust_producer
```

Expected: fail because `producers/rust/index.py` does not exist.

- [ ] **Step 3: Implement Rust source-level extraction**

Create `producers/rust/index.py` with a Python stdlib scanner:

- Discover `.rs` files under `src/` and any workspace member source roots.
- Return `69` when neither `Cargo.toml` nor `.rs` files exist.
- Emit `struct`, `enum`, `trait`, free `fn`, and `impl` method symbols.
- Track the active `impl TypeName` while scanning linearly.
- Resolve direct calls `name(` to free function symbols.
- Resolve member calls `.method(` to the first method symbol ending in `.method`.
- Use `source_kind: "rust_source"`, `language: "rust"`, `confidence: 0.65`, and `provenance: "rust_source"`.

Use the same `Artifact`, `Symbol`, `Reference`, `discover_files`, `stable_symbol_id`, and `write_artifact` helpers from `producers/lib/normalized.py`.

- [ ] **Step 4: Wire Rust wrapper**

Replace `producers/bin/code-intelligence-external-rust` with:

```sh
#!/bin/sh
set -eu

name=$(basename "$0")

if [ "${1:-}" != "index" ]; then
  echo "usage: $name index --output <normalized-json>" >&2
  exit 64
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
producer_root=$(dirname "$script_dir")
exec python3 "$producer_root/rust/index.py" "$@"
```

Run:

```bash
chmod +x producers/bin/code-intelligence-external-rust producers/rust/index.py
```

- [ ] **Step 5: Run Rust producer tests through script and wrapper**

Run:

```bash
python3 -m unittest producers.tests.test_rust_producer
(cd producers/tests/fixtures/rust && ../../../bin/code-intelligence-external-rust index --output /tmp/rust-normalized.json)
python3 -m json.tool /tmp/rust-normalized.json >/dev/null
```

Expected: unittest `OK`, wrapper exits `0`, JSON validates.

- [ ] **Step 6: Commit Rust producer**

Run:

```bash
git add producers/rust/index.py producers/bin/code-intelligence-external-rust producers/tests/fixtures/rust producers/tests/test_rust_producer.py
git commit -m "feat(producers): add rust external index producer"
```

---

### Task 5: Go Source-Level Producer

**Files:**
- Create: `producers/go/index.py`
- Create: `producers/tests/fixtures/go/go.mod`
- Create: `producers/tests/fixtures/go/service/service.go`
- Create: `producers/tests/fixtures/go/app/app.go`
- Create: `producers/tests/test_go_producer.py`
- Modify: `producers/bin/code-intelligence-external-go`

- [ ] **Step 1: Add Go fixture and failing tests**

Create fixture files:

```go
// producers/tests/fixtures/go/go.mod
module example.com/producer-go-fixture

go 1.22
```

```go
// producers/tests/fixtures/go/service/service.go
package service

type UserService struct{}

func (s UserService) Load(id string) string {
	return id
}

func MakeService() UserService {
	return UserService{}
}
```

```go
// producers/tests/fixtures/go/app/app.go
package app

import "example.com/producer-go-fixture/service"

func RenderUser(id string) string {
	svc := service.MakeService()
	return svc.Load(id)
}
```

Create `producers/tests/test_go_producer.py`:

```python
import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "producers" / "tests" / "fixtures" / "go"
PRODUCER = ROOT / "producers" / "go" / "index.py"


class GoProducerTests(unittest.TestCase):
    def run_producer(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "go-normalized.json"
            result = subprocess.run(["python3", str(PRODUCER), "index", "--output", str(output)], cwd=FIXTURE, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(output.read_text(encoding="utf-8"))

    def test_emits_go_symbols_imports_and_calls(self):
        payload = self.run_producer()
        by_name = {item["display_name"]: item for item in payload["symbols"]}
        self.assertEqual(payload["source_kind"], "go_source")
        self.assertIn("UserService", by_name)
        self.assertIn("UserService.Load", by_name)
        self.assertIn("MakeService", by_name)
        self.assertIn("RenderUser", by_name)
        refs = {(item["relationship"], item["to_external_symbol"]) for item in payload["references"]}
        self.assertIn(("calls", by_name["MakeService"]["external_symbol"]), refs)
        self.assertIn(("calls", by_name["UserService.Load"]["external_symbol"]), refs)

    def test_output_is_deterministic(self):
        self.assertEqual(self.run_producer(), self.run_producer())


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run Go producer tests to verify they fail**

Run:

```bash
python3 -m unittest producers.tests.test_go_producer
```

Expected: fail because `producers/go/index.py` does not exist.

- [ ] **Step 3: Implement Go source-level extraction**

Create `producers/go/index.py` with a Python stdlib scanner:

- Discover `.go` files while skipping `_test.go` for first-pass production rows.
- Return `69` when neither `go.mod` nor `.go` files exist.
- Emit package, `type`, free `func`, and receiver method symbols.
- Resolve selector calls such as `service.MakeService()` by function name and `.Load()` by method suffix.
- Emit import references for string imports whose package suffix maps to discovered package files.
- Use `source_kind: "go_source"`, `language: "go"`, `confidence: 0.65`, and `provenance: "go_source"`.

Use the shared normalized helpers from `producers/lib/normalized.py`.

- [ ] **Step 4: Wire Go wrapper**

Replace `producers/bin/code-intelligence-external-go` with:

```sh
#!/bin/sh
set -eu

name=$(basename "$0")

if [ "${1:-}" != "index" ]; then
  echo "usage: $name index --output <normalized-json>" >&2
  exit 64
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
producer_root=$(dirname "$script_dir")
exec python3 "$producer_root/go/index.py" "$@"
```

Run:

```bash
chmod +x producers/bin/code-intelligence-external-go producers/go/index.py
```

- [ ] **Step 5: Run Go producer tests through script and wrapper**

Run:

```bash
python3 -m unittest producers.tests.test_go_producer
(cd producers/tests/fixtures/go && ../../../bin/code-intelligence-external-go index --output /tmp/go-normalized.json)
python3 -m json.tool /tmp/go-normalized.json >/dev/null
```

Expected: unittest `OK`, wrapper exits `0`, JSON validates.

- [ ] **Step 6: Commit Go producer**

Run:

```bash
git add producers/go/index.py producers/bin/code-intelligence-external-go producers/tests/fixtures/go producers/tests/test_go_producer.py
git commit -m "feat(producers): add go external index producer"
```

---

### Task 6: Producer Contract and Import Integration Tests

**Files:**
- Modify: `tests/external_index_overlay.rs`
- Create: `tests/fixtures/external_index/generated-tier1/.gitkeep`

- [ ] **Step 1: Add failing Cargo integration tests for generated artifacts**

In `tests/external_index_overlay.rs`, add tests near the existing external importer tests:

```rust
#[test]
fn tier1_python_producer_artifact_imports_into_overlay() {
    let fixture = Utf8PathBuf::from_path_buf(
        std::env::current_dir()
            .unwrap()
            .join("producers/tests/fixtures/python"),
    )
    .unwrap();
    let artifact = run_repo_producer_fixture("python", &fixture, "python-normalized.json");
    let parsed = read_normalized_artifact(artifact.as_std_path()).expect("parse generated artifact");
    assert_eq!(parsed.language, "python");
    assert!(parsed.symbols.iter().any(|symbol| symbol.display_name == "render_user"));
    assert!(parsed.references.iter().any(|reference| reference.relationship == "calls"));
}

#[test]
fn tier1_typescript_producer_artifact_imports_into_overlay() {
    let fixture = Utf8PathBuf::from_path_buf(
        std::env::current_dir()
            .unwrap()
            .join("producers/tests/fixtures/typescript"),
    )
    .unwrap();
    let artifact = run_repo_producer_fixture("typescript", &fixture, "typescript-normalized.json");
    let parsed = read_normalized_artifact(artifact.as_std_path()).expect("parse generated artifact");
    assert_eq!(parsed.language, "typescript");
    assert!(parsed.symbols.iter().any(|symbol| symbol.display_name == "renderUser"));
    assert!(parsed.references.iter().any(|reference| reference.relationship == "calls"));
}
```

Add helper code in the same test module:

```rust
fn run_repo_producer_fixture(producer: &str, fixture: &Utf8Path, output_name: &str) -> Utf8PathBuf {
    let root = Utf8PathBuf::from_path_buf(std::env::current_dir().unwrap()).unwrap();
    let output = Utf8PathBuf::from_path_buf(
        tempfile::tempdir()
            .expect("tempdir")
            .keep()
            .join(output_name),
    )
    .unwrap();
    let executable = root
        .join("producers")
        .join("bin")
        .join(format!("code-intelligence-external-{producer}"));
    let status = std::process::Command::new(executable.as_std_path())
        .arg("index")
        .arg("--output")
        .arg(output.as_str())
        .current_dir(fixture.as_std_path())
        .status()
        .expect("run producer");
    assert!(status.success(), "producer {producer} failed with {status:?}");
    output
}
```

If the test module already has helpers for tempdirs or artifacts, adapt names to avoid duplicate function names while preserving the exact assertions above.

- [ ] **Step 2: Run the focused Cargo tests to verify failure or compile errors**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test tier1_ --test external_index_overlay
```

Expected before fixes: compile failure if imports are missing, or runtime failure if wrappers/producers are incomplete.

- [ ] **Step 3: Add missing imports and keep generated fixture directory**

At the top of `tests/external_index_overlay.rs`, ensure these imports exist:

```rust
use code_intelligence_mcp_server::external_index::artifact::read_normalized_artifact;
use code_intelligence_mcp_server::path::{Utf8Path, Utf8PathBuf};
```

Create `tests/fixtures/external_index/generated-tier1/.gitkeep` as an empty file if a persisted generated directory is required by the test harness.

- [ ] **Step 4: Run integration tests to verify pass**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test tier1_ --test external_index_overlay
```

Expected: both `tier1_*` tests pass.

- [ ] **Step 5: Commit integration coverage**

Run:

```bash
git add tests/external_index_overlay.rs tests/fixtures/external_index/generated-tier1/.gitkeep
git commit -m "test(producers): import generated tier1 artifacts"
```

---

### Task 7: Packaging Validation for Producer Support Files

**Files:**
- Modify: `producers/manifest.json`
- Modify: `npm/bundle.js`
- Modify: `npm/bundle.test.js`

- [ ] **Step 1: Add failing npm bundle tests for support files**

Extend `npm/bundle.test.js` with:

```javascript
test("validateBundle requires producer support files declared by manifest", () => {
	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ci-bundle-"));
	try {
		fs.writeFileSync(path.join(dir, "code-intelligence-mcp-server"), "");
		fs.chmodSync(path.join(dir, "code-intelligence-mcp-server"), 0o755);
		fs.mkdirSync(path.join(dir, "producers"));
		fs.mkdirSync(path.join(dir, "producers", "bin"), { recursive: true });
		fs.writeFileSync(
			path.join(dir, "producers", "manifest.json"),
			JSON.stringify({
				producers: [
					{
						executable: "producers/bin/code-intelligence-external-python",
						support_files: ["producers/lib/normalized.py", "producers/python/index.py"],
					},
				],
			}),
		);
		const wrapper = path.join(dir, "producers", "bin", "code-intelligence-external-python");
		fs.writeFileSync(wrapper, "");
		fs.chmodSync(wrapper, 0o755);

		assert.deepEqual(validateBundle(dir).missing, [
			"producers/lib/normalized.py",
			"producers/python/index.py",
		]);
	} finally {
		fs.rmSync(dir, { recursive: true, force: true });
	}
});
```

- [ ] **Step 2: Run npm tests to verify failure**

Run:

```bash
node --test npm/bundle.test.js
```

Expected: new test fails because `support_files` are not checked.

- [ ] **Step 3: Update manifest and bundle validation**

In `producers/manifest.json`, add `support_files` to the four Tier 1 producers:

```json
"support_files": [
  "producers/lib/__init__.py",
  "producers/lib/normalized.py",
  "producers/python/index.py"
]
```

Use producer-specific script paths for TypeScript, Rust, and Go. TypeScript should declare `producers/typescript/index.js`; Rust should declare `producers/rust/index.py`; Go should declare `producers/go/index.py`.

In `npm/bundle.js`, validate both legacy executable paths and manifest paths:

```javascript
function exists(filePath) {
	try {
		fs.accessSync(filePath, fs.constants.F_OK);
		return true;
	} catch {
		return false;
	}
}

function bundlePath(binDir, manifestPath) {
	return path.join(binDir, manifestPath);
}
```

Inside the producer loop:

```javascript
if (!isExecutable(bundlePath(binDir, producer.executable))) {
	missing.push(producer.executable);
}
for (const supportFile of producer.support_files || []) {
	if (!exists(bundlePath(binDir, supportFile))) {
		missing.push(supportFile);
	}
}
```

- [ ] **Step 4: Run npm tests and a real bundle simulation**

Run:

```bash
node --test npm/bundle.test.js
mkdir -p /tmp/code-intel-bundle/producers/bin /tmp/code-intel-bundle/producers/lib /tmp/code-intel-bundle/producers/python /tmp/code-intel-bundle/producers/typescript /tmp/code-intel-bundle/producers/rust /tmp/code-intel-bundle/producers/go
cp target/release/code-intelligence-mcp-server /tmp/code-intel-bundle/code-intelligence-mcp-server 2>/dev/null || cp target/debug/code-intelligence-mcp-server /tmp/code-intel-bundle/code-intelligence-mcp-server
cp producers/manifest.json /tmp/code-intel-bundle/producers/manifest.json
cp producers/bin/code-intelligence-external-{python,typescript,rust,go} /tmp/code-intel-bundle/producers/bin/
cp producers/lib/__init__.py producers/lib/normalized.py /tmp/code-intel-bundle/producers/lib/
cp producers/python/index.py /tmp/code-intel-bundle/producers/python/
cp producers/typescript/index.js /tmp/code-intel-bundle/producers/typescript/
cp producers/rust/index.py /tmp/code-intel-bundle/producers/rust/
cp producers/go/index.py /tmp/code-intel-bundle/producers/go/
chmod +x /tmp/code-intel-bundle/code-intelligence-mcp-server /tmp/code-intel-bundle/producers/bin/code-intelligence-external-*
node -e 'const {validateBundle}=require("./npm/bundle"); const result=validateBundle("/tmp/code-intel-bundle"); console.log(JSON.stringify(result)); process.exit(result.missing.length)'
```

Expected: npm tests pass and the simulation prints `{"missing":[]}`.

- [ ] **Step 5: Commit packaging validation**

Run:

```bash
git add producers/manifest.json npm/bundle.js npm/bundle.test.js
git commit -m "fix(package): validate producer support files"
```

---

### Task 8: Benchmark External Arm

**Files:**
- Modify: `bench/arms.py`
- Modify: `bench/tests/test_arms.py`
- Modify: `bench/README.md`

- [ ] **Step 1: Add failing benchmark arm test**

In `bench/tests/test_arms.py`, add:

```python
def test_external_arm_enables_tier1_producers_only():
    from bench.arms import ARMS

    arm = ARMS["code_intel_external"]
    assert arm.env["EXTERNAL_INDEX_AUTO"] == "true"
    assert arm.env["EXTERNAL_INDEX_ON_REFRESH"] == "explicit"
    assert arm.env["DESCRIPTIONS_ENABLED"] == "false"
    assert arm.env["RERANKER_ENABLED"] == "false"
    assert "EXTERNAL_INDEX_PRODUCER" not in arm.env
```

- [ ] **Step 2: Run benchmark arm tests to verify failure**

Run:

```bash
python3 -m pytest bench/tests/test_arms.py -q
```

Expected: fail with missing `code_intel_external`.

- [ ] **Step 3: Add the external arm**

In `bench/arms.py`, add a new arm named `code_intel_external` next to `code_intel_shipped`. It must inherit shipped defaults and set only:

```python
{
    "EXTERNAL_INDEX_AUTO": "true",
    "EXTERNAL_INDEX_ON_REFRESH": "explicit",
}
```

Do not set `DESCRIPTIONS_ENABLED`, `RERANKER_ENABLED`, or a single `EXTERNAL_INDEX_PRODUCER`; the daemon should select producers from project detection.

- [ ] **Step 4: Document benchmark gate**

Append a short note to the R007 section of `bench/README.md`:

```markdown
Next external-overlay benchmark arm: `code_intel_external`. It keeps R007 production defaults and enables explicit external producer execution only. Run it after TypeScript/Python producer smoke tests import non-zero rows for wolfmax and Django.
```

- [ ] **Step 5: Run benchmark arm tests**

Run:

```bash
python3 -m pytest bench/tests/test_arms.py -q
```

Expected: tests pass.

- [ ] **Step 6: Commit benchmark arm**

Run:

```bash
git add bench/arms.py bench/tests/test_arms.py bench/README.md
git commit -m "bench: add external producer arm"
```

---

### Task 9: Full Local Verification Gate

**Files:**
- No new files unless previous tasks expose missing docs or packaging paths.

- [ ] **Step 1: Run all producer tests**

Run:

```bash
python3 -m unittest producers.tests.test_normalized producers.tests.test_python_producer producers.tests.test_rust_producer producers.tests.test_go_producer
node --test producers/tests/test_typescript_producer.js
```

Expected: all producer tests pass.

- [ ] **Step 2: Run daemon and overlay tests with hash embeddings**

Run:

```bash
cargo fmt --check
EMBEDDINGS_BACKEND=hash cargo test external_index --test external_index_overlay
EMBEDDINGS_BACKEND=hash cargo test
```

Expected: formatting clean and tests pass.

- [ ] **Step 3: Run npm and benchmark harness tests**

Run:

```bash
node --test npm/bundle.test.js
python3 -m pytest bench/tests -q
```

Expected: tests pass.

- [ ] **Step 4: Run benchmark-repo smoke if checkouts exist**

Run:

```bash
if [ -d bench/state/repos/django ]; then
  (cd bench/state/repos/django && /Users/dikrana/Documents/trae_projects/code_intelligence_mcp_server/producers/bin/code-intelligence-external-python index --output /tmp/django-python-normalized.json)
  python3 - <<'PY'
import json
payload=json.load(open('/tmp/django-python-normalized.json'))
print(len(payload['symbols']), len(payload['references']))
assert len(payload['symbols']) > 0
assert len(payload['references']) > 0
PY
fi
if [ -d bench/state/repos/wolfmax ]; then
  (cd bench/state/repos/wolfmax && /Users/dikrana/Documents/trae_projects/code_intelligence_mcp_server/producers/bin/code-intelligence-external-typescript index --output /tmp/wolfmax-typescript-normalized.json)
  python3 - <<'PY'
import json
payload=json.load(open('/tmp/wolfmax-typescript-normalized.json'))
print(len(payload['symbols']), len(payload['references']))
assert len(payload['symbols']) > 0
assert len(payload['references']) > 0
PY
fi
```

Expected: if the benchmark repos exist locally, both smoke checks print non-zero symbol and reference counts.

- [ ] **Step 5: Commit any verification fixes**

If verification required changes, commit only those changes:

```bash
git add producers tests npm bench docs/superpowers/plans/2026-06-13-tier1-external-producers.md
git commit -m "fix(producers): stabilize tier1 verification"
```

If no files changed, skip this step.

---

### Task 10: Execution Summary and Benchmark Handoff

**Files:**
- Modify: `docs/superpowers/plans/2026-06-13-tier1-external-producers.md`

- [ ] **Step 1: Mark plan tasks complete as they land**

Update each checkbox in this plan from `- [ ]` to `- [x]` after the corresponding step is completed and verified.

- [ ] **Step 2: Capture producer counts for benchmark readiness**

Add a short "Producer Smoke Counts" section to this plan after Task 10 with the exact local counts printed by Task 9 Step 4. Use this format with numeric values only:

```markdown
## Producer Smoke Counts

- Django Python: 12345 symbols, 67890 references.
- Wolfmax TypeScript: 12345 symbols, 67890 references.
```

- [ ] **Step 3: Commit the updated plan**

Run:

```bash
git add docs/superpowers/plans/2026-06-13-tier1-external-producers.md
git commit -m "docs: update tier1 producer execution plan"
```

---

## Self-Review Notes

- Spec coverage: the plan covers all Tier 1 producers, keeps external indexing opt-in, preserves daemon/importer architecture, adds deterministic output tests, adds packaging support-file validation, and adds a benchmark arm only after producer smoke.
- Scope check: compiler-grade TypeScript/rust-analyzer/go/packages integration is not included in this phase; source-level extraction provides real normalized artifacts and honest provenance for the benchmark.
- Ambiguity resolved: TypeScript/JavaScript and Python are benchmark-critical; Rust and Go are Tier 1 completion gates with conservative source-level confidence.

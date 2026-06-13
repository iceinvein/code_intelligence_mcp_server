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
        source = "alpha\n  beta()\n"
        starts = line_index(source)
        self.assertEqual(starts, [0, 6, 15])
        self.assertEqual(position_for_offset(starts, 8), (2, 3))

    def test_discover_files_skips_vendor_and_orders_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "src").mkdir()
            (root / "src" / "b.py").write_text("def b(): pass\n", encoding="utf-8")
            (root / "src" / "a.py").write_text("def a(): pass\n", encoding="utf-8")
            (root / ".git").mkdir()
            (root / ".git" / "ignored.py").write_text("def ignored(): pass\n", encoding="utf-8")
            (root / "node_modules").mkdir()
            (root / "node_modules" / "ignored.py").write_text("def ignored(): pass\n", encoding="utf-8")

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

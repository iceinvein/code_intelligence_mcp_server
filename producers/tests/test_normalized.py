import json
import os
import tempfile
import unittest
from unittest import mock
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

    def test_stable_symbol_id_escapes_reserved_separators(self):
        value = stable_symbol_id("python", "pkg/app.py", "function", "pkg.app:run", 8, 21)
        self.assertEqual(value, "python:pkg/app.py:function:pkg.app%3Arun:8:21")

    def test_stable_symbol_id_preserves_none_distinctly_from_zero(self):
        none_value = stable_symbol_id("python", "pkg/app.py", "function", "pkg.app.run", None, None)
        zero_value = stable_symbol_id("python", "pkg/app.py", "function", "pkg.app.run", 0, 0)
        self.assertNotEqual(none_value, zero_value)
        self.assertEqual(none_value, "python:pkg/app.py:function:pkg.app.run:~:~")
        self.assertEqual(zero_value, "python:pkg/app.py:function:pkg.app.run:0:0")

    def test_line_index_and_position_for_offset_are_one_based(self):
        source = "alpha\n  beta()\n"
        starts = line_index(source)
        self.assertEqual(starts, [0, 6, 15])
        self.assertEqual(position_for_offset(starts, 8), (2, 3))

    def test_line_index_and_position_for_offset_use_utf8_byte_offsets(self):
        source = "éx\nz"
        starts = line_index(source)
        self.assertEqual(starts, [0, 4, 5])
        self.assertEqual(position_for_offset(starts, 2), (1, 3))
        self.assertEqual(position_for_offset(starts, 4), (2, 1))

    def test_position_for_offset_rejects_negative_and_out_of_range_offsets(self):
        starts = line_index("abc")
        self.assertEqual(starts, [0, 3])
        with self.assertRaises(ValueError):
            position_for_offset(starts, -1)
        with self.assertRaises(ValueError):
            position_for_offset(starts, 3)

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

    def test_discover_files_prunes_skipped_directories_before_descent(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "src").mkdir()
            (root / "src" / "app.py").write_text("def app(): pass\n", encoding="utf-8")
            (root / "src" / "node_modules").mkdir()
            (root / "src" / "node_modules" / "ignored.py").write_text("def ignored(): pass\n", encoding="utf-8")

            original_scandir = os.scandir

            def guarded_scandir(path):
                if Path(path).name == "node_modules":
                    self.fail("discover_files descended into a skipped directory")
                return original_scandir(path)

            with mock.patch("os.scandir", side_effect=guarded_scandir):
                found = [path.as_posix() for path in discover_files(root, {".py"})]

        self.assertEqual(found, ["src/app.py"])

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

    def test_write_artifact_sorting_is_total_for_ties(self):
        with tempfile.TemporaryDirectory() as tmp:
            left_output = Path(tmp) / "left.json"
            right_output = Path(tmp) / "right.json"
            symbol_a = Symbol("python:same.py:function:same:1:1", "a", "function", "same.py", 1, 1, 0, 1)
            symbol_z = Symbol("python:same.py:function:same:1:1", "z", "function", "same.py", 1, 1, 0, 1)
            reference_a = Reference("a", "python:same.py:function:same:1:1", "calls", "same.py", 1, 1, 1, 2, 0.7, "python_ast")
            reference_z = Reference("z", "python:same.py:function:same:1:1", "calls", "same.py", 1, 1, 1, 2, 0.7, "python_ast")
            left = Artifact("python_ast", "producer", "python", tmp, [symbol_z, symbol_a], [reference_z, reference_a])
            right = Artifact("python_ast", "producer", "python", tmp, [symbol_a, symbol_z], [reference_a, reference_z])

            write_artifact(left_output, left)
            write_artifact(right_output, right)
            left_text = left_output.read_text(encoding="utf-8")
            right_text = right_output.read_text(encoding="utf-8")
            payload = json.loads(left_output.read_text(encoding="utf-8"))

        self.assertEqual(left_text, right_text)
        self.assertEqual([item["display_name"] for item in payload["symbols"]], ["a", "z"])
        self.assertEqual([item["from_external_symbol"] for item in payload["references"]], ["a", "z"])

    def test_write_artifact_uses_trailing_newline_and_sorted_top_level_keys(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "artifact.json"
            artifact = Artifact(
                source_kind="python_ast",
                producer="code-intelligence-external-python",
                language="python",
                root_path=tmp,
                symbols=[],
                references=[],
            )

            write_artifact(output, artifact)
            text = output.read_text(encoding="utf-8")

        self.assertTrue(text.endswith("\n"))
        self.assertEqual(
            [line.strip().split(":", 1)[0] for line in text.splitlines()[1:7]],
            ['"language"', '"producer"', '"references"', '"root_path"', '"source_kind"', '"symbols"'],
        )


if __name__ == "__main__":
    unittest.main()

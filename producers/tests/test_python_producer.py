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
        self.assertIn("UserService.render", symbols)
        self.assertIn("make_service", symbols)
        self.assertIn("render_user", symbols)
        references = payload["references"]
        relationships = {
            (item["relationship"], item["to_external_symbol"])
            for item in references
        }
        target_ids = {
            item["display_name"]: item["external_symbol"]
            for item in payload["symbols"]
        }
        self.assertIn(("imports", target_ids["pkg.services"]), relationships)
        self.assertIn(("imports", target_ids["make_service"]), relationships)
        self.assertIn(("calls", target_ids["make_service"]), relationships)
        self.assertIn(("calls", target_ids["UserService.load"]), relationships)
        self.assertTrue(
            any(
                item["relationship"] == "calls"
                and item["to_external_symbol"] == target_ids["UserService.load"]
                and item["file_path"] == "pkg/services.py"
                for item in references
            )
        )

    def test_output_is_deterministic(self):
        self.assertEqual(self.run_producer(), self.run_producer())


if __name__ == "__main__":
    unittest.main()

import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "producers" / "tests" / "fixtures" / "go"
PRODUCER = ROOT / "producers" / "go" / "index.py"
WRAPPER = ROOT / "producers" / "bin" / "code-intelligence-external-go"


class GoProducerTests(unittest.TestCase):
    def run_producer(self, fixture=FIXTURE):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "go-normalized.json"
            result = subprocess.run(
                ["python3", str(PRODUCER), "index", "--output", str(output)],
                cwd=fixture,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(output.read_text(encoding="utf-8"))

    def write_fixture(self, root, files):
        root = Path(root)
        for relative, source in files.items():
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source, encoding="utf-8")

    def test_emits_symbols_imports_and_calls(self):
        payload = self.run_producer()
        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertEqual(payload["source_kind"], "go_source")
        self.assertEqual(payload["language"], "go")
        self.assertIn("service", symbols)
        self.assertIn("app", symbols)
        self.assertIn("UserService", symbols)
        self.assertIn("UserService.Load", symbols)
        self.assertIn("MakeService", symbols)
        self.assertIn("RenderUser", symbols)

        target_ids = {
            item["display_name"]: item["external_symbol"]
            for item in payload["symbols"]
        }
        references = payload["references"]
        self.assertTrue(
            all(item["relationship"] in {"call", "import"} for item in references)
        )
        self.assertTrue(
            all(item["from_external_symbol"] is not None for item in references)
        )
        self.assertIn(
            ("import", target_ids["service"]),
            {(item["relationship"], item["to_external_symbol"]) for item in references},
        )

        calls_to_make_service = [
            item
            for item in references
            if item["relationship"] == "call"
            and item["to_external_symbol"] == target_ids["MakeService"]
        ]
        calls_to_load = [
            item
            for item in references
            if item["relationship"] == "call"
            and item["to_external_symbol"] == target_ids["UserService.Load"]
        ]
        self.assertEqual(1, len(calls_to_make_service))
        self.assertEqual(1, len(calls_to_load))
        self.assertEqual(
            target_ids["RenderUser"],
            calls_to_make_service[0]["from_external_symbol"],
        )
        self.assertEqual(
            target_ids["RenderUser"],
            calls_to_load[0]["from_external_symbol"],
        )
        self.assertEqual(0.65, calls_to_make_service[0]["confidence"])
        self.assertEqual("go_source", calls_to_make_service[0]["provenance"])

        source = (FIXTURE / "service" / "service.go").read_text(encoding="utf-8")
        expected_start = len(source[: source.index("func MakeService")].encode("utf-8"))
        self.assertEqual(expected_start, symbols["MakeService"]["start_byte"])
        self.assertGreater(symbols["MakeService"]["end_byte"], expected_start)

    def test_output_is_deterministic(self):
        self.assertEqual(self.run_producer(), self.run_producer())

    def test_unresolved_package_selector_does_not_resolve_to_local_function(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "go.mod": "module example.com/false-edge\n\ngo 1.22\n",
                    "app/app.go": """
package app

func Marshal() string {
	return "local"
}

func Render(value any) string {
	json.Marshal(value)
	return ""
}
""".lstrip(),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["Marshal"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_method_suffix_without_receiver_type_does_not_resolve(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "go.mod": "module example.com/method-false-edge\n\ngo 1.22\n",
                    "app/app.go": """
package app

type UserService struct{}

func (s UserService) Load(id string) string {
	return id
}

func RenderUser(svc any, id string) string {
	return svc.Load(id)
}
""".lstrip(),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["UserService.Load"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_string_literals_and_comments_do_not_emit_calls(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "go.mod": "module example.com/non-code\n\ngo 1.22\n",
                    "app/app.go": """
package app

func MakeService() string {
	return "value"
}

func Render() string {
	// MakeService()
	_ = "MakeService()"
	return ""
}
""".lstrip(),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["MakeService"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_wrapper_missing_output_exits_usage(self):
        result = subprocess.run(
            [str(WRAPPER), "index"],
            cwd=FIXTURE,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertEqual(64, result.returncode, result.stderr)
        self.assertIn("usage:", result.stderr)

    def test_output_write_error_exits_usage(self):
        result = subprocess.run(
            ["python3", str(PRODUCER), "index", "--output", str(FIXTURE)],
            cwd=FIXTURE,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertEqual(64, result.returncode, result.stderr)
        self.assertIn("failed to write output", result.stderr)

    def test_no_go_project_or_files_exits_unavailable(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run(
                ["python3", str(PRODUCER), "index", "--output", str(Path(tmp) / "out.json")],
                cwd=tmp,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        self.assertEqual(69, result.returncode, result.stderr)
        self.assertIn("no Go project or source files found", result.stderr)

    def test_test_only_go_files_do_not_exit_unavailable(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "go-normalized.json"
            self.write_fixture(
                root,
                {
                    "pkg/only_test.go": """
package pkg

func TestOnly(t *testing.T) {
	helper()
}

func helper() {}
""".lstrip(),
                },
            )
            result = subprocess.run(
                ["python3", str(PRODUCER), "index", "--output", str(output)],
                cwd=root,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            self.assertEqual(0, result.returncode, result.stderr)
            payload = json.loads(output.read_text(encoding="utf-8"))

        self.assertEqual("go_source", payload["source_kind"])
        self.assertEqual([], payload["symbols"])
        self.assertEqual([], payload["references"])

    def test_wrapper_emits_valid_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "go-normalized.json"
            result = subprocess.run(
                [str(WRAPPER), "index", "--output", str(output)],
                cwd=FIXTURE,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            self.assertEqual(0, result.returncode, result.stderr)
            payload = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual("go_source", payload["source_kind"])


if __name__ == "__main__":
    unittest.main()

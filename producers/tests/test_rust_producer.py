import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "producers" / "tests" / "fixtures" / "rust"
PRODUCER = ROOT / "producers" / "rust" / "index.py"
WRAPPER = ROOT / "producers" / "bin" / "code-intelligence-external-rust"


class RustProducerTests(unittest.TestCase):
    def run_producer(self, fixture=FIXTURE):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "rust-normalized.json"
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

    def test_emits_symbols_and_calls(self):
        payload = self.run_producer()
        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertEqual(payload["source_kind"], "rust_source")
        self.assertEqual(payload["language"], "rust")
        self.assertIn("UserService", symbols)
        self.assertIn("UserService.load", symbols)
        self.assertIn("make_service", symbols)
        self.assertIn("render_user", symbols)

        target_ids = {
            item["display_name"]: item["external_symbol"]
            for item in payload["symbols"]
        }
        calls_to_make_service = [
            item
            for item in payload["references"]
            if item["relationship"] == "call"
            and item["to_external_symbol"] == target_ids["make_service"]
        ]
        calls_to_load = [
            item
            for item in payload["references"]
            if item["relationship"] == "call"
            and item["to_external_symbol"] == target_ids["UserService.load"]
        ]
        self.assertEqual(1, len(calls_to_make_service))
        self.assertEqual(1, len(calls_to_load))
        self.assertTrue(
            all(item["relationship"] == "call" for item in payload["references"])
        )
        self.assertTrue(
            all(item["from_external_symbol"] is not None for item in payload["references"])
        )
        self.assertEqual(
            target_ids["render_user"],
            calls_to_make_service[0]["from_external_symbol"],
        )
        self.assertEqual(
            target_ids["render_user"],
            calls_to_load[0]["from_external_symbol"],
        )
        self.assertEqual(0.65, calls_to_make_service[0]["confidence"])
        self.assertEqual("rust_source", calls_to_make_service[0]["provenance"])

        source = (FIXTURE / "src" / "lib.rs").read_text(encoding="utf-8")
        expected_start = len(source[: source.index("pub fn make_service")].encode("utf-8"))
        self.assertEqual(expected_start, symbols["make_service"]["start_byte"])
        self.assertGreater(symbols["make_service"]["end_byte"], expected_start)

    def test_output_is_deterministic(self):
        self.assertEqual(self.run_producer(), self.run_producer())

    def test_common_member_call_without_receiver_type_does_not_resolve(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "src/lib.rs": """
pub struct AnswerQuality;

impl AnswerQuality {
    pub fn as_str(&self) -> &'static str {
        "good"
    }
}

pub fn render_answer(answer: String) -> String {
    answer.as_str().to_string()
}
""".lstrip()
                },
            )
            payload = self.run_producer(root)

        target_ids = {
            item["display_name"]: item["external_symbol"]
            for item in payload["symbols"]
        }
        calls_to_as_str = [
            item
            for item in payload["references"]
            if item["relationship"] == "call"
            and item["to_external_symbol"] == target_ids["AnswerQuality.as_str"]
        ]
        self.assertEqual([], calls_to_as_str)

    def test_path_qualified_new_does_not_resolve_to_local_new(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "src/lib.rs": """
pub struct UserService;

impl UserService {
    pub fn new() -> Self {
        UserService
    }
}

pub fn new() -> i32 {
    1
}

pub fn build() -> UserService {
    UserService::new()
}
""".lstrip()
                },
            )
            payload = self.run_producer(root)

        target_ids = {
            item["display_name"]: item["external_symbol"]
            for item in payload["symbols"]
        }
        calls_to_new = [
            item
            for item in payload["references"]
            if item["relationship"] == "call"
            and item["to_external_symbol"] == target_ids["new"]
        ]
        self.assertEqual([], calls_to_new)

    def test_external_path_qualified_call_does_not_resolve_to_local_function(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "src/lib.rs": """
pub fn from_str() -> i32 {
    1
}

pub fn parse() -> i32 {
    serde_json::from_str()
}
""".lstrip()
                },
            )
            payload = self.run_producer(root)

        target_ids = {
            item["display_name"]: item["external_symbol"]
            for item in payload["symbols"]
        }
        calls_to_from_str = [
            item
            for item in payload["references"]
            if item["relationship"] == "call"
            and item["to_external_symbol"] == target_ids["from_str"]
        ]
        self.assertEqual([], calls_to_from_str)

    def test_parameter_shadowing_function_name_suppresses_direct_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "src/lib.rs": """
pub fn make_service() -> i32 {
    1
}

pub fn render(make_service: fn() -> i32) -> i32 {
    make_service()
}
""".lstrip()
                },
            )
            payload = self.run_producer(root)

        target_ids = {
            item["display_name"]: item["external_symbol"]
            for item in payload["symbols"]
        }
        calls_to_make_service = [
            item
            for item in payload["references"]
            if item["relationship"] == "call"
            and item["to_external_symbol"] == target_ids["make_service"]
        ]
        self.assertEqual([], calls_to_make_service)

    def test_receiver_shadowing_suppresses_stale_method_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "src/lib.rs": """
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
    let service = id;
    service.load(id)
}
""".lstrip()
                },
            )
            payload = self.run_producer(root)

        target_ids = {
            item["display_name"]: item["external_symbol"]
            for item in payload["symbols"]
        }
        calls_to_load = [
            item
            for item in payload["references"]
            if item["relationship"] == "call"
            and item["to_external_symbol"] == target_ids["UserService.load"]
        ]
        self.assertEqual([], calls_to_load)

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

    def test_no_rust_project_or_files_exits_unavailable(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run(
                ["python3", str(PRODUCER), "index", "--output", str(Path(tmp) / "out.json")],
                cwd=tmp,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        self.assertEqual(69, result.returncode, result.stderr)
        self.assertIn("no Rust project or source files found", result.stderr)

    def test_wrapper_emits_valid_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "rust-normalized.json"
            result = subprocess.run(
                [str(WRAPPER), "index", "--output", str(output)],
                cwd=FIXTURE,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            self.assertEqual(0, result.returncode, result.stderr)
            payload = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual("rust_source", payload["source_kind"])


if __name__ == "__main__":
    unittest.main()

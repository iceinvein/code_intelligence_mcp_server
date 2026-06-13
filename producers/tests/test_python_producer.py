import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "producers" / "tests" / "fixtures" / "python"
PRODUCER = ROOT / "producers" / "python" / "index.py"
WRAPPER = ROOT / "producers" / "bin" / "code-intelligence-external-python"


class PythonProducerTests(unittest.TestCase):
    def run_producer(self, fixture=FIXTURE):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "python-normalized.json"
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
        self.assertEqual(payload["source_kind"], "python_ast")
        self.assertEqual(payload["language"], "python")
        self.assertIn("UserService", symbols)
        self.assertIn("UserService.load", symbols)
        self.assertIn("UserService.render", symbols)
        self.assertIn("make_service", symbols)
        self.assertIn("render_user", symbols)
        self.assertIn("render_alias_user", symbols)
        self.assertIn("render_module_user", symbols)
        references = payload["references"]
        relationships = {
            (item["relationship"], item["to_external_symbol"])
            for item in references
        }
        self.assertTrue(
            all(item["relationship"] in {"call", "import"} for item in references)
        )
        target_ids = {
            item["display_name"]: item["external_symbol"]
            for item in payload["symbols"]
        }
        self.assertIn(("import", target_ids["pkg.services"]), relationships)
        self.assertIn(("import", target_ids["make_service"]), relationships)
        self.assertIn(("call", target_ids["make_service"]), relationships)
        self.assertIn(("call", target_ids["UserService.load"]), relationships)
        self.assertTrue(
            all(item["from_external_symbol"] is not None for item in references)
        )
        self.assertTrue(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == target_ids["make_service"]
                and item["from_external_symbol"] == target_ids["render_alias_user"]
                for item in references
            )
        )
        self.assertTrue(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == target_ids["make_service"]
                and item["from_external_symbol"] == target_ids["render_module_user"]
                for item in references
            )
        )
        self.assertTrue(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == target_ids["UserService.load"]
                and item["file_path"] == "pkg/services.py"
                for item in references
            )
        )
        self.assertTrue(
            any(
                item["relationship"] == "import"
                and item["to_external_symbol"] == target_ids["pkg.services"]
                and item["file_path"] == "pkg/views.py"
                and item["line"] == 1
                and item["column"] == 1
                for item in references
            )
        )
        self.assertTrue(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == target_ids["make_service"]
                and item["from_external_symbol"] == target_ids["render_user"]
                and item["column"] == 15
                for item in references
            )
        )
        services_source = (FIXTURE / "pkg" / "services.py").read_text(encoding="utf-8")
        expected_make_service_start = len(
            services_source[: services_source.index("def make_service")].encode("utf-8")
        )
        self.assertEqual(
            symbols["make_service"]["start_byte"],
            expected_make_service_start,
        )
        self.assertGreater(symbols["make_service"]["start_byte"], 0)

    def test_output_is_deterministic(self):
        self.assertEqual(self.run_producer(), self.run_producer())

    def test_bare_call_does_not_resolve_to_duplicate_in_other_module(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/a.py": "def shared():\n    return 1\n",
                    "pkg/b.py": "def shared():\n    return 2\n",
                    "pkg/c.py": "def caller():\n    return shared()\n",
                },
            )
            payload = self.run_producer(root)

        false_targets = {
            item["external_symbol"]
            for item in payload["symbols"]
            if item["file_path"] in {"pkg/a.py", "pkg/b.py"}
            and item["display_name"] == "shared"
        }
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] in false_targets
                for item in payload["references"]
            )
        )

    def test_plain_import_does_not_resolve_to_bare_function_collision(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/local.py": (
                        "def json():\n"
                        "    return {}\n"
                        "\n"
                        "import json\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "import"
                and item["to_external_symbol"] == symbols["json"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_assigned_type_does_not_leak_between_functions(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/services.py": (
                        "class UserService:\n"
                        "    def load(self, user_id):\n"
                        "        return user_id\n"
                        "\n"
                        "def make_service():\n"
                        "    return UserService()\n"
                    ),
                    "pkg/views.py": (
                        "from pkg.services import make_service\n"
                        "\n"
                        "def prepare(user_id):\n"
                        "    service = make_service()\n"
                        "    return service.load(user_id)\n"
                        "\n"
                        "def leak(user_id):\n"
                        "    return service.load(user_id)\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        calls_to_load = [
            item
            for item in payload["references"]
            if item["relationship"] == "call"
            and item["to_external_symbol"] == symbols["UserService.load"]["external_symbol"]
        ]
        self.assertEqual(
            [item["from_external_symbol"] for item in calls_to_load],
            [symbols["prepare"]["external_symbol"]],
        )

    def test_local_bindings_shadow_imported_names(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/services.py": (
                        "def make_service():\n"
                        "    return object()\n"
                    ),
                    "pkg/views.py": (
                        "import pkg.services as services\n"
                        "from pkg.services import make_service\n"
                        "\n"
                        "def render(make_service):\n"
                        "    return make_service()\n"
                        "\n"
                        "def render_alias():\n"
                        "    services = object()\n"
                        "    return services.make_service()\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["make_service"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_function_wide_local_bindings_shadow_imports_before_assignment(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/services.py": (
                        "def make_service():\n"
                        "    return object()\n"
                    ),
                    "pkg/views.py": (
                        "import pkg.services as services\n"
                        "from pkg.services import make_service\n"
                        "\n"
                        "def render():\n"
                        "    first = make_service()\n"
                        "    make_service = lambda: object()\n"
                        "    return first\n"
                        "\n"
                        "def render_alias():\n"
                        "    first = services.make_service()\n"
                        "    services = object()\n"
                        "    return first\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["make_service"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_root_shadow_blocks_dotted_plain_import_resolution(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/services.py": (
                        "def make_service():\n"
                        "    return object()\n"
                    ),
                    "pkg/views.py": (
                        "import pkg.services\n"
                        "\n"
                        "def render():\n"
                        "    first = pkg.services.make_service()\n"
                        "    pkg = object()\n"
                        "    return first\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["make_service"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_reassignment_clears_inferred_type_binding(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/services.py": (
                        "class UserService:\n"
                        "    def load(self, user_id):\n"
                        "        return user_id\n"
                        "\n"
                        "def make_service():\n"
                        "    return UserService()\n"
                    ),
                    "pkg/views.py": (
                        "from pkg.services import make_service\n"
                        "\n"
                        "def render(user_id):\n"
                        "    service = make_service()\n"
                        "    service = None\n"
                        "    return service.load(user_id)\n"
                        "\n"
                        "def render_annotated(user_id):\n"
                        "    service = make_service()\n"
                        "    service: object = None\n"
                        "    return service.load(user_id)\n"
                        "\n"
                        "def render_augmented(user_id):\n"
                        "    service = make_service()\n"
                        "    service += None\n"
                        "    return service.load(user_id)\n"
                        "\n"
                        "def render_destructured(user_id):\n"
                        "    service = make_service()\n"
                        "    service, other = (None, None)\n"
                        "    return service.load(user_id)\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["UserService.load"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_nested_function_self_does_not_resolve_to_outer_method_class(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/services.py": (
                        "class Service:\n"
                        "    def load(self):\n"
                        "        return None\n"
                        "\n"
                        "    def outer(self):\n"
                        "        def inner(self):\n"
                        "            return self.load()\n"
                        "        return inner\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["Service.load"]["external_symbol"]
                and item["from_external_symbol"] == symbols["Service.outer.inner"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_return_type_inference_respects_parameter_shadow(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/views.py": (
                        "class UserService:\n"
                        "    def load(self):\n"
                        "        return None\n"
                        "\n"
                        "def factory(UserService):\n"
                        "    return UserService()\n"
                        "\n"
                        "def render(UserService):\n"
                        "    service = factory(UserService)\n"
                        "    return service.load()\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["UserService.load"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_return_type_inference_ignores_unimported_class_name_collision(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/services.py": (
                        "class UserService:\n"
                        "    def load(self, user_id):\n"
                        "        return user_id\n"
                    ),
                    "pkg/views.py": (
                        "def make_service():\n"
                        "    return UserService()\n"
                        "\n"
                        "def render(user_id):\n"
                        "    service = make_service()\n"
                        "    return service.load(user_id)\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["UserService.load"]["external_symbol"]
                for item in payload["references"]
            )
        )

    def test_nested_function_return_does_not_infer_outer_return_type(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "pkg/views.py": (
                        "class UserService:\n"
                        "    def load(self, user_id):\n"
                        "        return user_id\n"
                        "\n"
                        "def factory():\n"
                        "    def inner():\n"
                        "        return UserService()\n"
                        "    return None\n"
                        "\n"
                        "def render(user_id):\n"
                        "    service = factory()\n"
                        "    return service.load(user_id)\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        self.assertFalse(
            any(
                item["relationship"] == "call"
                and item["to_external_symbol"] == symbols["UserService.load"]["external_symbol"]
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
        self.assertEqual(result.returncode, 64, result.stderr)
        self.assertIn("usage:", result.stderr)

    def test_src_layout_imports_use_source_root_module_names(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_fixture(
                root,
                {
                    "src/accounts/models.py": (
                        "def build_user():\n"
                        "    return {\"id\": 1}\n"
                    ),
                    "src/views.py": (
                        "import accounts.models as models\n"
                        "\n"
                        "def render():\n"
                        "    return models.build_user()\n"
                    ),
                },
            )
            payload = self.run_producer(root)

        symbols = {item["display_name"]: item for item in payload["symbols"]}
        relationships = {
            (item["relationship"], item["to_external_symbol"])
            for item in payload["references"]
        }
        self.assertIn("accounts.models", symbols)
        self.assertNotIn("src.accounts.models", symbols)
        self.assertIn(("import", symbols["accounts.models"]["external_symbol"]), relationships)
        self.assertIn(("call", symbols["build_user"]["external_symbol"]), relationships)


if __name__ == "__main__":
    unittest.main()

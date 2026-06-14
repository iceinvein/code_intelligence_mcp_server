"""Tests for bench/run.py CLI dispatching.

These tests only verify argparse plumbing and command routing. The
underlying logic is tested in the respective modules.
"""
import os
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]


def _run_cli(args):
    # Use the same interpreter that is running pytest so package dependencies
    # (e.g. PyYAML) are guaranteed to be available.
    return subprocess.run(
        [sys.executable, "-m", "bench.run", *args],
        capture_output=True,
        cwd=str(REPO_ROOT),
        env={**os.environ, "PYTHONPATH": str(REPO_ROOT)},
    )


def test_cli_help_lists_commands():
    result = _run_cli(["--help"])
    assert result.returncode == 0
    out = result.stdout.decode()
    for cmd in ["prep", "full", "arm", "question", "report", "diff", "list", "clean", "validate"]:
        assert cmd in out


def test_cli_validate_smoke_fixture():
    result = _run_cli(["validate", "bench/fixtures/smoke.yaml"])
    assert result.returncode == 0, result.stderr.decode()
    out = result.stdout.decode().lower()
    assert "ok" in out or "no errors" in out


def test_cli_list_does_not_crash_on_empty_results_dir():
    result = _run_cli(["list"])
    assert result.returncode == 0


def test_variant_env_maps_external_to_explicit_producers():
    from bench import run

    assert run._env_for_variant("no_desc") == {"BENCH_DISABLE_DESCRIPTIONS": "1"}
    assert run._env_for_variant("full") == {"DESCRIPTIONS_ENABLED": "1"}
    external_env = run._env_for_variant("external")
    assert external_env == {
        "BENCH_DISABLE_DESCRIPTIONS": "1",
        "DESCRIPTIONS_ENABLED": "false",
        "RERANKER_ENABLED": "false",
        "EXTERNAL_INDEX_AUTO": "true",
        "EXTERNAL_INDEX_ON_REFRESH": "explicit",
        "EXTERNAL_INDEX_TYPESCRIPT_COMMAND": str(
            REPO_ROOT / "producers" / "bin" / "code-intelligence-external-typescript"
        ),
        "EXTERNAL_INDEX_PYTHON_COMMAND": str(
            REPO_ROOT / "producers" / "bin" / "code-intelligence-external-python"
        ),
        "EXTERNAL_INDEX_RUST_COMMAND": str(
            REPO_ROOT / "producers" / "bin" / "code-intelligence-external-rust"
        ),
        "EXTERNAL_INDEX_GO_COMMAND": str(
            REPO_ROOT / "producers" / "bin" / "code-intelligence-external-go"
        ),
    }
    for key, command in external_env.items():
        if key.startswith("EXTERNAL_INDEX_") and key.endswith("_COMMAND"):
            assert Path(command).is_file()

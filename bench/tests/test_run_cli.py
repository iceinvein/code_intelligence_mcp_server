"""Tests for bench/run.py CLI dispatching.

These tests only verify argparse plumbing and command routing. The
underlying logic is tested in the respective modules.
"""
import json
import os
import subprocess
import sys
from argparse import Namespace
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


def test_stale_variant_index_is_wiped_before_rebuild(tmp_path, monkeypatch):
    """Reindex no-ops on unchanged file fingerprints, so a stale cache (new
    daemon binary with changed extraction) must wipe the data dir to force a
    real rebuild; POSTing reindex over the old dir silently keeps stale edges."""
    from bench import run as run_mod

    data_dir = tmp_path / "repo_hash"
    data_dir.mkdir(parents=True)
    (data_dir / "code-intelligence.db").write_text("stale")
    (data_dir / "bench-cache.json").write_text('{"daemon_bin": "old"}')

    built = {}

    def fake_build(name, repo_path, variant):
        built["called"] = True
        # the stale artifacts must be gone before the daemon rebuilds
        assert not (data_dir / "code-intelligence.db").exists()

    monkeypatch.setattr(run_mod, "_build_variant", fake_build)

    status = run_mod.ensure_variant_index(
        name="repo", repo_path=tmp_path / "checkout", variant="no_desc",
        meta_dict={"daemon_bin": "new"}, data_dir=data_dir,
    )
    assert status == "built"
    assert built.get("called")


def test_fresh_variant_index_is_left_alone(tmp_path, monkeypatch):
    import json as _json
    from bench import run as run_mod

    data_dir = tmp_path / "repo_hash"
    data_dir.mkdir(parents=True)
    meta = {"daemon_bin": "same"}
    (data_dir / "bench-cache.json").write_text(_json.dumps(meta))
    (data_dir / "code-intelligence.db").write_text("good")

    monkeypatch.setattr(run_mod, "_build_variant",
                        lambda *a, **k: pytest.fail("must not rebuild fresh index"))

    status = run_mod.ensure_variant_index(
        name="repo", repo_path=tmp_path / "checkout", variant="no_desc",
        meta_dict=meta, data_dir=data_dir,
    )
    assert status == "cached"
    assert (data_dir / "code-intelligence.db").read_text() == "good"


def test_question_set_loads_by_name_and_path(tmp_path):
    import json as _json
    from bench import run as run_mod

    # by name: resolves bench/fixtures/question_sets/<name>.json
    ids = run_mod.load_question_set("iteration")
    assert "wolfmax-arch-03" in ids
    assert "django-multi-hop-02" not in ids  # excluded cap-stress question

    # by path
    p = tmp_path / "custom.json"
    p.write_text(_json.dumps({"name": "c", "questions": ["a-1", "b-2"]}))
    assert run_mod.load_question_set(str(p)) == {"a-1", "b-2"}


def test_question_set_filters_fixture_questions():
    from bench import fixtures_io, config
    from bench import run as run_mod

    fixture = fixtures_io.load_fixture(config.FIXTURES_DIR / "smoke.yaml")
    keep = {fixture.questions[0].id}
    filtered = run_mod.filter_fixture(fixture, keep)
    assert [q.id for q in filtered.questions] == [fixture.questions[0].id]
    assert filtered.meta.repo == fixture.meta.repo


def test_question_set_with_no_matching_ids_raises():
    from bench import fixtures_io, config
    from bench import run as run_mod

    fixture = fixtures_io.load_fixture(config.FIXTURES_DIR / "smoke.yaml")
    with pytest.raises(SystemExit):
        run_mod.apply_question_set([fixture], {"nonexistent-question-id"})


def test_report_loads_persisted_round_provenance(tmp_path, monkeypatch):
    from bench import run as run_mod

    round_dir = tmp_path / "R123"
    round_dir.mkdir()
    score = {
        "arm": "code_intel_shipped",
        "repo": "fixture-repo",
        "question_id": "q1",
        "task_type": "symbol_lookup",
        "mech": 1.0,
        "judge_median": 9.0,
        "judge_range": 0,
        "citation_hit": True,
        "forbidden_hit": False,
        "hallucinated": False,
        "tool_calls": 1,
        "input_tokens": 100,
        "wall_ms": 10,
        "hit_turn_cap": False,
    }
    (round_dir / "scores.json").write_text(json.dumps(score) + "\n")
    (round_dir / "meta.json").write_text(
        json.dumps(
            {
                "daemon": {"git_sha": "git-exact", "binary_sha256": "bin-exact"},
                "fixtures": [{
                    "repo": "fixture-repo",
                    "upstream_sha": "upstream-exact",
                    "fixture_sha256": "fixture-exact",
                    "authored_against_schema_version": 22,
                    "question_ids": ["q1"],
                }],
                "models": {"agent": "agent-exact", "judges": {}},
                "binaries": {"agent_cli": "cli-exact", "codegraph": None},
                "configuration": {"arms": []},
                "comparator": {
                    "baseline_arm": "default",
                    "candidate_arms": ["code_intel_shipped"],
                },
            }
        )
    )
    monkeypatch.setattr(run_mod.config, "RESULTS_DIR", tmp_path)

    assert run_mod.cmd_report(Namespace(round="R123", out=None)) == 0
    rendered = (round_dir / "report.md").read_text()
    assert "git-exact" in rendered
    assert "fixture-exact" in rendered
    assert "default → code_intel_shipped" in rendered

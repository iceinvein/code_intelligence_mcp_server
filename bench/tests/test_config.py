"""Tests for bench/config.py."""
import os
from pathlib import Path

from bench import config


def _reload_clean(monkeypatch):
    # Self-isolating: clear any leftover BENCH_* env vars from prior tests
    # and reload the module so its constants are recomputed against a clean env.
    for k in list(os.environ):
        if k.startswith("BENCH_"):
            monkeypatch.delenv(k, raising=False)
    import importlib
    importlib.reload(config)


def test_defaults_resolve_without_env(monkeypatch):
    _reload_clean(monkeypatch)
    assert config.AGENT_MODEL == "claude-sonnet-4-6"
    assert config.JUDGE_HAIKU == "claude-haiku-4-5"
    assert config.JUDGE_SONNET == "claude-sonnet-4-6"
    assert config.JUDGE_OPUS == "claude-opus-4-8"
    assert config.PER_QUESTION_TIMEOUT_S == 180


def test_env_overrides_take_precedence(monkeypatch):
    monkeypatch.setenv("BENCH_AGENT_MODEL", "claude-opus-4-7")
    monkeypatch.setenv("BENCH_TIMEOUT_S", "60")
    # Reload the module so env reads at import time pick up the change.
    import importlib
    importlib.reload(config)
    assert config.AGENT_MODEL == "claude-opus-4-7"
    assert config.PER_QUESTION_TIMEOUT_S == 60


def test_state_dir_outside_repo_tree(monkeypatch):
    # Mutable state must not live under the repo: fixture checkouts inside the
    # working copy forced a bench-specific daemon exclude pattern and put ~27G
    # of index caches in the source tree.
    _reload_clean(monkeypatch)
    assert config.REPO_ROOT not in config.STATE_DIR.parents
    assert config.STATE_DIR == Path.home() / ".code-intelligence-bench"


def test_state_dir_env_override(monkeypatch, tmp_path):
    monkeypatch.setenv("BENCH_STATE_DIR", str(tmp_path / "bench-state"))
    import importlib
    importlib.reload(config)
    assert config.STATE_DIR == tmp_path / "bench-state"
    assert config.BENCH_REPOS_DIR == config.STATE_DIR / "repos"


def test_isolated_home_under_state(monkeypatch):
    _reload_clean(monkeypatch)
    assert config.BENCH_HOME == config.STATE_DIR / "home"
    assert config.BENCH_INDEXES_DIR == config.BENCH_HOME / ".code-intelligence" / "repos"

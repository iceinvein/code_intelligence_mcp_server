"""Tests for bench/config.py."""
import os

from bench import config


def test_defaults_resolve_without_env():
    assert config.AGENT_MODEL == "claude-sonnet-4-6"
    assert config.JUDGE_HAIKU == "claude-haiku-4-5"
    assert config.JUDGE_SONNET == "claude-sonnet-4-6"
    assert config.JUDGE_OPUS == "claude-opus-4-7"
    assert config.PER_QUESTION_TIMEOUT_S == 180


def test_env_overrides_take_precedence(monkeypatch):
    monkeypatch.setenv("BENCH_AGENT_MODEL", "claude-opus-4-7")
    monkeypatch.setenv("BENCH_TIMEOUT_S", "60")
    # Reload the module so env reads at import time pick up the change.
    import importlib
    importlib.reload(config)
    assert config.AGENT_MODEL == "claude-opus-4-7"
    assert config.PER_QUESTION_TIMEOUT_S == 60


def test_state_dir_under_bench():
    assert config.STATE_DIR.name == "state"
    assert config.STATE_DIR.parent.name == "bench"


def test_isolated_home_under_state():
    assert config.BENCH_HOME == config.STATE_DIR / "home"
    assert config.BENCH_INDEXES_DIR == config.BENCH_HOME / ".code-intelligence" / "repos"

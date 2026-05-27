"""Tests for bench/arms.py."""
import pytest

from bench import arms


def test_five_arms_defined():
    assert set(arms.ARMS.keys()) == {
        "default",
        "code_intel_full",
        "code_intel_no_descriptions",
        "code_intel_no_reranker",
        "codegraph",
    }


def test_default_arm_has_no_daemon_and_no_mcp_tools():
    a = arms.ARMS["default"]
    assert a.needs_daemon is False
    assert a.daemon_env == {}
    assert set(a.allowed_tools) == {"Read", "Grep", "Glob", "Bash"}
    assert a.index_variant is None


def test_code_intel_full_uses_full_variant_no_env():
    a = arms.ARMS["code_intel_full"]
    assert a.needs_daemon is True
    assert a.daemon_env == {}
    assert a.index_variant == "full"
    assert "mcp__code-intelligence__ask_code" in a.allowed_tools


def test_code_intel_no_descriptions_sets_env_and_uses_no_desc_variant():
    a = arms.ARMS["code_intel_no_descriptions"]
    assert a.daemon_env == {"BENCH_DISABLE_DESCRIPTIONS": "1"}
    assert a.index_variant == "no_desc"


def test_code_intel_no_reranker_sets_env_and_reuses_full_variant():
    a = arms.ARMS["code_intel_no_reranker"]
    assert a.daemon_env == {"BENCH_DISABLE_RERANKER": "1"}
    assert a.index_variant == "full"


def test_codegraph_arm_uses_codegraph_tools_no_daemon():
    a = arms.ARMS["codegraph"]
    assert a.needs_daemon is False
    assert a.is_codegraph is True
    assert "mcp__codegraph__codegraph_context" in a.allowed_tools


def test_distinct_index_variants_dedupes():
    variants = arms.distinct_index_variants(list(arms.ARMS.values()))
    assert variants == {"full", "no_desc"}

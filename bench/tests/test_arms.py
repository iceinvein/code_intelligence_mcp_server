"""Tests for bench/arms.py."""
import pytest

from bench import arms


def test_six_arms_defined():
    assert set(arms.ARMS.keys()) == {
        "default",
        "code_intel_full",
        "code_intel_shipped",
        "code_intel_external",
        "code_intel_no_reranker",
        "codegraph",
    }


def test_default_arm_has_no_daemon_and_no_mcp_tools():
    a = arms.ARMS["default"]
    assert a.needs_daemon is False
    assert a.daemon_env == {}
    assert set(a.allowed_tools) == {"Read", "Grep", "Glob", "Bash"}
    assert a.index_variant is None


def test_code_intel_full_enables_reranker_and_uses_full_variant():
    a = arms.ARMS["code_intel_full"]
    assert a.needs_daemon is True
    # Reranker ships off by default; the full arm opts in so it is the only
    # arm with the cross-encoder live.
    assert a.daemon_env == {"RERANKER_ENABLED": "1"}
    assert a.index_variant == "full"
    assert "mcp__code-intelligence__ask_code" in a.allowed_tools


def test_code_intel_shipped_sets_env_and_uses_no_desc_variant():
    # Production-default config: descriptions off, reranker off, plain no_desc index.
    a = arms.ARMS["code_intel_shipped"]
    assert a.daemon_env == {"BENCH_DISABLE_DESCRIPTIONS": "1"}
    assert a.index_variant == "no_desc"


def test_external_arm_enables_tier1_producers_only():
    a = arms.ARMS["code_intel_external"]
    assert a.needs_daemon is True
    assert a.index_variant == "external"
    assert a.daemon_env["EXTERNAL_INDEX_AUTO"] == "true"
    assert a.daemon_env["EXTERNAL_INDEX_ON_REFRESH"] == "explicit"
    assert a.daemon_env["DESCRIPTIONS_ENABLED"] == "false"
    assert a.daemon_env["RERANKER_ENABLED"] == "false"
    assert "EXTERNAL_INDEX_PRODUCER" not in a.daemon_env
    assert "mcp__code-intelligence__ask_code" in a.allowed_tools


def test_code_intel_no_reranker_leaves_reranker_off_and_reuses_full_variant():
    a = arms.ARMS["code_intel_no_reranker"]
    # No RERANKER_ENABLED → reranker never constructed. Same "full" index as
    # code_intel_full; the only difference is whether the reranker reorders.
    assert a.daemon_env == {}
    assert a.index_variant == "full"


def test_codegraph_arm_uses_codegraph_tools_no_daemon():
    a = arms.ARMS["codegraph"]
    assert a.needs_daemon is False
    assert a.is_codegraph is True
    assert "mcp__codegraph__codegraph_context" in a.allowed_tools


def test_distinct_index_variants_dedupes():
    variants = arms.distinct_index_variants(list(arms.ARMS.values()))
    assert variants == {"full", "no_desc", "external"}

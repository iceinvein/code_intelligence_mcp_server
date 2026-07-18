"""Tests for bench/report.py."""
from pathlib import Path

import pytest

from bench import report


def _scores(values):
    return [
        {
            "question_id": f"q{i}",
            "task_type": "symbol_lookup",
            "mech": v,
            "judge_median": v * 10,
            "judge_range": 1,
            "tool_calls": 2,
            "input_tokens": 50000,
            "wall_ms": 30000,
            "citation_hit": v > 0.6,
            "hallucinated": False,
            "forbidden_hit": False,
        }
        for i, v in enumerate(values, start=1)
    ]


def test_aggregate_arm():
    rows = _scores([1.0, 0.8, 0.6])
    agg = report.aggregate_arm("code_intel_full", rows)
    assert agg["judge_median"] == pytest.approx(8.0)
    assert agg["mech"] == pytest.approx(0.8)
    assert agg["citation_hit_rate"] == pytest.approx(2/3)
    assert agg["n"] == 3


def test_aggregate_includes_token_efficiency_and_turn_caps():
    rows = _scores([1.0, 0.8])
    rows[0]["hit_turn_cap"] = True
    agg = report.aggregate_arm("code_intel_full", rows)
    # 50000 input tokens/run, judge mean 9.0 -> ~5556 tokens per judge point
    assert agg["tokens_per_judge_point"] == pytest.approx(50000 / 9.0)
    assert agg["turn_capped_rate"] == pytest.approx(0.5)


def test_aggregate_token_efficiency_none_without_judge():
    rows = _scores([1.0])
    for r in rows:
        r["judge_median"] = None
    agg = report.aggregate_arm("x", rows)
    assert agg["tokens_per_judge_point"] is None


def test_aggregate_handles_skipped_arm():
    agg = report.aggregate_arm("codegraph", rows=[], skipped=True)
    assert agg["skipped"] is True
    assert agg["judge_median"] is None


def test_render_markdown_includes_headline():
    arms_data = {
        "default": report.aggregate_arm("default", _scores([0.5, 0.5])),
        "code_intel_full": report.aggregate_arm("code_intel_full", _scores([0.9, 0.9])),
        "code_intel_shipped": report.aggregate_arm("code_intel_shipped", _scores([0.95, 0.95])),
        "code_intel_no_reranker": report.aggregate_arm("code_intel_no_reranker", _scores([0.85, 0.85])),
        "codegraph": report.aggregate_arm("codegraph", rows=[], skipped=True),
    }
    md = report.render_markdown(
        round_id="R042",
        repos=["smoke"],
        arms_data=arms_data,
        outliers={"high_judge_disagreement": [], "hallucinated_citations": [],
                  "forbidden_hits": [], "regressed_vs_full": []},
        meta={
            "daemon": {"git_sha": "abc", "binary_sha256": "binary123"},
            "binaries": {"agent_cli": "claude 1.2.3", "codegraph": None},
            "models": {
                "agent": "claude-sonnet-4-6",
                "judges": {"haiku": "claude-haiku-4-5"},
            },
            "comparator": {
                "baseline_arm": "default",
                "candidate_arms": ["code_intel_full"],
            },
            "fixtures": [{
                "repo": "smoke",
                "upstream_sha": "fixture-upstream",
                "fixture_sha256": "fixture123",
                "authored_against_schema_version": 22,
                "question_ids": ["q1", "q2"],
            }],
            "configuration": {
                "arms": [{
                    "name": "code_intel_full",
                    "index_variant": "full",
                    "daemon_env": {"RERANKER_ENABLED": "1"},
                }],
            },
        },
    )
    assert "# Bench Round R042" in md
    assert "Headline" in md
    assert "skipped" in md
    assert "code_intel_full" in md
    assert "binary123" in md
    assert "fixture-upstream" in md
    assert "fixture123" in md
    assert "default → code_intel_full" in md


def test_render_markdown_handles_unjudged_multi_arm():
    # Both arms ran (n>0) but judge was skipped (judge_median is None).
    def _unjudged_scores(values):
        return [
            {
                "question_id": f"q{i}",
                "task_type": "symbol_lookup",
                "mech": v,
                # No judge_median / judge_range: aggregate_arm should produce judge_median=None.
                "tool_calls": 2,
                "input_tokens": 50000,
                "wall_ms": 30000,
                "citation_hit": v > 0.6,
                "hallucinated": False,
                "forbidden_hit": False,
            }
            for i, v in enumerate(values, start=1)
        ]

    arms_data = {
        "default": report.aggregate_arm("default", _unjudged_scores([0.5, 0.5])),
        "code_intel_full": report.aggregate_arm("code_intel_full", _unjudged_scores([0.9, 0.9])),
    }
    # Verify aggregate_arm correctly produces judge_median=None when no judge fields are present.
    assert arms_data["default"]["judge_median"] is None
    assert arms_data["code_intel_full"]["judge_median"] is None
    # Should not raise even with un-judged arms.
    md = report.render_markdown(
        round_id="R999",
        repos=["smoke"],
        arms_data=arms_data,
        outliers={
            "high_judge_disagreement": [],
            "hallucinated_citations": [],
            "forbidden_hits": [],
            "regressed_vs_full": [],
        },
        meta={"daemon_sha": "?", "codegraph_version": None, "agent_model": "x"},
    )
    assert "R999" in md
    # Headline should fall back to mech-only line rather than raising or returning empty.
    assert "no judge" in md

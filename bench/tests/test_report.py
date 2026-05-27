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


def test_aggregate_handles_skipped_arm():
    agg = report.aggregate_arm("codegraph", rows=[], skipped=True)
    assert agg["skipped"] is True
    assert agg["judge_median"] is None


def test_render_markdown_includes_headline():
    arms_data = {
        "default": report.aggregate_arm("default", _scores([0.5, 0.5])),
        "code_intel_full": report.aggregate_arm("code_intel_full", _scores([0.9, 0.9])),
        "code_intel_no_descriptions": report.aggregate_arm("code_intel_no_descriptions", _scores([0.95, 0.95])),
        "code_intel_no_reranker": report.aggregate_arm("code_intel_no_reranker", _scores([0.85, 0.85])),
        "codegraph": report.aggregate_arm("codegraph", rows=[], skipped=True),
    }
    md = report.render_markdown(
        round_id="R042",
        repos=["smoke"],
        arms_data=arms_data,
        outliers={"high_judge_disagreement": [], "hallucinated_citations": [],
                  "forbidden_hits": [], "regressed_vs_full": []},
        meta={"daemon_sha": "abc", "codegraph_version": None, "agent_model": "claude-sonnet-4-6"},
    )
    assert "# Bench Round R042" in md
    assert "Headline" in md
    assert "skipped" in md
    assert "code_intel_full" in md

from scripts.agent_qa.report import (
    aggregate_round,
    render_markdown,
    ScoredRun,
)


def _sr(question_id, toolset, repo, mech, judge, tokens, tool_calls, judge_baseline=None):
    return ScoredRun(
        question_id=question_id,
        toolset=toolset,
        repo=repo,
        mech_score=mech,
        judge_score=judge,
        input_tokens=tokens,
        output_tokens=20,
        tool_calls=list(tool_calls),
        wall_ms=1000,
        final_answer="...",
        stop_reason="end_turn",
        judge_baseline_score=judge_baseline,
    )


def test_aggregate_computes_per_toolset_averages():
    runs = [
        _sr("q1", "default", "self", 0.5, 6, 10000, ["read_file", "grep"]),
        _sr("q1", "code_intel", "self", 0.9, 9, 4000, ["ci_search_code"]),
        _sr("q2", "default", "self", 1.0, 8, 12000, ["grep", "grep"]),
        _sr("q2", "code_intel", "self", 1.0, 9, 5000, ["ci_search_code", "ci_get_definition"]),
    ]
    agg = aggregate_round(runs)
    assert agg.per_toolset["default"].avg_mech == 0.75
    assert agg.per_toolset["code_intel"].avg_mech == 0.95
    assert agg.per_toolset["default"].avg_judge == 7.0
    assert agg.per_toolset["code_intel"].avg_judge == 9.0
    assert agg.per_toolset["default"].avg_tokens == 11000
    assert agg.per_toolset["code_intel"].avg_tokens == 4500
    # Tool reach histogram
    assert agg.tool_reach["default"]["grep"] == 3
    assert agg.tool_reach["default"]["read_file"] == 1
    assert agg.tool_reach["code_intel"]["ci_search_code"] == 2
    assert agg.tool_reach["code_intel"]["ci_get_definition"] == 1


def test_aggregate_computes_per_question_deltas():
    runs = [
        _sr("q1", "default", "self", 0.5, 6, 10000, []),
        _sr("q1", "code_intel", "self", 0.9, 9, 4000, []),
    ]
    agg = aggregate_round(runs)
    delta = agg.per_question["q1"]
    assert abs(delta.mech_delta - 0.4) < 1e-9
    assert delta.judge_delta == 3
    assert delta.token_delta == -6000


def test_aggregate_uses_pair_specific_judge_baseline():
    runs = [
        _sr("q1", "default", "self", 1.0, 8, 10000, []),
        _sr("q1", "code_graph", "self", 1.0, 6, 12000, [], judge_baseline=4),
    ]

    agg = aggregate_round(runs)
    delta = agg.per_question_by_pair["code_graph"]["q1"]

    assert delta.judge_delta == 2


def test_render_markdown_includes_headlines():
    runs = [
        _sr("q1", "default", "self", 0.5, 6, 10000, ["grep"]),
        _sr("q1", "code_intel", "self", 1.0, 9, 4000, ["ci_search_code"]),
    ]
    agg = aggregate_round(runs)
    md = render_markdown(round_id=1, repos=["self"], aggregate=agg)
    assert "# Agent Q&A Benchmark Round 1" in md
    assert "default" in md and "code_intel" in md
    assert "q1" in md
    assert "Tool reach" in md

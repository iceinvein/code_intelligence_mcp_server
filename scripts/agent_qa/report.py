"""Aggregate scored runs into per-toolset averages, per-question deltas, and Markdown."""
from __future__ import annotations

from collections import Counter, defaultdict
from dataclasses import dataclass, field
from typing import Dict, List


@dataclass
class ScoredRun:
    question_id: str
    toolset: str  # "default" | "code_intel"
    repo: str
    mech_score: float
    judge_score: int
    input_tokens: int
    output_tokens: int
    tool_calls: List[str]
    wall_ms: int
    final_answer: str
    stop_reason: str


@dataclass
class ToolsetSummary:
    n: int
    avg_mech: float
    avg_judge: float
    avg_tokens: float
    avg_tool_calls: float


@dataclass
class QuestionDelta:
    question_id: str
    repo: str
    mech_delta: float  # code_intel - default
    judge_delta: int
    token_delta: int  # code_intel - default (negative = code_intel cheaper)


@dataclass
class AggregateResult:
    per_toolset: Dict[str, ToolsetSummary]
    per_question: Dict[str, QuestionDelta]
    tool_reach: Dict[str, Dict[str, int]]  # toolset -> tool name -> count


def _summarize(runs: List[ScoredRun]) -> ToolsetSummary:
    n = len(runs)
    if n == 0:
        return ToolsetSummary(0, 0.0, 0.0, 0.0, 0.0)
    return ToolsetSummary(
        n=n,
        avg_mech=sum(r.mech_score for r in runs) / n,
        avg_judge=sum(r.judge_score for r in runs) / n,
        avg_tokens=sum(r.input_tokens for r in runs) / n,
        avg_tool_calls=sum(len(r.tool_calls) for r in runs) / n,
    )


def aggregate_round(runs: List[ScoredRun]) -> AggregateResult:
    by_toolset: Dict[str, List[ScoredRun]] = defaultdict(list)
    for r in runs:
        by_toolset[r.toolset].append(r)

    per_toolset = {ts: _summarize(rs) for ts, rs in by_toolset.items()}

    by_q: Dict[str, Dict[str, ScoredRun]] = defaultdict(dict)
    for r in runs:
        by_q[r.question_id][r.toolset] = r

    per_question: Dict[str, QuestionDelta] = {}
    for qid, ts_map in by_q.items():
        d = ts_map.get("default")
        c = ts_map.get("code_intel")
        if d is None or c is None:
            continue
        per_question[qid] = QuestionDelta(
            question_id=qid,
            repo=d.repo,
            mech_delta=c.mech_score - d.mech_score,
            judge_delta=c.judge_score - d.judge_score,
            token_delta=c.input_tokens - d.input_tokens,
        )

    tool_reach: Dict[str, Dict[str, int]] = {}
    for ts, rs in by_toolset.items():
        ctr: Counter[str] = Counter()
        for r in rs:
            ctr.update(r.tool_calls)
        tool_reach[ts] = dict(ctr)

    return AggregateResult(
        per_toolset=per_toolset,
        per_question=per_question,
        tool_reach=tool_reach,
    )


def render_markdown(round_id: int, repos: List[str], aggregate: AggregateResult) -> str:
    lines: List[str] = []
    lines.append(f"# Agent Q&A Benchmark Round {round_id}")
    lines.append("")
    lines.append(f"**Repos:** {', '.join(repos)}")
    lines.append("")

    lines.append("## Toolset averages")
    lines.append("")
    lines.append("| toolset | n | avg mech | avg judge | avg input tokens | avg tool calls |")
    lines.append("|---|---:|---:|---:|---:|---:|")
    for ts, s in sorted(aggregate.per_toolset.items()):
        lines.append(
            f"| {ts} | {s.n} | {s.avg_mech:.2f} | {s.avg_judge:.2f} | {s.avg_tokens:,.0f} | {s.avg_tool_calls:.1f} |"
        )
    lines.append("")

    lines.append("## Per-question delta (code_intel - default)")
    lines.append("")
    lines.append("| question | repo | mech delta | judge delta | token delta |")
    lines.append("|---|---|---:|---:|---:|")
    for qid, d in sorted(aggregate.per_question.items()):
        lines.append(
            f"| {qid} | {d.repo} | {d.mech_delta:+.2f} | {d.judge_delta:+d} | {d.token_delta:+,} |"
        )
    lines.append("")

    lines.append("## Tool reach")
    lines.append("")
    for ts in sorted(aggregate.tool_reach.keys()):
        lines.append(f"### {ts}")
        lines.append("")
        lines.append("| tool | calls |")
        lines.append("|---|---:|")
        for name, count in sorted(aggregate.tool_reach[ts].items(), key=lambda kv: (-kv[1], kv[0])):
            lines.append(f"| {name} | {count} |")
        lines.append("")
    return "\n".join(lines)

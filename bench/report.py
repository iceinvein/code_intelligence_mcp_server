"""Aggregate raw runs + judge results into a markdown report."""
from __future__ import annotations

import statistics
from typing import Any


def aggregate_arm(arm_name: str, rows: list[dict], skipped: bool = False) -> dict:
    if skipped or not rows:
        return {
            "arm": arm_name,
            "skipped": skipped or not rows,
            "n": 0,
            "judge_median": None,
            "judge_range_mean": None,
            "mech": None,
            "mech_p25": None,
            "citation_hit_rate": None,
            "forbidden_hit_rate": None,
            "hallucinated_citation_rate": None,
            "tool_calls_mean": None,
            "input_tokens_mean": None,
            "wall_seconds_mean": None,
            "tokens_per_judge_point": None,
            "turn_capped_rate": None,
        }
    n = len(rows)
    judge_medians_raw = [r.get("judge_median") for r in rows]
    judge_ranges_raw = [r.get("judge_range") for r in rows]
    judge_medians = [v for v in judge_medians_raw if v is not None]
    judge_ranges = [v for v in judge_ranges_raw if v is not None]
    mechs = [r["mech"] for r in rows]
    input_tokens_mean = sum(r["input_tokens"] for r in rows) / n
    judge_mean = sum(judge_medians) / len(judge_medians) if judge_medians else None
    return {
        "arm": arm_name,
        "skipped": False,
        "n": n,
        "judge_median": judge_mean,
        "judge_range_mean": sum(judge_ranges) / len(judge_ranges) if judge_ranges else None,
        "mech": sum(mechs) / n,
        "mech_p25": statistics.quantiles(mechs, n=4)[0] if n >= 4 else min(mechs),
        "citation_hit_rate": sum(1 for r in rows if r["citation_hit"]) / n,
        "forbidden_hit_rate": sum(1 for r in rows if r.get("forbidden_hit")) / n,
        "hallucinated_citation_rate": sum(1 for r in rows if r.get("hallucinated")) / n,
        "tool_calls_mean": sum(r["tool_calls"] for r in rows) / n,
        "input_tokens_mean": input_tokens_mean,
        "wall_seconds_mean": sum(r["wall_ms"] for r in rows) / 1000 / n,
        "tokens_per_judge_point": (input_tokens_mean / judge_mean
                                   if judge_mean else None),
        "turn_capped_rate": sum(1 for r in rows if r.get("hit_turn_cap")) / n,
    }


def _fmt(v: Any, decimals: int = 2) -> str:
    if v is None:
        return "skipped"
    if isinstance(v, float):
        return f"{v:.{decimals}f}"
    return str(v)


def _has_judge(agg: dict | None) -> bool:
    return bool(agg and not agg["skipped"] and agg.get("judge_median") is not None)


def _build_headline(arms_data: dict[str, dict]) -> str:
    full = arms_data.get("code_intel_full")
    default = arms_data.get("default")
    no_desc = arms_data.get("code_intel_shipped")
    no_rerank = arms_data.get("code_intel_no_reranker")
    codegraph = arms_data.get("codegraph")

    lines = []
    if _has_judge(full) and _has_judge(default):
        dj = full["judge_median"] - default["judge_median"]
        dm = full["mech"] - default["mech"]
        lines.append(f"code_intel_full vs default: {dj:+.1f} judge / {dm:+.2f} mech.")
    if _has_judge(no_desc) and _has_judge(full):
        dj = no_desc["judge_median"] - full["judge_median"]
        lines.append(f"code_intel_shipped vs code_intel_full: {dj:+.1f} judge.")
    if _has_judge(no_rerank) and _has_judge(full):
        dj = no_rerank["judge_median"] - full["judge_median"]
        lines.append(f"code_intel_no_reranker vs code_intel_full: {dj:+.1f} judge.")
    if _has_judge(codegraph) and _has_judge(default):
        dj = codegraph["judge_median"] - default["judge_median"]
        lines.append(f"codegraph vs default: {dj:+.1f} judge.")
    if not lines:
        # No judges available; fall back to mech deltas when both arms ran.
        if full and default and not full["skipped"] and not default["skipped"]:
            dm = full["mech"] - default["mech"]
            lines.append(f"code_intel_full vs default (no judge): {dm:+.2f} mech.")
    return "\n".join(lines) or "(insufficient data for headline)"


def render_markdown(
    *,
    round_id: str,
    repos: list[str],
    arms_data: dict[str, dict],
    outliers: dict[str, list],
    meta: dict,
) -> str:
    lines: list[str] = []
    lines.append(f"# Bench Round {round_id}\n")
    lines.append(
        f"**Repos:** {', '.join(repos)}  **Arms:** {len(arms_data)}  "
        f"**Daemon SHA:** {meta.get('daemon_sha', '?')}  **Codegraph:** {meta.get('codegraph_version') or 'not installed'}  "
        f"**Agent:** {meta.get('agent_model', '?')}\n"
    )
    lines.append("## Headline\n")
    lines.append(_build_headline(arms_data))
    lines.append("")
    lines.append("## Per-arm aggregate\n")
    lines.append("| arm | n | judge | mech | citation | tools | tokens | tok/judge-pt | capped | wall |")
    lines.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for arm_name, agg in arms_data.items():
        if agg["skipped"]:
            lines.append(f"| {arm_name} | 0 | skipped | skipped | skipped | skipped | skipped | skipped | skipped | skipped |")
        else:
            tok_pt = agg.get("tokens_per_judge_point")
            capped = agg.get("turn_capped_rate")
            lines.append(
                f"| {arm_name} | {agg['n']} | "
                f"{_fmt(agg['judge_median'])} ±{_fmt(agg['judge_range_mean'])} | "
                f"{_fmt(agg['mech'])} | "
                f"{_fmt(agg['citation_hit_rate'])} | "
                f"{_fmt(agg['tool_calls_mean'], 1)} | "
                f"{int(agg['input_tokens_mean']):,} | "
                f"{'n/a' if tok_pt is None else f'{int(tok_pt):,}'} | "
                f"{'n/a' if capped is None else f'{capped:.0%}'} | "
                f"{_fmt(agg['wall_seconds_mean'], 0)}s |"
            )
    lines.append("")
    lines.append("## Failures worth inspecting\n")
    for category, items in outliers.items():
        lines.append(f"### {category}")
        if not items:
            lines.append("- (none)")
        else:
            for item in items:
                lines.append(f"- {item}")
        lines.append("")
    return "\n".join(lines)

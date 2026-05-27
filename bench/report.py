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
        }
    n = len(rows)
    judge_medians = [r["judge_median"] for r in rows]
    judge_ranges = [r["judge_range"] for r in rows]
    mechs = [r["mech"] for r in rows]
    return {
        "arm": arm_name,
        "skipped": False,
        "n": n,
        "judge_median": sum(judge_medians) / n,
        "judge_range_mean": sum(judge_ranges) / n,
        "mech": sum(mechs) / n,
        "mech_p25": statistics.quantiles(mechs, n=4)[0] if n >= 4 else min(mechs),
        "citation_hit_rate": sum(1 for r in rows if r["citation_hit"]) / n,
        "forbidden_hit_rate": sum(1 for r in rows if r.get("forbidden_hit")) / n,
        "hallucinated_citation_rate": sum(1 for r in rows if r.get("hallucinated")) / n,
        "tool_calls_mean": sum(r["tool_calls"] for r in rows) / n,
        "input_tokens_mean": sum(r["input_tokens"] for r in rows) / n,
        "wall_seconds_mean": sum(r["wall_ms"] for r in rows) / 1000 / n,
    }


def _fmt(v: Any, decimals: int = 2) -> str:
    if v is None:
        return "skipped"
    if isinstance(v, float):
        return f"{v:.{decimals}f}"
    return str(v)


def _build_headline(arms_data: dict[str, dict]) -> str:
    full = arms_data.get("code_intel_full")
    default = arms_data.get("default")
    no_desc = arms_data.get("code_intel_no_descriptions")
    no_rerank = arms_data.get("code_intel_no_reranker")
    codegraph = arms_data.get("codegraph")

    lines = []
    if full and default and not full["skipped"] and not default["skipped"]:
        dj = full["judge_median"] - default["judge_median"]
        dm = full["mech"] - default["mech"]
        lines.append(f"code_intel_full vs default: {dj:+.1f} judge / {dm:+.2f} mech.")
    if no_desc and full and not no_desc["skipped"] and not full["skipped"]:
        dj = no_desc["judge_median"] - full["judge_median"]
        lines.append(f"code_intel_no_descriptions vs code_intel_full: {dj:+.1f} judge.")
    if no_rerank and full and not no_rerank["skipped"] and not full["skipped"]:
        dj = no_rerank["judge_median"] - full["judge_median"]
        lines.append(f"code_intel_no_reranker vs code_intel_full: {dj:+.1f} judge.")
    if codegraph and default and not codegraph["skipped"] and not default["skipped"]:
        dj = codegraph["judge_median"] - default["judge_median"]
        lines.append(f"codegraph vs default: {dj:+.1f} judge.")
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
    lines.append("| arm | n | judge | mech | citation | tools | tokens | wall |")
    lines.append("|---|---:|---:|---:|---:|---:|---:|---:|")
    for arm_name, agg in arms_data.items():
        if agg["skipped"]:
            lines.append(f"| {arm_name} | 0 | skipped | skipped | skipped | skipped | skipped | skipped |")
        else:
            lines.append(
                f"| {arm_name} | {agg['n']} | "
                f"{_fmt(agg['judge_median'])} ±{_fmt(agg['judge_range_mean'])} | "
                f"{_fmt(agg['mech'])} | "
                f"{_fmt(agg['citation_hit_rate'])} | "
                f"{_fmt(agg['tool_calls_mean'], 1)} | "
                f"{int(agg['input_tokens_mean']):,} | "
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

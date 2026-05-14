#!/usr/bin/env python3
"""Path 2 evaluation: R9896 (evidence-only ask_code) vs R007 (broken) and R9899 (1.5B fix)."""
from __future__ import annotations

import json
import random
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT))

from scripts.agent_qa.qa_schema import load_qa_set
from scripts.agent_qa.claude_cli import run_one_shot
from scripts.agent_qa.judge import judge_pair


def _load_run(path: Path, qid: str, toolset: str) -> dict:
    d = json.loads(path.read_text())
    for r in d["runs"]:
        if r["question_id"] == qid and r["toolset"] == toolset:
            return r
    raise KeyError(f"{qid}/{toolset} not in {path}")


def _summarize_tools(rec: dict) -> str:
    tcs = rec["tool_calls"]
    mcp = sum(1 for t in tcs if "mcp__code-intelligence" in t["name"])
    gr = sum(1 for t in tcs if t["name"] in ("Grep", "Read", "Glob"))
    return f"tot={len(tcs):>2} mcp={mcp} g+r={gr}"


def main() -> None:
    import os
    judge_model = os.environ.get("JUDGE_MODEL", "claude-haiku-4-5-20251001")

    qa = {e.id: e for e in load_qa_set(REPO_ROOT / "scripts/queries_qa_self.json")}
    qs = ["self-q1", "self-q8", "self-q12", "self-q14", "self-q15"]

    conds = {
        "R007 (1.5B broken)":      ("docs/benchmark_rounds/agent/R007.json", "code_intel"),
        "R9899 (1.5B fix synth)":  ("docs/benchmark_rounds/agent/R9899.json", "code_intel"),
        "R9897 (3B fix synth)":    ("docs/benchmark_rounds/agent/R9897.json", "code_intel"),
        "R9896 (evidence-only)":   ("docs/benchmark_rounds/agent/R9896.json", "code_intel"),
    }

    pairs = [
        ("R007 (1.5B broken)",     "R9896 (evidence-only)"),
        ("R9899 (1.5B fix synth)", "R9896 (evidence-only)"),
        ("R9897 (3B fix synth)",   "R9896 (evidence-only)"),
    ]

    print("=== tool-call summary ===\n")
    print(f"{'q':<10} " + "  ".join(f"{c:<24}" for c in conds))
    for q in qs:
        row = [f"{q:<10}"]
        for label, (p, ts) in conds.items():
            rec = _load_run(REPO_ROOT / p, q, ts)
            row.append(f"  {_summarize_tools(rec):<24}")
        print("".join(row))

    print("\n=== pairwise judge ===\n")
    summary = []
    for left_label, right_label in pairs:
        left_path, left_ts = conds[left_label]
        right_path, right_ts = conds[right_label]
        print(f"\n[{left_label}]  vs  [{right_label}]")
        print(f"{'q':<10} {'left':>5} {'right':>6} {'delta':>6}")
        totals = {"l": 0, "r": 0, "n": 0}
        for q in qs:
            entry = qa[q]
            left = _load_run(REPO_ROOT / left_path, q, left_ts)["final_answer"]
            right = _load_run(REPO_ROOT / right_path, q, right_ts)["final_answer"]
            seed = random.randint(0, 1)
            try:
                jr = judge_pair(
                    complete_fn=lambda system, user: run_one_shot(
                        prompt=user, system_prompt=system, model=judge_model
                    ),
                    question=entry.question,
                    rubric=entry.rubric,
                    default_answer=left,
                    code_intel_answer=right,
                    seed=seed,
                )
                l_s = jr.default_score
                r_s = jr.code_intel_score
                print(f"{q:<10} {l_s:>5} {r_s:>6} {r_s - l_s:>+6}")
                totals["l"] += l_s
                totals["r"] += r_s
                totals["n"] += 1
            except Exception as e:
                print(f"{q:<10} judge failed: {e}")
        if totals["n"]:
            avg_l = totals["l"] / totals["n"]
            avg_r = totals["r"] / totals["n"]
            delta = avg_r - avg_l
            print(f"{'avg':<10} {avg_l:>5.2f} {avg_r:>6.2f} {delta:>+6.2f}")
            summary.append((left_label, right_label, avg_l, avg_r, delta))

    print("\n=== final summary ===")
    for left, right, l, r, d in summary:
        print(f"  {left:<26} -> {right:<26}  avg {l:.2f} -> {r:.2f}  delta {d:+.2f}")


if __name__ == "__main__":
    main()

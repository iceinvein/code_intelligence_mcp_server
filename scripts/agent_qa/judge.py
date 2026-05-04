"""LLM-as-judge that scores two answers (anonymized as A/B) against a rubric.

The judge is decoupled from any particular client: callers pass a `complete_fn`
of shape `(system, user) -> str`. The bench CLI wires this to
`scripts.agent_qa.claude_cli.run_one_shot` so the judge runs through the same
Claude Code session as the agent.
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass
from typing import Any, Callable


CompleteFn = Callable[[str, str], str]


JUDGE_SYSTEM = (
    "You are a strict grader. Score two answers (A and B) against the rubric on a 0-10 "
    "scale where 0 is wrong/missing and 10 is fully correct and well-explained. Reward "
    "concise correct answers over verbose padded ones. Penalize hallucinated file paths "
    "or symbol names. Respond with ONLY a single JSON object on one line: "
    '{"A_score": int, "B_score": int, "A_justification": "...", "B_justification": "..."}.'
)


@dataclass
class ParsedJudgement:
    a_score: int
    b_score: int
    a_justification: str
    b_justification: str


@dataclass
class JudgeResult:
    default_score: int
    code_intel_score: int
    default_justification: str
    code_intel_justification: str
    raw_response: str


def build_judge_prompt(question: str, rubric: str, answer_a: str, answer_b: str) -> str:
    return (
        f"QUESTION:\n{question}\n\n"
        f"RUBRIC:\n{rubric}\n\n"
        f"ANSWER A:\n{answer_a}\n\n"
        f"ANSWER B:\n{answer_b}\n\n"
        "Output the JSON object now."
    )


def parse_judge_response(raw: str) -> ParsedJudgement:
    match = re.search(r"\{[^{}]*\}", raw, re.DOTALL)
    if not match:
        raise ValueError(f"no JSON object in judge response: {raw!r}")
    obj = json.loads(match.group(0))

    def _clamp(v: Any) -> int:
        try:
            n = int(v)
        except (TypeError, ValueError):
            n = 0
        return max(0, min(10, n))

    return ParsedJudgement(
        a_score=_clamp(obj.get("A_score", 0)),
        b_score=_clamp(obj.get("B_score", 0)),
        a_justification=str(obj.get("A_justification", "")),
        b_justification=str(obj.get("B_justification", "")),
    )


def judge_pair(
    complete_fn: CompleteFn,
    question: str,
    rubric: str,
    default_answer: str,
    code_intel_answer: str,
    seed: int,
) -> JudgeResult:
    """seed=0: A=default, B=code_intel. seed=1: A=code_intel, B=default."""
    if seed % 2 == 0:
        answer_a, answer_b = default_answer, code_intel_answer
        a_label, b_label = "default", "code_intel"
    else:
        answer_a, answer_b = code_intel_answer, default_answer
        a_label, b_label = "code_intel", "default"

    prompt = build_judge_prompt(question, rubric, answer_a, answer_b)
    raw = complete_fn(JUDGE_SYSTEM, prompt)
    parsed = parse_judge_response(raw)

    scores = {a_label: parsed.a_score, b_label: parsed.b_score}
    justs = {a_label: parsed.a_justification, b_label: parsed.b_justification}
    return JudgeResult(
        default_score=scores["default"],
        code_intel_score=scores["code_intel"],
        default_justification=justs["default"],
        code_intel_justification=justs["code_intel"],
        raw_response=raw,
    )

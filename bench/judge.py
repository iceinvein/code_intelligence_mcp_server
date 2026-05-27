"""Multi-judge wrapper: each (arm, question) scored by 3 judges; median + range reported."""
from __future__ import annotations

import json
import re
import statistics
from dataclasses import dataclass, field
from typing import Any

from bench import config


JUDGE_SYSTEM = (
    "You are a strict grader. Score this answer against the rubric on a 0-10 scale.\n"
    "0 = wrong or missing. 10 = fully correct and well-explained.\n"
    "Reward concise correct answers; penalize verbose padded ones.\n"
    "Penalize hallucinated file paths, missing citations, vague hand-waving.\n"
    "Respond with one JSON object: {\"score\": int, \"justification\": \"...\"}."
)


@dataclass
class JudgeResult:
    model: str
    question_id: str
    score: int
    justification: str
    raw_response: str
    error: str | None = None


@dataclass
class JudgeAggregate:
    question_id: str
    scores: dict[str, int]  # "haiku" | "sonnet" | "opus" -> score
    justifications: dict[str, str]
    median: float
    range: int


_JSON_BLOCK = re.compile(r"\{[^{}]*\"score\"[^{}]*\}", re.DOTALL)


def _build_user_prompt(
    question_id: str,
    question: str,
    rubric: str,
    citations: list[dict],
    mech_context: dict,
    answer: str,
) -> str:
    citations_block = "\n".join(
        f"  - {c.get('file', '?')}:{c.get('line_range', ['?', '?'])} symbol={c.get('symbol', '?')}"
        for c in citations
    ) or "  (none)"
    mech_block = "\n".join(f"  - {k}: {v}" for k, v in mech_context.items()) or "  (none computed)"
    return (
        f"QUESTION_ID: {question_id}\n\n"
        f"QUESTION:\n{question}\n\n"
        f"RUBRIC:\n{rubric}\n\n"
        f"EXPECTED CITATIONS (canonical correct answers):\n{citations_block}\n\n"
        f"MECHANICAL CONTEXT (computed before judging):\n{mech_block}\n\n"
        f"ANSWER:\n{answer}\n\n"
        "Output the JSON now."
    )


def _parse_response(raw: str) -> tuple[int, str]:
    m = _JSON_BLOCK.search(raw)
    if not m:
        raise ValueError(f"no JSON object in judge response: {raw!r}")
    obj = json.loads(m.group(0))
    score = max(0, min(10, int(obj.get("score", 0))))
    just = str(obj.get("justification", ""))
    return score, just


def judge_one(
    *,
    client: Any,
    model: str,
    question_id: str,
    question: str,
    rubric: str,
    citations: list[dict],
    mech_context: dict,
    answer: str,
    max_retries: int = 1,
) -> JudgeResult:
    user_prompt = _build_user_prompt(question_id, question, rubric, citations, mech_context, answer)
    last_raw = ""
    last_err = ""
    for attempt in range(max_retries + 1):
        try:
            response = client.messages.create(
                model=model,
                max_tokens=1024,
                system=JUDGE_SYSTEM if attempt == 0 else JUDGE_SYSTEM + "\n\nIMPORTANT: respond with valid JSON only.",
                messages=[{"role": "user", "content": user_prompt}],
            )
            last_raw = response.content[0].text
            score, just = _parse_response(last_raw)
            return JudgeResult(
                model=model,
                question_id=question_id,
                score=score,
                justification=just,
                raw_response=last_raw,
            )
        except (ValueError, json.JSONDecodeError) as e:
            last_err = str(e)
            continue
        except Exception as e:
            last_err = f"api_error: {e}"
            break
    return JudgeResult(
        model=model,
        question_id=question_id,
        score=0,
        justification="",
        raw_response=last_raw,
        error=last_err or "parse_failed",
    )


def judge_all(
    *,
    client: Any,
    question_id: str,
    question: str,
    rubric: str,
    citations: list[dict],
    mech_context: dict,
    answer: str,
) -> JudgeAggregate:
    """Run 3 judges and aggregate to median + range."""
    judges = {
        "haiku": config.JUDGE_HAIKU,
        "sonnet": config.JUDGE_SONNET,
        "opus": config.JUDGE_OPUS,
    }
    results: dict[str, JudgeResult] = {}
    for label, model in judges.items():
        results[label] = judge_one(
            client=client,
            model=model,
            question_id=question_id,
            question=question,
            rubric=rubric,
            citations=citations,
            mech_context=mech_context,
            answer=answer,
        )

    scores = {label: r.score for label, r in results.items()}
    justifications = {label: r.justification for label, r in results.items()}
    median = statistics.median(scores.values())
    rng = max(scores.values()) - min(scores.values())
    return JudgeAggregate(
        question_id=question_id,
        scores=scores,
        justifications=justifications,
        median=median,
        range=rng,
    )

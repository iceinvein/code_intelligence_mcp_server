"""Multi-judge wrapper: each (arm, question) scored by 3 judges; median + range reported."""
from __future__ import annotations

import json
import statistics
import subprocess
from dataclasses import dataclass, field

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


def _extract_json_object(text: str) -> dict:
    """Return the first balanced ``{...}`` object containing a ``score`` key.

    A plain regex cannot do this: judge justifications routinely contain literal
    braces (e.g. ``injects { user, session } into context``) and the models often
    wrap the object in a ```json``` fence. This does a string-aware balanced-brace
    scan so braces and quotes *inside* string values do not terminate the object,
    then falls through to the next ``{`` if a candidate fails to parse or lacks a
    score (e.g. a brace that opens inside prose before the real object).
    """
    s = text.strip()
    for start, ch0 in enumerate(s):
        if ch0 != "{":
            continue
        depth = 0
        in_str = False
        esc = False
        for i in range(start, len(s)):
            ch = s[i]
            if in_str:
                if esc:
                    esc = False
                elif ch == "\\":
                    esc = True
                elif ch == '"':
                    in_str = False
                continue
            if ch == '"':
                in_str = True
            elif ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    try:
                        obj = json.loads(s[start:i + 1])
                    except json.JSONDecodeError:
                        break  # malformed candidate; try the next '{'
                    if isinstance(obj, dict) and "score" in obj:
                        return obj
                    break  # parsed but not our object; try the next '{'
    raise ValueError(f"no JSON object with score in judge response: {text!r}")


def _parse_response(raw: str) -> tuple[int, str]:
    obj = _extract_json_object(raw)
    score = max(0, min(10, int(obj.get("score", 0))))
    just = str(obj.get("justification", ""))
    return score, just


def _extract_text_from_claude_output(stdout_bytes: bytes) -> str:
    """Parse the single-JSON output from `claude --print --output-format json`.

    The CLI emits: {"type":"result","subtype":"success",...,"result":"<text>",...}
    Returns the text content. Falls back gracefully for unexpected shapes.
    """
    raw = stdout_bytes.decode("utf-8", errors="replace").strip()
    if not raw:
        return ""
    try:
        obj = json.loads(raw)
    except json.JSONDecodeError:
        # Not JSON: return raw text directly (may still contain the score block)
        return raw

    # Primary shape (claude --print --output-format json): {"result": "..."}
    if isinstance(obj, dict) and "result" in obj:
        return str(obj["result"])
    # Alternate shape: {"text": "..."}
    if isinstance(obj, dict) and "text" in obj:
        return str(obj["text"])
    # Alternate shape: {"content": [{"type": "text", "text": "..."}]}
    if isinstance(obj, dict) and "content" in obj and isinstance(obj["content"], list):
        for block in obj["content"]:
            if isinstance(block, dict) and block.get("type") == "text":
                return str(block.get("text", ""))
    # List of text items
    if isinstance(obj, list):
        for item in obj:
            if isinstance(item, dict) and "text" in item:
                return str(item["text"])
    return raw  # last resort


def judge_one(
    *,
    model: str,
    question_id: str,
    question: str,
    rubric: str,
    citations: list[dict],
    mech_context: dict,
    answer: str,
    max_retries: int = 1,
    cwd: str | None = None,
) -> JudgeResult:
    user_prompt = _build_user_prompt(question_id, question, rubric, citations, mech_context, answer)
    cwd = cwd or str(config.REPO_ROOT)

    last_raw = ""
    last_err = ""
    system_prompt = JUDGE_SYSTEM
    for attempt in range(max_retries + 1):
        if attempt > 0:
            system_prompt = JUDGE_SYSTEM + "\n\nIMPORTANT: respond with valid JSON only."
        try:
            result = subprocess.run(
                [
                    config.CLAUDE_BINARY, "--print",
                    "--model", model,
                    "--system-prompt", system_prompt,
                    "--allowed-tools", "",
                    "--output-format", "json",
                ],
                input=user_prompt.encode(),
                capture_output=True,
                timeout=120,
                cwd=cwd,
            )
            last_raw = _extract_text_from_claude_output(result.stdout)
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
        except subprocess.TimeoutExpired as e:
            last_err = f"timeout: {e}"
            break
        except Exception as e:
            last_err = f"cli_error: {e}"
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
    question_id: str,
    question: str,
    rubric: str,
    citations: list[dict],
    mech_context: dict,
    answer: str,
    cwd: str | None = None,
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
            model=model,
            question_id=question_id,
            question=question,
            rubric=rubric,
            citations=citations,
            mech_context=mech_context,
            answer=answer,
            cwd=cwd,
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

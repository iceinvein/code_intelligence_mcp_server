from scripts.agent_qa.judge import (
    build_judge_prompt,
    parse_judge_response,
    judge_pair,
    JUDGE_SYSTEM,
)


def test_prompt_includes_question_rubric_and_both_answers():
    prompt = build_judge_prompt(
        question="Where is X?",
        rubric="Cite src/foo.rs.",
        answer_a="A says src/foo.rs.",
        answer_b="B says src/bar.rs.",
    )
    assert "Where is X?" in prompt
    assert "Cite src/foo.rs." in prompt
    assert "A says src/foo.rs." in prompt
    assert "B says src/bar.rs." in prompt
    assert "JSON" in prompt


def test_parse_extracts_scores_and_justifications():
    raw = """Some chatter before.
{"A_score": 8, "B_score": 5, "A_justification": "names file", "B_justification": "wrong file"}
trailing"""
    parsed = parse_judge_response(raw)
    assert parsed.a_score == 8
    assert parsed.b_score == 5
    assert "names file" in parsed.a_justification
    assert "wrong file" in parsed.b_justification


def test_parse_clamps_to_0_10():
    raw = '{"A_score": 12, "B_score": -3, "A_justification": "x", "B_justification": "y"}'
    parsed = parse_judge_response(raw)
    assert parsed.a_score == 10
    assert parsed.b_score == 0


def _fake_complete(text: str):
    """Return a complete_fn that always emits the given text regardless of input."""
    def _fn(_system: str, _user: str) -> str:
        return text
    return _fn


def test_judge_pair_de_anonymizes_with_seed_zero():
    text = '{"A_score": 9, "B_score": 4, "A_justification": "good", "B_justification": "bad"}'
    result = judge_pair(
        complete_fn=_fake_complete(text),
        question="q",
        rubric="r",
        default_answer="DEFAULT_TEXT",
        code_intel_answer="CI_TEXT",
        seed=0,  # deterministic ordering: A=default, B=code_intel
    )
    assert result.default_score == 9
    assert result.code_intel_score == 4
    assert result.default_justification == "good"
    assert result.code_intel_justification == "bad"


def test_judge_pair_de_anonymizes_with_seed_one():
    # seed=1 swaps so A=code_intel, B=default
    text = '{"A_score": 9, "B_score": 4, "A_justification": "good", "B_justification": "bad"}'
    result = judge_pair(
        complete_fn=_fake_complete(text),
        question="q",
        rubric="r",
        default_answer="DEFAULT_TEXT",
        code_intel_answer="CI_TEXT",
        seed=1,
    )
    assert result.code_intel_score == 9
    assert result.default_score == 4
    assert result.code_intel_justification == "good"
    assert result.default_justification == "bad"


def test_judge_pair_passes_prompt_components_to_complete_fn():
    """The complete_fn should receive the JUDGE_SYSTEM as system and the
    rendered prompt (containing question/rubric/both answers) as user."""
    seen: dict = {}

    def _fn(system: str, user: str) -> str:
        seen["system"] = system
        seen["user"] = user
        return '{"A_score": 5, "B_score": 5, "A_justification": "x", "B_justification": "y"}'

    judge_pair(
        complete_fn=_fn,
        question="MY_QUESTION",
        rubric="MY_RUBRIC",
        default_answer="DEF",
        code_intel_answer="CI",
        seed=0,
    )
    assert seen["system"] == JUDGE_SYSTEM
    assert "MY_QUESTION" in seen["user"]
    assert "MY_RUBRIC" in seen["user"]
    assert "DEF" in seen["user"]
    assert "CI" in seen["user"]


def test_judge_system_is_concise_and_instructive():
    assert "concise" in JUDGE_SYSTEM.lower()
    assert "JSON" in JUDGE_SYSTEM

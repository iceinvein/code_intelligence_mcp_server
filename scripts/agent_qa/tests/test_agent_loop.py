from typing import Any, Dict, List

from scripts.agent_qa.agent_loop import (
    Toolbox,
    ToolDispatch,
    run_agent,
    SYSTEM_PROMPT,
)


class FakeUsage:
    def __init__(self, in_t: int, out_t: int):
        self.input_tokens = in_t
        self.output_tokens = out_t


class FakeTextBlock:
    type = "text"

    def __init__(self, text: str):
        self.text = text


class FakeToolUseBlock:
    type = "tool_use"

    def __init__(self, tu_id: str, name: str, input: Dict[str, Any]):
        self.id = tu_id
        self.name = name
        self.input = input


class FakeMessage:
    def __init__(self, content: List[Any], usage: FakeUsage, stop_reason: str):
        self.content = content
        self.usage = usage
        self.stop_reason = stop_reason


class FakeMessages:
    def __init__(self, scripted: List[FakeMessage]):
        self._scripted = scripted
        self.calls = 0

    def create(self, **_kwargs):
        msg = self._scripted[self.calls]
        self.calls += 1
        return msg


class FakeAnthropic:
    def __init__(self, scripted: List[FakeMessage]):
        self.messages = FakeMessages(scripted)


def _dispatch_grep(name: str, args: Dict[str, Any]) -> str:
    if name == "grep":
        return "src/foo.rs:10:fn alpha"
    raise RuntimeError(f"unexpected tool: {name}")


def test_loop_executes_tool_then_returns_final_answer():
    scripted = [
        FakeMessage(
            content=[
                FakeTextBlock("I will search."),
                FakeToolUseBlock("t1", "grep", {"pattern": "alpha"}),
            ],
            usage=FakeUsage(1000, 50),
            stop_reason="tool_use",
        ),
        FakeMessage(
            content=[FakeTextBlock("Found in src/foo.rs:10.")],
            usage=FakeUsage(1500, 30),
            stop_reason="end_turn",
        ),
    ]
    toolbox = Toolbox(
        tool_defs=[{"name": "grep", "description": "x", "input_schema": {"type": "object"}}],
        dispatch=_dispatch_grep,
    )
    record = run_agent(
        client=FakeAnthropic(scripted),
        model="fake-model",
        question="Where is alpha?",
        toolbox=toolbox,
    )
    assert record.final_answer == "Found in src/foo.rs:10."
    assert record.input_tokens == 2500
    assert record.output_tokens == 80
    assert record.stop_reason == "end_turn"
    assert len(record.tool_calls) == 1
    assert record.tool_calls[0].name == "grep"
    assert record.tool_calls[0].args == {"pattern": "alpha"}
    assert "src/foo.rs" in record.tool_calls[0].result_text


def test_loop_marks_dispatch_error_as_tool_error():
    def bad_dispatch(name: str, args: Dict[str, Any]) -> str:
        raise RuntimeError("boom")

    scripted = [
        FakeMessage(
            content=[FakeToolUseBlock("t1", "grep", {})],
            usage=FakeUsage(100, 10),
            stop_reason="tool_use",
        ),
        FakeMessage(
            content=[FakeTextBlock("aborting")],
            usage=FakeUsage(150, 5),
            stop_reason="end_turn",
        ),
    ]
    toolbox = Toolbox(
        tool_defs=[{"name": "grep", "description": "x", "input_schema": {"type": "object"}}],
        dispatch=bad_dispatch,
    )
    record = run_agent(
        client=FakeAnthropic(scripted),
        model="fake-model",
        question="q",
        toolbox=toolbox,
    )
    assert record.tool_calls[0].is_error is True
    assert "boom" in record.tool_calls[0].result_text


def test_system_prompt_is_concise_and_instructive():
    assert "cite" in SYSTEM_PROMPT.lower()
    assert "tools" in SYSTEM_PROMPT.lower()
    assert len(SYSTEM_PROMPT) < 2000

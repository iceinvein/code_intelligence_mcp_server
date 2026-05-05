"""Per-run record schema. Serialized to RNNN.json by the CLI."""
from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Any, Dict, List


@dataclass
class ToolCallRecord:
    name: str
    args: Dict[str, Any]
    result_text: str
    result_bytes: int
    duration_ms: int
    is_error: bool = False

    def to_dict(self) -> dict:
        return asdict(self)


@dataclass
class RunRecord:
    question_id: str
    toolset: str  # "default" or "code_intel"
    model: str
    repo: str
    final_answer: str
    input_tokens: int  # uncached prompt tokens
    output_tokens: int
    wall_ms: int
    stop_reason: str
    cache_creation_input_tokens: int = 0  # tokens written to prompt cache this run
    cache_read_input_tokens: int = 0  # tokens read from prompt cache this run
    tool_calls: List[ToolCallRecord] = field(default_factory=list)

    @property
    def total_input_tokens(self) -> int:
        """All input tokens the model saw, including cache hits and writes."""
        return self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens

    def to_dict(self) -> dict:
        d = asdict(self)
        d["total_input_tokens"] = self.total_input_tokens
        d["tool_calls"] = [tc.to_dict() if isinstance(tc, ToolCallRecord) else tc for tc in self.tool_calls]
        return d

"""Arm definitions: the five rows in the benchmark cross-product.

Each arm has:
- daemon_env:    env vars set on the daemon process (empty for arms without daemon)
- needs_daemon:  True if our code-intelligence daemon must be started for this arm
- is_codegraph:  True for the codegraph competitor arm
- index_variant: which pre-built index variant to use (full | no_desc | None)
- allowed_tools: tool allowlist for claude --print
- tool_guidance: single paragraph appended to the system prompt
"""
from __future__ import annotations

from dataclasses import dataclass, field


# Code-intelligence MCP tools that the agent may call. Keep aligned with what
# the daemon exposes via the MCP protocol.
CODE_INTEL_MCP_TOOLS = [
    "mcp__code-intelligence__ask_code",
    "mcp__code-intelligence__investigate",
    "mcp__code-intelligence__search_code",
    "mcp__code-intelligence__get_definition",
    "mcp__code-intelligence__find_references",
    "mcp__code-intelligence__get_call_hierarchy",
    "mcp__code-intelligence__find_affected_code",
    "mcp__code-intelligence__trace_data_flow",
    "mcp__code-intelligence__explore_dependency_graph",
    "mcp__code-intelligence__bind_workspace",
    "mcp__code-intelligence__hydrate_symbols",
]

# Codegraph MCP tools (per codegraph's documented tool surface).
CODEGRAPH_MCP_TOOLS = [
    "mcp__codegraph__codegraph_search",
    "mcp__codegraph__codegraph_context",
    "mcp__codegraph__codegraph_trace",
    "mcp__codegraph__codegraph_callers",
    "mcp__codegraph__codegraph_callees",
    "mcp__codegraph__codegraph_impact",
    "mcp__codegraph__codegraph_explore",
    "mcp__codegraph__codegraph_node",
    "mcp__codegraph__codegraph_files",
]

DEFAULT_TOOLS = ["Read", "Grep", "Glob", "Bash"]


@dataclass(frozen=True)
class Arm:
    name: str
    needs_daemon: bool
    is_codegraph: bool
    index_variant: str | None  # "full" | "no_desc" | None
    daemon_env: dict[str, str] = field(default_factory=dict)
    allowed_tools: list[str] = field(default_factory=list)
    tool_guidance: str = ""


ARMS: dict[str, Arm] = {
    "default": Arm(
        name="default",
        needs_daemon=False,
        is_codegraph=False,
        index_variant=None,
        daemon_env={},
        allowed_tools=list(DEFAULT_TOOLS),
        tool_guidance="Use Grep/Glob to locate files, Read to inspect them. Bash for git/find.",
    ),
    "code_intel_full": Arm(
        name="code_intel_full",
        needs_daemon=True,
        is_codegraph=False,
        index_variant="full",
        daemon_env={},
        allowed_tools=list(DEFAULT_TOOLS) + list(CODE_INTEL_MCP_TOOLS),
        tool_guidance=(
            "Start with `mcp__code-intelligence__ask_code` for codebase questions. "
            "Fall back to Read/Grep when results are insufficient."
        ),
    ),
    "code_intel_no_descriptions": Arm(
        name="code_intel_no_descriptions",
        needs_daemon=True,
        is_codegraph=False,
        index_variant="no_desc",
        daemon_env={"BENCH_DISABLE_DESCRIPTIONS": "1"},
        allowed_tools=list(DEFAULT_TOOLS) + list(CODE_INTEL_MCP_TOOLS),
        tool_guidance=(
            "Start with `mcp__code-intelligence__ask_code` for codebase questions. "
            "Fall back to Read/Grep when results are insufficient."
        ),
    ),
    "code_intel_no_reranker": Arm(
        name="code_intel_no_reranker",
        needs_daemon=True,
        is_codegraph=False,
        index_variant="full",
        daemon_env={"BENCH_DISABLE_RERANKER": "1"},
        allowed_tools=list(DEFAULT_TOOLS) + list(CODE_INTEL_MCP_TOOLS),
        tool_guidance=(
            "Start with `mcp__code-intelligence__ask_code` for codebase questions. "
            "Fall back to Read/Grep when results are insufficient."
        ),
    ),
    "codegraph": Arm(
        name="codegraph",
        needs_daemon=False,
        is_codegraph=True,
        index_variant=None,
        daemon_env={},
        allowed_tools=list(DEFAULT_TOOLS) + list(CODEGRAPH_MCP_TOOLS),
        tool_guidance=(
            "Start with `mcp__codegraph__codegraph_context` for codebase questions. "
            "Fall back to Read/Grep when results are insufficient."
        ),
    ),
}


def distinct_index_variants(arms: list[Arm]) -> set[str]:
    """Return the set of index variants required by the given arms (excludes None)."""
    return {a.index_variant for a in arms if a.index_variant is not None}

"""Arm definitions: the rows in the benchmark cross-product.

Each arm has:
- daemon_env:    env vars set on the daemon process (empty for arms without daemon)
- needs_daemon:  True if our code-intelligence daemon must be started for this arm
- is_codegraph:  True for the codegraph competitor arm
- index_variant: which pre-built index variant to use (full | no_desc | external | None)
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
    index_variant: str | None  # "full" | "no_desc" | "external" | None
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
        # Reranker ships off by default (RERANKER_ENABLED defaults false); the
        # full arm opts in so the cross-encoder actually reorders results. This
        # is the only arm with the reranker live, making full vs no_reranker a
        # real A/B on the reranker effect.
        daemon_env={"RERANKER_ENABLED": "1"},
        allowed_tools=list(DEFAULT_TOOLS) + list(CODE_INTEL_MCP_TOOLS),
        tool_guidance=(
            "Start with `mcp__code-intelligence__ask_code` for codebase questions. "
            "Fall back to Read/Grep when results are insufficient."
        ),
    ),
    "code_intel_shipped": Arm(
        name="code_intel_shipped",
        needs_daemon=True,
        is_codegraph=False,
        index_variant="no_desc",
        # Production-default config: descriptions off (DESCRIPTIONS_ENABLED=false in
        # prod, judged neutral in R005) and reranker off (RERANKER_ENABLED=false). The
        # no_desc index is plain Tree-sitter; the provenance-overlay schema is present
        # but inert (the bundled producers are stubs, so zero external rows are
        # imported). BENCH_DISABLE_DESCRIPTIONS forces the description field empty at
        # write and query so the index matches what ships. This is the arm that
        # measures whether the shipped product improved or regressed. Cross-round
        # baseline: R005 code_intel_no_descriptions (same config, pre-evidence-pack).
        daemon_env={"BENCH_DISABLE_DESCRIPTIONS": "1"},
        allowed_tools=list(DEFAULT_TOOLS) + list(CODE_INTEL_MCP_TOOLS),
        tool_guidance=(
            "Start with `mcp__code-intelligence__ask_code` for codebase questions. "
            "Fall back to Read/Grep when results are insufficient."
        ),
    ),
    "code_intel_external": Arm(
        name="code_intel_external",
        needs_daemon=True,
        is_codegraph=False,
        index_variant="external",
        # R007 production-default baseline plus explicit external producer execution.
        # Do not set EXTERNAL_INDEX_PRODUCER here; the daemon detects the project
        # languages and selects the matching shipped producers.
        daemon_env={
            "BENCH_DISABLE_DESCRIPTIONS": "1",
            "DESCRIPTIONS_ENABLED": "false",
            "RERANKER_ENABLED": "false",
            "EXTERNAL_INDEX_AUTO": "true",
            "EXTERNAL_INDEX_ON_REFRESH": "explicit",
        },
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
        # No RERANKER_ENABLED → the reranker is never constructed, so the query
        # path runs BM25+vector without cross-encoder reordering. Same "full"
        # index as code_intel_full; the only difference between the two arms is
        # whether the reranker reorders.
        daemon_env={},
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

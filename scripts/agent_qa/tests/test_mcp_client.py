from scripts.agent_qa.mcp_client import to_anthropic_tool_defs


def test_maps_mcp_tools_to_anthropic_shape():
    mcp_tools = [
        {
            "name": "search_code",
            "description": "Hybrid search.",
            "inputSchema": {
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            },
        }
    ]
    out = to_anthropic_tool_defs(mcp_tools, prefix="ci_")
    assert len(out) == 1
    t = out[0]
    assert t["name"] == "ci_search_code"
    assert t["description"] == "Hybrid search."
    assert t["input_schema"]["type"] == "object"
    assert t["input_schema"]["required"] == ["query"]


def test_skips_tools_without_input_schema():
    mcp_tools = [{"name": "broken", "description": "x"}]
    assert to_anthropic_tool_defs(mcp_tools) == []

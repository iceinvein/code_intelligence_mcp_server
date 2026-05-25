"""Tests for benchmark toolset wiring.

These stay at the helper level so they do not invoke Claude or MCP servers.
"""
from __future__ import annotations

import os
import unittest
from pathlib import Path
from unittest.mock import patch

import scripts.bench_agent_qa as bench


class BenchAgentQaToolsetTests(unittest.TestCase):
    def test_code_intel_toolset_instructs_and_allows_mcp_tools(self):
        env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith("AGENT_SYSTEM_PROMPT_EXTRA")
        }
        with patch.dict(os.environ, env, clear=True):
            prompt = bench._system_prompt_for("code_intel")
            allowed = bench._allowed_tools_for("code_intel")

        self.assertIn("code-intelligence MCP", prompt)
        self.assertIn("ask_code", prompt)
        self.assertIn("pack.rows", prompt)
        self.assertIn("mcp__code-intelligence__ask_code", allowed)
        self.assertIn("mcp__code-intelligence__investigate", allowed)

    def test_default_toolset_stays_neutral(self):
        env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith("AGENT_SYSTEM_PROMPT_EXTRA")
        }
        with patch.dict(os.environ, env, clear=True):
            prompt = bench._system_prompt_for("default")
            allowed = bench._allowed_tools_for("default")

        self.assertNotIn("code-intelligence MCP", prompt)
        self.assertTrue(all(not tool.startswith("mcp__") for tool in allowed))

    def test_code_graph_toolset_instructs_and_allows_codegraph_tools(self):
        env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith("AGENT_SYSTEM_PROMPT_EXTRA")
        }
        with patch.dict(os.environ, env, clear=True):
            prompt = bench._system_prompt_for("code_graph")
            allowed = bench._allowed_tools_for("code_graph")

        self.assertIn("codegraph MCP", prompt)
        self.assertIn("codegraph_search", prompt)
        self.assertIn("mcp__codegraph__codegraph_search", allowed)
        self.assertIn("mcp__codegraph__codegraph_context", allowed)

    def test_code_intel_mcp_config_uses_streamable_http(self):
        cfg = bench._build_code_intel_mcp_config(
            "http://127.0.0.1:17800/mcp",
            Path("/tmp/my repo"),
        )

        entry = cfg["mcpServers"]["code-intelligence"]
        self.assertEqual(entry["type"], "streamable-http")
        self.assertIn("repo=%2Ftmp%2Fmy+repo", entry["url"])
        self.assertNotIn("command", entry)


if __name__ == "__main__":
    unittest.main()

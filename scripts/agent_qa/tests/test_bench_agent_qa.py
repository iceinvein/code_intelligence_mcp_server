"""Tests for benchmark toolset wiring.

These stay at the helper level so they do not invoke Claude or MCP servers.
"""
from __future__ import annotations

import os
import unittest
from pathlib import Path
from unittest.mock import patch

import scripts.bench_agent_qa as bench
from scripts.agent_qa.qa_schema import Expected, QAEntry


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
        self.assertIn("coverage is complete", prompt)
        self.assertIn("do not call Read, Grep, or Glob", prompt)
        self.assertIn("missing source bodies", prompt)
        self.assertIn("Call `mcp__code-intelligence__ask_code` at most once", prompt)
        self.assertIn("do not repeat code-intelligence calls", prompt)
        self.assertIn("mcp__code-intelligence__ask_code", allowed)
        self.assertIn("mcp__code-intelligence__investigate", allowed)
        self.assertNotIn("mcp__code-intelligence__search_code", allowed)
        self.assertNotIn("mcp__code-intelligence__find_references", allowed)
        self.assertNotIn("Read", allowed)
        self.assertNotIn("Grep", allowed)
        self.assertNotIn("Glob", allowed)
        self.assertEqual(
            bench._extra_args_for("code_intel"),
            ["--disallowed-tools", "Read Grep Glob Bash"],
        )

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

    def test_score_question_records_scores_code_graph_when_judge_skipped(self):
        entry = QAEntry(
            id="q1",
            question="Where is the thing?",
            expected=Expected(files=["src/main.rs"], symbols=[], facts=[]),
            rubric="Names src/main.rs.",
        )
        records = {
            "default": {"final_answer": "src/main.rs"},
            "code_graph": {"final_answer": "src/main.rs"},
        }

        bench._score_question_records(
            entry,
            records,
            skip_judge=True,
            complete_fn=lambda _system, _user: "{}",
        )

        self.assertEqual(records["default"]["mech_score"], 1.0)
        self.assertEqual(records["code_graph"]["mech_score"], 1.0)
        self.assertEqual(records["default"]["judge_score"], 0)
        self.assertEqual(records["code_graph"]["judge_score"], 0)

    def test_score_question_records_records_pair_baseline(self):
        entry = QAEntry(
            id="q1",
            question="Where is the thing?",
            expected=Expected(files=["src/main.rs"], symbols=[], facts=[]),
            rubric="Names src/main.rs.",
        )
        records = {
            "default": {"final_answer": "src/main.rs"},
            "code_graph": {"final_answer": "src/main.rs"},
        }

        with patch("scripts.bench_agent_qa.random.randint", return_value=0):
            bench._score_question_records(
                entry,
                records,
                skip_judge=False,
                complete_fn=lambda _system, _user: (
                    '{"A_score": 4, "B_score": 7, '
                    '"A_justification": "baseline", "B_justification": "candidate"}'
                ),
            )

        self.assertEqual(records["default"]["judge_score"], 4)
        self.assertEqual(records["code_graph"]["judge_score"], 7)
        self.assertEqual(records["code_graph"]["judge_baseline_score"], 4)
        self.assertEqual(records["default"]["judge_scores_by_pair"], {"code_graph": 4})

    def test_scored_runs_from_records_includes_code_graph(self):
        records = {
            "default": self._record("q1", "default"),
            "code_intel": self._record("q1", "code_intel"),
            "code_graph": self._record("q1", "code_graph"),
        }
        for rec in records.values():
            rec["mech_score"] = 1.0
            rec["judge_score"] = 8

        scored = bench._scored_runs_from_records(records)

        self.assertEqual(
            {run.toolset for run in scored},
            {"default", "code_intel", "code_graph"},
        )

    def _record(self, question_id: str, toolset: str):
        return {
            "question_id": question_id,
            "toolset": toolset,
            "repo": "custom",
            "input_tokens": 10,
            "total_input_tokens": 20,
            "output_tokens": 2,
            "tool_calls": [{"name": "Read"}],
            "wall_ms": 123,
            "final_answer": "src/main.rs",
            "stop_reason": "end_turn",
        }


if __name__ == "__main__":
    unittest.main()

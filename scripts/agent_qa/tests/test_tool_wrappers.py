import os
from pathlib import Path

import pytest

from scripts.agent_qa.tool_wrappers import (
    DefaultToolset,
    ToolError,
    DEFAULT_TOOL_DEFS,
)


@pytest.fixture
def sandbox(tmp_path: Path) -> Path:
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "foo.rs").write_text("fn alpha() {}\nfn beta() {}\n")
    (tmp_path / "src" / "bar.rs").write_text("fn gamma() {}\n")
    (tmp_path / "README.md").write_text("hello world\n")
    return tmp_path


def test_read_file_returns_content(sandbox: Path):
    ts = DefaultToolset(base_dir=sandbox)
    out = ts.read_file(path="src/foo.rs")
    assert "fn alpha()" in out


def test_read_file_rejects_escape(sandbox: Path):
    ts = DefaultToolset(base_dir=sandbox)
    with pytest.raises(ToolError):
        ts.read_file(path="../etc/passwd")


def test_read_file_rejects_absolute(sandbox: Path):
    ts = DefaultToolset(base_dir=sandbox)
    with pytest.raises(ToolError):
        ts.read_file(path="/etc/passwd")


def test_grep_finds_matches(sandbox: Path):
    ts = DefaultToolset(base_dir=sandbox)
    out = ts.grep(pattern="fn ", path="src")
    assert "alpha" in out
    assert "gamma" in out


def test_grep_no_matches_returns_empty_marker(sandbox: Path):
    ts = DefaultToolset(base_dir=sandbox)
    out = ts.grep(pattern="zzzzzz_no_match", path=".")
    assert out.strip() == "(no matches)"


def test_glob_lists_paths(sandbox: Path):
    ts = DefaultToolset(base_dir=sandbox)
    out = ts.glob(pattern="src/*.rs")
    lines = sorted(out.strip().splitlines())
    assert lines == ["src/bar.rs", "src/foo.rs"]


def test_bash_runs_in_sandbox(sandbox: Path):
    ts = DefaultToolset(base_dir=sandbox)
    out = ts.bash(command="ls src")
    assert "foo.rs" in out and "bar.rs" in out


def test_bash_reports_nonzero_exit(sandbox: Path):
    ts = DefaultToolset(base_dir=sandbox)
    out = ts.bash(command="exit 7")
    assert "exit_code: 7" in out


def test_default_tool_defs_shape():
    names = {t["name"] for t in DEFAULT_TOOL_DEFS}
    assert names == {"read_file", "grep", "glob", "bash"}
    for t in DEFAULT_TOOL_DEFS:
        assert "description" in t
        assert "input_schema" in t
        assert t["input_schema"]["type"] == "object"

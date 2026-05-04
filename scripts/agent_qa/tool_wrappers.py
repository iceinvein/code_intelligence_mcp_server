"""Default Claude Code-flavored tool wrappers used by the agent benchmark.

These run locally inside a configured base directory. All paths are sandboxed:
absolute paths and `..` escapes are rejected.
"""
from __future__ import annotations

import glob as _glob
import shlex
import subprocess
from pathlib import Path
from typing import Any, Dict, List


class ToolError(RuntimeError):
    pass


def _resolve_safe(base_dir: Path, rel: str) -> Path:
    if not rel:
        raise ToolError("path is required")
    p = Path(rel)
    if p.is_absolute():
        raise ToolError(f"absolute paths not allowed: {rel}")
    full = (base_dir / p).resolve()
    base_resolved = base_dir.resolve()
    try:
        full.relative_to(base_resolved)
    except ValueError:
        raise ToolError(f"path escapes base dir: {rel}") from None
    return full


class DefaultToolset:
    def __init__(self, base_dir: Path):
        self.base_dir = Path(base_dir)

    def read_file(self, path: str, max_bytes: int = 200_000) -> str:
        target = _resolve_safe(self.base_dir, path)
        if not target.is_file():
            raise ToolError(f"not a file: {path}")
        data = target.read_bytes()[:max_bytes]
        return data.decode("utf-8", errors="replace")

    def grep(self, pattern: str, path: str = ".", max_lines: int = 200) -> str:
        target = _resolve_safe(self.base_dir, path)
        if not pattern:
            raise ToolError("pattern is required")
        cmd = ["grep", "-rnI", "--", pattern, str(target)]
        try:
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=20)
        except subprocess.TimeoutExpired:
            raise ToolError("grep timed out") from None
        if proc.returncode == 1:  # grep: no matches
            return "(no matches)"
        if proc.returncode > 1:
            raise ToolError(f"grep failed: {proc.stderr.strip()}")
        lines = proc.stdout.splitlines()
        # Strip the absolute base_dir prefix to keep paths repo-relative.
        prefix = str(self.base_dir.resolve()) + "/"
        rel = [ln[len(prefix):] if ln.startswith(prefix) else ln for ln in lines]
        if len(rel) > max_lines:
            rel = rel[:max_lines] + [f"... ({len(lines) - max_lines} more lines truncated)"]
        return "\n".join(rel) if rel else "(no matches)"

    def glob(self, pattern: str, max_results: int = 200) -> str:
        if not pattern:
            raise ToolError("pattern is required")
        if pattern.startswith("/"):
            raise ToolError("absolute glob patterns not allowed")
        if ".." in Path(pattern).parts:
            raise ToolError("'..' not allowed in glob")
        full_pat = str(self.base_dir / pattern)
        matches = sorted(_glob.glob(full_pat, recursive=True))
        prefix = str(self.base_dir) + "/"
        rel = [m[len(prefix):] if m.startswith(prefix) else m for m in matches]
        if len(rel) > max_results:
            rel = rel[:max_results] + [f"... ({len(matches) - max_results} more truncated)"]
        return "\n".join(rel) if rel else "(no matches)"

    def bash(self, command: str, timeout_s: int = 30, max_bytes: int = 100_000) -> str:
        if not command:
            raise ToolError("command is required")
        try:
            proc = subprocess.run(
                ["bash", "-c", command],
                capture_output=True,
                text=True,
                timeout=timeout_s,
                cwd=str(self.base_dir),
            )
        except subprocess.TimeoutExpired:
            return f"(timed out after {timeout_s}s)"
        out = (proc.stdout or "")[:max_bytes]
        err = (proc.stderr or "")[:max_bytes]
        return f"exit_code: {proc.returncode}\nstdout:\n{out}\nstderr:\n{err}".strip()


DEFAULT_TOOL_DEFS: List[Dict[str, Any]] = [
    {
        "name": "read_file",
        "description": "Read a UTF-8 text file inside the repository. Returns the file contents (truncated to 200KB).",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Repo-relative file path."}
            },
            "required": ["path"],
        },
    },
    {
        "name": "grep",
        "description": "Recursive case-sensitive text search (grep -rnI) inside the repository. Returns up to 200 matching lines as 'path:line:match'.",
        "input_schema": {
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string", "description": "Repo-relative starting directory or file. Defaults to '.'."},
            },
            "required": ["pattern"],
        },
    },
    {
        "name": "glob",
        "description": "List files matching a repo-relative glob pattern (use ** for recursion). Returns up to 200 paths.",
        "input_schema": {
            "type": "object",
            "properties": {"pattern": {"type": "string"}},
            "required": ["pattern"],
        },
    },
    {
        "name": "bash",
        "description": "Run a bash command in the repo root. 30s timeout. Returns exit_code, stdout, stderr (each truncated to 100KB).",
        "input_schema": {
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        },
    },
]


def dispatch_default(tool_name: str, args: Dict[str, Any], toolset: DefaultToolset) -> str:
    if tool_name == "read_file":
        return toolset.read_file(**args)
    if tool_name == "grep":
        return toolset.grep(**args)
    if tool_name == "glob":
        return toolset.glob(**args)
    if tool_name == "bash":
        return toolset.bash(**args)
    raise ToolError(f"unknown default tool: {tool_name}")

"""Daemon and codegraph process lifecycle for the bench."""
from __future__ import annotations

import os
import socket
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from bench import config
from bench.arms import Arm


BENCH_HOME = config.BENCH_HOME


@dataclass
class Daemon:
    arm_name: str
    port: int
    process: subprocess.Popen

    def build_mcp_config(self) -> dict:
        """Build the --mcp-config JSON passed to claude --print."""
        return {
            "mcpServers": {
                "code-intelligence": {
                    "type": "streamable-http",
                    "url": f"http://127.0.0.1:{self.port}/mcp",
                }
            }
        }

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()


def _wait_for_port(port: int, timeout_s: float = 30.0) -> None:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(0.5)
            try:
                s.connect(("127.0.0.1", port))
                return
            except (ConnectionRefusedError, socket.timeout, OSError):
                time.sleep(0.5)
    raise RuntimeError(f"daemon did not bind port {port} within {timeout_s}s")


def pick_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def maybe_start_daemon(arm: Arm, port: int, home: Path | None = None) -> Daemon | None:
    """Start the daemon for this arm if it needs one. Returns None for arms that don't.

    If home is provided it overrides BENCH_HOME as the HOME directory for the daemon
    process. This enables per-variant isolation: each index variant lives under its
    own HOME so full and no_desc indexes can coexist side-by-side.
    """
    if not arm.needs_daemon:
        return None

    effective_home = home if home is not None else BENCH_HOME
    effective_home.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["HOME"] = str(effective_home)
    env.update(arm.daemon_env)

    cmd = [str(config.DAEMON_BINARY), "--port", str(port)]
    process = subprocess.Popen(
        cmd,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        _wait_for_port(port, timeout_s=config.DAEMON_HEALTH_TIMEOUT_S)
    except RuntimeError:
        process.terminate()
        raise

    return Daemon(arm_name=arm.name, port=port, process=process)


def ensure_codegraph_installed() -> str | None:
    """Return the codegraph version string if installed, None otherwise."""
    try:
        result = subprocess.run(
            [config.CODEGRAPH_BINARY, "--version"],
            capture_output=True,
            timeout=5,
        )
        if result.returncode == 0:
            return result.stdout.decode().strip()
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    return None

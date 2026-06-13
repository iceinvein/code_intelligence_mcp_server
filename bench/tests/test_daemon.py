"""Tests for bench/daemon.py."""
import subprocess
from dataclasses import dataclass

import pytest

from bench import daemon
from bench.arms import ARMS


@dataclass
class FakeProc:
    pid: int = 12345
    returncode: int | None = None
    def poll(self):
        return self.returncode
    def terminate(self):
        self.returncode = -15
    def kill(self):
        self.returncode = -9
    def wait(self, timeout=None):
        self.returncode = -15
        return self.returncode


def test_daemon_for_default_arm_returns_none():
    d = daemon.maybe_start_daemon(ARMS["default"], port=12345)
    assert d is None


def test_daemon_for_codegraph_arm_returns_none():
    d = daemon.maybe_start_daemon(ARMS["codegraph"], port=12345)
    assert d is None


def test_daemon_for_code_intel_uses_correct_env_and_home(monkeypatch, tmp_path):
    captured = {}
    def fake_popen(cmd, env=None, **kwargs):
        captured["cmd"] = cmd
        captured["env"] = dict(env) if env else {}
        return FakeProc()
    monkeypatch.setattr(subprocess, "Popen", fake_popen)
    monkeypatch.setattr(daemon, "_wait_for_port", lambda *a, **k: None)
    monkeypatch.setattr(daemon, "BENCH_HOME", tmp_path)

    d = daemon.maybe_start_daemon(ARMS["code_intel_shipped"], port=18888)
    assert d is not None
    assert d.port == 18888
    assert "BENCH_DISABLE_DESCRIPTIONS" in captured["env"]
    assert captured["env"]["BENCH_DISABLE_DESCRIPTIONS"] == "1"
    assert captured["env"]["HOME"] == str(tmp_path)


def test_daemon_stop_terminates_process():
    proc = FakeProc()
    d = daemon.Daemon(arm_name="code_intel_full", port=18888, process=proc)
    d.stop()
    assert proc.returncode == -15

"""Tests for bench/repos.py."""
import json
import subprocess

import pytest

from bench import repos


def test_repo_hash_is_stable():
    h1 = repos.repo_hash("/path/to/repo")
    h2 = repos.repo_hash("/path/to/repo")
    assert h1 == h2
    assert len(h1) == 16


def test_index_is_fresh_returns_false_when_no_cache(tmp_path):
    assert repos.index_is_fresh(tmp_path / "missing", current_meta={}) is False


def test_index_is_fresh_returns_true_on_match(tmp_path):
    data_dir = tmp_path / "data"
    data_dir.mkdir()
    meta = {"daemon_sha": "abc", "repo_upstream_sha": "def", "variant": "full", "schema_version": 22}
    (data_dir / "bench-cache.json").write_text(json.dumps(meta))
    assert repos.index_is_fresh(data_dir, current_meta=meta) is True


def test_index_is_fresh_returns_false_on_drift(tmp_path):
    data_dir = tmp_path / "data"
    data_dir.mkdir()
    old = {"daemon_sha": "abc", "repo_upstream_sha": "def", "variant": "full", "schema_version": 22}
    new = {**old, "daemon_sha": "xyz"}
    (data_dir / "bench-cache.json").write_text(json.dumps(old))
    assert repos.index_is_fresh(data_dir, current_meta=new) is False


def test_ensure_repo_checkout_clones_if_missing(monkeypatch, tmp_path):
    calls = []
    def fake_run(cmd, **kwargs):
        calls.append(cmd)
        class R: stdout = b"a" * 40; returncode = 0
        return R()
    monkeypatch.setattr(subprocess, "run", fake_run)
    target = tmp_path / "myrepo"
    repos.ensure_repo_checkout(name="myrepo", upstream_url="https://e/repo.git",
                                upstream_sha="abc123", target_dir=target)
    assert any("clone" in str(c) for c in calls)


def test_daemon_binary_hash_is_stable_16_hex(monkeypatch, tmp_path):
    import importlib
    from bench import config, repos

    fake_bin = tmp_path / "daemon"
    fake_bin.write_bytes(b"binary-contents-v1")
    monkeypatch.setattr(config, "DAEMON_BINARY", fake_bin)

    h1 = repos.daemon_binary_hash()
    h2 = repos.daemon_binary_hash()
    full = repos.daemon_binary_sha256()
    assert h1 == h2
    assert len(h1) == 16
    assert len(full) == 64
    assert full.startswith(h1)
    int(h1, 16)  # hex
    int(full, 16)

    fake_bin.write_bytes(b"binary-contents-v2")
    assert repos.daemon_binary_hash() != h1

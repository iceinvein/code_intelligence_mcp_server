"""Regression tests for the pinned protoc cache bootstrap."""

import hashlib
import os
import stat
import subprocess
import zipfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
PROTOC_WRAPPER = REPO_ROOT / "scripts" / "protoc"


def _platform_arch() -> str:
    machine = os.uname().machine
    if machine in {"arm64", "aarch64"}:
        return "osx-aarch_64"
    if machine in {"x86_64", "amd64"}:
        return "osx-x86_64"
    raise AssertionError(f"unsupported test architecture: {machine}")


def _fake_archive(path: Path, *, complete: bool = True) -> str:
    executable = zipfile.ZipInfo("bin/protoc")
    executable.create_system = 3
    executable.external_attr = (stat.S_IFREG | 0o755) << 16
    script = b'#!/bin/sh\nprintf "libprotoc 29.3\\n"\n'

    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr(executable, script)
        archive.writestr("include/google/protobuf/descriptor.proto", "message Descriptor {}\n")
        if complete:
            archive.writestr(
                "include/google/protobuf/compiler/plugin.proto",
                "message Plugin {}\n",
            )
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _env(cache: Path, archive: Path, checksum: str) -> dict[str, str]:
    return {
        **os.environ,
        "XDG_CACHE_HOME": str(cache),
        "PROTOC_DOWNLOAD_URL": archive.as_uri(),
        "PROTOC_EXPECTED_SHA256": checksum,
        "PROTOC_LOCK_TIMEOUT_SECONDS": "20",
    }


def _run(env: dict[str, str]) -> subprocess.CompletedProcess:
    return subprocess.run(
        [str(PROTOC_WRAPPER), "--version"],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )


def test_parallel_bootstrap_publishes_one_complete_cache(tmp_path):
    archive = tmp_path / "protoc.zip"
    checksum = _fake_archive(archive)
    cache = tmp_path / "cache"
    env = _env(cache, archive, checksum)

    with ThreadPoolExecutor(max_workers=16) as pool:
        results = list(pool.map(lambda _index: _run(env), range(32)))

    assert all(result.returncode == 0 for result in results), [
        {
            "returncode": result.returncode,
            "stdout": result.stdout,
            "stderr": result.stderr,
        }
        for result in results
        if result.returncode != 0
    ]
    assert all(result.stdout.strip() == "libprotoc 29.3" for result in results)

    installed = cache / "code-intel-protoc" / "29.3" / _platform_arch()
    assert (installed / "bin" / "protoc").stat().st_mode & stat.S_IXUSR
    assert (installed / "include/google/protobuf/descriptor.proto").is_file()
    assert (installed / "include/google/protobuf/compiler/plugin.proto").is_file()
    assert not Path(f"{installed}.lock").exists()
    assert not list(installed.parent.glob(f".{_platform_arch()}.tmp.*"))


def test_incomplete_cache_is_repaired_only_from_verified_archive(tmp_path):
    archive = tmp_path / "protoc.zip"
    checksum = _fake_archive(archive)
    cache = tmp_path / "cache"
    installed = cache / "code-intel-protoc" / "29.3" / _platform_arch()
    (installed / "bin").mkdir(parents=True)
    stale = installed / "bin" / "protoc"
    stale.write_text('#!/bin/sh\nprintf "libprotoc 29.3\\n"\n')
    stale.chmod(0o755)

    result = _run(_env(cache, archive, checksum))

    assert result.returncode == 0, result.stderr
    assert (installed / "include/google/protobuf/descriptor.proto").is_file()
    assert (installed / "include/google/protobuf/compiler/plugin.proto").is_file()


def test_bad_checksum_never_publishes_partial_cache(tmp_path):
    archive = tmp_path / "protoc.zip"
    _fake_archive(archive)
    cache = tmp_path / "cache"

    result = _run(_env(cache, archive, "0" * 64))

    assert result.returncode != 0
    assert "checksum mismatch" in result.stderr
    installed = cache / "code-intel-protoc" / "29.3" / _platform_arch()
    assert not installed.exists()


def _lock_dir(cache: Path) -> Path:
    return cache / "code-intel-protoc" / "29.3" / f"{_platform_arch()}.lock"


def test_lock_left_by_a_dead_owner_is_reclaimed(tmp_path):
    """A lock whose recorded owner no longer exists must not block forever."""
    archive = tmp_path / "protoc.zip"
    checksum = _fake_archive(archive)
    cache = tmp_path / "cache"
    lock = _lock_dir(cache)
    lock.mkdir(parents=True)
    # PID 2^22 is above every configured pid_max, so it cannot be live.
    (lock / "pid").write_text("4194304\n")

    result = _run(_env(cache, archive, checksum))

    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "libprotoc 29.3"
    assert not lock.exists()


def test_lock_without_a_recorded_owner_is_never_stolen(tmp_path):
    """An owner that has not published its pid yet still holds the lock.

    This is the window a reclaiming waiter must not treat as stale, otherwise it
    deletes a live lock and the holder's own pid write fails.
    """
    archive = tmp_path / "protoc.zip"
    checksum = _fake_archive(archive)
    cache = tmp_path / "cache"
    lock = _lock_dir(cache)
    lock.mkdir(parents=True)

    env = _env(cache, archive, checksum)
    env["PROTOC_LOCK_TIMEOUT_SECONDS"] = "1"
    result = _run(env)

    assert result.returncode != 0
    assert "timed out waiting" in result.stderr
    assert lock.exists(), "a lock with no recorded owner must be left alone"

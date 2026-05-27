"""Bench CLI dispatcher.

Usage:
    python3 -m bench.run <command> [args]
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from bench import config, fixtures_io


def cmd_validate(args) -> int:
    path = Path(args.fixture)
    if not path.exists():
        print(f"error: fixture not found: {path}", file=sys.stderr)
        return 2
    repo_root = Path(args.repo_root) if args.repo_root else config.REPO_ROOT
    errs = fixtures_io.validate_fixture(path, repo_root)
    if errs:
        print(f"{len(errs)} validation error(s):", file=sys.stderr)
        for e in errs:
            print(f"  - {e}", file=sys.stderr)
        return 1
    print(f"OK: {path} has no validation errors against {repo_root}")
    return 0


def cmd_list(args) -> int:
    results = config.RESULTS_DIR
    if not results.exists():
        return 0
    rounds = sorted([p.name for p in results.iterdir() if p.is_dir() and p.name.startswith("R")])
    if not rounds:
        return 0
    for r in rounds:
        meta_path = results / r / "meta.json"
        if meta_path.exists():
            print(f"{r}  {meta_path.read_text().strip()[:80]}")
        else:
            print(f"{r}  (no meta)")
    return 0


def cmd_prep(args) -> int:
    import subprocess as _subprocess
    from bench import arms as arms_mod, daemon as daemon_mod, fixtures_io, repos

    requested_arms = (args.arms.split(",") if args.arms
                      else list(arms_mod.ARMS.keys()))
    arms_to_prep = [arms_mod.ARMS[n] for n in requested_arms if n in arms_mod.ARMS]

    # Discover repos via fixture files (mirrors cmd_full).
    if args.repos:
        repo_names = [r.strip() for r in args.repos.split(",")]
    else:
        repo_names = sorted(
            p.stem for p in config.FIXTURES_DIR.glob("*.yaml")
            if p.stem != "smoke"
        )

    print(f"prep: arms={','.join(a.name for a in arms_to_prep)} repos={','.join(repo_names) or '(none)'}")

    if args.check:
        variants = arms_mod.distinct_index_variants(arms_to_prep)
        print(f"dry-run: would build index variants {variants} for repos {repo_names}")
        return 0

    # Ensure isolated HOME and the model symlink exist.
    config.BENCH_HOME.mkdir(parents=True, exist_ok=True)
    bench_ci_dir = config.BENCH_HOME / ".code-intelligence"
    bench_ci_dir.mkdir(parents=True, exist_ok=True)
    models_link = bench_ci_dir / "models"
    real_models = Path.home() / ".code-intelligence" / "models"
    if not models_link.exists():
        if real_models.exists():
            models_link.symlink_to(real_models)
            print(f"symlinked models: {models_link} -> {real_models}")
        else:
            print(
                f"warning: {real_models} does not exist; models will need to be downloaded "
                f"on first daemon start (this is slow and uses bandwidth)",
                file=sys.stderr,
            )

    # Codegraph install check.
    cg_version = daemon_mod.ensure_codegraph_installed()
    if cg_version:
        print(f"codegraph installed: {cg_version}")
    else:
        print("codegraph not installed; codegraph arm will be skipped during full run")

    # Per-repo checkout.
    config.BENCH_REPOS_DIR.mkdir(parents=True, exist_ok=True)
    for name in repo_names:
        fixture_path = config.FIXTURES_DIR / f"{name}.yaml"
        if not fixture_path.exists():
            print(f"warning: fixture not found: {fixture_path}; skipping {name}", file=sys.stderr)
            continue
        fixture = fixtures_io.load_fixture(fixture_path)
        meta = fixture.meta
        target = config.BENCH_REPOS_DIR / name

        # Special case: local-path fixtures (upstream_url is an absolute path).
        # Treat as already-checked-out; symlink rather than clone.
        upstream_url = meta.upstream_url
        if Path(upstream_url).is_absolute() and Path(upstream_url).exists():
            if not target.exists():
                target.parent.mkdir(parents=True, exist_ok=True)
                target.symlink_to(upstream_url)
                print(f"linked {name}: {target} -> {upstream_url}")
            # Don't try to fetch / checkout SHA on a symlink; the user is responsible
            # for the repo's local state matching meta.upstream_sha. Warn if it doesn't.
            try:
                head = _subprocess.run(
                    ["git", "-C", str(target), "rev-parse", "HEAD"],
                    check=True, capture_output=True,
                ).stdout.decode().strip()
                if head != meta.upstream_sha:
                    print(
                        f"warning: {name} HEAD is {head} but fixture pins {meta.upstream_sha}; "
                        f"citations may not line up",
                        file=sys.stderr,
                    )
            except _subprocess.CalledProcessError:
                pass
            continue

        # Standard remote clone.
        try:
            repos.ensure_repo_checkout(
                name=name,
                upstream_url=meta.upstream_url,
                upstream_sha=meta.upstream_sha,
                target_dir=target,
            )
            print(f"checked out {name} at {meta.upstream_sha[:12]}")
        except _subprocess.CalledProcessError as e:
            print(
                f"error: failed to check out {name}: {e.stderr.decode() if e.stderr else e}",
                file=sys.stderr,
            )
            return 2

    # Build index variants per repo per variant.
    variants = arms_mod.distinct_index_variants(arms_to_prep)
    if not variants:
        print("no daemon arms requested; skipping index variant builds")
    else:
        print(f"building index variants {variants} (this may take a while)")
        for name in repo_names:
            repo_path = config.BENCH_REPOS_DIR / name
            if not repo_path.exists():
                continue
            repo_hash_str = repos.repo_hash(str(repo_path.resolve()))
            for variant in variants:
                data_dir = repos.variant_data_dir(repo_hash_str, variant)
                meta_dict = {
                    "daemon_sha": repos.current_daemon_sha(),
                    "repo_upstream_sha": _read_pinned_sha(name),
                    "variant": variant,
                    "schema_version": 22,
                }
                if repos.index_is_fresh(data_dir, meta_dict):
                    print(f"  {name}/{variant}: cached, skipping rebuild")
                    continue
                print(f"  {name}/{variant}: building")
                _build_variant(name, repo_path, variant)
                repos.write_cache(data_dir, meta_dict)
                print(f"  {name}/{variant}: done")

    print("prep: complete")
    return 0


def _read_pinned_sha(name: str) -> str:
    """Read upstream_sha from the fixture meta block."""
    fixture = fixtures_io.load_fixture(config.FIXTURES_DIR / f"{name}.yaml")
    return fixture.meta.upstream_sha


def _build_variant(repo_name: str, repo_path: Path, variant: str) -> None:
    """Spawn the daemon with BENCH_HOME and the right BENCH_DISABLE_* env, then
    POST /api/repos/<hash>/reindex, wait for it to complete (poll the JSON API),
    then stop the daemon."""
    import os
    import subprocess as _subprocess
    import time as _time
    import urllib.error
    import urllib.request
    from bench import daemon as daemon_mod, repos

    env_extra: dict[str, str] = {}
    if variant == "no_desc":
        env_extra["BENCH_DISABLE_DESCRIPTIONS"] = "1"

    port = daemon_mod.pick_free_port()
    config.BENCH_HOME.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["HOME"] = str(config.BENCH_HOME)
    env.update(env_extra)

    proc = _subprocess.Popen(
        [str(config.DAEMON_BINARY), "--port", str(port)],
        env=env,
        stdout=_subprocess.DEVNULL,
        stderr=_subprocess.DEVNULL,
    )
    try:
        daemon_mod._wait_for_port(port, timeout_s=config.DAEMON_HEALTH_TIMEOUT_S)

        # The API port is mcp_port + 2 per the server architecture.
        api_port = port + 2
        repo_hash_str = repos.repo_hash(str(repo_path.resolve()))

        # POST /api/repos/<hash>/reindex
        req = urllib.request.Request(
            f"http://127.0.0.1:{api_port}/api/repos/{repo_hash_str}/reindex",
            method="POST",
        )
        try:
            urllib.request.urlopen(req, timeout=10)
        except urllib.error.URLError as e:
            print(
                f"  WARN: reindex POST failed: {e}; continuing (daemon may auto-index on bind)",
                file=sys.stderr,
            )

        # Wait for indexing + description backfill to settle.
        # Poll /api/jobs and exit when no running jobs remain for this repo.
        deadline = _time.monotonic() + 1800  # 30 min cap
        while _time.monotonic() < deadline:
            _time.sleep(15)
            try:
                with urllib.request.urlopen(
                    f"http://127.0.0.1:{api_port}/api/jobs", timeout=5
                ) as r:
                    body = json.loads(r.read().decode())
                running = [
                    j for j in body.get("running", [])
                    if j.get("repo_id", "").startswith(repo_hash_str[:8])
                ]
                if not running:
                    print(f"  no running jobs for {repo_hash_str[:8]}; build settled")
                    break
                print(f"  {repo_name}/{variant}: still running ({len(running)} jobs)")
            except (urllib.error.URLError, json.JSONDecodeError) as e:
                print(f"  WARN: poll failed: {e}", file=sys.stderr)
                break
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except _subprocess.TimeoutExpired:
            proc.kill()


def cmd_full(args) -> int:
    from bench import arms as arms_mod, fixtures_io, orchestrator

    requested_arms = (args.arms.split(",") if args.arms
                      else list(arms_mod.ARMS.keys()))
    arms_to_run = [arms_mod.ARMS[n] for n in requested_arms if n in arms_mod.ARMS]

    # Discover fixtures.
    if args.repos:
        repo_names = [r.strip() for r in args.repos.split(",")]
    else:
        # Default: every *.yaml in fixtures/, EXCEPT smoke.yaml (which is dev-only).
        repo_names = sorted(
            p.stem for p in config.FIXTURES_DIR.glob("*.yaml")
            if p.stem != "smoke"
        )
        if not repo_names:
            print(
                "error: no fixtures found in bench/fixtures/. Author at least one with "
                "`python3 -m bench.run authoring init <name>` then fill it in. "
                "Or pass --repos smoke for a dev cycle.",
                file=sys.stderr,
            )
            return 2

    # Load each fixture and verify the repo is checked out.
    repos_to_run: list[tuple] = []
    for name in repo_names:
        fixture_path = config.FIXTURES_DIR / f"{name}.yaml"
        if not fixture_path.exists():
            print(f"error: fixture not found: {fixture_path}", file=sys.stderr)
            return 2
        fixture = fixtures_io.load_fixture(fixture_path)
        # For smoke, use config.REPO_ROOT (this repo). For real fixtures, use the
        # bench-managed checkout.
        if name == "smoke":
            repo_path = config.REPO_ROOT
        else:
            repo_path = config.BENCH_REPOS_DIR / name
            if not repo_path.exists():
                print(
                    f"error: {repo_path} not found. Run `python3 -m bench.run prep` first.",
                    file=sys.stderr,
                )
                return 2
        repos_to_run.append((fixture, repo_path))

    # Pick round id.
    config.RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    existing = sorted([p.name for p in config.RESULTS_DIR.iterdir()
                       if p.is_dir() and p.name.startswith("R")])
    if args.round is not None:
        round_id = f"R{args.round:03d}"
    elif existing:
        last = int(existing[-1][1:])
        round_id = f"R{last+1:03d}"
    else:
        round_id = "R001"

    results_dir = config.RESULTS_DIR / round_id
    if results_dir.exists():
        print(f"error: {results_dir} exists; pass --round explicitly", file=sys.stderr)
        return 2

    print(
        f"running {round_id}: arms=[{','.join(a.name for a in arms_to_run)}] "
        f"repos=[{','.join(name for name in repo_names)}]"
    )

    # Judging uses `claude --print` (same path as agent runs). No API key needed
    # when running under a Claude Code subscription.
    judge_enabled = True

    summary = orchestrator.run_cycle(
        arms_to_run=arms_to_run,
        repos=repos_to_run,
        results_dir=results_dir,
        judge_enabled=judge_enabled,
    )
    print(f"completed {summary['n_runs']} runs; results in {results_dir}")
    return 0


def cmd_arm(args) -> int:
    print("arm: not yet implemented")
    return 0


def cmd_question(args) -> int:
    print("question: not yet implemented")
    return 0


def cmd_report(args) -> int:
    from collections import defaultdict
    from bench import report as report_mod

    round_dir = config.RESULTS_DIR / args.round
    if not round_dir.exists():
        print(f"error: {round_dir} not found", file=sys.stderr)
        return 2

    scores_path = round_dir / "scores.json"
    if not scores_path.exists():
        print(f"error: {scores_path} not found", file=sys.stderr)
        return 2
    scores = [json.loads(line) for line in scores_path.read_text().splitlines() if line.strip()]

    by_arm = defaultdict(list)
    for s in scores:
        by_arm[s["arm"]].append(s)

    arms_data = {
        arm_name: report_mod.aggregate_arm(arm_name, rows)
        for arm_name, rows in by_arm.items()
    }

    md = report_mod.render_markdown(
        round_id=args.round,
        repos=sorted({s["repo"] for s in scores}),
        arms_data=arms_data,
        outliers={
            "high_judge_disagreement": [],
            "hallucinated_citations": [],
            "forbidden_hits": [],
            "regressed_vs_full": [],
        },
        meta={
            "daemon_sha": "?",
            "codegraph_version": None,
            "agent_model": config.AGENT_MODEL,
        },
    )

    out_path = Path(args.out) if args.out else round_dir / "report.md"
    out_path.write_text(md)
    print(f"wrote {out_path}")
    return 0


def cmd_diff(args) -> int:
    print("diff: not yet implemented")
    return 0


def cmd_clean(args) -> int:
    print("clean: not yet implemented")
    return 0


def cmd_authoring_init(args) -> int:
    repo = args.repo
    out_path = Path(args.out) if args.out else config.FIXTURES_DIR / f"{repo}.yaml"
    if out_path.exists():
        print(f"error: {out_path} already exists", file=sys.stderr)
        return 2

    import datetime as _datetime
    today = _datetime.date.today().isoformat()
    template = f"""meta:
  repo: {repo}
  upstream_url: "TODO_url_or_path"
  upstream_sha: "TODO_sha"
  authored_at: "{today}"
  authored_against_schema_version: 22

questions:
  # 4 symbol_lookup questions
  - id: {repo}-symbol-01
    task_type: symbol_lookup
    difficulty: easy
    question: "TODO"
    rubric: |
      TODO: explicit penalty triggers.
    expected:
      citations:
        - {{ file: "TODO", line_range: [1, 1], symbol: "TODO" }}
      files: ["TODO"]
      facts: ["TODO"]
      forbidden: []
      forbidden_strict: false
  # ... add 3 more symbol_lookup, 4 concept, 4 multi_hop, 3 impact, 3 architectural, 2 negative
"""
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(template)
    print(f"wrote scaffold: {out_path}")
    print("Edit the file to fill in 20 questions per the AUTHORING.md distribution.")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="bench")
    sub = parser.add_subparsers(dest="command", required=True)

    p_prep = sub.add_parser("prep")
    p_prep.add_argument("--arms", default=None)
    p_prep.add_argument("--repos", default=None)
    p_prep.add_argument("--check", action="store_true")
    p_prep.set_defaults(func=cmd_prep)

    p_full = sub.add_parser("full")
    p_full.add_argument("--arms", default=None)
    p_full.add_argument("--repos", default=None)
    p_full.add_argument("--round", type=int, default=None)
    p_full.set_defaults(func=cmd_full)

    p_arm = sub.add_parser("arm")
    p_arm.add_argument("name")
    p_arm.add_argument("--repos", default=None)
    p_arm.add_argument("--questions", default=None)
    p_arm.add_argument("--round", type=int, required=True)
    p_arm.set_defaults(func=cmd_arm)

    p_question = sub.add_parser("question")
    p_question.add_argument("id")
    p_question.add_argument("--arms", default=None)
    p_question.add_argument("--round", type=int, required=True)
    p_question.set_defaults(func=cmd_question)

    p_report = sub.add_parser("report")
    p_report.add_argument("round")
    p_report.add_argument("--out", default=None)
    p_report.set_defaults(func=cmd_report)

    p_diff = sub.add_parser("diff")
    p_diff.add_argument("round_a")
    p_diff.add_argument("round_b")
    p_diff.set_defaults(func=cmd_diff)

    p_list = sub.add_parser("list")
    p_list.set_defaults(func=cmd_list)

    p_clean = sub.add_parser("clean")
    p_clean.add_argument("--all", action="store_true")
    p_clean.add_argument("--indexes", action="store_true")
    p_clean.add_argument("--variant", default=None)
    p_clean.add_argument("--rounds-before", default=None)
    p_clean.set_defaults(func=cmd_clean)

    p_auth = sub.add_parser("authoring")
    auth_sub = p_auth.add_subparsers(dest="authoring_cmd", required=True)
    p_auth_init = auth_sub.add_parser("init")
    p_auth_init.add_argument("repo")
    p_auth_init.add_argument("--out", default=None)
    p_auth_init.set_defaults(func=cmd_authoring_init)

    p_validate = sub.add_parser("validate")
    p_validate.add_argument("fixture")
    p_validate.add_argument("--repo-root", default=None)
    p_validate.set_defaults(func=cmd_validate)

    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())

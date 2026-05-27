"""Bench CLI dispatcher.

Usage:
    python3 -m bench.run <command> [args]
"""
from __future__ import annotations

import argparse
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
    from bench import arms as arms_mod
    from bench import daemon as daemon_mod

    requested_arms = (args.arms.split(",") if args.arms
                      else list(arms_mod.ARMS.keys()))
    arms_to_prep = [arms_mod.ARMS[n] for n in requested_arms if n in arms_mod.ARMS]

    print(f"prep: arms={','.join(a.name for a in arms_to_prep)}")

    if args.check:
        variants = arms_mod.distinct_index_variants(arms_to_prep)
        print(f"(dry-run mode: would build index variants: {variants})")
        return 0

    cg_version = daemon_mod.ensure_codegraph_installed()
    if cg_version:
        print(f"codegraph installed: {cg_version}")
    else:
        print("codegraph not installed; codegraph arm will be skipped during full run")

    print("prep: complete (smoke-mode only; real wolfmax/django index variant builds added later)")
    return 0


def cmd_full(args) -> int:
    import json
    from bench import arms as arms_mod, fixtures_io, orchestrator

    requested_arms = (args.arms.split(",") if args.arms
                      else list(arms_mod.ARMS.keys()))
    arms_to_run = [arms_mod.ARMS[n] for n in requested_arms if n in arms_mod.ARMS]

    fixture_path = config.FIXTURES_DIR / "smoke.yaml"
    if not fixture_path.exists():
        print("error: no fixtures available (smoke fixture missing)", file=sys.stderr)
        return 2
    fixture = fixtures_io.load_fixture(fixture_path)

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

    print(f"running {round_id} for arms: {','.join(a.name for a in arms_to_run)}")

    summary = orchestrator.run_cycle(
        arms_to_run=arms_to_run,
        repos=[(fixture, config.REPO_ROOT)],
        results_dir=results_dir,
        judge_client=None,
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
    print("report: not yet implemented")
    return 0


def cmd_diff(args) -> int:
    print("diff: not yet implemented")
    return 0


def cmd_clean(args) -> int:
    print("clean: not yet implemented")
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

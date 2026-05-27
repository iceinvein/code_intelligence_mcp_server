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
    print("prep: not yet implemented (Task 14 wires this to bench/repos.py + bench/daemon.py)")
    return 0


def cmd_full(args) -> int:
    print("full: not yet implemented (Task 14 wires this to runner + judge + report)")
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

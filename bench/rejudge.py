"""Resume-aware rejudge: re-score only the rate-limit casualties in a round.

A *casualty* is a judge.jsonl row where all three judges scored 0 with empty
justifications -- the shape `judge.judge_one` writes when the `claude --print`
call fails (rate limit, timeout, parse error). Genuine zero scores carry a
non-empty justification, so they are never touched.

Re-running is idempotent and resumable: a successfully rejudged row gets real
justifications and is skipped on the next pass. Each fixed row is persisted
immediately, so a mid-run rate-limit stop keeps all progress. The run self-exits
after `--max-consec` consecutive failures rather than hammering a still-closed
rate-limit window.

Success is decided per judge via `JudgeResult.error is None` (not by inspecting
scores), so a partially rate-limited triple -- e.g. haiku ok but opus 429 -- is
rejected wholesale and left for the next resume pass instead of writing a median
polluted by spurious zeros.

Usage:
    python3 -m bench.rejudge R008            # resume rejudging casualties
    python3 -m bench.rejudge R008 --dry-run  # just list casualties, no API calls
"""
from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
from pathlib import Path

from bench import config, fixtures_io
from bench import judge as judge_mod


def _load_jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.open() if line.strip()]


def _write_jsonl(path: Path, rows: list[dict]) -> None:
    with path.open("w") as f:
        for r in rows:
            f.write(json.dumps(r, default=str) + "\n")


def _is_casualty(row: dict) -> bool:
    scores = row.get("scores") or {}
    justs = row.get("justifications") or {}
    if not scores:
        return False
    all_zero = all(v in (0, None) for v in scores.values())
    no_just = not any((j or "").strip() for j in justs.values())
    return all_zero and no_just


def _load_fixture_questions() -> dict:
    qmap = {}
    for yaml_path in config.FIXTURES_DIR.glob("*.yaml"):
        if yaml_path.stem == "smoke":
            continue
        fx = fixtures_io.load_fixture(yaml_path)
        for q in fx.questions:
            qmap[q.id] = q
    return qmap


def main() -> int:
    ap = argparse.ArgumentParser(description="Resume-aware rejudge of rate-limit casualties.")
    ap.add_argument("round", help="round id, e.g. R008")
    ap.add_argument("--max-consec", type=int, default=6,
                    help="stop after this many consecutive judge-call failures (rate-limit guard)")
    ap.add_argument("--sleep", type=float, default=1.5, help="seconds between questions")
    ap.add_argument("--dry-run", action="store_true", help="list casualties; make no API calls")
    args = ap.parse_args()

    rd = config.RESULTS_DIR / args.round
    jpath, rpath, spath = rd / "judge.jsonl", rd / "runs.jsonl", rd / "scores.json"
    for p in (jpath, rpath, spath):
        if not p.exists():
            print(f"error: {p} not found", file=sys.stderr)
            return 1

    judge_rows = _load_jsonl(jpath)
    run_by = {(r["arm"], r["question_id"]): r for r in _load_jsonl(rpath)}
    scores = _load_jsonl(spath)
    score_by = {(s["arm"], s["question_id"]): s for s in scores}
    qmap = _load_fixture_questions()

    casualties = [r for r in judge_rows if _is_casualty(r)]
    print(f"{args.round}: {len(casualties)} casualties / {len(judge_rows)} judge rows")
    if args.dry_run:
        for r in casualties:
            print(f"  cas: {r['arm']:22s} {r['question_id']}")
        return 0
    if not casualties:
        print("nothing to do.")
        return 0

    judges = {"haiku": config.JUDGE_HAIKU, "sonnet": config.JUDGE_SONNET, "opus": config.JUDGE_OPUS}
    consec = 0
    fixed = 0
    for row in judge_rows:
        if not _is_casualty(row):
            continue
        key = (row["arm"], row["question_id"])
        q = qmap.get(row["question_id"])
        run = run_by.get(key)
        srec = score_by.get(key)
        if q is None or run is None or srec is None:
            print(f"  SKIP {key}: missing question/run/score record")
            continue

        citations = [{"file": c.file, "line_range": list(c.line_range), "symbol": c.symbol}
                     for c in q.expected.citations]
        mech_context = {
            "citation_hit": srec["citation_hit"],
            "hallucinated_citation": srec["hallucinated"],
            "forbidden_hit": srec["forbidden_hit"],
        }

        results = {
            label: judge_mod.judge_one(
                model=model, question_id=q.id, question=q.question, rubric=q.rubric,
                citations=citations, mech_context=mech_context, answer=run["final_answer"],
            )
            for label, model in judges.items()
        }
        errored = [label for label, r in results.items() if r.error]
        if errored:
            consec += 1
            errs = ", ".join(f"{l}:{results[l].error}" for l in errored)
            print(f"  FAIL {key} (consec {consec}/{args.max_consec}) [{errs}]")
            if consec >= args.max_consec:
                print(f"  stopping after {consec} consecutive failures (rate limit?). Re-run to resume.")
                break
            time.sleep(args.sleep)
            continue

        sc = {label: r.score for label, r in results.items()}
        js = {label: r.justification for label, r in results.items()}
        row["scores"], row["justifications"] = sc, js
        row["median"] = statistics.median(sc.values())
        row["range"] = max(sc.values()) - min(sc.values())
        srec["judge_median"] = row["median"]
        srec["judge_range"] = row["range"]
        fixed += 1
        consec = 0
        print(f"  OK   {key} -> median {row['median']} {sc}")
        # Persist after every fix so a rate-limit stop keeps progress.
        _write_jsonl(jpath, judge_rows)
        _write_jsonl(spath, scores)
        time.sleep(args.sleep)

    remaining = sum(1 for r in judge_rows if _is_casualty(r))
    print(f"fixed {fixed}; {remaining} casualties remain")
    return 0


if __name__ == "__main__":
    sys.exit(main())

"""Scoring layers: mechanical, citation verification, forbidden penalty."""
from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from bench.fixtures_io import Citation, Question


# Matches: "src/foo.rs:42", "src/foo.rs:42-88", "[src/foo.rs:42]",
# "src/foo.rs line 42", "src/foo.rs at line 42-88"
_CITATION_PATTERNS = [
    re.compile(r"(?P<file>[\w./\-]+\.\w+):(?P<start>\d+)(?:-(?P<end>\d+))?"),
    re.compile(r"(?P<file>[\w./\-]+\.\w+)\s+(?:at\s+)?line\s+(?P<start>\d+)(?:-(?P<end>\d+))?"),
]


@dataclass
class CitationVerification:
    raw_match: str
    file: str
    start_line: int
    end_line: int
    ok: bool
    reason: str = ""


def _cite_appears(c: Citation, answer: str) -> bool:
    """True when the answer mentions c.file AND a line number falling within c.line_range."""
    a = answer.lower()
    if c.file.lower() not in a:
        return False
    pat = re.compile(
        re.escape(c.file) + r"(?::|.{0,30}line\s+)(?P<start>\d+)(?:-(?P<end>\d+))?",
        re.IGNORECASE,
    )
    for m in pat.finditer(answer):
        start = int(m.group("start"))
        end = int(m.group("end")) if m.group("end") else start
        if not (end < c.line_range[0] or start > c.line_range[1]):
            return True
    return False


def mech_score(q: Question, answer: str) -> dict:
    if not answer.strip():
        return {
            "citation_hit": False,
            "file_score": 0.0,
            "fact_score": 0.0,
            "raw": 0.0,
        }

    a = answer.lower()

    # 1. Citation hit. Empty citations list = no canonical citation required
    # (typical for negative-task questions) -> vacuously satisfied.
    if q.expected.citations:
        citation_hit = any(_cite_appears(c, answer) for c in q.expected.citations)
    else:
        citation_hit = True

    # 2. File coverage. Empty files list = no required files -> vacuously 1.0.
    if q.expected.files:
        file_hits = sum(1 for f in q.expected.files if f.lower() in a)
        file_score = file_hits / len(q.expected.files)
    else:
        file_score = 1.0

    # 3. Fact coverage: each fact is str or list-of-synonyms (OR alternatives).
    # A fact is satisfied when at least one of its alternatives appears in the answer.
    if q.expected.facts:
        fact_hits = 0
        for fact in q.expected.facts:
            alternatives = fact if isinstance(fact, list) else [fact]
            if any(syn.lower() in a for syn in alternatives):
                fact_hits += 1
        fact_score = fact_hits / len(q.expected.facts)
    else:
        fact_score = 1.0

    raw = 0.5 * (1.0 if citation_hit else 0.0) + 0.25 * file_score + 0.25 * fact_score

    return {
        "citation_hit": citation_hit,
        "file_score": file_score,
        "fact_score": fact_score,
        "raw": raw,
    }


def _extract_citations(answer: str) -> list[tuple[str, int, int]]:
    """Yield (file, start_line, end_line) tuples from cited file:line patterns."""
    seen: set[tuple[str, int, int]] = set()
    out: list[tuple[str, int, int]] = []
    for pat in _CITATION_PATTERNS:
        for m in pat.finditer(answer):
            file = m.group("file")
            # Filter false positives: must contain at least one '/' or '.' indicating a path.
            if "/" not in file and "." not in file:
                continue
            try:
                start = int(m.group("start"))
            except (IndexError, ValueError, TypeError):
                continue
            try:
                end_raw = m.group("end")
                end = int(end_raw) if end_raw else start
            except (IndexError, ValueError, TypeError):
                end = start
            key = (file, start, end)
            if key in seen:
                continue
            seen.add(key)
            out.append(key)
    return out


def compute_citation_multiplier(answer: str, repo_path: Path) -> tuple[float, list[CitationVerification]]:
    """Verify every file:line citation in the answer. Return (multiplier, per-citation results).

    Multiplier:
      0 hallucinations -> 1.0
      >=1 hallucination -> 0.5
      all citations hallucinated -> 0.0
    """
    cites = _extract_citations(answer)
    if not cites:
        return 1.0, []

    results: list[CitationVerification] = []
    for file, start, end in cites:
        full = repo_path / file
        if not full.exists():
            results.append(CitationVerification(
                raw_match=f"{file}:{start}-{end}",
                file=file, start_line=start, end_line=end,
                ok=False, reason="file_does_not_exist",
            ))
            continue
        try:
            lines = full.read_text().splitlines()
        except UnicodeDecodeError:
            results.append(CitationVerification(
                raw_match=f"{file}:{start}-{end}",
                file=file, start_line=start, end_line=end,
                ok=False, reason="binary_file",
            ))
            continue
        if start > len(lines):
            results.append(CitationVerification(
                raw_match=f"{file}:{start}-{end}",
                file=file, start_line=start, end_line=end,
                ok=False, reason="line_out_of_range",
            ))
            continue
        results.append(CitationVerification(
            raw_match=f"{file}:{start}-{end}",
            file=file, start_line=start, end_line=end,
            ok=True,
        ))

    total = len(results)
    bad = sum(1 for r in results if not r.ok)
    if bad == 0:
        multiplier = 1.0
    elif bad == total:
        multiplier = 0.0
    else:
        multiplier = 0.5
    return multiplier, results


def forbidden_hits(q: Question, answer: str) -> list[str]:
    a = answer.lower()
    return [f for f in q.expected.forbidden if f.lower() in a]


def final_mech(q: Question, answer: str, citation_multiplier: float) -> float:
    raw = mech_score(q, answer)["raw"]
    hits = forbidden_hits(q, answer)
    if q.expected.forbidden_strict and hits:
        return 0.0
    penalty = 0.25 * len(hits)
    return max(0.0, raw * citation_multiplier - penalty)

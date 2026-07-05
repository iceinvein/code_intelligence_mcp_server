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
    re.compile(r"(?P<file>[\w./\-]+\.\w+)\s+(?:at\s+)?lines?\s+(?P<start>\d+)(?:-(?P<end>\d+))?"),
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
        re.escape(c.file) + r"(?::|.{0,30}lines?\s+|.{0,30}\(L)(?P<start>\d+)(?:-(?P<end>\d+))?",
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


# Extensions accepted for bare-filename citations (no directory component). Without
# this gate, "127.0.0.1:17800", host:port pairs, and version strings parse as
# citations, fail verification, and get scored as hallucinations.
_CODE_EXTENSIONS = {
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "c", "h",
    "cpp", "hpp", "cc", "hh", "rb", "php", "swift", "kt", "kts", "scala", "sql",
    "sh", "bash", "zsh", "yaml", "yml", "toml", "json", "md", "txt", "html",
    "css", "scss", "vue", "svelte", "cfg", "ini", "xml", "proto", "graphql",
}


def _looks_like_path(file: str) -> bool:
    if "/" in file:
        return True
    ext = file.rsplit(".", 1)[-1].lower()
    return ext in _CODE_EXTENSIONS


def _extract_citations(answer: str) -> list[tuple[str, int, int]]:
    """Yield (file, start_line, end_line) tuples from cited file:line patterns."""
    seen: set[tuple[str, int, int]] = set()
    out: list[tuple[str, int, int]] = []
    for pat in _CITATION_PATTERNS:
        for m in pat.finditer(answer):
            file = m.group("file")
            if not _looks_like_path(file):
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


def _fs_line_reader(repo_path: Path):
    """Default line reader: the repo working tree. Returns None for missing/binary."""
    def read(file: str) -> list[str] | None:
        full = repo_path / file
        if not full.exists():
            return None
        try:
            return full.read_text().splitlines()
        except UnicodeDecodeError:
            return None
    return read


def compute_citation_multiplier(
    answer: str,
    repo_path: Path,
    read_lines=None,
) -> tuple[float, list[CitationVerification]]:
    """Verify every file:line citation in the answer. Return (multiplier, per-citation results).

    read_lines(file) -> list[str] | None overrides how file content is fetched
    (e.g. a pinned git tree instead of the working tree, which may have drifted).

    Multiplier:
      0 hallucinations -> 1.0
      >=1 hallucination -> 0.5
      all citations hallucinated -> 0.0
    """
    cites = _extract_citations(answer)
    if not cites:
        return 1.0, []
    if read_lines is None:
        read_lines = _fs_line_reader(repo_path)

    results: list[CitationVerification] = []
    for file, start, end in cites:
        lines = read_lines(file)
        if lines is None:
            results.append(CitationVerification(
                raw_match=f"{file}:{start}-{end}",
                file=file, start_line=start, end_line=end,
                ok=False, reason="file_does_not_exist",
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


# A forbidden-term mention is only a hit when it is affirmative. Correct answers to
# negative questions ("Is there a RedisCache?") must name the term while denying it
# ("there is no RedisCache"), so bare substring matching zeroes exactly the answers
# the fixture wants to reward.
_NEGATION_MARKERS = (
    "no ", "not ", "n't", "never", "without", "neither", "nor ",
    "absent", "lacks", "lack of", "instead of", "rather than",
)
_SENTENCE_BOUNDARIES = (".", "!", "?", "\n", ";")


def _is_negated_mention(answer_lower: str, idx: int, window: int = 80) -> bool:
    """True when a negation marker precedes idx within the same sentence."""
    ctx = answer_lower[max(0, idx - window):idx]
    for b in _SENTENCE_BOUNDARIES:
        p = ctx.rfind(b)
        if p != -1:
            ctx = ctx[p + 1:]
    return any(m in ctx for m in _NEGATION_MARKERS)


def forbidden_hits(q: Question, answer: str) -> list[str]:
    a = answer.lower()
    hits: list[str] = []
    for f in q.expected.forbidden:
        fl = f.lower()
        idx = 0
        while True:
            i = a.find(fl, idx)
            if i == -1:
                break
            if not _is_negated_mention(a, i):
                hits.append(f)
                break
            idx = i + len(fl)
    return hits


def final_mech(q: Question, answer: str, citation_multiplier: float) -> float:
    raw = mech_score(q, answer)["raw"]
    hits = forbidden_hits(q, answer)
    if q.expected.forbidden_strict and hits:
        return 0.0
    penalty = 0.25 * len(hits)
    return max(0.0, raw * citation_multiplier - penalty)

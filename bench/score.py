"""Scoring layers: mechanical, citation verification, forbidden penalty."""
from __future__ import annotations

import re
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from bench.fixtures_io import Citation, Question


# Matches: "src/foo.rs:42", "src/foo.rs:42-88", "[src/foo.rs:42]",
# "src/foo.rs line 42", "src/foo.rs at line 42-88"
_CITATION_PATTERNS = [
    re.compile(r"(?P<file>[\w./\-]+\.\w+):(?P<start>\d+)(?:[-–—](?P<end>\d+))?"),
    re.compile(
        r"(?P<file>[\w./\-]+\.\w+)\s*,?\s+(?:at\s+)?lines?\s+"
        r"(?P<start>\d+)(?:[-–—](?P<end>\d+))?"
    ),
]


@dataclass
class CitationVerification:
    raw_match: str
    file: str
    start_line: int
    end_line: int
    ok: bool
    reason: str = ""
    # Shortened-but-resolvable path (unique suffix match with the cited line in
    # range). Classifying these as hallucinations conflated citation *style*
    # with fabrication: R010-R012 diagnosis found ~0 true fabrications.
    imprecise: bool = False
    resolved_file: str | None = None


def _is_suffix_of(cited: str, full: str) -> bool:
    """True when `cited` names `full` exactly or as a path suffix on a '/' boundary."""
    return cited == full or full.endswith("/" + cited)


def _strip_markdown_emphasis(answer: str) -> str:
    """Drop asterisks and backticks so "**`x.py`**, line **183**" parses.

    Neither character can appear in a path or line number, and citation
    extraction works on values, not offsets, so a global strip is safe.
    Underscores stay: they are real path characters (__init__.py).
    """
    return re.sub(r"[*`]", "", answer)


def _cite_appears(c: Citation, answer: str) -> bool:
    """True when the answer cites c.file (full path, or an unambiguous path
    suffix) with a line number overlapping c.line_range."""
    answer = _strip_markdown_emphasis(answer)
    a = answer.lower()
    if c.file.lower() in a:
        pat = re.compile(
            re.escape(c.file) + r"(?::|.{0,30}lines?\s+|.{0,30}\(L)(?P<start>\d+)(?:-(?P<end>\d+))?",
            re.IGNORECASE,
        )
        for m in pat.finditer(answer):
            start = int(m.group("start"))
            end = int(m.group("end")) if m.group("end") else start
            if not (end < c.line_range[0] or start > c.line_range[1]):
                return True
    # Agents habitually shorten long paths in prose ("upgrade-helper.ts:83" for
    # packages/backend/src/api/crypto/upgrade-helper.ts). Accept a suffix-cited
    # expected file when the line range overlaps.
    for file, start, end in _extract_citations(answer):
        if _is_suffix_of(file, c.file) and not (end < c.line_range[0] or start > c.line_range[1]):
            return True
    return False


def _file_mentioned(expected: str, answer_lower: str) -> bool:
    """True when the answer names `expected` as a full path or a '/'-boundary
    suffix with at least two segments.

    The server's `cite` field hands agents short unique path forms
    ("lib/receipt-signer.ts"), so suffix mentions are genuine file coverage.
    Bare basenames stay excluded: they match too many files to be evidence.
    """
    expected = expected.lower()
    parts = expected.split("/")
    # Longest first so the common case (full path) exits early.
    suffixes = ["/".join(parts[-k:]) for k in range(len(parts), 1, -1)]
    if not suffixes:  # single-segment expected path: full-path match only
        suffixes = [expected]
    for suffix in suffixes:
        # Left boundary: the char before the match must not extend the path
        # ("mylib/receipt-signer.ts" must not satisfy "lib/receipt-signer.ts").
        pat = re.compile(r"(?<![\w./-])" + re.escape(suffix))
        if pat.search(answer_lower):
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
        file_hits = sum(1 for f in q.expected.files if _file_mentioned(f, a))
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
    # URL captures, not paths: "http://127.0.0.1:47831" matches the citation
    # regex as file "//127.0.0.1" + line 47831, and "https://x.dev/a:8080"
    # as file "//x.dev/a". Loopback callback URLs are genuine facts in auth
    # flows; flagging them conflated URLs with fabricated citations.
    if file.startswith("//") or "://" in file:
        return False
    if "/" in file:
        return True
    ext = file.rsplit(".", 1)[-1].lower()
    return ext in _CODE_EXTENSIONS


def _extract_citations(answer: str) -> list[tuple[str, int, int]]:
    """Yield (file, start_line, end_line) tuples from cited file:line patterns."""
    answer = _strip_markdown_emphasis(answer)
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


_FILE_LIST_CACHE: dict[str, list[str]] = {}


def _fs_file_lister(repo_path: Path):
    """Default repo file lister (git ls-files, falling back to a walk)."""
    key = str(repo_path.resolve())

    def list_files() -> list[str]:
        if key not in _FILE_LIST_CACHE:
            files: list[str] = []
            try:
                out = subprocess.run(
                    ["git", "-C", str(repo_path), "ls-files"],
                    capture_output=True, check=True,
                )
                files = out.stdout.decode().splitlines()
            except (subprocess.CalledProcessError, FileNotFoundError):
                pass
            if not files:
                files = [
                    str(p.relative_to(repo_path))
                    for p in repo_path.rglob("*")
                    if p.is_file() and ".git" not in p.parts
                ]
            _FILE_LIST_CACHE[key] = files
        return _FILE_LIST_CACHE[key]

    return list_files


def _resolve_by_suffix(
    file: str, start: int, read_lines, list_files, answer: str = "",
) -> tuple[str | None, str]:
    """Try to resolve a shortened path. Returns (resolved_file, failure_reason).

    A citation resolves when exactly one repo file matches the cited path as a
    suffix AND contains the cited start line. Multiple viable candidates fall
    back to answer context: agents name the full path once, then repeat a
    short form in prose (R030 django: django/db/backends/base/operations.py
    in the file list, "base/operations.py:18" in the body). A reader of the
    whole answer resolves the repeat, so when exactly one viable candidate's
    full path appears elsewhere in the answer, the citation resolves to it.
    Zero candidates means the path is fabricated.
    """
    candidates = [f for f in list_files() if _is_suffix_of(file, f)]
    if not candidates:
        return None, "file_does_not_exist"
    viable = []
    for cand in candidates:
        lines = read_lines(cand)
        if lines is not None and start <= len(lines):
            viable.append(cand)
    if len(viable) == 1:
        return viable[0], ""
    if len(viable) > 1:
        answer_lower = answer.lower()
        mentioned = [
            c for c in viable
            if re.search(r"(?<![\w./-])" + re.escape(c.lower()), answer_lower)
        ]
        if len(mentioned) == 1:
            return mentioned[0], ""
        return None, "ambiguous_suffix"
    return None, "line_out_of_range"


def compute_citation_multiplier(
    answer: str,
    repo_path: Path,
    read_lines=None,
    list_files=None,
) -> tuple[float, list[CitationVerification]]:
    """Verify every file:line citation in the answer. Return (multiplier, per-citation results).

    read_lines(file) -> list[str] | None and list_files() -> list[str] override
    how the repo tree is consulted (e.g. a pinned git tree instead of a working
    tree that may have drifted).

    Shortened paths that uniquely resolve by suffix (with the cited line in
    range) verify OK but are flagged imprecise. Only unresolvable citations
    count as hallucinated for the multiplier:
      0 hallucinations -> 1.0
      >=1 hallucination -> 0.5
      all citations hallucinated -> 0.0
    """
    cites = _extract_citations(answer)
    if not cites:
        return 1.0, []
    if read_lines is None:
        read_lines = _fs_line_reader(repo_path)
    if list_files is None:
        list_files = _fs_file_lister(repo_path)

    results: list[CitationVerification] = []
    for file, start, end in cites:
        lines = read_lines(file)
        if lines is None:
            resolved, reason = _resolve_by_suffix(file, start, read_lines, list_files, answer)
            if resolved is not None:
                results.append(CitationVerification(
                    raw_match=f"{file}:{start}-{end}",
                    file=file, start_line=start, end_line=end,
                    ok=True, imprecise=True, resolved_file=resolved,
                ))
            elif reason == "ambiguous_suffix":
                # The cited path matches >= 2 files that all contain the line
                # and the answer names neither in full. That is citation
                # STYLE, not fabrication: the file exists and the agent read
                # one of the candidates (R010-R012 diagnosis found ~0 true
                # fabrications; every flag since was a shortened cite). Count
                # it imprecise, keep the reason for diagnostics, and reserve
                # the multiplier for paths that resolve to nothing.
                results.append(CitationVerification(
                    raw_match=f"{file}:{start}-{end}",
                    file=file, start_line=start, end_line=end,
                    ok=True, imprecise=True, reason=reason,
                ))
            else:
                results.append(CitationVerification(
                    raw_match=f"{file}:{start}-{end}",
                    file=file, start_line=start, end_line=end,
                    ok=False, reason=reason,
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


# A forbidden-term mention is only a hit when it asserts the forbidden thing
# exists in THIS codebase. Correct answers to negative questions ("Is there a
# RedisCache?") must name the term while denying it ("there is no RedisCache",
# "search returned zero matches"), analogies ("Elysia's equivalent of app.use()
# in Express") compare rather than claim, and third-party attribution ("simplejwt
# provides JWTAuthentication") is the pointer the rubric asks for. Bare substring
# matching zeroes exactly the answers the fixture wants to reward.
_NEGATION_MARKERS = (
    "no ", "not ", "n't", "never", "without", "neither", "nor ",
    "absent", "lacks", "lack of", "instead of", "rather than",
    "zero ", "no match", "none",
    # analogy: the term is a comparison point, not a claim about this repo
    "equivalent", "similar to", "analogous", "akin to", "unlike",
    "counterpart", "as opposed to",
    # third-party attribution: naming what an external package provides
    "third-party", "third party", "github.com/", "pip install",
    "external package",
)
_SENTENCE_END = (".", "!", "?", ";")


def _sentence_around(answer_lower: str, idx: int, end: int, window: int = 120) -> str:
    """The sentence containing answer_lower[idx:end], clipped to +/- window chars.

    A '.', '!', '?', or ';' only ends a sentence when followed by whitespace or
    end-of-text, so code tokens like `app.use()` or `.py` do not truncate the
    scan. Newlines always end a sentence (list bullets are scored alone).
    """
    lo = max(0, idx - window)
    hi = min(len(answer_lower), end + window)
    start = lo
    for i in range(idx - 1, lo - 1, -1):
        c = answer_lower[i]
        if c == "\n" or (
            c in _SENTENCE_END
            and (i + 1 >= len(answer_lower) or answer_lower[i + 1].isspace())
        ):
            start = i + 1
            break
    stop = hi
    for i in range(end, hi):
        c = answer_lower[i]
        if c == "\n" or (
            c in _SENTENCE_END
            and (i + 1 >= len(answer_lower) or answer_lower[i + 1].isspace())
        ):
            stop = i + 1
            break
    return answer_lower[start:stop]


# A repo path token: an optional slash path ending in a filename with a code
# extension (src/foo/bar.rs, cache.py). Requiring the extension keeps prose
# slashes ("token obtain/refresh/verify") from reading as locations. URLs are
# stripped first so docs links do not read as in-repo locations.
_URL_RE = re.compile(r"(?:https?://|www\.)\S+")
_PATH_TOKEN_RE = re.compile(
    r"\b[\w.-]+(?:/[\w.-]+)*"
    r"\.(?:py|rs|ts|tsx|js|jsx|go|java|rb|c|cc|cpp|h|hpp|cs|kt|swift)\b"
)


def _locates_in_repo(sentence_lower: str) -> bool:
    return bool(_PATH_TOKEN_RE.search(_URL_RE.sub(" ", sentence_lower)))


def forbidden_hits(q: Question, answer: str) -> list[str]:
    """Forbidden terms the answer affirmatively claims exist in this repo.

    Once a term has at least one denied mention, the answer has established
    the term does not exist here; later affirmative mentions (third-party
    attribution, qualified paths of external packages) only count when their
    sentence locates the term in the repo via a path token.
    """
    a = answer.lower()
    hits: list[str] = []
    for f in q.expected.forbidden:
        fl = f.lower()
        mentions: list[tuple[bool, str]] = []  # (negated, sentence)
        idx = 0
        while True:
            i = a.find(fl, idx)
            if i == -1:
                break
            sentence = _sentence_around(a, i, i + len(fl))
            mentions.append((any(m in sentence for m in _NEGATION_MARKERS), sentence))
            idx = i + len(fl)
        if not mentions:
            continue
        denied = any(neg for neg, _ in mentions)
        for neg, sentence in mentions:
            if neg:
                continue
            if denied and not _locates_in_repo(sentence):
                continue
            hits.append(f)
            break
    return hits


def final_mech(q: Question, answer: str, citation_multiplier: float) -> float:
    raw = mech_score(q, answer)["raw"]
    hits = forbidden_hits(q, answer)
    if q.expected.forbidden_strict and hits:
        return 0.0
    penalty = 0.25 * len(hits)
    return max(0.0, raw * citation_multiplier - penalty)

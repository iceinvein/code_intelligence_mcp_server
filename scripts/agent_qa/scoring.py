"""Mechanical scoring: substring hit-rate of expected files/symbols/facts in the answer."""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import List

from scripts.agent_qa.qa_schema import QAEntry, FactGroup


@dataclass
class MechScore:
    files_hit: float
    symbols_hit: float
    facts_hit: float
    combined: float


def _normalize(s: str) -> str:
    return re.sub(r"\s+", " ", s).lower()


def _hit(needle: str, haystack: str) -> bool:
    return _normalize(needle) in haystack


def _group_hit(group: FactGroup, haystack: str) -> bool:
    if isinstance(group, list):
        return any(_hit(item, haystack) for item in group)
    return _hit(group, haystack)


def _ratio(matched: int, total: int) -> float:
    if total == 0:
        return 1.0  # vacuous: nothing required, nothing missing
    return matched / total


def mech_score(entry: QAEntry, answer: str) -> MechScore:
    hay = _normalize(answer)
    files_matched = sum(1 for f in entry.expected.files if _hit(f, hay))
    symbols_matched = sum(1 for s in entry.expected.symbols if _hit(s, hay))
    facts_matched = sum(1 for g in entry.expected.facts if _group_hit(g, hay))

    files_hit = _ratio(files_matched, len(entry.expected.files))
    symbols_hit = _ratio(symbols_matched, len(entry.expected.symbols))
    facts_hit = _ratio(facts_matched, len(entry.expected.facts))

    # Combined: weighted average over non-empty buckets so empty ones don't dominate.
    weights: List[tuple[float, float]] = []
    if entry.expected.files:
        weights.append((0.4, files_hit))
    if entry.expected.symbols:
        weights.append((0.4, symbols_hit))
    if entry.expected.facts:
        weights.append((0.2, facts_hit))
    if not weights:
        combined = 1.0
    else:
        wsum = sum(w for w, _ in weights)
        combined = sum(w * h for w, h in weights) / wsum

    return MechScore(
        files_hit=files_hit,
        symbols_hit=symbols_hit,
        facts_hit=facts_hit,
        combined=combined,
    )

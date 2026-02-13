# Search Quality Benchmark — Archive

This file contains the detailed round-by-round history, failure pattern catalog, and improvement workflow that was previously in `SEARCH_BENCHMARK.md`. The main file was condensed to focus on running benchmarks and current results.

## Common Failure Patterns

These are known issues to watch for when reviewing results. Each pattern maps to a specific area of the ranking pipeline.

### 1. Single-File Flooding (Severity: HIGH)

**Symptom:** 6-8 out of 10 results come from the same file.

**Root Cause:** No per-file diversity cap in result assembly.

**Fix Location:** `src/retrieval/ranking/diversify.rs` -- `diversify_by_file()` caps results per file to `max(limit/3, 2)`.

### 2. Keyword Semantic Mismatch (Severity: HIGH)

**Symptom:** Results match on keyword substrings rather than semantic relevance.

**Root Cause:** Symbol name field has too high a boost relative to code body for NL queries.

**Fix Location:** `src/retrieval/query.rs` or `src/storage/tantivy.rs` -- adjust field boosts.

### 3. Test Fixture Pollution (Severity: MEDIUM)

**Symptom:** Test helper functions rank above production code.

**Fix Location:** `src/retrieval/ranking/score.rs` -- test file penalty (currently -10.0).

### 4. Module Re-Export Noise (Severity: MEDIUM)

**Symptom:** One-liner `pub mod config;` re-exports rank #1.

**Fix Location:** `src/retrieval/ranking/score.rs` -- negative score for re-export-only symbols.

### 5. Meta-Matching (Severity: HIGH — Unfixable by BM25)

**Symptom:** Pattern-detection functions rank for queries about those patterns. E.g., `extract_concept_tags` (which checks for `json!(`, `response`, `formatting` in code) ranks #1 for "JSON serialization".

**Root Cause:** BM25 can't distinguish "code that detects a pattern" from "code that implements the pattern". The function's executable code contains the query terms as string literals.

**Fix:** Only vector/semantic search can solve this. BM25 approaches tried and failed: string literal stripping (R28), comment stripping (R35), concept tags (R26-R33).

### 6. Evaluator Noise

**Established noise floor:** ±0.3-0.5 points per round on CI average. Per-query can vary ±1-3 for ambiguous queries (Q2, Q3, Q15). Five no-code-change stability rounds (R32, R36, R38, R39, R44) confirm this.

**Mitigation:** Starting R47, the benchmark script produces deterministic raw results. Only the scoring phase is subjective.

## Key Lessons Learned

1. **Concept tags have zero value for common patterns** (R26-R33) — Tags like "error_handling" fire on 80%+ of files, giving near-zero IDF. Only rare tags ("websocket") discriminate.
2. **Pre-truncation diversity is dangerous** (R30) — Running `diversify_by_file` before truncation destroys targeted queries. Use `limit*3` pool expansion instead.
3. **Schema re-index shifts IDF statistics** — Can cause non-obvious regressions even with minor changes.
4. **Morphological variants cause IDF dilution if applied broadly** (R41) — Restrict to NAME tokens only, cap budget at 10-15.
5. **`expand_with_edges` can bypass scoring penalties** (R43) — Edge-expanded hits must go through intent enforcement.
6. **LLM descriptions are the most effective BM25 enhancement** (R47) — +0.80 improvement, bridging vocabulary gaps that morphological variants and concept tags couldn't.

## Detailed Round History

### Rounds 1-4 (Pre-Standardization)

Early rounds used partial query sets (9 queries in R1, 5 in R2-R4). Key finding: `diversify_by_file()` had an early-return bug (`hits.len() <= limit`) that prevented it from ever running. Fixed in commit `752ab54`.

- R1 CI avg: 5.3 (9 queries) | R5 CI avg: 3.9 (first full 15-query baseline)

### Round 5 (Full 15-Query Baseline)

CI avg: **3.9** | Augment avg: **9.5**

Key finding: The RRF scoring path completely bypassed `structural_adjustment()` and `intent_adjustment()`. These post-scoring signals were only in the non-RRF path which is never called during normal search.

### Rounds 6-12 (Core Scoring Fixes)

Major fixes applied:
- R6: RRF path now applies scoring signals. CI avg: 4.9 (+1.0)
- R7: Intent multipliers, definition boost. CI avg: 5.0
- R10: `term_coverage_adjustment` signal. CI avg: 5.7
- R11: `symbol_importance_adjustment`. CI avg: 5.9
- R12: `test_symbol_penalty` with in-file `#[cfg(test)]` detection. CI avg: 5.8

### Rounds 13-25 (Synonym Expansion Era)

Evaluator agents R13-R23 scored more generously (many 10s). R24+ recalibrated to stricter scoring. Cross-era comparisons need ±1-2 point adjustment.

- R24: Synonyms + term coverage refinements. CI avg: 6.5
- R25: Import tags + synonym expansion. CI avg: 6.7 (peak before concept tag era)

### Rounds 26-33 (Concept Tag Era)

Six rounds of concept tag work produced no measurable improvement on target queries (Q7, Q9, Q10). Key discovery in R32: `extract_concept_tags()` was dead code — never called from the pipeline. All R30-R31 score changes were evaluator noise.

- R26: Concept tags for function bodies (dead code). CI avg: 6.7
- R28: Strip string literals + concept tag fix. CI avg: 6.5
- R32: Stability check revealed concept tags were never connected. CI avg: 5.8
- R33: Concept tags wired in. Broad tags confirmed useless (80%+ IDF). CI avg: 6.2

### Rounds 34-37 (Comment Stripping + WebSocket Injection)

- R34: Removed broad concept tags, doubled test penalty to -10.0. Q13 +2. CI avg: 5.9
- R35: Comment stripping for BM25 index (schema v10). Removed doc-comment meta-matching but code-level meta-matching persists. CI avg: 6.0
- R37: WebSocket name injection (schema v12). Q7 2→3 (first movement in 32 rounds). CI avg: 5.5

### Rounds 38-39 (Stability Checks)

No code changes. Established evaluator noise floor at ±0.3-0.5 points.
- R38: CI avg: 5.3 | R39: CI avg: 5.5

### Rounds 40-43 (Vector Promotion + NL Descriptions)

- R40: Vector promotion (guaranteed slots). Didn't help stuck queries — BGE-base-en-v1.5 has same vocabulary gap as BM25. CI avg: 5.4
- R41: Morphological variants on ALL body identifiers. Q15 +3 but Q4/Q6/Q14 regressed -2 each (IDF dilution). CI avg: 5.4
- R42: Name-only variants fixed IDF dilution (schema v14). CI avg: 5.5
- R43: Intent enforcement pipeline fix + vector promotion bug fix. CI avg: 5.7. First round with zero regressions since R39.

### Rounds 44-45 (Stability Checks)

LLM description infrastructure merged but not yet generating descriptions for this codebase.
- R44: CI avg: 5.7 | R45: CI avg: 5.6

### Round 47 (LLM Descriptions Active)

See main `SEARCH_BENCHMARK.md` for full details. CI avg: **6.4** (+0.80, largest improvement in history).

use crate::retrieval::RankedHit;
use crate::storage::sqlite::SqliteStore;
use std::collections::{HashMap, HashSet};

/// Diversify results by file path to prevent any single file from dominating results.
///
/// Three-pass strategy:
/// 1. **Primary pass:** Up to `max_per_file` results per file (diverse selection)
/// 2. **Overflow pass:** Up to `max_per_file + 1` total per file (controlled relaxation)
/// 3. **Backfill pass:** Fill remaining slots ignoring caps (never return fewer than available)
///
/// This ensures diversity is *preferred* but slots are never wasted when the
/// underlying search legitimately returns results concentrated in one file.
pub fn diversify_by_file(hits: Vec<RankedHit>, limit: usize) -> Vec<RankedHit> {
    if hits.is_empty() {
        return hits;
    }

    let max_per_file = (limit / 5).max(2);
    let total_cap_per_file = max_per_file + 1;
    let mut out = Vec::with_capacity(limit.min(hits.len()));
    let mut deferred = Vec::new();
    let mut backfill = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    // Track files that already have a non-file (function/struct/impl) symbol selected.
    // File-level symbols from these files are redundant and should be deferred to backfill.
    let mut files_with_non_file: HashSet<String> = HashSet::new();

    // Precompute which files have non-file candidates in the pool.
    // When a file symbol arrives before its non-file siblings (due to higher score),
    // we defer it proactively so the named symbols get the diversity slots instead.
    let files_with_non_file_candidates: HashSet<String> = hits
        .iter()
        .filter(|h| h.kind != "file")
        .map(|h| h.file_path.clone())
        .collect();

    // Pass 1: diverse selection (up to max_per_file per file)
    for h in &hits {
        let n = counts.get(&h.file_path).copied().unwrap_or(0);
        if n < max_per_file {
            // Defer file symbols when non-file symbols from the same file exist
            // in the pool — named symbols are more useful than file-level entries.
            if h.kind == "file"
                && (files_with_non_file.contains(&h.file_path)
                    || files_with_non_file_candidates.contains(&h.file_path))
            {
                deferred.push(h.clone());
                continue;
            }
            *counts.entry(h.file_path.clone()).or_insert(0) += 1;
            if h.kind != "file" {
                files_with_non_file.insert(h.file_path.clone());
            }
            out.push(h.clone());
        } else {
            deferred.push(h.clone());
        }
    }

    // Pass 2: controlled overflow (up to total_cap per file)
    // Skip file-level symbols when a non-file symbol from the same file is already
    // selected — the file symbol is redundant and wastes a result slot.
    for h in deferred {
        if out.len() >= limit {
            break;
        }
        let n = counts.get(&h.file_path).copied().unwrap_or(0);
        if n < total_cap_per_file {
            if h.kind == "file" && files_with_non_file.contains(&h.file_path) {
                backfill.push(h);
                continue;
            }
            *counts.entry(h.file_path.clone()).or_insert(0) += 1;
            if h.kind != "file" {
                files_with_non_file.insert(h.file_path.clone());
            }
            out.push(h);
        } else {
            backfill.push(h);
        }
    }

    // Pass 3: backfill remaining slots (never waste a slot)
    for h in backfill {
        if out.len() >= limit {
            break;
        }
        out.push(h);
    }

    out
}

/// Diversify results by similarity cluster
pub fn diversify_by_cluster(
    sqlite: &SqliteStore,
    hits: Vec<RankedHit>,
    limit: usize,
) -> Vec<RankedHit> {
    if hits.is_empty() || limit <= 1 {
        return hits;
    }

    let max_per_cluster = 2usize;
    let mut out = Vec::with_capacity(limit.min(hits.len()));
    let mut deferred = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();

    for h in hits {
        if out.len() >= limit {
            break;
        }
        let key = sqlite.get_similarity_cluster_key(&h.id).ok().flatten();
        match key {
            Some(k) => {
                let n = counts.get(&k).copied().unwrap_or(0);
                if n < max_per_cluster {
                    counts.insert(k, n + 1);
                    out.push(h);
                } else {
                    deferred.push(h);
                }
            }
            None => out.push(h),
        }
    }

    for h in deferred {
        if out.len() >= limit {
            break;
        }
        out.push(h);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, score: f32, file: &str) -> RankedHit {
        RankedHit {
            id: id.to_string(),
            score,
            name: id.to_string(),
            kind: "function".to_string(),
            file_path: file.to_string(),
            exported: true,
            language: "rust".to_string(),
        }
    }

    fn file_hit(id: &str, score: f32, file: &str) -> RankedHit {
        RankedHit {
            id: id.to_string(),
            score,
            name: id.to_string(),
            kind: "file".to_string(),
            file_path: file.to_string(),
            exported: true,
            language: "rust".to_string(),
        }
    }

    #[test]
    fn diversify_prefers_diverse_results_at_limit_5() {
        // 4 from score.rs + 1 from rrf.rs, limit=5
        // max_per_file=2, total_cap=3
        // Pass 1: a1, a2 (score.rs, count≤2), b1(rrf.rs) → 3 results
        // Pass 2: a3(score.rs, count=2<3) → 1 overflow
        // Pass 3: a4 backfill → 1 more
        // Total: 5 results, rrf.rs promoted into early results
        let hits = vec![
            hit("a1", 10.0, "score.rs"),
            hit("a2", 9.0, "score.rs"),
            hit("a3", 8.0, "score.rs"),
            hit("a4", 7.0, "score.rs"),
            hit("b1", 6.0, "rrf.rs"),
        ];
        let result = diversify_by_file(hits, 5);
        assert_eq!(result.len(), 5, "Should return all 5 results");
        // rrf.rs should be promoted into early results
        assert!(result.iter().any(|h| h.file_path == "rrf.rs"));
        // First 3 results should include rrf.rs (promoted by diversity)
        let top3_files: Vec<&str> = result[..3].iter().map(|h| h.file_path.as_str()).collect();
        assert!(top3_files.contains(&"rrf.rs"), "rrf.rs should be in top 3");
    }

    #[test]
    fn diversify_backfills_when_single_file_dominates() {
        // All 5 results from one file, limit=5
        // max_per_file=1, total_cap=2
        // Pass 1: a1 → 1 result
        // Pass 2: a2 (count=1<2) → 1 overflow
        // Pass 3: a3, a4, a5 backfill → 3 more
        // Total: 5 results (never waste slots)
        let hits = vec![
            hit("a1", 10.0, "score.rs"),
            hit("a2", 9.0, "score.rs"),
            hit("a3", 8.0, "score.rs"),
            hit("a4", 7.0, "score.rs"),
            hit("a5", 6.0, "score.rs"),
        ];
        let result = diversify_by_file(hits, 5);
        assert_eq!(result.len(), 5, "Should return all 5 results via backfill");
    }

    #[test]
    fn diversify_promotes_diverse_files_at_limit_10() {
        // limit=10: max_per_file=2, total_cap=3
        // Pass 1: 2 each from 4 files = 8 results
        // Pass 2: a3 (overflow, count 2<3) = 9 results
        // Pass 3: a4 backfill = 10 results
        // Key: all 4 files represented in top 8 despite score.rs dominating by score
        let hits = vec![
            hit("a1", 10.0, "score.rs"),
            hit("a2", 9.5, "score.rs"),
            hit("a3", 9.0, "score.rs"),
            hit("a4", 8.5, "score.rs"),
            hit("b1", 8.0, "rrf.rs"),
            hit("b2", 7.5, "rrf.rs"),
            hit("c1", 7.0, "mod.rs"),
            hit("c2", 6.5, "mod.rs"),
            hit("d1", 6.0, "diversify.rs"),
            hit("d2", 5.5, "diversify.rs"),
        ];
        let result = diversify_by_file(hits, 10);
        assert_eq!(result.len(), 10, "Should return all 10 results");
        // All 4 files should be represented
        let unique_files: std::collections::HashSet<&str> =
            result.iter().map(|h| h.file_path.as_str()).collect();
        assert_eq!(unique_files.len(), 4, "All 4 files should be present");
        // rrf.rs, mod.rs, diversify.rs should all appear in the first 8 (primary pass)
        let top8_files: std::collections::HashSet<&str> =
            result[..8].iter().map(|h| h.file_path.as_str()).collect();
        assert!(top8_files.contains("rrf.rs"), "rrf.rs in primary pass");
        assert!(top8_files.contains("mod.rs"), "mod.rs in primary pass");
        assert!(top8_files.contains("diversify.rs"), "diversify.rs in primary pass");
    }

    #[test]
    fn diversify_preserves_all_when_diverse() {
        // All hits from different files — nothing should be dropped
        let hits = vec![
            hit("a", 10.0, "a.rs"),
            hit("b", 9.0, "b.rs"),
            hit("c", 8.0, "c.rs"),
            hit("d", 7.0, "d.rs"),
            hit("e", 6.0, "e.rs"),
        ];
        let result = diversify_by_file(hits, 5);
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn file_symbol_suppressed_when_function_from_same_file_exists() {
        // Q1-like scenario: fn from reranker.rs at #1, file symbol deferred.
        // With a 6th candidate from mod.rs available, the file symbol should be
        // skipped in pass 2 and the freed slot goes to mod.rs instead.
        let hits = vec![
            hit("apply_reranker_scores", 14.83, "reranker.rs"),
            hit("reciprocal_rank_fusion", 14.11, "rrf.rs"),
            file_hit("reranker.rs", 12.61, "reranker.rs"),
            hit("apply_popularity_boost", 12.32, "score.rs"),
            hit("structural_adjustment", 11.50, "score.rs"),
            hit("rank_hits_with_signals", 10.00, "mod.rs"),
        ];
        let result = diversify_by_file(hits, 5);
        assert_eq!(result.len(), 5);
        // The file-level "reranker.rs" should NOT be in top 5
        // since we already have apply_reranker_scores from that file
        assert!(
            !result.iter().any(|h| h.kind == "file" && h.file_path == "reranker.rs"),
            "File symbol should be suppressed when function from same file exists"
        );
        // mod.rs function should fill the freed slot
        assert!(
            result.iter().any(|h| h.name == "rank_hits_with_signals"),
            "mod.rs function should fill the freed slot"
        );
    }

    #[test]
    fn file_symbol_deferred_when_non_file_candidates_exist() {
        // Q8-like scenario: file symbol is the top result (score 1010)
        // but non-file symbols exist from the same file.
        // Named symbols should take priority over the file symbol.
        let hits = vec![
            file_hit("schema.rs", 1010.0, "schema.rs"),
            hit("TodoRow", 419.0, "schema.rs"),
            hit("RepositoryRow", 367.0, "schema.rs"),
            hit("PackageRow", 361.0, "schema.rs"),
            hit("setup_test_db", 4.19, "descriptions.rs"),
        ];
        let result = diversify_by_file(hits, 5);
        assert_eq!(result.len(), 5);
        // Named symbols should take the file's diversity slots
        assert_eq!(result[0].name, "TodoRow");
        assert_eq!(result[1].name, "RepositoryRow");
        // File symbol deferred to overflow/backfill
        assert!(
            result.iter().any(|h| h.kind == "file" && h.file_path == "schema.rs"),
            "File symbol should still appear via overflow/backfill"
        );
    }

    #[test]
    fn all_file_symbols_preserved_when_no_functions() {
        // Q5-like scenario: all results are file-level symbols
        // No suppression should happen since no functions exist to prefer
        let hits = vec![
            file_hit("pipeline/mod.rs", 15.76, "pipeline/mod.rs"),
            file_hit("extract/cpp.rs", 15.59, "extract/cpp.rs"),
            file_hit("pipeline/scan.rs", 15.59, "pipeline/scan.rs"),
            file_hit("pipeline/parallel.rs", 15.01, "pipeline/parallel.rs"),
            file_hit("extract/c.rs", 11.71, "extract/c.rs"),
        ];
        let result = diversify_by_file(hits, 5);
        assert_eq!(result.len(), 5, "All file symbols should be preserved");
        assert!(result.iter().all(|h| h.kind == "file"));
    }

    #[test]
    fn file_symbol_backfilled_when_slots_remain() {
        // File symbol is deferred from pass 1 (non-file candidate exists)
        // but backfilled in pass 3 when there are empty slots.
        let hits = vec![
            hit("fn_a", 10.0, "a.rs"),
            file_hit("a.rs", 9.0, "a.rs"),
            hit("fn_b", 8.0, "b.rs"),
        ];
        let result = diversify_by_file(hits, 5);
        assert_eq!(result.len(), 3, "All candidates should be returned");
        // fn_a should come before the file symbol from the same file
        assert_eq!(result[0].name, "fn_a");
        assert!(
            result.iter().any(|h| h.kind == "file"),
            "File symbol should be backfilled when slots remain"
        );
    }

    #[test]
    fn file_symbol_backfilled_when_no_function_from_same_file() {
        // File symbol from a.rs has no competing function — it should be kept.
        let hits = vec![
            hit("fn_b", 10.0, "b.rs"),
            file_hit("a.rs", 9.0, "a.rs"),
            hit("fn_c", 8.0, "c.rs"),
        ];
        let result = diversify_by_file(hits, 5);
        assert_eq!(result.len(), 3);
        assert!(
            result.iter().any(|h| h.kind == "file" && h.file_path == "a.rs"),
            "File symbol should be kept when no function from same file"
        );
    }
}

/// Check if a kind represents a definition
pub fn is_definition_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class"
            | "interface"
            | "type_alias"
            | "struct"
            | "enum"
            | "function"
            | "method"
            | "const"
            | "trait"
            | "module"
    )
}

/// Diversify results by kind (definitions, tests, others)
pub fn diversify_by_kind(hits: Vec<RankedHit>, limit: usize) -> Vec<RankedHit> {
    if hits.len() <= limit {
        return hits;
    }

    let mut defs = Vec::new();
    let mut tests = Vec::new();
    let mut others = Vec::new();

    for h in hits {
        let is_test = h.file_path.contains(".test.")
            || h.file_path.contains(".spec.")
            || h.file_path.contains("/tests/")
            || h.file_path.contains("/__tests__/");

        if is_test {
            tests.push(h);
        } else if is_definition_kind(&h.kind) {
            defs.push(h);
        } else {
            others.push(h);
        }
    }

    let mut out = Vec::with_capacity(limit);
    let mut d_idx = 0;
    let mut t_idx = 0;
    let mut o_idx = 0;

    // Ensure diversity: pick top 1 from each category if available
    if d_idx < defs.len() {
        out.push(defs[d_idx].clone());
        d_idx += 1;
    }
    if o_idx < others.len() && out.len() < limit {
        out.push(others[o_idx].clone());
        o_idx += 1;
    }
    if t_idx < tests.len() && out.len() < limit {
        out.push(tests[t_idx].clone());
        t_idx += 1;
    }

    // Fill the rest by score
    while out.len() < limit {
        let d_score = defs.get(d_idx).map(|h| h.score).unwrap_or(-1.0);
        let t_score = tests.get(t_idx).map(|h| h.score).unwrap_or(-1.0);
        let o_score = others.get(o_idx).map(|h| h.score).unwrap_or(-1.0);

        if d_score < 0.0 && t_score < 0.0 && o_score < 0.0 {
            break;
        }

        if d_score >= t_score && d_score >= o_score {
            out.push(defs[d_idx].clone());
            d_idx += 1;
        } else if t_score >= d_score && t_score >= o_score {
            out.push(tests[t_idx].clone());
            t_idx += 1;
        } else {
            out.push(others[o_idx].clone());
            o_idx += 1;
        }
    }

    out
}

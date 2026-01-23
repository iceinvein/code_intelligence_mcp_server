use crate::config::Config;
use crate::retrieval::query::Intent;
use crate::retrieval::{HitSignals, RankedHit};
use crate::storage::sqlite::SqliteStore;
use crate::storage::tantivy::SearchHit as KeywordHit;
use crate::storage::vector::VectorHit;
use anyhow::Result;
use std::collections::HashMap;

use super::diversify::is_definition_kind;

/// Rank hits from keyword and vector search
#[cfg(test)]
pub fn rank_hits(
    keyword_hits: &[KeywordHit],
    vector_hits: &[VectorHit],
    config: &Config,
    intent: &Option<Intent>,
    query: &str,
) -> Vec<RankedHit> {
    rank_hits_with_signals(keyword_hits, vector_hits, config, intent, query).0
}

/// Rank hits and return signals for debugging
pub fn rank_hits_with_signals(
    keyword_hits: &[KeywordHit],
    vector_hits: &[VectorHit],
    config: &Config,
    intent: &Option<Intent>,
    query: &str,
) -> (Vec<RankedHit>, HashMap<String, HitSignals>) {
    let mut max_kw = 0.0f32;
    for h in keyword_hits {
        if h.score > max_kw {
            max_kw = h.score;
        }
    }

    let mut max_vec = 0.0f32;
    let mut vec_scores = HashMap::new();
    for h in vector_hits {
        let dist = h.distance.unwrap_or(1.0);
        let sim = 1.0 / (1.0 + dist.max(0.0));
        vec_scores.insert(h.id.clone(), sim);
        if sim > max_vec {
            max_vec = sim;
        }
    }

    let mut kw_scores = HashMap::new();
    for h in keyword_hits {
        let s = if max_kw > 0.0 { h.score / max_kw } else { 0.0 };
        kw_scores.insert(h.id.clone(), s);
    }

    let mut merged = HashMap::<String, RankedHit>::new();
    let mut signals = HashMap::<String, HitSignals>::new();

    let (vector_w, keyword_w) =
        normalize_pair(config.rank_vector_weight, config.rank_keyword_weight);

    // Process vector hits
    for h in vector_hits {
        let v = vec_scores.get(&h.id).copied().unwrap_or(0.0);
        let v = if max_vec > 0.0 { v / max_vec } else { 0.0 };
        let kw = kw_scores.get(&h.id).copied().unwrap_or(0.0);
        let base_score = vector_w * v + keyword_w * kw;
        let structural = structural_adjustment(config, h.exported, &h.file_path, intent, query);
        let intent_mult = intent_adjustment(intent, &h.kind, &h.file_path, h.exported);
        let mut score = (base_score + structural) * intent_mult;

        // Definition Bias
        let mut definition_bias = 0.0;
        if !matches!(intent, Some(Intent::Callers(_))) {
            let q = query.trim();
            let q_no_space = q.replace(' ', "");
            if (h.name.eq_ignore_ascii_case(q) || h.name.eq_ignore_ascii_case(&q_no_space))
                && is_definition_kind(&h.kind)
            {
                score += 10.0;
                definition_bias += 10.0;
            } else if h.name.to_lowercase().contains(&q.to_lowercase())
                && is_definition_kind(&h.kind)
            {
                score += 1.0;
                definition_bias += 1.0;
            }
        }

        signals.insert(
            h.id.clone(),
            HitSignals {
                keyword_score: kw,
                vector_score: v,
                base_score,
                structural_adjust: structural,
                intent_mult,
                definition_bias,
                popularity_boost: 0.0,
            },
        );

        merged.insert(
            h.id.clone(),
            RankedHit {
                id: h.id.clone(),
                score,
                name: h.name.clone(),
                kind: h.kind.clone(),
                file_path: h.file_path.clone(),
                exported: h.exported,
                language: h.language.clone(),
            },
        );
    }

    // Process keyword hits
    for h in keyword_hits {
        let kw = kw_scores.get(&h.id).copied().unwrap_or(0.0);
        let v = vec_scores.get(&h.id).copied().unwrap_or(0.0);
        let v = if max_vec > 0.0 { v / max_vec } else { 0.0 };
        let base_score = vector_w * v + keyword_w * kw;
        let structural = structural_adjustment(config, h.exported, &h.file_path, intent, query);
        let intent_mult = intent_adjustment(intent, &h.kind, &h.file_path, h.exported);
        let mut score = (base_score + structural) * intent_mult;

        // Definition Bias
        let mut definition_bias = 0.0;
        if !matches!(intent, Some(Intent::Callers(_))) {
            let q = query.trim();
            let q_no_space = q.replace(' ', "");
            if (h.name.eq_ignore_ascii_case(q) || h.name.eq_ignore_ascii_case(&q_no_space))
                && is_definition_kind(&h.kind)
            {
                score += 10.0;
                definition_bias += 10.0;
            } else if h.name.to_lowercase().contains(&q.to_lowercase())
                && is_definition_kind(&h.kind)
            {
                score += 1.0;
                definition_bias += 1.0;
            }
        }

        signals.insert(
            h.id.clone(),
            HitSignals {
                keyword_score: kw,
                vector_score: v,
                base_score,
                structural_adjust: structural,
                intent_mult,
                definition_bias,
                popularity_boost: 0.0,
            },
        );

        merged
            .entry(h.id.clone())
            .and_modify(|existing| {
                if score > existing.score {
                    existing.score = score;
                }
                if existing.name.is_empty() {
                    existing.name = h.name.clone();
                }
                if existing.kind.is_empty() {
                    existing.kind = h.kind.clone();
                }
                if existing.file_path.is_empty() {
                    existing.file_path = h.file_path.clone();
                }
                existing.exported = existing.exported || h.exported;
            })
            .or_insert_with(|| RankedHit {
                id: h.id.clone(),
                score,
                name: h.name.clone(),
                kind: h.kind.clone(),
                file_path: h.file_path.clone(),
                exported: h.exported,
                language: "".to_string(),
            });
    }

    let mut out = merged.into_values().collect::<Vec<_>>();
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.file_path.cmp(&b.file_path))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.id.cmp(&b.id))
    });
    (out, signals)
}

/// Apply popularity boost based on incoming edges
///
/// #[deprecated] Note: This function uses O(N) database queries and is replaced by
/// apply_popularity_boost_with_signals which uses batch PageRank lookup.
#[deprecated(note = "Use apply_popularity_boost_with_signals for O(1) batch PageRank lookup")]
#[cfg(test)]
pub fn apply_popularity_boost(
    sqlite: &SqliteStore,
    mut hits: Vec<RankedHit>,
    config: &Config,
) -> Result<Vec<RankedHit>> {
    if hits.is_empty() || config.rank_popularity_weight == 0.0 || config.rank_popularity_cap == 0 {
        return Ok(hits);
    }

    for h in hits.iter_mut() {
        let count = sqlite.count_incoming_edges(&h.id).unwrap_or(0);
        let capped = count.min(config.rank_popularity_cap) as f32;
        let denom = config.rank_popularity_cap as f32;
        if denom > 0.0 {
            h.score += config.rank_popularity_weight * (capped / denom);
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.file_path.cmp(&b.file_path))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(hits)
}

/// Apply popularity boost with signals tracking using PageRank scores
///
/// This function boosts search result scores based on symbol PageRank from the
/// symbol_metrics table. PageRank considers the importance of linking symbols,
/// not just the count of incoming edges.
///
/// The PageRank scores are normalized to the 0-1 range before applying the
/// configured weight, ensuring consistent boost magnitudes across different
/// codebases.
pub fn apply_popularity_boost_with_signals(
    sqlite: &SqliteStore,
    mut hits: Vec<RankedHit>,
    hit_signals: &mut HashMap<String, HitSignals>,
    config: &Config,
) -> Result<Vec<RankedHit>> {
    if hits.is_empty() || config.rank_popularity_weight == 0.0 {
        return Ok(hits);
    }

    // Collect symbol IDs for batch lookup
    let symbol_ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();

    // Batch load PageRank scores from symbol_metrics table
    let pagerank_map = sqlite
        .batch_get_symbol_metrics(&symbol_ids)
        .unwrap_or_default();

    // Find max PageRank for normalization
    let max_pagerank = pagerank_map
        .values()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);

    // Avoid division by zero - if all PageRanks are 0 or empty, skip boost
    if max_pagerank <= 0.0 {
        return Ok(hits);
    }

    // Apply normalized PageRank boost to each hit
    for h in hits.iter_mut() {
        let pagerank = pagerank_map.get(&h.id).copied().unwrap_or(0.0);
        let normalized = pagerank / max_pagerank;
        let boost = config.rank_popularity_weight * normalized as f32;

        h.score += boost;
        hit_signals
            .entry(h.id.clone())
            .and_modify(|s| s.popularity_boost += boost)
            .or_insert(HitSignals {
                keyword_score: 0.0,
                vector_score: 0.0,
                base_score: 0.0,
                structural_adjust: 0.0,
                intent_mult: 1.0,
                definition_bias: 0.0,
                popularity_boost: boost,
            });
    }

    // Re-sort by score after applying boosts
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.file_path.cmp(&b.file_path))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(hits)
}

fn normalize_pair(a: f32, b: f32) -> (f32, f32) {
    let sum = a + b;
    if sum > 0.0 {
        (a / sum, b / sum)
    } else {
        (0.5, 0.5)
    }
}

fn structural_adjustment(
    config: &Config,
    exported: bool,
    file_path: &str,
    _intent: &Option<Intent>,
    query: &str,
) -> f32 {
    let mut score = 0.0;
    if exported {
        score += config.rank_exported_boost;
    }

    // Glue Code Filtering
    if file_path.ends_with("index.ts") || file_path.ends_with("index.tsx") {
        score -= 5.0;
    }

    let path = file_path.to_lowercase();
    if path.contains("/node_modules/")
        || path.contains("/target/")
        || path.contains("/dist/")
        || path.contains("/build/")
        || path.contains("/vendor/")
        || path.contains("/generated/")
        || path.contains("/gen/")
        || path.contains(".min.")
    {
        score -= 15.0;
    }

    if path.contains("/src/")
        || path.contains("/lib/")
        || path.contains("/app/")
        || path.contains("/packages/")
    {
        score += 1.0;
    }

    // Subdirectory Semantics
    let terms: Vec<&str> = query
        .split_whitespace()
        .map(|s| s.trim())
        .filter(|s| s.len() > 2)
        .collect();

    let path_parts: Vec<&str> = file_path.split('/').collect();
    for term in terms {
        if path_parts.iter().any(|p| {
            if p.eq_ignore_ascii_case(term) {
                return true;
            }
            if let Some((stem, _)) = p.rsplit_once('.') {
                if stem.eq_ignore_ascii_case(term) {
                    return true;
                }
            }
            false
        }) {
            score += 2.0;
        }
    }

    score
}

fn intent_adjustment(intent: &Option<Intent>, kind: &str, file_path: &str, exported: bool) -> f32 {
    // Test Penalty (0.5x multiplier)
    let is_test = file_path.contains(".test.")
        || file_path.contains(".spec.")
        || file_path.contains("/__tests__/")
        || file_path.contains("/tests/");

    if is_test && !matches!(intent, Some(Intent::Test)) {
        return 0.5;
    }

    let Some(intent) = intent else {
        return 1.0;
    };

    match intent {
        Intent::Definition => {
            let is_def = matches!(
                kind,
                "class" | "interface" | "type_alias" | "struct" | "enum" | "const"
            );
            if is_def && exported {
                1.5
            } else {
                1.0
            }
        }
        Intent::Schema => {
            let path = file_path.to_lowercase();
            if path.contains("schema") {
                75.0
            } else if path.contains("model") || path.contains("entity") || path.contains("entities")
            {
                50.0
            } else if path.contains("db/")
                || path.contains("database/")
                || path.contains("migrations/")
                || path.contains("sql/")
            {
                25.0
            } else {
                0.5
            }
        }
        Intent::Callers(_) => 1.0,
        Intent::Test => 1.0,
        // New intents (FNDN-15) - default multiplier of 1.0
        Intent::Implementation => 1.0,
        Intent::Config => 1.0,
        Intent::Error => 1.0,
        Intent::API => 1.0,
        Intent::Hook => 1.0,
        Intent::Middleware => 1.0,
        Intent::Migration => 1.0,
    }
}

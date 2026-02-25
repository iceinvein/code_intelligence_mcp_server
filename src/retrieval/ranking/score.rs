use crate::config::Config;
use crate::retrieval::query::Intent;
use crate::retrieval::{HitSignals, RankedHit};
use crate::storage::sqlite::SqliteStore;
use crate::storage::tantivy::SearchHit as KeywordHit;
use crate::storage::vector::VectorHit;
use crate::text;
use anyhow::Result;
use std::collections::HashMap;

use super::diversify::is_definition_kind;

/// Apply selection boost with signals tracking based on user selection history
///
/// This function boosts search result scores based on previous user selections
/// for the same query-symbol pairs. Users tend to select the same symbols for
/// the same queries, indicating relevance.
///
/// The boost is computed from query_selections table considering:
/// - Position bias: selections at higher positions get more weight
/// - Time decay: recent selections have more influence than old ones
pub fn apply_selection_boost_with_signals(
    sqlite: &SqliteStore,
    mut hits: Vec<RankedHit>,
    hit_signals: &mut HashMap<String, HitSignals>,
    query_normalized: &str,
    config: &Config,
) -> Result<Vec<RankedHit>> {
    if hits.is_empty() || !config.learning_enabled || config.learning_selection_boost == 0.0 {
        return Ok(hits);
    }

    // Build (query, symbol_id) pairs for batch lookup
    let pairs: Vec<(String, String)> = hits
        .iter()
        .map(|h| (query_normalized.to_string(), h.id.clone()))
        .collect();

    // Batch load selection boost scores
    let boost_map = match sqlite.batch_get_selection_boosts(&pairs) {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!(
                error = %e,
                pair_count = pairs.len(),
                "Selection boost lookup failed, using empty boosts (degraded learning)"
            );
            HashMap::new()
        }
    };

    // Normalize boosts to [0, 1] range before applying config weight,
    // so the max boost is always exactly learning_selection_boost.
    let max_boost = boost_map
        .values()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);

    if max_boost <= 0.0 {
        return Ok(hits);
    }

    // Apply normalized boosts to hits
    for h in hits.iter_mut() {
        let key = format!("{}|{}", query_normalized, h.id);
        let boost = boost_map.get(&key).copied().unwrap_or(0.0);

        if boost > 0.0 {
            let normalized = boost / max_boost;
            let final_boost = config.learning_selection_boost * normalized;
            h.score += final_boost;

            hit_signals
                .entry(h.id.clone())
                .and_modify(|s| s.learning_boost += final_boost)
                .or_insert_with(|| HitSignals {
                    keyword_score: 0.0,
                    vector_score: 0.0,
                    base_score: 0.0,
                    structural_adjust: 0.0,
                    intent_mult: 1.0,
                    definition_bias: 0.0,
                    term_coverage: 0.0,
                    symbol_importance: 0.0,
                    test_symbol_penalty: 0.0,
                    popularity_boost: 0.0,
                    learning_boost: final_boost,
                    affinity_boost: 0.0,
                    docstring_boost: 0.0,
                    package_boost: 0.0,
                });
        }
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

/// Apply file affinity boost with signals tracking
///
/// This function boosts search result scores based on user file affinity patterns.
/// Files that are frequently viewed or edited receive higher affinity scores,
/// which decay over time to favor recent engagement.
///
/// The affinity boost is computed from user_file_affinity table considering:
/// - View count (1x weight): how often a file is viewed
/// - Edit count (2x weight): edits indicate stronger engagement than views
/// - Time decay: exp(-0.05 * age_in_days) with lambda=0.05 (slower than selections)
///
/// Affinity scores are normalized to the 0-1 range before applying the
/// configured boost weight.
pub fn apply_file_affinity_boost_with_signals(
    sqlite: &SqliteStore,
    mut hits: Vec<RankedHit>,
    hit_signals: &mut HashMap<String, HitSignals>,
    config: &Config,
) -> Result<Vec<RankedHit>> {
    if hits.is_empty() || !config.learning_enabled || config.learning_file_affinity_boost == 0.0 {
        return Ok(hits);
    }

    // Collect unique file paths from hits
    let mut file_paths_set = std::collections::HashSet::new();
    for h in &hits {
        file_paths_set.insert(h.file_path.as_str());
    }
    let file_paths: Vec<&str> = file_paths_set.into_iter().collect();

    // Batch load affinity boost scores
    let affinity_map = match sqlite.batch_get_affinity_boosts(&file_paths) {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!(
                error = %e,
                file_count = file_paths.len(),
                "File affinity boost lookup failed, using empty boosts (degraded learning)"
            );
            HashMap::new()
        }
    };

    // Find max affinity_score for normalization (avoid division by zero)
    let max_affinity = affinity_map
        .values()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);

    // Avoid division by zero - if all affinities are 0 or empty, skip boost
    if max_affinity <= 0.0 {
        return Ok(hits);
    }

    // Apply normalized affinity boost to each hit
    for h in hits.iter_mut() {
        let affinity = affinity_map.get(&h.file_path).copied().unwrap_or(0.0);
        if affinity > 0.0 {
            let normalized = affinity / max_affinity;
            let final_boost = config.learning_file_affinity_boost * normalized;
            h.score += final_boost;

            hit_signals
                .entry(h.id.clone())
                .and_modify(|s| s.affinity_boost += final_boost)
                .or_insert_with(|| HitSignals {
                    keyword_score: 0.0,
                    vector_score: 0.0,
                    base_score: 0.0,
                    structural_adjust: 0.0,
                    intent_mult: 1.0,
                    definition_bias: 0.0,
                    term_coverage: 0.0,
                    symbol_importance: 0.0,
                    test_symbol_penalty: 0.0,
                    popularity_boost: 0.0,
                    learning_boost: 0.0,
                    affinity_boost: final_boost,
                    docstring_boost: 0.0,
                    package_boost: 0.0,
                });
        }
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
        let structural = structural_adjustment(config, h.exported, &h.file_path, &h.kind, &h.name, intent, query);
        let intent_mult = intent_adjustment(intent, &h.kind, &h.file_path, h.exported, &h.name);
        let def_bias = definition_bias(query, &h.name, &h.kind, intent);
        let tc = term_coverage_adjustment(query, &h.name, &h.file_path, None);
        let score = (base_score + structural + def_bias + tc) * intent_mult;

        signals.insert(
            h.id.clone(),
            HitSignals {
                keyword_score: kw,
                vector_score: v,
                base_score,
                structural_adjust: structural,
                intent_mult,
                definition_bias: def_bias,
                term_coverage: tc,
                symbol_importance: 0.0,
                test_symbol_penalty: 0.0,
                popularity_boost: 0.0,
                learning_boost: 0.0,
                affinity_boost: 0.0,
                docstring_boost: 0.0,
                package_boost: 0.0,
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
        let structural = structural_adjustment(config, h.exported, &h.file_path, &h.kind, &h.name, intent, query);
        let intent_mult = intent_adjustment(intent, &h.kind, &h.file_path, h.exported, &h.name);
        let def_bias = definition_bias(query, &h.name, &h.kind, intent);
        let tc = term_coverage_adjustment(query, &h.name, &h.file_path, None);
        let score = (base_score + structural + def_bias + tc) * intent_mult;

        signals.insert(
            h.id.clone(),
            HitSignals {
                keyword_score: kw,
                vector_score: v,
                base_score,
                structural_adjust: structural,
                intent_mult,
                definition_bias: def_bias,
                term_coverage: tc,
                symbol_importance: 0.0,
                test_symbol_penalty: 0.0,
                popularity_boost: 0.0,
                learning_boost: 0.0,
                affinity_boost: 0.0,
                docstring_boost: 0.0,
                package_boost: 0.0,
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
    let pagerank_map = match sqlite.batch_get_symbol_metrics(&symbol_ids) {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!(
                error = %e,
                symbol_count = symbol_ids.len(),
                "PageRank metrics lookup failed, using empty scores (degraded popularity ranking)"
            );
            HashMap::new()
        }
    };

    // Find max PageRank for normalization
    let max_pagerank = pagerank_map
        .values()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);

    // Avoid division by zero - if all PageRanks are 0 or empty, skip boost
    if max_pagerank <= 0.0 {
        return Ok(hits);
    }

    // Scale the popularity weight relative to current score magnitudes.
    // Without scaling, the absolute boost (default 0.05) can dwarf small RRF scores
    // (~0.01-0.04), causing high-PageRank symbols to dominate unrelated queries.
    let avg_score = if hits.is_empty() {
        1.0
    } else {
        let sum: f32 = hits.iter().map(|h| h.score.abs()).sum();
        (sum / hits.len() as f32).max(0.001)
    };
    let scaled_weight = config.rank_popularity_weight * avg_score;

    // Apply normalized PageRank boost to each hit
    for h in hits.iter_mut() {
        let pagerank = pagerank_map.get(&h.id).copied().unwrap_or(0.0);
        let normalized = pagerank / max_pagerank;
        let boost = scaled_weight * normalized as f32;

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
                term_coverage: 0.0,
                popularity_boost: boost,
                learning_boost: 0.0,
                affinity_boost: 0.0,
                docstring_boost: 0.0,
                package_boost: 0.0,
                symbol_importance: 0.0,
                test_symbol_penalty: 0.0,
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

/// Apply JSDoc documentation boost with signals tracking
///
/// This function boosts search result scores for symbols that have JSDoc documentation.
/// Symbols with JSDoc receive a 1.5x boost to promote well-documented code.
pub fn apply_docstring_boost_with_signals(
    sqlite: &SqliteStore,
    mut hits: Vec<RankedHit>,
    hit_signals: &mut HashMap<String, HitSignals>,
) -> Result<Vec<RankedHit>> {
    const DOCSTRING_BOOST: f32 = 0.5; // 1.5x multiplier = 1.0 + 0.5 boost

    for h in hits.iter_mut() {
        if sqlite.has_docstring(&h.id).unwrap_or(false) {
            h.score *= 1.5;

            hit_signals
                .entry(h.id.clone())
                .and_modify(|s| {
                    s.docstring_boost += DOCSTRING_BOOST;
                    // Also adjust base_score to reflect the 1.5x multiplier
                    s.base_score *= 1.5;
                })
                .or_insert(HitSignals {
                    keyword_score: 0.0,
                    vector_score: 0.0,
                    base_score: 0.0,
                    structural_adjust: 0.0,
                    intent_mult: 1.0,
                    definition_bias: 0.0,
                    term_coverage: 0.0,
                    popularity_boost: 0.0,
                    learning_boost: 0.0,
                    affinity_boost: 0.0,
                    docstring_boost: DOCSTRING_BOOST,
                    package_boost: 0.0,
                    symbol_importance: 0.0,
                    test_symbol_penalty: 0.0,
                });
        }
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

/// Compute definition bias for a hit based on query-to-name matching.
///
/// For short queries (1-2 words): Strong boost for exact name match (+10) or substring (+1).
/// For longer queries (3+ words): Check if the symbol name matches any significant query
/// token. This handles queries like "EmbeddingCache get put" where "EmbeddingCache" should
/// boost the EmbeddingCache struct.
pub(crate) fn definition_bias(
    query: &str,
    hit_name: &str,
    hit_kind: &str,
    intent: &Option<Intent>,
) -> f32 {
    if matches!(intent, Some(Intent::Callers(_))) || !is_definition_kind(hit_kind) {
        return 0.0;
    }

    let q = query.trim();
    let word_count = q.split_whitespace().count();

    if word_count <= 2 {
        // Short query: strong exact match bias
        let q_no_space = q.replace(' ', "");
        if hit_name.eq_ignore_ascii_case(q) || hit_name.eq_ignore_ascii_case(&q_no_space) {
            10.0
        } else if hit_name.to_lowercase().contains(&q.to_lowercase()) {
            1.0
        } else {
            0.0
        }
    } else {
        // Multi-word query: check each token for symbol name match.
        // Look for CamelCase or snake_case tokens that could be symbol names.
        let name_lower = hit_name.to_lowercase();
        let mut best = 0.0f32;

        for token in q.split_whitespace() {
            if token.len() < 3 {
                continue;
            }
            let token_lower = token.to_lowercase();

            // Exact match with a query token — but only give the strong 5.0
            // boost when the token looks like a symbol name (has interior
            // uppercase like CamelCase or contains underscore like snake_case).
            // This prevents common English words in NL queries (e.g. "error"
            // in "Error handling") from inflating a function literally named
            // "error" above more relevant results. (R65)
            if name_lower == token_lower
                && (token.chars().skip(1).any(|c| c.is_uppercase())
                    || token.contains('_'))
            {
                best = best.max(5.0);
            }
            // Symbol name contains the token (e.g. "EmbeddingCache" contains "cache")
            // Also try stemmed form so "transactions" matches "withTransaction"
            else if token.len() >= 4
                && (name_lower.contains(&token_lower)
                    || {
                        let token_stem = simple_stem(&token_lower);
                        token_stem.len() >= 4
                            && token_stem != token_lower
                            && name_lower.contains(&token_stem)
                    })
            {
                best = best.max(0.5);
            }
        }
        best
    }
}

/// Stopwords to exclude from term-coverage computation.
/// These are common English words that appear in NL queries but carry no
/// discriminative power for matching symbols or file paths.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "from", "how", "does", "what", "this", "that", "with",
    "are", "was", "were", "been", "has", "have", "had", "not", "but", "its",
    "can", "all", "will", "into", "when", "which", "where", "who", "why",
];

/// Compute a term-coverage multiplier for multi-word NL queries.
///
/// For queries with 3+ significant terms, measures what fraction of the query
/// terms appear in the hit's name + file path. Results that only match a single
/// common word (e.g., "file" in "file watcher debounce") get penalized, while
/// results matching multiple terms get boosted.
///
/// Returns an additive score adjustment:
/// - 0.0 for short queries (≤2 significant terms) — no effect
/// - Positive for high coverage, negative for low coverage on NL queries
pub(crate) fn term_coverage_adjustment(
    query: &str,
    hit_name: &str,
    file_path: &str,
    body_text: Option<&str>,
) -> f32 {
    let terms: Vec<&str> = query
        .split_whitespace()
        .filter(|t| t.len() >= 3)
        .filter(|t| !STOPWORDS.contains(&t.to_lowercase().as_str()))
        .collect();

    // Only apply to multi-word NL queries (2+ meaningful terms)
    if terms.len() < 2 {
        return 0.0;
    }

    let name_lower = hit_name.to_lowercase();
    // Split CamelCase name into parts for matching: "EmbeddingCache" → ["embedding", "cache"]
    // Pass the original name (before lowercasing) so CamelCase detection works
    let name_parts = split_camel_case(hit_name);

    let path_parts: Vec<String> = file_path
        .split('/')
        .map(|p| {
            // Strip file extension
            if let Some((stem, _)) = p.rsplit_once('.') {
                stem.to_lowercase()
            } else {
                p.to_lowercase()
            }
        })
        .collect();

    // Pre-tokenize body text: split identifiers and lowercase for matching.
    // This captures terms like "debounce" from `watch_debounce_ms` in function bodies.
    let body_tokens: String = body_text
        .map(|t| crate::text::split_identifier_like(t).to_lowercase())
        .unwrap_or_default();

    let total = terms.len() as f32;
    let mut matched = 0.0f32;

    for term in &terms {
        let term_lower = term.to_lowercase();
        let term_stem = simple_stem(&term_lower);

        // Collect synonyms for this term bidirectionally (used as fallback matching below).
        // "handler" → ["callback", "listener", "hook", "delegate"] even though
        // "handler" is a value (not a key) in the SYNONYMS table.
        let synonyms: Vec<&str> = text::get_related_terms(&term_lower);

        // Check 1: term appears in symbol name (exact or substring)
        let in_name = name_lower.contains(&term_lower)
            || name_parts.iter().any(|p| {
                p == &term_lower
                    || (term_stem.len() >= 3 && stems_match(&simple_stem(p), &term_stem))
            });

        // Check 2: term appears in file path segments (exact, stem, or prefix)
        let in_path = path_parts.iter().any(|p| {
            p == &term_lower
                || p.contains(&term_lower)
                || term_lower.contains(p.as_str())
                || (term_stem.len() >= 3 && stems_match(&simple_stem(p), &term_stem))
                || p.starts_with(&term_lower)
                || term_lower.starts_with(p.as_str())
        });

        // Check 3: term appears in body text (tokenized source code)
        // Worth less than name/path to avoid over-boosting large functions
        // that happen to mention a term once in 200 lines.
        let in_body = !body_tokens.is_empty()
            && body_tokens.split_whitespace().any(|tok| {
                tok == term_lower
                    || (term_stem.len() >= 3 && stems_match(&simple_stem(tok), &term_stem))
            });

        // Check 4: synonym of query term appears in name/path/body.
        // This bridges vocabulary gaps: "websocket" query matches "socket"
        // in body, "serialization" matches "serde" in body, etc.
        // Worth less than direct matches to avoid false positives.
        let via_synonym = if !in_name && !in_path && !in_body && !synonyms.is_empty() {
            let in_name_syn = synonyms.iter().any(|syn| {
                name_lower.contains(syn)
                    || name_parts.iter().any(|p| p == syn)
            });
            let in_path_syn = synonyms.iter().any(|syn| {
                path_parts.iter().any(|p| p.contains(syn) || syn.contains(p.as_str()))
            });
            let in_body_syn = !body_tokens.is_empty()
                && synonyms.iter().any(|syn| {
                    body_tokens.split_whitespace().any(|tok| tok == *syn)
                });
            in_name_syn || in_path_syn || in_body_syn
        } else {
            false
        };

        if in_name || in_path {
            matched += 1.5; // name/path match weighted heavily (R63: up from 1.0)
        } else if in_body {
            matched += 0.5; // body match reduced to widen name vs body gap (R63: down from 0.75)
        } else if via_synonym {
            matched += 0.25; // synonym reduced proportionally (R63: down from 0.35)
        }
    }

    let coverage = matched / total;

    // Scale: coverage 0.0 → -3.0 penalty, threshold → -1.0, above → positive boost
    // For 4+ term queries, raise the neutral threshold: matching 1/4 terms should be
    // penalized more than matching 1/2 terms. This pushes down partial-term matches
    // like "limits.rs" matching only "limit" from "rate limiting and request throttling".
    let threshold = if terms.len() >= 4 { 0.40 } else { 0.33 };
    let raw = (coverage - threshold) * 6.0 - 1.0;
    raw.clamp(-3.0, 2.0)
}

/// Split a CamelCase or snake_case identifier into lowercase parts.
/// "EmbeddingCache" → ["embedding", "cache"]
/// "upsert_file_fingerprint" → ["upsert", "file", "fingerprint"]
fn split_camel_case(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    // First split by underscores
    for segment in s.split('_') {
        if segment.is_empty() {
            continue;
        }
        // Then split CamelCase
        let mut current = String::new();
        for ch in segment.chars() {
            if ch.is_uppercase() && !current.is_empty() {
                parts.push(current.to_lowercase());
                current = String::new();
            }
            current.push(ch);
        }
        if !current.is_empty() {
            parts.push(current.to_lowercase());
        }
    }
    parts
}

fn normalize_pair(a: f32, b: f32) -> (f32, f32) {
    let sum = a + b;
    if sum > 0.0 {
        (a / sum, b / sum)
    } else {
        (0.5, 0.5)
    }
}

/// Compare two stems with prefix tolerance.
/// "scor" (from "scoring") and "score" share prefix "scor" (len 4 ≥ 3), so they match.
/// This handles cases where different suffixes strip to slightly different lengths.
fn stems_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // Check if one stem is a prefix of the other (min prefix len 3)
    let min_len = a.len().min(b.len());
    if min_len >= 3 && (a.starts_with(b) || b.starts_with(a)) {
        return true;
    }
    false
}

/// Simple morphological stemmer for path segment matching.
/// Strips common English suffixes so "parsing" and "parser" share stem "pars".
fn simple_stem(s: &str) -> String {
    // Try suffixes longest-first to avoid partial strips
    for suffix in &["tion", "sion", "ing", "ers", "er", "ed", "es", "s"] {
        if let Some(stem) = s.strip_suffix(suffix) {
            if stem.len() >= 3 {
                return stem.to_string();
            }
        }
    }
    s.to_string()
}

/// Compute a scoring adjustment based on symbol size (line count) and export status.
///
/// BM25 gives disproportionately high scores to short documents — a 5-line private
/// helper can outrank a 200-line core function. This signal compensates by boosting
/// larger, more important symbols and penalizing trivial helpers.
///
/// Uses log2(line_count) centered at ~45 lines (log2(45) ≈ 5.5):
/// - 5 lines → ~-1.3 (small helper penalty)
/// - 20 lines → ~-0.5 (small function)
/// - 45 lines → ~0.0 (neutral)
/// - 100 lines → ~+0.5 (substantial function)
/// - 200 lines → ~+0.9 (core function)
///
/// Private (non-exported) functions with ≤10 lines get an additional -0.5 penalty
/// to suppress small test helpers and internal utilities.
///
/// Returns 0.0 when line_count is unavailable (0).
pub(crate) fn symbol_importance_adjustment(line_count: u32, exported: bool) -> f32 {
    if line_count == 0 {
        return 0.0;
    }

    let log_lines = (line_count as f32).log2();
    // Center at ~45 lines (log2(45) ≈ 5.5), scale by 0.4
    let raw = (log_lines - 5.5) * 0.4;
    // R101: Steeper clamp (-2.5 vs -1.5) so 1-2 line symbols (local vars,
    // single assignments) get more penalty. Main beneficiary: camelCase consts
    // like funnelId, startTime that escape the all-lowercase name penalty.
    let mut adj = raw.clamp(-2.5, 1.0);

    // Extra penalty for small private helpers (test utilities, internal helpers)
    if !exported && line_count <= 10 {
        adj -= 0.5;
    }

    adj
}

/// Penalty for test symbols that live inside production files.
///
/// When a symbol is detected as test code (inside `#[cfg(test)] mod tests` or
/// annotated with `#[test]`), it should be penalized in search results since
/// users searching for "how does X work" want production code, not test code.
///
/// Returns a negative value (-10.0) for test symbols, 0.0 otherwise.
/// The penalty is deliberately strong because BM25 gives test functions
/// artificially high scores (short document bias + keyword density).
/// R34: Increased from -5.0 to -10.0 so test helpers don't survive into
/// the final top-5 via diversity backfill.
pub(crate) fn test_symbol_penalty(is_test: bool) -> f32 {
    if is_test { -10.0 } else { 0.0 }
}

pub(crate) fn structural_adjustment(
    config: &Config,
    exported: bool,
    file_path: &str,
    kind: &str,
    name: &str,
    _intent: &Option<Intent>,
    query: &str,
) -> f32 {
    let mut score = 0.0;
    if exported {
        score += config.rank_exported_boost;
    }

    // Local variable noise penalty: penalize const/variable symbols with short,
    // generic names that are likely local variables inside functions rather than
    // meaningful exported APIs. The TS indexer hoisting bug marks function-body
    // consts as exported=1, so we can't rely on the export flag alone.
    // Examples: key, result, from, sent, limit, now, data, error, page, url
    if matches!(kind, "const" | "variable") {
        // Destructured bindings like "{ code, error, set, request }" are always local
        if name.starts_with('{') || name.starts_with('[') {
            score -= 5.0;
        } else {
            // Penalize all-lowercase const/variable names — these are almost
            // always local variables inside functions, not meaningful API exports.
            // The TS indexer hoisting bug marks function-body consts as exported=1,
            // so we can't rely on the export flag. Compound names (camelCase,
            // snake_case, PascalCase, SCREAMING_CASE) are fine.
            // R100: Extended threshold from 9→14 for medium penalty.
            // R101: Tiered penalty — very short names (page, url, sent, data)
            // are more generic and deserve stronger suppression.
            let is_all_lower = name.chars().all(|c| c.is_lowercase() || c.is_ascii_digit());
            if is_all_lower && name.len() <= 5 {
                score -= 5.0;
            } else if is_all_lower && name.len() <= 14 {
                score -= 3.0;
            }

            // R102: camelCase local variable penalty — short camelCase const names
            // (funnelId, startTime, userId) that have ZERO overlap with query terms
            // are very likely hoisted local variables matching from parent scope's
            // BM25 context rather than being genuinely relevant to the query.
            // Only fires for ≤12 chars and 2+ query terms to avoid false positives.
            // R103: Reduced from -3.0 to -1.5 to avoid cascading through diversity
            // pipeline (Q5 rate-limit regression). Still effective for Handler 3x
            // and Schema 25x intents where -1.5 becomes -4.5 to -37.5.
            // R104: Skip for schema files — table definitions like `appControl` in
            // db/schema/*.ts legitimately don't match "database schema" query terms
            // but are highly relevant. Schema 75x amplifies the -1.5 penalty to
            // -112.5, which is devastatingly false-positive. Use file path check
            // (not intent type) to only guard schema files, not all db/ files.
            let is_schema_path = file_path.to_lowercase().contains("schema");
            if !is_all_lower && !is_schema_path {
                let is_camel = name.chars().next().is_some_and(|c| c.is_lowercase())
                    && name.chars().any(|c| c.is_uppercase());
                if is_camel && name.len() <= 12 {
                    let query_terms: Vec<String> = query
                        .split_whitespace()
                        .filter(|t| t.len() >= 3)
                        .filter(|t| !STOPWORDS.contains(&t.to_lowercase().as_str()))
                        .map(|t| t.to_lowercase())
                        .collect();
                    if query_terms.len() >= 2 {
                        let name_parts = split_camel_case(name);
                        let has_overlap = query_terms.iter().any(|qt| {
                            let qt_stem = simple_stem(qt);
                            name_parts.iter().any(|np| {
                                np == qt
                                    || (qt_stem.len() >= 3
                                        && stems_match(&simple_stem(np), &qt_stem))
                            })
                        });
                        if !has_overlap {
                            score -= 1.5;
                        }
                    }
                }
            }
        }
    }

    // Module re-export penalty: `pub mod foo;` declarations are near-useless
    // for search results — they contain no implementation, just a re-export.
    if kind == "module" {
        score -= 5.0;
    }

    // Glue Code Filtering: barrel/re-export files named literally "index.ts(x)"
    // but NOT route files like "admin.index.tsx" (TanStack Router / Remix convention
    // where dot-separated segments denote nested routes).
    let file_name = file_path.rsplit('/').next().unwrap_or(file_path);
    if file_name == "index.ts" || file_name == "index.tsx" {
        score -= 5.0;
    }

    // Module aggregator file penalty: mod.rs, mod.ts files are typically
    // re-export hubs (`pub mod foo; pub mod bar;`) with no implementation.
    // They surface as file symbols and crowd out actual implementations.
    if kind == "file" {
        if file_path.ends_with("mod.rs") || file_path.ends_with("mod.ts") {
            // Module aggregator: re-export hubs with no implementation
            score -= 5.0;
        } else {
            // General file symbols: less useful than specific function/class symbols.
            // They represent entire files without highlighting which symbol matters.
            score -= 1.0;
        }
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

    // Penalize non-source helper directories (npm installers, shell scripts, docs)
    if path.starts_with("npm/")
        || path.starts_with("scripts/")
        || path.starts_with("docs/")
        || path.starts_with("examples/")
        || path.starts_with(".github/")
    {
        score -= 10.0;
    }

    if path.contains("/src/")
        || path.starts_with("src/")
        || path.contains("/lib/")
        || path.starts_with("lib/")
        || path.contains("/app/")
        || path.starts_with("app/")
        || path.contains("/packages/")
        || path.starts_with("packages/")
    {
        score += 1.0;
    }

    // OS platform utility penalty: platform-specific code (platform/windows/,
    // platform/linux/, etc.) is cross-platform boilerplate that rarely answers
    // application-level queries. Mild penalty keeps them available but below
    // core application logic.
    if path.contains("/platform/") && !query.to_lowercase().contains("platform") {
        score -= 3.0;
    }

    // Test file penalty: integration/unit test files should rank below production code.
    // This is separate from the intent_adjustment 0.01x multiplier — that handles
    // the case where intent is non-Test. This penalty applies structurally so that
    // test files don't outrank production files via term_coverage alone.
    if is_test_file(file_path) {
        score -= 2.0;
    }

    // False cognate penalty: "accessibility" (UI a11y) vs "access" (access control).
    // When the query mentions "access" but NOT "accessibility"/"a11y", symbols with
    // "accessibility" in their name or path are likely UI accessibility code, not RBAC.
    // Note: This cannot be name-based here since we don't have the symbol name,
    // so we rely on the platform penalty already penalizing platform/ paths.

    // Subdirectory Semantics — boost when query terms match path segments
    let terms: Vec<&str> = query
        .split_whitespace()
        .map(|s| s.trim())
        .filter(|s| s.len() > 2)
        .collect();

    let path_parts: Vec<&str> = file_path.split('/').collect();
    for term in &terms {
        let term_lower = term.to_lowercase();
        let term_stem = simple_stem(&term_lower);
        if path_parts.iter().any(|p| {
            let p_lower = p.to_lowercase();
            if p_lower == term_lower {
                return true;
            }
            // Stem match (strip extension)
            let p_stem_src = if let Some((stem, _)) = p_lower.rsplit_once('.') {
                stem
            } else {
                &p_lower
            };
            if p_stem_src == term_lower {
                return true;
            }
            // Morphological stem match: "parsing" ↔ "parser" via stem "pars"
            if simple_stem(p_stem_src) == term_stem && term_stem.len() >= 3 {
                return true;
            }
            // Prefix/plural match: "handler" matches "handlers", "config" matches "configs"
            if p_lower.starts_with(&term_lower) || term_lower.starts_with(&p_lower) {
                return true;
            }
            false
        }) {
            score += 1.0;
        }
    }

    score
}

pub(crate) fn is_test_file(file_path: &str) -> bool {
    let path_lower = file_path.to_lowercase();
    file_path.contains(".test.")
        || file_path.contains(".spec.")
        || file_path.contains("/__tests__/")
        || file_path.contains("/tests/")
        || file_path.starts_with("tests/")
        || file_path.ends_with("_test.rs")
        || file_path.ends_with("_test.go")
        || file_path.ends_with("_test.py")
        || file_path.ends_with("_test.ts")
        || file_path.ends_with("_test.tsx")
        || file_path.ends_with("_test.js")
        || file_path.ends_with("_test.jsx")
        || file_path.contains("/test_")
        || file_path.contains("/conftest")
        // Mock/fixture/helper files (e.g., test.mocks.ts, __mocks__/, fixtures/)
        || path_lower.contains("mock")
        || path_lower.contains("__fixtures__")
        || path_lower.contains("/fixtures/")
        // Test helper patterns (e.g., test-helpers.ts, admin-test-helpers.ts)
        || path_lower.contains("test-helper")
}

/// Check if a symbol name looks like a test function/helper
pub(crate) fn is_test_symbol(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with("test_")
        || n.starts_with("create_test_")
        || n.starts_with("make_test_")
        || n.starts_with("setup_test")
        || n.starts_with("mock_")
        || n == "setup"
        || n == "teardown"
        || n == "tests"
        || (n.starts_with("test") && n.len() > 4 && n.as_bytes()[4].is_ascii_uppercase())
        // Mock patterns: MockTransaction, createMockDb, txMock, fakeFoo, stubBar
        || name.starts_with("Mock")
        || name.ends_with("Mock")
        || name.contains("Mock")
        || n.starts_with("fake")
        || n.starts_with("stub")
}

pub(crate) fn intent_adjustment(intent: &Option<Intent>, kind: &str, file_path: &str, _exported: bool, name: &str) -> f32 {
    // Test Penalty (0.01x multiplier): aggressively suppress test code in non-test queries.
    // Combined with final intent enforcement (double-applied for <1.0 multipliers),
    // effective suppression is 0.0001x — tests should never appear in production results.
    if is_test_file(file_path) && !matches!(intent, Some(Intent::Test)) {
        return 0.01;
    }

    // Symbol-level test penalty: penalize test-named symbols even in production files.
    // This prevents test functions (test_*, create_test_*) from flooding results
    // when they live alongside production code in the same file.
    if !matches!(intent, Some(Intent::Test)) && is_test_symbol(name) {
        return 0.01;
    }

    let Some(intent) = intent else {
        return 1.0;
    };

    match intent {
        Intent::Definition => {
            let is_def = matches!(
                kind,
                "class" | "interface" | "type_alias" | "struct" | "enum" | "const" | "impl"
            );
            if is_def {
                // Boost definition-like symbols. Impl blocks aren't "exported" per se
                // but are equally relevant when user asks for struct/type definitions.
                1.5
            } else {
                1.0
            }
        }
        Intent::Schema => {
            let path = file_path.to_lowercase();
            // Don't boost test/mock utility files even in schema-adjacent paths.
            // Files with "helper", "mock", "stub", "fake", "fixture" in the path
            // are almost always test infrastructure, not production schema code.
            let is_test_adjacent = path.contains("helper")
                || path.contains("mock")
                || path.contains("stub")
                || path.contains("fake")
                || path.contains("fixture");
            if is_test_adjacent {
                0.5
            } else if path.contains("schema") {
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
        Intent::Test => {
            // When user wants tests, boost test files
            if is_test_file(file_path) {
                2.0
            } else {
                0.5
            }
        }
        Intent::Implementation => {
            // Boost function/method definitions - the actual implementations
            if matches!(kind, "function" | "method" | "impl") {
                1.5
            } else if matches!(kind, "class" | "struct" | "trait") {
                1.3
            } else {
                1.0
            }
        }
        Intent::Config => {
            // Boost config/settings files and const definitions
            let path = file_path.to_lowercase();
            if path.contains("config")
                || path.contains("settings")
                || path.contains(".env")
                || path.contains("options")
            {
                3.0
            } else if matches!(kind, "const" | "variable") {
                1.5
            } else {
                0.8
            }
        }
        Intent::Error => {
            // Boost error handling code, suppress unrelated results.
            // text.rs SYNONYMS dictionary contains "error"/"fail" string literals
            // that cause BM25 meta-matching. Suppressing non-error results pushes
            // actual error types (PathError, tool_internal_error) above the noise.
            let path = file_path.to_lowercase();
            let name_lower = name.to_lowercase();
            if path.contains("error") || path.contains("exception") {
                3.0
            } else if name_lower.contains("error")
                || name_lower.contains("fail")
                || name_lower.contains("exception")
                || name_lower.contains("panic")
                || name_lower.contains("fallback")
            {
                2.5
            } else if matches!(kind, "enum" | "class" | "type_alias") {
                1.0
            } else {
                0.2
            }
        }
        Intent::Api => {
            // Boost route/handler/endpoint files
            let path = file_path.to_lowercase();
            if path.contains("handler")
                || path.contains("route")
                || path.contains("controller")
                || path.contains("endpoint")
                || path.contains("api")
            {
                3.0
            } else if path.contains("server") || path.contains("service") {
                1.5
            } else {
                0.8
            }
        }
        Intent::Hook => {
            // Boost hook definitions
            let path = file_path.to_lowercase();
            if path.contains("hook") || path.contains("use") {
                2.0
            } else if matches!(kind, "function") {
                1.3
            } else {
                1.0
            }
        }
        Intent::Middleware => {
            // Boost middleware/interceptor files
            let path = file_path.to_lowercase();
            if path.contains("middleware") || path.contains("interceptor") || path.contains("guard")
            {
                3.0
            } else if path.contains("plugin") {
                1.5
            } else {
                0.8
            }
        }
        Intent::Migration => {
            // Boost migration files
            let path = file_path.to_lowercase();
            if path.contains("migration") || path.contains("migrate") {
                5.0
            } else if path.contains("schema") || path.contains("sql") {
                2.0
            } else {
                0.5
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::path::Utf8PathBuf;
    use crate::storage::sqlite::schema::{SymbolMetricsRow, SymbolRow};
    use crate::storage::sqlite::SqliteStore;
    use std::collections::HashMap;

    /// Create a minimal test config
    fn test_config(popularity_weight: f32) -> Config {
        use crate::config::{EmbeddingsBackend, EmbeddingsDevice};
        use crate::path::Utf8PathBuf;
        Config {
            base_dir: Utf8PathBuf::from("/tmp/test"),
            db_path: Utf8PathBuf::from("/tmp/test.db"),
            vector_db_path: Utf8PathBuf::from("/tmp/vectors"),
            tantivy_index_path: Utf8PathBuf::from("/tmp/tantivy"),
            embeddings_backend: EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_device: EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 64,
            vector_search_limit: 20,
            vector_guaranteed_results: 3,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.5,
            rank_keyword_weight: 0.5,
            rank_exported_boost: 0.0,
            rank_index_file_boost: 0.0,
            rank_test_penalty: 0.1,
            rank_popularity_weight: popularity_weight,
            rank_popularity_cap: 0, // No longer used
            index_patterns: vec!["**/*.ts".to_string()],
            exclude_patterns: vec!["**/node_modules/**".to_string()],
            watch_mode: false,
            watch_debounce_ms: 250,
            watch_min_index_interval_ms: 5000,
            max_context_bytes: 200_000,
            index_node_modules: false,
            repo_roots: vec![],
            reranker_enabled: false,
            reranker_model_path: None,
            reranker_top_k: 5,
            reranker_cache_dir: None,
            learning_enabled: false,
            learning_selection_boost: 0.0,
            learning_file_affinity_boost: 0.0,
            max_context_tokens: 8000,
            token_encoding: "cl100k_base".to_string(),
            parallel_workers: 4,
            embedding_cache_enabled: true,
            pagerank_damping: 0.85,
            pagerank_iterations: 20,
            synonym_expansion_enabled: true,
            acronym_expansion_enabled: true,
            rrf_enabled: true,
            rrf_k: 60.0,
            rrf_keyword_weight: 1.0,
            rrf_vector_weight: 1.0,
            rrf_graph_weight: 0.5,
            hyde_enabled: false,
            hyde_llm_backend: "openai".to_string(),
            hyde_api_key: None,
            hyde_max_tokens: 512,
            metrics_enabled: true,
            metrics_port: 9090,
            package_detection_enabled: true,
            llm_enabled: true,
            llm_device: EmbeddingsDevice::Cpu,
            llm_model_dir: None,
            llm_max_tokens: 30,
            llm_batch_commit: 10,
            leader_election_enabled: false,
            leader_heartbeat_interval_ms: 10_000,
            leader_ttl_seconds: 30,
            embedding_truncate_dim: None,
        }
    }

    /// Helper to insert a symbol for testing
    fn insert_test_symbol(sqlite: &SqliteStore, id: &str, name: &str) {
        let symbol = SymbolRow {
            id: id.to_string(),
            file_path: format!("/path/to/{}.rs", name),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: name.to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            end_line: 2,
            text: format!("fn {}() {{}}", name),
        };
        sqlite.upsert_symbol(&symbol).unwrap();
    }

    /// Helper to create a test hit
    fn make_hit(id: &str, name: &str, score: f32) -> RankedHit {
        RankedHit {
            id: id.to_string(),
            score,
            name: name.to_string(),
            kind: "function".to_string(),
            file_path: format!("/path/to/{}.rs", name),
            exported: true,
            language: "rust".to_string(),
        }
    }

    #[test]
    fn page_rank_boosts_important_symbols() {
        let db_path_buf = std::env::temp_dir().join("test_page_rank_boosts.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            // Insert symbols first (required for foreign key constraint)
            insert_test_symbol(&sqlite, "symbol1", "symbol1");
            insert_test_symbol(&sqlite, "symbol2", "symbol2");
            insert_test_symbol(&sqlite, "symbol3", "symbol3");

            // Insert PageRank values: symbol3 > symbol2 > symbol1
            let metrics = vec![
                SymbolMetricsRow {
                    symbol_id: "symbol1".to_string(),
                    pagerank: 0.01,
                    in_degree: 1,
                    out_degree: 0,
                    updated_at: 0,
                },
                SymbolMetricsRow {
                    symbol_id: "symbol2".to_string(),
                    pagerank: 0.05,
                    in_degree: 5,
                    out_degree: 2,
                    updated_at: 0,
                },
                SymbolMetricsRow {
                    symbol_id: "symbol3".to_string(),
                    pagerank: 0.1,
                    in_degree: 10,
                    out_degree: 5,
                    updated_at: 0,
                },
            ];
            for m in metrics {
                sqlite.upsert_symbol_metrics(&m).unwrap();
            }

            let hits = vec![
                make_hit("symbol1", "symbol1", 10.0),
                make_hit("symbol2", "symbol2", 10.0),
                make_hit("symbol3", "symbol3", 10.0),
            ];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.1);

            let result =
                apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config)
                    .unwrap();

            // After PageRank boost, symbol3 (highest PageRank) should be first
            assert_eq!(result[0].id, "symbol3");
            assert_eq!(result[1].id, "symbol2");
            assert_eq!(result[2].id, "symbol1");
            assert!(hit_signals.get("symbol3").unwrap().popularity_boost > 0.0);
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn page_rank_normalization_works() {
        let db_path_buf = std::env::temp_dir().join("test_page_rank_normalization.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            insert_test_symbol(&sqlite, "low", "low");
            insert_test_symbol(&sqlite, "high", "high");

            let metrics = vec![
                SymbolMetricsRow {
                    symbol_id: "low".to_string(),
                    pagerank: 0.01,
                    in_degree: 1,
                    out_degree: 0,
                    updated_at: 0,
                },
                SymbolMetricsRow {
                    symbol_id: "high".to_string(),
                    pagerank: 0.1,
                    in_degree: 10,
                    out_degree: 5,
                    updated_at: 0,
                },
            ];
            for m in metrics {
                sqlite.upsert_symbol_metrics(&m).unwrap();
            }

            let hits = vec![make_hit("low", "low", 10.0), make_hit("high", "high", 10.0)];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.1);

            let _result =
                apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config)
                    .unwrap();

            let high_boost = hit_signals.get("high").unwrap().popularity_boost;
            let low_boost = hit_signals.get("low").unwrap().popularity_boost;

            // Verify normalization: high should get ~10x more boost than low (0.1/0.01 = 10)
            assert!((high_boost / low_boost - 10.0).abs() < 0.01);
            // Both boosts should be positive (scaled relative to avg score of 10.0)
            // With weight=0.1 and avg_score=10.0, scaled_weight=1.0
            // So high_boost = 1.0 * 1.0 = 1.0, low_boost = 1.0 * 0.1 = 0.1
            assert!(high_boost > 0.0);
            assert!(low_boost > 0.0);
            assert!((high_boost - 1.0).abs() < 0.001);
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn page_rank_handles_missing_metrics() {
        let db_path_buf = std::env::temp_dir().join("test_page_rank_missing.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            insert_test_symbol(&sqlite, "has_metrics", "has_metrics");

            let metrics = vec![SymbolMetricsRow {
                symbol_id: "has_metrics".to_string(),
                pagerank: 0.05,
                in_degree: 5,
                out_degree: 2,
                updated_at: 0,
            }];
            for m in metrics {
                sqlite.upsert_symbol_metrics(&m).unwrap();
            }

            let hits = vec![
                make_hit("has_metrics", "has_metrics", 10.0),
                make_hit("no_metrics", "no_metrics", 10.0),
            ];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.1);

            let result =
                apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config)
                    .unwrap();

            assert!(hit_signals.get("has_metrics").unwrap().popularity_boost > 0.0);
            assert_eq!(hit_signals.get("no_metrics").unwrap().popularity_boost, 0.0);
            assert_eq!(result[0].id, "has_metrics");
            assert_eq!(result[1].id, "no_metrics");
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn page_rank_handles_empty_result_set() {
        let db_path_buf = std::env::temp_dir().join("test_page_rank_empty.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            let hits = vec![
                make_hit("symbol1", "symbol1", 10.0),
                make_hit("symbol2", "symbol2", 5.0),
            ];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.1);

            let result =
                apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config)
                    .unwrap();

            // No boost applied (no metrics in DB)
            // Note: hit_signals may not contain entries for symbols with no boost
            assert!(hit_signals
                .get("symbol1")
                .is_none_or(|s| s.popularity_boost == 0.0));
            assert!(hit_signals
                .get("symbol2")
                .is_none_or(|s| s.popularity_boost == 0.0));
            // Original scores unchanged
            assert_eq!(result[0].id, "symbol1");
            assert_eq!(result[0].score, 10.0);
            assert_eq!(result[1].id, "symbol2");
            assert_eq!(result[1].score, 5.0);
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn page_rank_empty_hits_returns_early() {
        let db_path_buf = std::env::temp_dir().join("test_page_rank_empty_hits.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            let hits: Vec<RankedHit> = vec![];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.1);

            let result =
                apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config)
                    .unwrap();

            assert!(result.is_empty());
            assert!(hit_signals.is_empty());
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn page_rank_zero_weight_returns_early() {
        let db_path_buf = std::env::temp_dir().join("test_page_rank_zero_weight.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            insert_test_symbol(&sqlite, "symbol1", "symbol1");

            let metrics = vec![SymbolMetricsRow {
                symbol_id: "symbol1".to_string(),
                pagerank: 0.1,
                in_degree: 10,
                out_degree: 5,
                updated_at: 0,
            }];
            for m in metrics {
                sqlite.upsert_symbol_metrics(&m).unwrap();
            }

            let hits = vec![make_hit("symbol1", "symbol1", 10.0)];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.0);

            let result =
                apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config)
                    .unwrap();

            // No boost applied when weight is 0
            assert_eq!(result[0].score, 10.0);
            // hit_signals may be empty when weight is 0 (early return)
            assert!(hit_signals
                .get("symbol1")
                .is_none_or(|s| s.popularity_boost == 0.0));
        }

        let _ = std::fs::remove_file(&db_path);
    }

    // ── term_coverage_adjustment tests ──

    #[test]
    fn term_coverage_no_effect_on_short_queries() {
        // Single-word queries should return 0.0 (no effect)
        assert_eq!(term_coverage_adjustment("EmbeddingCache", "EmbeddingCache", "src/storage/cache.rs", None), 0.0);
        // 2-word queries now DO get term_coverage (threshold lowered to 2)
        let adj = term_coverage_adjustment("get put", "get", "src/cache.rs", None);
        assert!(adj != 0.0, "2-word queries should now have term_coverage effect, got {adj}");
    }

    #[test]
    fn term_coverage_penalizes_single_term_match() {
        // "file watcher debounce" matching only "file" → low coverage → negative
        let adj = term_coverage_adjustment(
            "file watcher debounce reindex",
            "upsert_file_fingerprint",
            "src/storage/sqlite/queries/files.rs",
            None,
        );
        assert!(adj < 0.0, "expected penalty for single-term match, got {adj}");
    }

    #[test]
    fn term_coverage_boosts_multi_term_match() {
        // Symbol matching 2+ terms should score higher than one matching 1 term
        // "embedding cache storage vector" → "EmbeddingCache" matches 2 terms (embedding, cache)
        let multi = term_coverage_adjustment(
            "embedding cache storage vector",
            "EmbeddingCache",
            "src/storage/cache.rs",
            None,
        );
        // vs a symbol that only matches "storage" via path
        let single = term_coverage_adjustment(
            "embedding cache storage vector",
            "SqliteStore",
            "src/storage/sqlite/operations.rs",
            None,
        );
        assert!(multi > single, "multi-term match ({multi}) should score higher than single-term ({single})");
    }

    #[test]
    fn term_coverage_rewards_high_coverage() {
        // "ranking scoring system" with terms matching via stems
        let adj = term_coverage_adjustment(
            "ranking and scoring system work",
            "rank_hits_with_signals",
            "src/retrieval/ranking/score.rs",
            None,
        );
        // "ranking" matches path "ranking", "scoring" matches path "score" via stem
        // That's 2/4 = 0.50 coverage → should be close to 0 or slightly positive
        assert!(adj > -2.0, "good coverage match should not be strongly penalized, got {adj}");

        // Compare against a result that matches nothing
        let bad = term_coverage_adjustment(
            "ranking and scoring system work",
            "parse_go_mod",
            "src/package/parsers/go.rs",
            None,
        );
        assert!(adj > bad, "matched result ({adj}) should score higher than unmatched ({bad})");
    }

    #[test]
    fn term_coverage_handles_camel_case_names() {
        // CamelCase: "EmbeddingCache" should split into ["embedding", "cache"]
        let adj = term_coverage_adjustment(
            "embedding cache get put",
            "EmbeddingCache",
            "src/storage/cache.rs",
            None,
        );
        // "embedding" and "cache" both match the name parts
        assert!(adj > -1.0, "CamelCase name matching multiple terms should score well, got {adj}");
    }

    #[test]
    fn term_coverage_uses_morphological_stemming() {
        // "parsing" should match "parser" via stemming
        let adj = term_coverage_adjustment(
            "tree sitter parsing extractors",
            "language_for_id",
            "src/indexer/parser.rs",
            None,
        );
        let adj2 = term_coverage_adjustment(
            "tree sitter parsing extractors",
            "parse_go_mod",
            "src/package/parsers/go.rs",
            None,
        );
        // parser.rs should do better because "parser" stem-matches "parsing" and "indexer" is related
        // Both have some matching but parser.rs path matches more terms
        // At minimum, neither should be strongly penalized
        assert!(adj >= adj2 || (adj - adj2).abs() < 1.0,
            "parser.rs ({adj}) should score >= go.rs ({adj2}) or be close");
    }

    #[test]
    fn term_coverage_body_text_boosts_relevant_symbols() {
        // spawn_watch_loop's body contains "debounce" via watch_debounce_ms
        let with_body = term_coverage_adjustment(
            "file watcher debounce reindex change",
            "spawn_watch_loop",
            "src/indexer/pipeline/mod.rs",
            Some("pub fn spawn_watch_loop() { let interval = config.watch_debounce_ms; check_for_changes(); }"),
        );
        let without_body = term_coverage_adjustment(
            "file watcher debounce reindex change",
            "spawn_watch_loop",
            "src/indexer/pipeline/mod.rs",
            None,
        );
        assert!(with_body > without_body,
            "body text should boost coverage: with={with_body}, without={without_body}");
    }

    #[test]
    fn term_coverage_body_text_counts_three_quarter() {
        // Body-only matches should count 0.75x compared to name/path matches (R27: up from 0.5)
        // This prevents large functions from being over-boosted just because they mention a term once
        let body_only = term_coverage_adjustment(
            "websocket handler connection protocol",
            "classify_elysia_method",
            "src/indexer/extract/elysia.rs",
            Some("fn classify_elysia_method() { match m { \"ws\" => WebSocket, _ => Route } }"),
        );
        // "websocket" matches body (WebSocket splits to web socket), "handler" no match,
        // "connection" no match, "protocol" no match → low coverage but not zero
        assert!(body_only > -3.0, "body text match should reduce penalty, got {body_only}");
    }

    #[test]
    fn split_camel_case_works() {
        assert_eq!(split_camel_case("embeddingcache"), vec!["embeddingcache"]);
        assert_eq!(split_camel_case("EmbeddingCache"), vec!["embedding", "cache"]);
        assert_eq!(split_camel_case("upsert_file_fingerprint"), vec!["upsert", "file", "fingerprint"]);
        // All-caps sequences split into individual chars (expected for acronyms)
        assert_eq!(split_camel_case("HTMLParser"), vec!["h", "t", "m", "l", "parser"]);
    }

    #[test]
    fn stems_match_works() {
        assert!(stems_match("rank", "rank"));
        assert!(stems_match("scor", "score")); // "scoring" stem vs "score" stem
        assert!(stems_match("pars", "pars"));  // "parsing" and "parser" share stem
        assert!(!stems_match("ab", "abc"));    // too short prefix
        assert!(!stems_match("rank", "file")); // unrelated
    }

    // ── symbol_importance_adjustment tests ──

    #[test]
    fn symbol_importance_zero_lines_returns_zero() {
        assert_eq!(symbol_importance_adjustment(0, true), 0.0);
        assert_eq!(symbol_importance_adjustment(0, false), 0.0);
    }

    #[test]
    fn symbol_importance_penalizes_tiny_functions() {
        // 5-line function: log2(5) ≈ 2.32 → (2.32 - 5.5) * 0.4 ≈ -1.27
        let adj = symbol_importance_adjustment(5, true);
        assert!(adj < -1.0, "5-line exported function should be penalized, got {adj}");

        // Private 5-line helper gets additional -0.5 penalty
        let adj_priv = symbol_importance_adjustment(5, false);
        assert!(adj_priv < adj, "private helper ({adj_priv}) should be penalized more than exported ({adj})");
        assert!((adj_priv - (adj - 0.5)).abs() < 0.01, "private penalty should be -0.5 extra");
    }

    #[test]
    fn symbol_importance_boosts_large_functions() {
        // 200-line function: log2(200) ≈ 7.64 → (7.64 - 5.5) * 0.4 ≈ 0.86
        let adj = symbol_importance_adjustment(200, true);
        assert!(adj > 0.5, "200-line function should get a boost, got {adj}");
        assert!(adj <= 1.0, "boost should be clamped at 1.0, got {adj}");
    }

    #[test]
    fn symbol_importance_neutral_around_45_lines() {
        // 45 lines: log2(45) ≈ 5.49 → (5.49 - 5.5) * 0.4 ≈ -0.004 ≈ 0
        let adj = symbol_importance_adjustment(45, true);
        assert!(adj.abs() < 0.1, "45-line function should be near-neutral, got {adj}");
    }

    #[test]
    fn symbol_importance_private_small_extra_penalty() {
        // Private ≤10 lines gets -0.5 extra
        let exported_10 = symbol_importance_adjustment(10, true);
        let private_10 = symbol_importance_adjustment(10, false);
        assert!((private_10 - (exported_10 - 0.5)).abs() < 0.01);

        // Private 11 lines does NOT get extra penalty
        let exported_11 = symbol_importance_adjustment(11, true);
        let private_11 = symbol_importance_adjustment(11, false);
        assert_eq!(exported_11, private_11, "private >10 lines should not get extra penalty");
    }

    #[test]
    fn symbol_importance_clamp_bounds() {
        // Very small: 1 line → log2(1) = 0 → (0 - 5.5) * 0.4 = -2.2, clamped to -2.2
        let adj = symbol_importance_adjustment(1, true);
        assert!((adj - -2.2).abs() < 0.01, "should be -2.2, got {adj}");

        // Very large: 10000 lines → log2(10000) ≈ 13.29 → (13.29 - 5.5) * 0.4 ≈ 3.12, clamped to 1.0
        let adj = symbol_importance_adjustment(10000, true);
        assert_eq!(adj, 1.0, "should be clamped to 1.0, got {adj}");
    }

    // ── test_symbol_penalty tests ──

    #[test]
    fn test_symbol_penalty_penalizes_test_code() {
        assert_eq!(test_symbol_penalty(true), -10.0);
        assert_eq!(test_symbol_penalty(false), 0.0);
    }

    #[test]
    fn test_symbol_detection_via_batch_query() {
        let db_path_buf = std::env::temp_dir().join("test_batch_check_test_symbols.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            // Insert a mod tests module that spans bytes 100-500
            let test_mod = SymbolRow {
                id: "mod_tests".to_string(),
                file_path: "src/foo.rs".to_string(),
                language: "rust".to_string(),
                kind: "module".to_string(),
                name: "tests".to_string(),
                exported: false,
                start_byte: 100,
                end_byte: 500,
                start_line: 10,
                end_line: 50,
                text: "#[cfg(test)]\nmod tests {\n}".to_string(),
            };
            sqlite.upsert_symbol(&test_mod).unwrap();

            // A test helper INSIDE mod tests (byte range 150-200)
            let test_helper = SymbolRow {
                id: "test_helper".to_string(),
                file_path: "src/foo.rs".to_string(),
                language: "rust".to_string(),
                kind: "function".to_string(),
                name: "make_hit".to_string(),
                exported: false,
                start_byte: 150,
                end_byte: 200,
                start_line: 15,
                end_line: 20,
                text: "fn make_hit() {}".to_string(),
            };
            sqlite.upsert_symbol(&test_helper).unwrap();

            // A #[test] function INSIDE mod tests
            let test_fn = SymbolRow {
                id: "test_fn".to_string(),
                file_path: "src/foo.rs".to_string(),
                language: "rust".to_string(),
                kind: "function".to_string(),
                name: "it_works".to_string(),
                exported: false,
                start_byte: 250,
                end_byte: 400,
                start_line: 25,
                end_line: 45,
                text: "#[test]\nfn it_works() { assert!(true); }".to_string(),
            };
            sqlite.upsert_symbol(&test_fn).unwrap();

            // A production function OUTSIDE mod tests (byte range 10-90)
            let prod_fn = SymbolRow {
                id: "prod_fn".to_string(),
                file_path: "src/foo.rs".to_string(),
                language: "rust".to_string(),
                kind: "function".to_string(),
                name: "do_work".to_string(),
                exported: true,
                start_byte: 10,
                end_byte: 90,
                start_line: 1,
                end_line: 8,
                text: "pub fn do_work() { /* real code */ }".to_string(),
            };
            sqlite.upsert_symbol(&prod_fn).unwrap();

            let ids = vec![
                "test_helper".to_string(),
                "test_fn".to_string(),
                "prod_fn".to_string(),
                "mod_tests".to_string(),
            ];
            let test_set = sqlite.batch_check_test_symbols(&ids).unwrap();

            // test_helper: inside mod tests → detected
            assert!(test_set.contains("test_helper"), "helper inside mod tests should be detected");
            // test_fn: has #[test] in text → detected
            assert!(test_set.contains("test_fn"), "#[test] function should be detected");
            // mod_tests itself: NOT inside itself (excluded by m.id != s.id) — this is correct
            // because we only want to penalize functions inside the mod, not the mod declaration
            assert!(!test_set.contains("mod_tests"), "mod tests declaration should not self-match");
            // prod_fn: outside mod tests, no #[test] → NOT detected
            assert!(!test_set.contains("prod_fn"), "production function should not be detected");
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn selection_boost_applied_and_capped() {
        let db_path_buf = std::env::temp_dir().join("test_selection_boost_capped.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            insert_test_symbol(&sqlite, "sym_a", "sym_a");
            insert_test_symbol(&sqlite, "sym_b", "sym_b");

            // Record multiple selections to build up boost values
            for _ in 0..10 {
                sqlite
                    .insert_query_selection("error handling", "error handling", "sym_a", 1)
                    .unwrap();
            }
            sqlite
                .insert_query_selection("error handling", "error handling", "sym_b", 3)
                .unwrap();

            let hits = vec![
                make_hit("sym_a", "sym_a", 5.0),
                make_hit("sym_b", "sym_b", 5.0),
            ];
            let mut hit_signals = HashMap::new();
            let mut config = test_config(0.0);
            config.learning_enabled = true;
            config.learning_selection_boost = 0.1;

            let result = apply_selection_boost_with_signals(
                &sqlite,
                hits,
                &mut hit_signals,
                "error handling",
                &config,
            )
            .unwrap();

            // sym_a had more selections → highest boost → should be first
            assert_eq!(result[0].id, "sym_a");
            // Max boost should be exactly config weight (0.1) due to normalization
            let a_boost = hit_signals.get("sym_a").unwrap().learning_boost;
            assert!(
                (a_boost - 0.1).abs() < f32::EPSILON,
                "Max selection boost should be exactly config weight, got {}",
                a_boost
            );
            // sym_b should have a smaller boost
            let b_boost = hit_signals.get("sym_b").unwrap().learning_boost;
            assert!(b_boost > 0.0 && b_boost < 0.1, "sym_b boost should be >0 and <0.1, got {}", b_boost);
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn selection_boost_no_selections_no_change() {
        let db_path_buf = std::env::temp_dir().join("test_selection_boost_empty.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            insert_test_symbol(&sqlite, "sym_x", "sym_x");

            let hits = vec![make_hit("sym_x", "sym_x", 7.0)];
            let mut hit_signals = HashMap::new();
            let mut config = test_config(0.0);
            config.learning_enabled = true;
            config.learning_selection_boost = 0.1;

            let result = apply_selection_boost_with_signals(
                &sqlite,
                hits,
                &mut hit_signals,
                "some query",
                &config,
            )
            .unwrap();

            // Score should be unchanged
            assert!(
                (result[0].score - 7.0).abs() < f32::EPSILON,
                "Score should be unchanged with no selections, got {}",
                result[0].score
            );
            assert!(hit_signals.is_empty(), "No signals should be recorded");
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn file_affinity_boost_applied_and_normalized() {
        let db_path_buf = std::env::temp_dir().join("test_affinity_boost.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            insert_test_symbol(&sqlite, "sym_1", "sym_1");
            insert_test_symbol(&sqlite, "sym_2", "sym_2");

            // Record file access — sym_1's file gets many edits, sym_2 gets one view
            for _ in 0..5 {
                sqlite
                    .upsert_file_affinity("/path/to/sym_1.rs", 0, 1)
                    .unwrap();
            }
            sqlite
                .upsert_file_affinity("/path/to/sym_2.rs", 1, 0)
                .unwrap();

            let hits = vec![
                make_hit("sym_1", "sym_1", 5.0),
                make_hit("sym_2", "sym_2", 5.0),
            ];
            let mut hit_signals = HashMap::new();
            let mut config = test_config(0.0);
            config.learning_enabled = true;
            config.learning_file_affinity_boost = 0.05;

            let result = apply_file_affinity_boost_with_signals(
                &sqlite,
                hits,
                &mut hit_signals,
                &config,
            )
            .unwrap();

            // sym_1 had more edits (2x weighted) → higher affinity → first
            assert_eq!(result[0].id, "sym_1");
            // Max boost should be exactly config weight (0.05) due to normalization
            let boost_1 = hit_signals.get("sym_1").unwrap().affinity_boost;
            assert!(
                (boost_1 - 0.05).abs() < f32::EPSILON,
                "Max affinity boost should be exactly config weight, got {}",
                boost_1
            );
            // sym_2 should have a smaller boost
            let boost_2 = hit_signals.get("sym_2").unwrap().affinity_boost;
            assert!(boost_2 > 0.0 && boost_2 < 0.05, "sym_2 boost should be >0 and <0.05, got {}", boost_2);
        }

        let _ = std::fs::remove_file(&db_path);
    }
}

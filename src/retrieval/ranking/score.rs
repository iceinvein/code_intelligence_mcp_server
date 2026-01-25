use crate::config::Config;
use crate::retrieval::query::Intent;
use crate::retrieval::{HitSignals, RankedHit};
use crate::storage::sqlite::SqliteStore;
use crate::storage::tantivy::SearchHit as KeywordHit;
use crate::storage::vector::VectorHit;
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
    let boost_map = sqlite
        .batch_get_selection_boosts(&pairs)
        .unwrap_or_default();

    // Apply boosts to hits
    for h in hits.iter_mut() {
        let key = format!("{}|{}", query_normalized, h.id);
        let boost = boost_map.get(&key).copied().unwrap_or(0.0);

        if boost > 0.0 {
            let final_boost = config.learning_selection_boost * boost;
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
                    popularity_boost: 0.0,
                    learning_boost: final_boost,
                    affinity_boost: 0.0,
                    docstring_boost: 0.0,
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
    let affinity_map = sqlite
        .batch_get_affinity_boosts(&file_paths)
        .unwrap_or_default();

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
                    popularity_boost: 0.0,
                    learning_boost: 0.0,
                    affinity_boost: final_boost,
                    docstring_boost: 0.0,
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
                learning_boost: 0.0,
                affinity_boost: 0.0,
                docstring_boost: 0.0,
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
                learning_boost: 0.0,
                affinity_boost: 0.0,
                docstring_boost: 0.0,
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
                learning_boost: 0.0,
                affinity_boost: 0.0,
                docstring_boost: 0.0,
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
                    popularity_boost: 0.0,
                    learning_boost: 0.0,
                    affinity_boost: 0.0,
                    docstring_boost: DOCSTRING_BOOST,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::storage::sqlite::SqliteStore;
    use crate::storage::sqlite::schema::{SymbolMetricsRow, SymbolRow};
    use std::collections::HashMap;

    /// Create a minimal test config
    fn test_config(popularity_weight: f32) -> Config {
        use crate::config::{EmbeddingsBackend, EmbeddingsDevice};
        use std::path::PathBuf;
        Config {
            base_dir: PathBuf::from("/tmp/test"),
            db_path: PathBuf::from("/tmp/test.db"),
            vector_db_path: PathBuf::from("/tmp/vectors"),
            tantivy_index_path: PathBuf::from("/tmp/tantivy"),
            embeddings_backend: EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_model_url: None,
            embeddings_model_sha256: None,
            embeddings_auto_download: false,
            embeddings_model_repo: None,
            embeddings_model_revision: None,
            embeddings_model_hf_token: None,
            embeddings_device: EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 64,
            vector_search_limit: 20,
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
            max_context_bytes: 200_000,
            index_node_modules: false,
            repo_roots: vec![],
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
        let db_path = std::env::temp_dir().join("test_page_rank_boosts.db");
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
                SymbolMetricsRow { symbol_id: "symbol1".to_string(), pagerank: 0.01, in_degree: 1, out_degree: 0, updated_at: 0 },
                SymbolMetricsRow { symbol_id: "symbol2".to_string(), pagerank: 0.05, in_degree: 5, out_degree: 2, updated_at: 0 },
                SymbolMetricsRow { symbol_id: "symbol3".to_string(), pagerank: 0.1, in_degree: 10, out_degree: 5, updated_at: 0 },
            ];
            for m in metrics { sqlite.upsert_symbol_metrics(&m).unwrap(); }

            let hits = vec![make_hit("symbol1", "symbol1", 10.0), make_hit("symbol2", "symbol2", 10.0), make_hit("symbol3", "symbol3", 10.0)];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.1);

            let result = apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config).unwrap();

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
        let db_path = std::env::temp_dir().join("test_page_rank_normalization.db");
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            insert_test_symbol(&sqlite, "low", "low");
            insert_test_symbol(&sqlite, "high", "high");

            let metrics = vec![
                SymbolMetricsRow { symbol_id: "low".to_string(), pagerank: 0.01, in_degree: 1, out_degree: 0, updated_at: 0 },
                SymbolMetricsRow { symbol_id: "high".to_string(), pagerank: 0.1, in_degree: 10, out_degree: 5, updated_at: 0 },
            ];
            for m in metrics { sqlite.upsert_symbol_metrics(&m).unwrap(); }

            let hits = vec![make_hit("low", "low", 10.0), make_hit("high", "high", 10.0)];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.1);

            let _result = apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config).unwrap();

            let high_boost = hit_signals.get("high").unwrap().popularity_boost;
            let low_boost = hit_signals.get("low").unwrap().popularity_boost;

            // Verify normalization: high should get ~10x more boost than low (0.1/0.01 = 10)
            assert!((high_boost / low_boost - 10.0).abs() < 0.01);
            // Both boosts should be in 0-0.1 range
            assert!(high_boost > 0.0 && high_boost <= 0.1);
            assert!(low_boost > 0.0 && low_boost <= 0.1);
            // Max PageRank symbol gets normalized to 1.0
            assert!((high_boost - 0.1).abs() < 0.001);
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn page_rank_handles_missing_metrics() {
        let db_path = std::env::temp_dir().join("test_page_rank_missing.db");
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
            for m in metrics { sqlite.upsert_symbol_metrics(&m).unwrap(); }

            let hits = vec![make_hit("has_metrics", "has_metrics", 10.0), make_hit("no_metrics", "no_metrics", 10.0)];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.1);

            let result = apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config).unwrap();

            assert!(hit_signals.get("has_metrics").unwrap().popularity_boost > 0.0);
            assert_eq!(hit_signals.get("no_metrics").unwrap().popularity_boost, 0.0);
            assert_eq!(result[0].id, "has_metrics");
            assert_eq!(result[1].id, "no_metrics");
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn page_rank_handles_empty_result_set() {
        let db_path = std::env::temp_dir().join("test_page_rank_empty.db");
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            let hits = vec![make_hit("symbol1", "symbol1", 10.0), make_hit("symbol2", "symbol2", 5.0)];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.1);

            let result = apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config).unwrap();

            // No boost applied (no metrics in DB)
            // Note: hit_signals may not contain entries for symbols with no boost
            assert!(hit_signals.get("symbol1").map_or(true, |s| s.popularity_boost == 0.0));
            assert!(hit_signals.get("symbol2").map_or(true, |s| s.popularity_boost == 0.0));
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
        let db_path = std::env::temp_dir().join("test_page_rank_empty_hits.db");
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            let hits: Vec<RankedHit> = vec![];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.1);

            let result = apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config).unwrap();

            assert!(result.is_empty());
            assert!(hit_signals.is_empty());
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn page_rank_zero_weight_returns_early() {
        let db_path = std::env::temp_dir().join("test_page_rank_zero_weight.db");
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
            for m in metrics { sqlite.upsert_symbol_metrics(&m).unwrap(); }

            let hits = vec![make_hit("symbol1", "symbol1", 10.0)];
            let mut hit_signals = HashMap::new();
            let config = test_config(0.0);

            let result = apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &config).unwrap();

            // No boost applied when weight is 0
            assert_eq!(result[0].score, 10.0);
            // hit_signals may be empty when weight is 0 (early return)
            assert!(hit_signals.get("symbol1").map_or(true, |s| s.popularity_boost == 0.0));
        }

        let _ = std::fs::remove_file(&db_path);
    }
}

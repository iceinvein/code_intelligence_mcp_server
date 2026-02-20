//! Retrieval module for code intelligence search

pub mod assembler;
mod cache;
mod fast_paths;
mod framework_patterns;
mod hybrid;
pub mod hyde;
mod postprocess;
pub(crate) mod query;
pub(crate) mod ranking;

use crate::path::Utf8PathBuf;
use crate::retrieval::hyde::HypotheticalCodeGenerator;
use crate::text::get_related_terms;
use crate::{
    config::Config,
    embeddings::Embedder,
    metrics::MetricsRegistry,
    reranker::Reranker,
    retrieval::assembler::{ContextAssembler, ContextItem},
    storage::{
        sqlite::{SqliteStore, SymbolRow},
        tantivy::TantivyIndex,
        vector::LanceVectorTable,
    },
};
use anyhow::{anyhow, Result};
use cache::RetrieverCaches;
use query::{
    contains_code_snippet, decompose_query, detect_intent, normalize_and_expand_query,
    parse_query_controls, trim_query, Intent,
};
use ranking::{
    apply_reranker_scores, diversify_by_cluster, diversify_by_file, diversify_by_kind,
    expand_with_edges, prepare_rerank_docs, should_rerank,
};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Clone, Serialize)]
pub struct RankedHit {
    pub id: String,
    pub score: f32,
    pub name: String,
    pub kind: String,
    pub file_path: String,
    /// Used internally for filtering, not serialized in responses
    #[serde(skip_serializing)]
    pub exported: bool,
    /// Used internally for filtering, not serialized in responses
    #[serde(skip_serializing)]
    pub language: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub limit: usize,
    pub hits: Vec<RankedHit>,
    pub context: String,
    // Note: `context_items` removed - info is in context string
    // Note: `hit_signals` removed - use explain_search tool for debugging
}

/// Extended search response with scoring signals for debugging/explain_search
#[derive(Debug, Clone)]
pub struct SearchResponseWithSignals {
    pub response: SearchResponse,
    pub hit_signals: HashMap<String, HitSignals>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HitSignals {
    pub keyword_score: f32,
    pub vector_score: f32,
    pub base_score: f32,
    pub structural_adjust: f32,
    pub intent_mult: f32,
    pub definition_bias: f32,
    pub term_coverage: f32,
    pub symbol_importance: f32,
    pub test_symbol_penalty: f32,
    pub popularity_boost: f32,
    pub learning_boost: f32,
    pub affinity_boost: f32,
    pub docstring_boost: f32,
    pub package_boost: f32,
}

#[derive(Clone)]
pub struct Retriever {
    pub(super) config: Arc<Config>,
    pub(super) db_path: Utf8PathBuf,
    pub(super) tantivy: Arc<TantivyIndex>,
    pub(super) vectors: Arc<LanceVectorTable>,
    pub(super) embedder: Arc<AsyncMutex<Box<dyn Embedder + Send>>>,
    pub(super) reranker: Option<Arc<dyn Reranker>>,
    pub(super) hyde_generator: Option<HypotheticalCodeGenerator>,
    pub(super) cache: Arc<Mutex<RetrieverCaches>>,
    pub(super) cache_config_key: String,
    pub(super) metrics: Arc<MetricsRegistry>,
}

/// Promote top vector search results into the final ranking for NL queries.
///
/// Post-RRF adjustments (term_coverage, definition_bias) systematically favor
/// BM25 results because they reward lexical matching. This buries semantically
/// correct vector results below the top-`limit` cutoff.
///
/// This function ensures at least `guaranteed_slots` of the top vector results
/// appear in the top `limit` positions by boosting their scores to the 70th
/// percentile of current results.
///
/// Skips test symbols and module re-exports (they shouldn't be force-promoted).
fn promote_vector_results(
    results: &mut Vec<RankedHit>,
    vector_ranked: &[RankedHit],
    test_symbols: &HashSet<String>,
    _signals: &mut HashMap<String, HitSignals>,
    limit: usize,
    guaranteed_slots: usize,
    query: &str,
) {
    if guaranteed_slots == 0 || vector_ranked.is_empty() || results.is_empty() {
        return;
    }

    // Top vector results (by vector rank order), excluding tests, modules, and files.
    // File-level symbols are redundant when specific functions from the same file
    // already exist in BM25 results — promoting them would waste a result slot.
    let top_vector: Vec<&RankedHit> = vector_ranked
        .iter()
        .filter(|h| !test_symbols.contains(&h.id))
        .filter(|h| h.kind != "module" && h.kind != "file")
        .take(guaranteed_slots * 2)
        .collect();

    // Which of these are already in the final results?
    let final_ids: HashSet<&str> = results.iter().map(|h| h.id.as_str()).collect();
    // Respect file diversity: don't inject from files already well-represented.
    // Count how many non-file results each file has.
    let mut file_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for h in results.iter() {
        if h.kind != "file" {
            *file_counts.entry(h.file_path.as_str()).or_insert(0) += 1;
        }
    }
    let max_per_file_for_promotion = (limit / 5).max(2); // same as diversify_by_file

    let missing: Vec<&RankedHit> = top_vector
        .into_iter()
        .filter(|h| !final_ids.contains(h.id.as_str()))
        .filter(|h| {
            // Skip if this file already has enough entries (respect diversity)
            let count = file_counts.get(h.file_path.as_str()).copied().unwrap_or(0);
            count < max_per_file_for_promotion + 1
        })
        .filter(|h| {
            // Skip vector-only results with poor query-term coverage.
            // Vector promotion bypasses scoring adjustments (structural, tc, si),
            // assigning the 70th-percentile score regardless of actual relevance.
            // This lets false-positive vector matches (e.g., CHECK_INTERVAL_MS
            // matching "check" from "health check") leapfrog genuinely relevant
            // results. Gate on term_coverage to ensure promoted results actually
            // match multiple query terms via name/path.
            let tc = ranking::term_coverage_adjustment(query, &h.name, &h.file_path, None);
            tc > -1.0
        })
        .take(guaranteed_slots)
        .collect();

    if missing.is_empty() {
        return;
    }

    // Target score: 70th percentile of current results (30% from top).
    // Injected vector results appear in upper half without displacing
    // genuinely strong BM25 matches at the very top.
    let target_idx = (limit as f32 * 0.3) as usize;
    let target_score = results
        .get(target_idx.min(results.len().saturating_sub(1)))
        .map(|h| h.score)
        .unwrap_or(5.0);

    // Inject missing vector results, replacing the bottom entries
    for vec_hit in &missing {
        let injected = RankedHit {
            id: vec_hit.id.clone(),
            score: target_score,
            name: vec_hit.name.clone(),
            kind: vec_hit.kind.clone(),
            file_path: vec_hit.file_path.clone(),
            exported: vec_hit.exported,
            language: vec_hit.language.clone(),
        };
        // Don't insert signals for promoted vector results. The final intent
        // enforcement pass will compute intent_adjustment on the fly — correctly
        // applying test-symbol penalties (e.g., setup_test_db → 0.05x) and
        // intent multipliers that the hardcoded 1.0 was bypassing.

        // Replace the lowest-scored entry (last after sort)
        if results.len() >= limit {
            results.pop();
        }
        results.push(injected);
    }

    results.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
            .then_with(|| a.name.cmp(&b.name))
    });
}

impl Retriever {
    pub fn new(
        config: Arc<Config>,
        tantivy: Arc<TantivyIndex>,
        vectors: Arc<LanceVectorTable>,
        embedder: Arc<AsyncMutex<Box<dyn Embedder + Send>>>,
        reranker: Option<Arc<dyn Reranker>>,
        hyde_generator: Option<HypotheticalCodeGenerator>,
        metrics: Arc<MetricsRegistry>,
    ) -> Self {
        let cache = RetrieverCaches::new();
        let cache_config_key = format!(
            "t={}|k={}|ha={:.3}|vw={:.3}|kw={:.3}|eb={:.3}|ib={:.3}|tp={:.3}|pw={:.3}|pc={}",
            config.max_context_tokens,
            config.vector_search_limit,
            config.hybrid_alpha,
            config.rank_vector_weight,
            config.rank_keyword_weight,
            config.rank_exported_boost,
            config.rank_index_file_boost,
            config.rank_test_penalty,
            config.rank_popularity_weight,
            config.rank_popularity_cap
        );
        Self {
            db_path: config.db_path.clone(),
            config,
            tantivy,
            vectors,
            embedder,
            reranker,
            hyde_generator,
            cache: Arc::new(Mutex::new(cache)),
            cache_config_key,
            metrics,
        }
    }

    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        exported_only: bool,
    ) -> Result<SearchResponseWithSignals> {
        let _timer = self.metrics.search_duration.start_timer();

        let started_at_unix_s = unix_now_s();
        let started = Instant::now();

        let sqlite = SqliteStore::open(&self.db_path)?;
        sqlite.init()?;

        let current_last_update = sqlite.most_recent_symbol_update().unwrap_or(None);
        let current_index_run_started_at = sqlite
            .latest_index_run()
            .ok()
            .flatten()
            .map(|r| r.started_at_unix_s);
        let cache_key = format!(
            "v2|cfg={}|q={}|l={}|e={}",
            self.cache_config_key,
            trim_query(query, 500),
            limit,
            exported_only
        );
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if cache.last_symbol_update_unix_s != current_last_update
                || cache.last_index_run_started_at_unix_s != current_index_run_started_at
            {
                cache.responses.clear();
                cache.embeddings.clear();
                cache.contexts.clear();
                cache.last_symbol_update_unix_s = current_last_update;
                cache.last_index_run_started_at_unix_s = current_index_run_started_at;
            }
            if let Some(resp) = cache.responses.get(&cache_key) {
                return Ok(SearchResponseWithSignals {
                    response: resp,
                    hit_signals: HashMap::new(),
                });
            }
        }

        let (query_without_controls, controls) = parse_query_controls(query);

        // Fast path: direct ID lookup
        if let Some(resp) = fast_paths::handle_id_lookup(
            self,
            &sqlite,
            &controls,
            &query_without_controls,
            query,
            cache_key.clone(),
            limit,
            exported_only,
            started_at_unix_s,
            started,
        )? {
            return Ok(resp);
        }

        // Intent Detection
        let intent = detect_intent(&query_without_controls);

        // Detect NL queries for dynamic RRF weight adjustment.
        let is_nl_query = !contains_code_snippet(&query_without_controls)
            && query_without_controls.split_whitespace().count() >= 3;

        // Decompose compound queries from the ORIGINAL (pre-synonym-expansion) query.
        // Using synonym-expanded query would include synonym terms in sub-queries,
        // causing false-positive coverage matches (e.g., "admin" synonym makes
        // adminAppControlRouter "cover" the "role-based permissions" sub-query).
        let normalized_query = normalize_and_expand_query(&query_without_controls, false, false);
        let sub_queries = decompose_query(&normalized_query, 3);

        // Determine query for smart truncation
        let smart_truncation_query = if sub_queries.len() == 1 {
            Some(query_without_controls.as_str())
        } else {
            Some(sub_queries[0].as_str())
        };

        // Fast path: Callers intent (graph traversal, no search)
        if let Some(Intent::Callers(name)) = &intent {
            if let Some(resp) = fast_paths::handle_callers_intent(
                self,
                &sqlite,
                name,
                &query_without_controls,
                query,
                cache_key.clone(),
                limit,
                exported_only,
                started_at_unix_s,
                started,
            )? {
                return Ok(resp);
            }
        }

        // Execute hybrid search (BM25 + vector + RRF fusion + structural scoring)
        let hybrid_result = hybrid::execute_hybrid_search(
            self,
            &sqlite,
            &query_without_controls,
            &sub_queries,
            &intent,
            is_nl_query,
            limit,
        )
        .await?;

        let ranked = hybrid_result.ranked;
        let mut hit_signals = hybrid_result.hit_signals;
        let vector_ranked_for_promotion = hybrid_result.vector_ranked_for_promotion;
        let keyword_ms = hybrid_result.keyword_ms;
        let vector_ms = hybrid_result.vector_ms;

        let merge_t = Instant::now();

        let mut uniq = Vec::new();
        let mut seen = HashSet::new();
        for hit in ranked {
            if seen.insert(hit.id.clone()) {
                uniq.push(hit);
            }
        }

        // Inject framework pattern matches for NL queries
        let fw_injection_count = if is_nl_query {
            framework_patterns::inject_framework_patterns(
                &sqlite,
                &query_without_controls,
                &mut uniq,
                &mut seen,
            ).unwrap_or(0)
        } else {
            0
        };

        // Apply query control filters and boost signals.
        // Use simple normalized query (lowercase+trim, no synonym/stem expansion)
        // for selection boost lookup — must match the key stored by report_selection.
        let original_query_normalized = query_without_controls.to_lowercase();
        let hits = postprocess::filter_and_boost(
            &sqlite,
            uniq,
            &mut hit_signals,
            &controls,
            exported_only,
            original_query_normalized.trim(),
            &intent,
            &self.config,
        )?;

        // Apply cross-encoder reranking if available
        let mut hits = if let Some(reranker) = &self.reranker {
            if should_rerank(hits.len(), 3) {
                // Collect symbol texts for reranking
                let mut texts = HashMap::new();
                for hit in &hits {
                    if let Some(row) = sqlite.get_symbol_by_id(&hit.id).ok().flatten() {
                        texts.insert(hit.id.clone(), row.text);
                    }
                }

                let docs = prepare_rerank_docs(&hits, &texts);
                // Use the first sub-query for reranking (or original query)
                let rerank_query = &sub_queries[0];
                if let Ok(rerank_scores) = reranker.rerank(rerank_query, &docs).await {
                    apply_reranker_scores(&hits, &rerank_scores, 0.3) // 30% reranker weight
                } else {
                    hits
                }
            } else {
                hits
            }
        } else {
            hits
        };

        // R30v2: Gentle diversity — truncate to a larger pool so that
        // diversify_by_file has room to promote diverse results, but doesn't
        // aggressively displace relevant same-file results the way pre-truncation
        // diversity did (which regressed Q14 -3, Q15 -4 by promoting noise).
        // Expand pool when framework patterns injected high-scoring symbols that
        // would otherwise squeeze genuine BM25 results out of the pool window.
        let pool_size = limit * 4 + fw_injection_count;
        hits = diversify_by_cluster(&sqlite, hits, pool_size);
        hits.truncate(pool_size);

        // Save pre-expansion candidates for gap-fill after file-symbol dedup.
        let pre_expansion_candidates = hits.clone();
        let (hits, expanded_ids) = expand_with_edges(&sqlite, hits, pool_size, &intent, &query_without_controls)?;

        // Apply file/kind diversity on the expanded pool,
        // then truncate to final limit. This gives diversity enough headroom
        // to promote cross-file results without destroying same-file clusters.
        let mut hits = diversify_by_file(hits, limit);
        hits = diversify_by_kind(hits, limit);
        hits.truncate(limit);

        // Promote top vector results AFTER diversity + truncation.
        // This is the correct placement: post-RRF adjustments, edge expansion,
        // and diversity filtering have all run. Vector results that were buried
        // by BM25-friendly adjustments get promoted into the final top-N.
        if is_nl_query && self.config.vector_guaranteed_results > 0 && !vector_ranked_for_promotion.is_empty() {
            let hit_ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();
            let test_symbols = sqlite.batch_check_test_symbols(&hit_ids).unwrap_or_default();
            promote_vector_results(
                &mut hits,
                &vector_ranked_for_promotion,
                &test_symbols,
                &mut hit_signals,
                limit,
                self.config.vector_guaranteed_results,
                &query_without_controls,
            );
            hits.truncate(limit);
        }

        // Final intent enforcement: apply suppressive intent multipliers to ALL
        // hits, including those added by expand_with_edges. Edge expansion derives
        // scores from parent symbols, which can reintroduce test helpers (e.g.,
        // create_test_normalizer as a reference of PathNormalizer) with high scores
        // that bypass earlier intent penalties. This final pass catches them.
        //
        // Also checks SQL-based test detection (mod tests containment) which
        // catches test functions that escape name/file heuristics (e.g.,
        // framework_tags_make_websocket_handler_searchable in tantivy.rs).
        {
            let final_hit_ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();
            let final_test_symbols = sqlite.batch_check_test_symbols(&final_hit_ids).unwrap_or_default();
            let is_test_intent = matches!(intent, Some(Intent::Test));

            for hit in &mut hits {
                // SQL-based test detection: symbols inside `mod tests` blocks
                // that escape name/file heuristics. Apply 0.01x multiplier
                // (same as is_test_symbol/is_test_file) so they're effectively
                // invisible in non-test queries.
                if !is_test_intent && final_test_symbols.contains(&hit.id) {
                    hit.score *= 0.01;
                    continue;
                }

                let intent_mult = if let Some(sig) = hit_signals.get(&hit.id) {
                    sig.intent_mult
                } else {
                    // Edge-expanded or vector-promoted hit without signals:
                    // compute intent adjustment on the fly
                    ranking::intent_adjustment(
                        &intent,
                        &hit.kind,
                        &hit.file_path,
                        hit.exported,
                        &hit.name,
                    )
                };
                if intent_mult < 1.0 {
                    hit.score *= intent_mult;
                }
            }
            // Re-sort after enforcement
            hits.sort_by(|a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then_with(|| b.exported.cmp(&a.exported))
                    .then_with(|| a.name.cmp(&b.name))
            });
            hits.truncate(limit);
        }

        // Drop results with negligible scores. These are test symbols or
        // heavily suppressed results that survived pool expansion but add
        // no value to the user. Threshold 0.5 is well below any
        // meaningful result (lowest legitimate scores are ~3.0).
        // Also remove test file results in non-test queries — they should
        // never appear regardless of score.
        let is_test_intent = matches!(intent, Some(Intent::Test));
        hits.retain(|h| {
            h.score >= 0.5 && (is_test_intent || !ranking::is_test_file(&h.file_path))
        });

        // Post-pipeline gap fill: if fewer than `limit` results survived
        // enforcement + min-score filtering, backfill from the pre-expansion pool.
        // Respects file diversity: limits per-file count during backfill.
        if hits.len() < limit {
            let is_test_intent = matches!(intent, Some(Intent::Test));
            let mut hit_ids: std::collections::HashSet<String> =
                hits.iter().map(|h| h.id.clone()).collect();
            let gap_max_per_file = (limit / 5).max(2) + 2; // slightly above diversity total_cap to allow gap fill after concentration bump
            let mut gap_file_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for h in &hits {
                if h.kind != "file" {
                    *gap_file_counts.entry(h.file_path.clone()).or_insert(0) += 1;
                }
            }

            // Source 1: non-file symbols from pre-expansion pool
            for h in &pre_expansion_candidates {
                if hits.len() >= limit {
                    break;
                }
                if h.kind != "file" && !hit_ids.contains(&h.id) {
                    // Skip test files in non-test queries
                    if !is_test_intent && ranking::is_test_file(&h.file_path) {
                        continue;
                    }
                    // Respect file diversity in gap fill
                    let fc = gap_file_counts.get(&h.file_path).copied().unwrap_or(0);
                    if fc >= gap_max_per_file {
                        continue;
                    }
                    // Apply intent enforcement to gap-filled result
                    let intent_mult = ranking::intent_adjustment(
                        &intent, &h.kind, &h.file_path, h.exported, &h.name,
                    );
                    let adj_score = if intent_mult < 1.0 {
                        h.score * intent_mult
                    } else {
                        h.score
                    };
                    if adj_score >= 0.5 {
                        hit_ids.insert(h.id.clone());
                        *gap_file_counts.entry(h.file_path.clone()).or_insert(0) += 1;
                        let mut gap_hit = h.clone();
                        gap_hit.score = adj_score;
                        hits.push(gap_hit);
                    }
                }
            }

            // Source 2: file-expansion — for files in pre-expansion that are only
            // represented by a file symbol (no function from same file in hits),
            // surface their top exported function.
            if hits.len() < limit {
                let files_with_fn: std::collections::HashSet<String> = hits
                    .iter()
                    .filter(|h| h.kind != "file")
                    .map(|h| h.file_path.clone())
                    .collect();
                for h in &pre_expansion_candidates {
                    if hits.len() >= limit {
                        break;
                    }
                    if h.kind != "file" {
                        continue;
                    }
                    if files_with_fn.contains(&h.file_path) {
                        continue;
                    }
                    if let Ok(symbols) = sqlite.list_symbols_by_file(&h.file_path) {
                        for row in symbols {
                            if row.kind == "file" || !row.exported {
                                continue;
                            }
                            if !hit_ids.contains(&row.id) {
                                // Check test status
                                let test_ids = sqlite
                                    .batch_check_test_symbols(std::slice::from_ref(&row.id))
                                    .unwrap_or_default();
                                if !is_test_intent && test_ids.contains(&row.id) {
                                    continue;
                                }
                                let intent_mult = ranking::intent_adjustment(
                                    &intent, &row.kind, &row.file_path, row.exported, &row.name,
                                );
                                let adj_score = h.score * 0.7 * if intent_mult < 1.0 {
                                    intent_mult
                                } else {
                                    1.0
                                };
                                if adj_score >= 0.5 {
                                    hit_ids.insert(row.id.clone());
                                    hits.push(RankedHit {
                                        id: row.id.clone(),
                                        score: adj_score,
                                        name: row.name,
                                        kind: row.kind,
                                        file_path: row.file_path,
                                        exported: row.exported,
                                        language: row.language,
                                    });
                                    break; // one function per file
                                }
                            }
                        }
                    }
                }
            }

            // Re-sort after gap fill to maintain proper score ordering
            hits.sort_by(|a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then_with(|| b.exported.cmp(&a.exported))
                    .then_with(|| a.name.cmp(&b.name))
            });
            hits.truncate(limit);
        }

        // Sub-query coverage enforcement: for compound queries split on "and",
        // ensure each sub-query branch has at least one representative. Without
        // this, one dominant branch can crowd out the other entirely (e.g.,
        // "onboarding" crowds out "invitation" in "Invitation system and user onboarding").
        let mut coverage_injected = false;
        if sub_queries.len() > 1 && hits.len() >= 2 {
            let hit_ids_set: HashSet<String> = hits.iter().map(|h| h.id.clone()).collect();
            for sq in &sub_queries {
                // Extract significant terms from this sub-query (>3 chars, skip stopwords)
                let mut raw_terms: Vec<String> = sq
                    .split_whitespace()
                    .filter(|w| w.len() > 3)
                    .filter(|w| {
                        !matches!(
                            w.to_lowercase().as_str(),
                            "does" | "have" | "with" | "from" | "that" | "this" | "what" | "how"
                        )
                    })
                    .map(|w| w.to_lowercase())
                    .collect();
                // Deduplicate stem variants: if "permission" and "permissions"
                // are both present, keep only the shorter one. Without this,
                // normalize_query's stems cause the 2-term check to double-count
                // (e.g., "permissions.rs" matches both → 2 matches from 1 word).
                raw_terms.sort_by_key(|t| t.len());
                raw_terms = raw_terms.into_iter().fold(Vec::new(), |mut acc, t| {
                    if !acc.iter().any(|existing: &String| t.starts_with(existing.as_str())) {
                        acc.push(t);
                    }
                    acc
                });
                if raw_terms.is_empty() {
                    continue;
                }

                // Expand terms: split hyphens, basic stemming (strip trailing 's')
                let mut terms: Vec<String> = raw_terms.clone();
                for t in &raw_terms {
                    if t.contains('-') {
                        for part in t.split('-') {
                            if part.len() > 3 {
                                terms.push(part.to_string());
                            }
                        }
                    }
                    if t.ends_with('s') && t.len() > 4 && !t.ends_with("ss") {
                        terms.push(t[..t.len() - 1].to_string());
                    }
                }
                terms.sort();
                terms.dedup();

                // Check if any current result matches this sub-query.
                // For multi-term sub-queries, require 2+ term matches to prevent
                // false positives (e.g., "permissions" in a platform/macos path
                // falsely satisfying "role-based permissions" coverage).
                let has_match = hits.iter().any(|h| {
                    let name_lower = h.name.to_lowercase();
                    let path_lower = h.file_path.to_lowercase();
                    if raw_terms.len() >= 2 {
                        // Count ORIGINAL terms (not stems) to avoid double-counting.
                        // e.g., "access" + stem "acces" both match "accessRouter" → 2 matches
                        // from 1 real term. Use raw_terms to require distinct real terms.
                        let match_count = raw_terms.iter()
                            .filter(|t| name_lower.contains(t.as_str()) || path_lower.contains(t.as_str()))
                            .count();
                        match_count >= 2
                    } else {
                        terms.iter().any(|t| name_lower.contains(t) || path_lower.contains(t))
                    }
                });

                if !has_match {
                    // No direct match — expand with synonyms for candidate search.
                    // This bridges vocabulary gaps (e.g., "role" → "admin", "auth").
                    let mut synonym_terms = terms.clone();
                    for t in &terms {
                        for related in get_related_terms(t) {
                            let r = related.to_string();
                            if !synonym_terms.contains(&r) {
                                synonym_terms.push(r);
                            }
                        }
                    }

                    // Find the best candidate from pre-expansion pool.
                    // Prefer name matches over path-only matches, and
                    // prefer functions/structs over consts/variables.
                    let mut best_candidate: Option<&RankedHit> = None;
                    let mut best_priority = 0u8; // higher = better match quality
                    for c in &pre_expansion_candidates {
                        if hit_ids_set.contains(&c.id) || c.kind == "file" || c.score < 0.5 {
                            continue;
                        }
                        let name_lower = c.name.to_lowercase();
                        let path_lower = c.file_path.to_lowercase();
                        let name_match = synonym_terms.iter().any(|t| name_lower.contains(t));
                        let path_match = synonym_terms.iter().any(|t| path_lower.contains(t));
                        if !name_match && !path_match {
                            continue;
                        }
                        // Require sufficient term coverage to prevent overly-generic
                        // matches. For 2+ term sub-queries, the candidate must match
                        // at least 2 distinct terms (including hyphen-split parts and
                        // stems). Without this, "request" alone matches
                        // requestAccessibilityPermission for "request throttling",
                        // and the low-scoring injection triggers gap detection truncation.
                        if raw_terms.len() >= 2 {
                            let term_match_count = raw_terms.iter()
                                .filter(|t| name_lower.contains(t.as_str()) || path_lower.contains(t.as_str()))
                                .count();
                            if term_match_count < 2 {
                                continue;
                            }
                        }
                        // Priority: name+meaningful kind > name+any kind > path-only+meaningful > path-only
                        let is_meaningful = !matches!(c.kind.as_str(), "const" | "variable" | "property");
                        let priority = match (name_match, is_meaningful) {
                            (true, true) => 4,
                            (true, false) => 3,
                            (false, true) => 2,
                            (false, false) => 1,
                        };
                        if priority > best_priority {
                            best_priority = priority;
                            best_candidate = Some(c);
                            if priority == 4 { break; } // best possible, stop searching
                        }
                    }
                    // Fallback: if no candidate in pre-expansion pool, do a direct
                    // BM25 search for the uncovered sub-query. This handles cases
                    // where framework injection floods the pool and the middleware/
                    // secondary symbols are pushed below pool_size.
                    let mut fallback_hit: Option<RankedHit> = None;
                    if best_candidate.is_none() {
                        if let Ok(fallback_results) = self.tantivy.search(sq, 20) {
                            let fallback_ids: Vec<String> = fallback_results.iter().map(|h| h.id.clone()).collect();
                            let fallback_tests = sqlite.batch_check_test_symbols(&fallback_ids).unwrap_or_default();
                            let mut fb_best_priority = 0u8;
                            for fh in &fallback_results {
                                if hit_ids_set.contains(&fh.id) || fh.kind == "file" {
                                    continue;
                                }
                                if !is_test_intent && (fallback_tests.contains(&fh.id) || ranking::is_test_file(&fh.file_path)) {
                                    continue;
                                }
                                let name_lower = fh.name.to_lowercase();
                                let path_lower = fh.file_path.to_lowercase();
                                let name_match = synonym_terms.iter().any(|t| name_lower.contains(t));
                                let path_match = synonym_terms.iter().any(|t| path_lower.contains(t));
                                if !name_match && !path_match {
                                    continue;
                                }
                                if raw_terms.len() >= 2 {
                                    let term_match_count = raw_terms.iter()
                                        .filter(|t| name_lower.contains(t.as_str()) || path_lower.contains(t.as_str()))
                                        .count();
                                    if term_match_count < 2 {
                                        continue;
                                    }
                                }
                                let is_meaningful = !matches!(fh.kind.as_str(), "const" | "variable" | "property");
                                let priority = match (name_match, is_meaningful) {
                                    (true, true) => 4,
                                    (true, false) => 3,
                                    (false, true) => 2,
                                    (false, false) => 1,
                                };
                                if priority > fb_best_priority {
                                    fb_best_priority = priority;
                                    // Give it a reasonable score: half of the lowest current result
                                    let inject_score = hits.last().map(|h| h.score * 0.5).unwrap_or(1.0).max(1.0);
                                    fallback_hit = Some(RankedHit {
                                        id: fh.id.clone(),
                                        score: inject_score,
                                        name: fh.name.clone(),
                                        kind: fh.kind.clone(),
                                        file_path: fh.file_path.clone(),
                                        exported: fh.exported,
                                        language: String::new(),
                                    });
                                    if priority == 4 { break; }
                                }
                            }
                        }
                    }

                    if let Some(c) = best_candidate {
                        // Replace the lowest-scoring result
                        if let Some(last) = hits.last_mut() {
                            *last = c.clone();
                            coverage_injected = true;
                        }
                    } else if let Some(fb) = fallback_hit {
                        if let Some(last) = hits.last_mut() {
                            *last = fb;
                            coverage_injected = true;
                        }
                    }
                }
            }
            // Re-sort after potential injection
            hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        }

        // Name deduplication: collapse symbols with identical names from different
        // files (e.g., has_accessibility_permission in platform/windows/ and
        // platform/linux/). Keeps the highest-scoring instance.
        {
            let mut seen_names: HashSet<String> = HashSet::new();
            hits.retain(|h| {
                // file-level symbols use unique paths as names, skip dedup
                if h.kind == "file" {
                    return true;
                }
                seen_names.insert(h.name.clone())
            });
        }

        // Post-dedup gap fill: if name dedup removed results, backfill from
        // pre_expansion_candidates to maintain `limit` results.
        if hits.len() < limit {
            let is_test_intent = matches!(intent, Some(Intent::Test));
            let hit_ids: HashSet<String> = hits.iter().map(|h| h.id.clone()).collect();
            let hit_names: HashSet<String> = hits.iter()
                .filter(|h| h.kind != "file")
                .map(|h| h.name.clone())
                .collect();
            for c in &pre_expansion_candidates {
                if hits.len() >= limit {
                    break;
                }
                if c.kind != "file"
                    && !hit_ids.contains(&c.id)
                    && !hit_names.contains(&c.name)
                    && c.score >= 0.5
                    && (is_test_intent || !ranking::is_test_file(&c.file_path))
                {
                    hits.push(c.clone());
                }
            }
        }

        // Score-gap detection: drop trailing noise after extreme score drops.
        // A ~2.5x+ drop between consecutive results (ratio < 0.4) indicates
        // a noise result that survived the pipeline. Only scan positions 3+
        // to never truncate the top 3 results.
        // Skip when sub-query coverage injected a result — the injection is
        // intentional and its lower score is expected (it covers a different
        // sub-query branch, not the dominant one).
        if hits.len() >= 4 && !coverage_injected {
            let mut truncate_at = hits.len();
            for i in 3..hits.len() {
                if hits[i - 1].score > 0.0 && hits[i].score / hits[i - 1].score < 0.4 {
                    truncate_at = i;
                    break;
                }
            }
            if truncate_at < hits.len() {
                hits.truncate(truncate_at);
            }
        }

        // Post-score-gap gap fill: if score-gap detection removed results,
        // backfill from pre-expansion pool to maintain `limit` results.
        // Candidates must score above the gap threshold relative to the last
        // kept result to avoid re-injecting the same noise score-gap removed.
        if hits.len() < limit {
            let min_score = hits.last().map(|h| h.score * 0.4).unwrap_or(0.5);
            let hit_ids: HashSet<String> = hits.iter().map(|h| h.id.clone()).collect();
            let hit_names: HashSet<String> = hits.iter()
                .filter(|h| h.kind != "file")
                .map(|h| h.name.clone())
                .collect();
            for c in &pre_expansion_candidates {
                if hits.len() >= limit {
                    break;
                }
                if c.kind != "file"
                    && !hit_ids.contains(&c.id)
                    && !hit_names.contains(&c.name)
                    && c.score >= min_score
                    && (is_test_intent || !ranking::is_test_file(&c.file_path))
                {
                    hits.push(c.clone());
                }
            }
        }

        let mut roots = Vec::new();
        let mut extra = Vec::new();

        for h in &hits {
            if let Some(row) = sqlite.get_symbol_by_id(&h.id).ok().flatten() {
                if expanded_ids.contains(&h.id) {
                    extra.push(row);
                } else {
                    roots.push(row);
                }
            }
        }

        let (context, _context_items) =
            self.assemble_context_cached(&sqlite, &roots, &extra, smart_truncation_query)?;

        let merge_ms = merge_t.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;

        let run = crate::storage::sqlite::SearchRunRow {
            started_at_unix_s,
            duration_ms,
            keyword_ms,
            vector_ms,
            merge_ms,
            query: trim_query(query, 200),
            query_limit: limit as u64,
            exported_only,
            result_count: hits.len() as u64,
        };
        let _ = sqlite.insert_search_run(&run);

        // Record Prometheus metrics
        self.metrics.search_results_total.inc_by(hits.len() as f64);

        let resp = SearchResponse {
            query: query.to_string(),
            limit,
            hits,
            context,
        };
        self.cache_insert_response(cache_key, resp.clone(), &_context_items);

        // Note: timer observes duration when dropped
        Ok(SearchResponseWithSignals {
            response: resp,
            hit_signals,
        })
    }

    pub(super) fn cache_insert_response(
        &self,
        key: String,
        resp: SearchResponse,
        context_items: &[ContextItem],
    ) {
        let size =
            resp.context.len() + context_items.iter().map(|i| i.tokens * 4).sum::<usize>();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.responses.insert(key, resp, size);
    }

    pub(super) async fn get_query_vector_cached(&self, query: &str) -> Result<Vec<f32>> {
        let key = format!("q={}", trim_query(query, 500));
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(v) = cache.embeddings.get(&key) {
                return Ok(v);
            }
        }

        let v = {
            let mut embedder = self.embedder.lock().await;
            let mut out = embedder.query_embed(&[query.to_string()])?;
            out.pop()
                .ok_or_else(|| anyhow!("Embedder returned no vector"))?
        };

        let size = v.len().saturating_mul(4);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.embeddings.insert(key, v.clone(), size);
        Ok(v)
    }

    pub(super) fn assemble_context_cached(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        extra: &[SymbolRow],
        query: Option<&str>,
    ) -> Result<(String, Vec<ContextItem>)> {
        let mut root_ids = roots.iter().map(|r| r.id.as_str()).collect::<Vec<_>>();
        root_ids.sort_unstable();
        let mut extra_ids = extra.iter().map(|r| r.id.as_str()).collect::<Vec<_>>();
        extra_ids.sort_unstable();

        // Include query hash in cache key to prevent stale cached results for different queries
        let query_hash = query
            .map(|q| {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut h = DefaultHasher::new();
                q.hash(&mut h);
                format!("{:x}", h.finish())
            })
            .unwrap_or_else(|| "none".to_string());

        let key = format!(
            "m=default|q={}|t={}|r={}|x={}",
            query_hash,
            self.config.max_context_tokens,
            root_ids.join(","),
            extra_ids.join(",")
        );
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(v) = cache.contexts.get(&key) {
                return Ok(v);
            }
        }

        let assembler = ContextAssembler::new(self.config.clone());
        let v = assembler.assemble_context_with_items(store, roots, extra, query)?;
        let size = v.0.len() + v.1.iter().map(|i| i.tokens * 4).sum::<usize>();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.contexts.insert(key, v.clone(), size);
        Ok(v)
    }

    /// Get reference to vector store for vector queries
    pub fn get_vector_store(&self) -> &LanceVectorTable {
        &self.vectors
    }

    /// Get embedding for a single text string
    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let mut embedder = self.embedder.lock().await;
        let mut results = embedder.embed(&[text.to_string()])?;
        results
            .pop()
            .ok_or_else(|| anyhow!("Embedder returned no vector"))
    }

    pub fn assemble_definitions(&self, symbols: &[SymbolRow]) -> Result<String> {
        let sqlite = SqliteStore::open(&self.db_path)?;
        sqlite.init()?;
        let assembler = ContextAssembler::new(self.config.clone());
        Ok(assembler
            .format_context(&sqlite, symbols, &[], &[], None)?
            .0)
    }

    pub fn load_symbol_rows_by_ids(&self, ids: &[String]) -> Result<Vec<SymbolRow>> {
        let sqlite = SqliteStore::open(&self.db_path)?;
        sqlite.init()?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(row) = sqlite.get_symbol_by_id(id)? {
                out.push(row);
            }
        }
        Ok(out)
    }
}

/// Detect programming language from query text for HyDE
pub(super) fn detect_language_from_query(query: &str) -> &'static str {
    let q = query.to_lowercase();
    if q.contains("rust") || q.contains("fn ") || q.contains("impl") {
        "rust"
    } else if q.contains("typescript") || q.contains("interface") || q.contains("type ") {
        "typescript"
    } else if q.contains("python") || q.contains("def ") || q.contains("class ") {
        "python"
    } else if q.contains("go") || q.contains("func ") {
        "go"
    } else {
        "typescript" // Default
    }
}

fn unix_now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

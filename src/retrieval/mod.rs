//! Retrieval module for code intelligence search

pub mod assembler;
mod cache;
mod fast_paths;
mod framework_patterns;
mod hybrid;
pub mod hyde;
mod postprocess;
mod query;
mod ranking;

use crate::path::Utf8PathBuf;
use crate::retrieval::hyde::HypotheticalCodeGenerator;
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

    let missing: Vec<&RankedHit> = top_vector
        .into_iter()
        .filter(|h| !final_ids.contains(h.id.as_str()))
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

        // Normalize and expand query
        let expanded_query = normalize_and_expand_query(
            &query_without_controls,
            self.config.synonym_expansion_enabled,
            self.config.acronym_expansion_enabled,
        );

        // Decompose compound queries (e.g., "auth and database") into sub-queries
        let sub_queries = decompose_query(&expanded_query, 3);

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
        if is_nl_query {
            let _ = framework_patterns::inject_framework_patterns(
                &sqlite,
                &query_without_controls,
                &mut uniq,
                &mut seen,
            );
        }

        // Apply query control filters and boost signals
        let hits = postprocess::filter_and_boost(
            &sqlite,
            uniq,
            &mut hit_signals,
            &controls,
            exported_only,
            &expanded_query,
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

        // R30v2: Gentle diversity — truncate to a larger pool (limit*3) so that
        // diversify_by_file has room to promote diverse results, but doesn't
        // aggressively displace relevant same-file results the way pre-truncation
        // diversity did (which regressed Q14 -3, Q15 -4 by promoting noise).
        hits = diversify_by_cluster(&sqlite, hits, limit * 3);
        hits.truncate(limit * 3);

        // Save pre-expansion candidates for gap-fill after file-symbol dedup.
        let pre_expansion_candidates = hits.clone();
        let (hits, expanded_ids) = expand_with_edges(&sqlite, hits, limit, &intent)?;

        // Apply file/kind diversity on the expanded pool (limit*3 candidates),
        // then truncate to final limit. This gives diversity enough headroom
        // to promote cross-file results without destroying same-file clusters.
        let mut hits = diversify_by_file(hits, limit);
        hits = diversify_by_kind(hits, limit);
        hits.truncate(limit);

        // Post-diversity gap-fill: if file-symbol dedup in diversify_by_file
        // left fewer than `limit` results, backfill from pre-expansion pool.
        // This runs AFTER diversity so it can't influence diversity's choices.
        if hits.len() < limit {
            let hit_ids: std::collections::HashSet<String> =
                hits.iter().map(|h| h.id.clone()).collect();
            for h in &pre_expansion_candidates {
                if hits.len() >= limit {
                    break;
                }
                if h.kind != "file" && !hit_ids.contains(&h.id) {
                    hits.push(h.clone());
                }
            }
        }

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
        hits.retain(|h| h.score >= 0.5);

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

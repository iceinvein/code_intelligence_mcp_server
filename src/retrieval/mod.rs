//! Retrieval module for code intelligence search

pub mod assembler;
mod cache;
pub mod hyde;
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
    parse_query_controls, trim_query, Intent, QueryControls,
};
use ranking::{
    apply_docstring_boost_with_signals, apply_file_affinity_boost_with_signals,
    apply_package_boost_with_signals, apply_popularity_boost_with_signals, apply_reranker_scores,
    apply_selection_boost_with_signals, diversify_by_cluster, diversify_by_file, diversify_by_kind,
    expand_with_edges, get_graph_ranked_hits, prepare_rerank_docs, rank_hits_with_signals,
    reciprocal_rank_fusion, should_rerank,
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

#[derive(Debug, Clone, Serialize)]
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
    config: Arc<Config>,
    db_path: Utf8PathBuf,
    tantivy: Arc<TantivyIndex>,
    vectors: Arc<LanceVectorTable>,
    embedder: Arc<AsyncMutex<Box<dyn Embedder + Send>>>,
    reranker: Option<Arc<dyn Reranker>>,
    hyde_generator: Option<HypotheticalCodeGenerator>,
    cache: Arc<Mutex<RetrieverCaches>>,
    cache_config_key: String,
    metrics: Arc<MetricsRegistry>,
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

        if let Some(id) = &controls.id {
            if let Some(row) = sqlite.get_symbol_by_id(id)? {
                if exported_only && !row.exported {
                    return Ok(SearchResponseWithSignals {
                        response: SearchResponse {
                            query: query.to_string(),
                            limit,
                            hits: vec![],
                            context: String::new(),
                        },
                        hit_signals: HashMap::new(),
                    });
                }

                let hits = vec![RankedHit {
                    id: row.id.clone(),
                    score: 1.0,
                    name: row.name.clone(),
                    kind: row.kind.clone(),
                    file_path: row.file_path.clone(),
                    exported: row.exported,
                    language: row.language.clone(),
                }];

                let (context, _context_items) = self.assemble_context_cached(
                    &sqlite,
                    std::slice::from_ref(&row),
                    &[],
                    Some(query_without_controls.as_str()),
                )?;

                let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                let run = crate::storage::sqlite::SearchRunRow {
                    started_at_unix_s,
                    duration_ms,
                    keyword_ms: 0,
                    vector_ms: 0,
                    merge_ms: 0,
                    query: trim_query(query, 200),
                    query_limit: limit as u64,
                    exported_only,
                    result_count: hits.len() as u64,
                };
                let _ = sqlite.insert_search_run(&run);

                let resp = SearchResponse {
                    query: query.to_string(),
                    limit,
                    hits,
                    context,
                };
                self.cache_insert_response(cache_key, resp.clone(), &[]);
                return Ok(SearchResponseWithSignals {
                    response: resp,
                    hit_signals: HashMap::new(),
                });
            }
        }

        // Intent Detection
        let intent = detect_intent(&query_without_controls);

        // Detect NL queries for dynamic RRF weight adjustment.
        // NL queries (3+ words, not code) benefit from higher vector weight
        // because BM25 often matches irrelevant identifiers while vector search
        // captures semantic intent (e.g., "WebSocket handler" → elysia.rs WS code).
        let is_nl_query = !contains_code_snippet(&query_without_controls)
            && query_without_controls.split_whitespace().count() >= 3;

        // Normalize and expand query
        let expanded_query = normalize_and_expand_query(
            &query_without_controls,
            self.config.synonym_expansion_enabled,
            self.config.acronym_expansion_enabled,
        );

        // Decompose compound queries (e.g., "auth and database") into sub-queries
        let sub_queries = decompose_query(&expanded_query, 3); // max_depth=3

        // Determine query for smart truncation
        // Use first sub-query for relevance scoring (primary user intent)
        let smart_truncation_query = if sub_queries.len() == 1 {
            Some(query_without_controls.as_str())
        } else {
            Some(sub_queries[0].as_str())
        };

        if let Some(Intent::Callers(name)) = &intent {
            let targets = sqlite.search_symbols_by_exact_name(name, None, 5)?;
            if let Some(target) = targets.first() {
                let edges = sqlite.list_edges_to(&target.id, limit * 2)?;
                let mut hits = Vec::new();
                let mut seen_hits = HashSet::new();

                for e in edges {
                    if e.edge_type == "call" || e.edge_type == "reference" {
                        if seen_hits.contains(&e.from_symbol_id) {
                            continue;
                        }
                        if let Some(row) = sqlite.get_symbol_by_id(&e.from_symbol_id)? {
                            if exported_only && !row.exported {
                                continue;
                            }
                            seen_hits.insert(row.id.clone());
                            hits.push(RankedHit {
                                id: row.id,
                                score: 1.0,
                                name: row.name,
                                kind: row.kind,
                                file_path: row.file_path,
                                exported: row.exported,
                                language: row.language,
                            });
                        }
                    }
                }

                if !hits.is_empty() {
                    hits.truncate(limit);
                    let rows = hits
                        .iter()
                        .filter_map(|h| sqlite.get_symbol_by_id(&h.id).ok().flatten())
                        .collect::<Vec<_>>();

                    let (context, _context_items) = self.assemble_context_cached(
                        &sqlite,
                        &rows,
                        &[],
                        Some(query_without_controls.as_str()),
                    )?;

                    let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                    let run = crate::storage::sqlite::SearchRunRow {
                        started_at_unix_s,
                        duration_ms,
                        keyword_ms: 0,
                        vector_ms: 0,
                        merge_ms: 0,
                        query: trim_query(query, 200),
                        query_limit: limit as u64,
                        exported_only,
                        result_count: hits.len() as u64,
                    };
                    let _ = sqlite.insert_search_run(&run);

                    let resp = SearchResponse {
                        query: query.to_string(),
                        limit,
                        hits,
                        context,
                    };
                    self.cache_insert_response(cache_key, resp.clone(), &[]);
                    return Ok(SearchResponseWithSignals {
                        response: resp,
                        hit_signals: HashMap::new(),
                    });
                }
            }
        }

        // Conditional: single-query path vs multi-query path based on decomposition
        // Single query preserves existing behavior; multi-query uses unified RRF
        let (ranked, mut hit_signals): (Vec<RankedHit>, HashMap<String, HitSignals>) =
            if sub_queries.len() == 1 {
                // SINGLE-QUERY PATH: Use existing logic unchanged
                let search_query = &sub_queries[0];

                let k = if contains_code_snippet(search_query) {
                    self.config.vector_search_limit.max(limit).max(5)
                } else {
                    self.config.vector_search_limit.max(limit * 3).max(40)
                };
                let keyword_t = Instant::now();
                let keyword_hits = self.tantivy.search(search_query, k)?;
                let _keyword_ms = keyword_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

                let vector_t = Instant::now();

                // Vector search with graceful degradation
                let (vector_hits, _vector_degraded) = match self.get_query_vector_cached(search_query).await {
                    Ok(query_vector) => {
                        match self.vectors.search(&query_vector, k).await {
                            Ok(mut hits) => {
                                // HyDE: Add hypothetical document retrieval (best-effort)
                                if self.config.hyde_enabled {
                                    if let Some(generator) = &self.hyde_generator {
                                        let language = detect_language_from_query(search_query);
                                        if let Ok(hyde_result) = generator.generate(search_query, language).await {
                                            let mut embedder = self.embedder.lock().await;
                                            if let Ok(hyde_embeddings) =
                                                embedder.embed(&[hyde_result.hypothetical_code])
                                            {
                                                if let Some(hyde_vector) = hyde_embeddings.first() {
                                                    if let Ok(mut hyde_hits) =
                                                        self.vectors.search(hyde_vector, k / 2).await
                                                    {
                                                        hits.append(&mut hyde_hits);
                                                    }
                                                }
                                            }
                                        }
                                        // HyDE failures are silently ignored - it's a best-effort enhancement
                                    }
                                }
                                (hits, false)
                            }
                            Err(e) => {
                                // Vector search failed - degrade gracefully
                                tracing::warn!(
                                    query = %search_query,
                                    error = %e,
                                    "LanceDB vector search failed, degrading to keyword-only search"
                                );
                                self.metrics.search_errors_total.inc();
                                (Vec::new(), true)
                            }
                        }
                    }
                    Err(e) => {
                        // Embedding generation failed - degrade gracefully
                        tracing::warn!(
                            query = %search_query,
                            error = %e,
                            "Query embedding generation failed, degrading to keyword-only search"
                        );
                        self.metrics.search_errors_total.inc();
                        (Vec::new(), true)
                    }
                };

                let _vector_ms = vector_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

                // Use RRF if enabled, otherwise use existing score fusion
                if self.config.rrf_enabled {
                    // Convert keyword_hits to RankedHit for RRF
                    let keyword_ranked: Vec<RankedHit> = keyword_hits
                        .iter()
                        .map(|h| RankedHit {
                            id: h.id.clone(),
                            score: h.score,
                            name: h.name.clone(),
                            kind: h.kind.clone(),
                            file_path: h.file_path.clone(),
                            exported: h.exported,
                            language: String::new(), // Will be filled from DB if needed
                        })
                        .collect();

                    // Convert vector_hits to RankedHit for RRF
                    let vector_ranked: Vec<RankedHit> = vector_hits
                        .iter()
                        .map(|h| RankedHit {
                            id: h.id.clone(),
                            score: 1.0 / (1.0 + h.distance.unwrap_or(1.0).max(0.0)), // Convert distance to score
                            name: h.name.clone(),
                            kind: h.kind.clone(),
                            file_path: h.file_path.clone(),
                            exported: h.exported,
                            language: h.language.clone(),
                        })
                        .collect();

                    // Get graph-ranked hits
                    let graph_hits =
                        if let Ok(graph) = get_graph_ranked_hits(&keyword_ranked, &sqlite) {
                            graph
                        } else {
                            keyword_ranked.clone()
                        };

                    // Apply RRF with dynamic weights based on query type.
                    // NL queries get higher vector weight because BM25 often
                    // matches irrelevant identifiers for conceptual queries.
                    // R27 tested equal weights but it regressed Q4 by -4 points
                    // (vector was correctly finding config.rs, equal weights let
                    // BM25 noise drown it out). Reverted to original 0.5x/1.5x.
                    let weights = if is_nl_query {
                        (
                            self.config.rrf_keyword_weight * 0.5,
                            self.config.rrf_vector_weight * 1.5,
                            self.config.rrf_graph_weight,
                        )
                    } else {
                        (
                            self.config.rrf_keyword_weight,
                            self.config.rrf_vector_weight,
                            self.config.rrf_graph_weight,
                        )
                    };

                    let mut rrf_results = reciprocal_rank_fusion(
                        &keyword_ranked,
                        &vector_ranked,
                        &graph_hits,
                        weights,
                    );

                    // Build lookup maps for original keyword/vector scores (for diagnostics)
                    let kw_score_map: HashMap<&str, f32> = keyword_ranked
                        .iter()
                        .map(|h| (h.id.as_str(), h.score))
                        .collect();
                    let vec_score_map: HashMap<&str, f32> = vector_ranked
                        .iter()
                        .map(|h| (h.id.as_str(), h.score))
                        .collect();

                    // Apply structural and intent adjustments post-RRF.
                    // RRF only considers rank position, not content signals like
                    // test penalties, intent multipliers, or directory semantics.
                    let line_count_ids: Vec<String> = rrf_results.iter().map(|h| h.id.clone()).collect();
                    let line_counts = sqlite.batch_get_symbol_line_counts(&line_count_ids).unwrap_or_default();
                    let test_symbols = sqlite.batch_check_test_symbols(&line_count_ids).unwrap_or_default();
                    let symbol_texts = sqlite.batch_get_symbol_texts(&line_count_ids).unwrap_or_default();

                    let mut signals = HashMap::new();
                    for hit in rrf_results.iter_mut() {
                        let structural = ranking::structural_adjustment(
                            &self.config,
                            hit.exported,
                            &hit.file_path,
                            &hit.kind,
                            &intent,
                            &query_without_controls,
                        );
                        let intent_mult = ranking::intent_adjustment(
                            &intent,
                            &hit.kind,
                            &hit.file_path,
                            hit.exported,
                            &hit.name,
                        );

                        let base_score = hit.score;
                        let def_bias = ranking::definition_bias(
                            &query_without_controls,
                            &hit.name,
                            &hit.kind,
                            &intent,
                        );
                        let body = symbol_texts.get(&hit.id).map(|s| s.as_str());
                        let tc = ranking::term_coverage_adjustment(
                            &query_without_controls,
                            &hit.name,
                            &hit.file_path,
                            body,
                        );
                        let lc = line_counts.get(&hit.id).copied().unwrap_or(0);
                        let si = ranking::symbol_importance_adjustment(lc, hit.exported);
                        let is_test = test_symbols.contains(&hit.id);
                        let tp = ranking::test_symbol_penalty(is_test);
                        hit.score = (hit.score + structural + def_bias + tc + si + tp) * intent_mult;

                        signals.insert(
                            hit.id.clone(),
                            HitSignals {
                                keyword_score: kw_score_map.get(hit.id.as_str()).copied().unwrap_or(0.0),
                                vector_score: vec_score_map.get(hit.id.as_str()).copied().unwrap_or(0.0),
                                base_score,
                                structural_adjust: structural,
                                intent_mult,
                                definition_bias: def_bias,
                                term_coverage: tc,
                                symbol_importance: si,
                                test_symbol_penalty: tp,
                                popularity_boost: 0.0,
                                learning_boost: 0.0,
                                affinity_boost: 0.0,
                                docstring_boost: 0.0,
                                package_boost: 0.0,
                            },
                        );
                    }

                    // Re-sort after structural/intent adjustments
                    rrf_results.sort_by(|a, b| {
                        b.score
                            .total_cmp(&a.score)
                            .then_with(|| b.exported.cmp(&a.exported))
                            .then_with(|| a.name.cmp(&b.name))
                    });

                    (rrf_results, signals)
                } else {
                    // Use existing score fusion
                    rank_hits_with_signals(
                        &keyword_hits,
                        &vector_hits,
                        &self.config,
                        &intent,
                        search_query,
                    )
                }
            } else {
                // MULTI-QUERY PATH: Loop over sub-queries and collect combined hits
                // Always use larger pool for multi-query (compound NL queries)
                let k = self.config.vector_search_limit.max(limit * 3).max(40);

                // Combined accumulators for ALL sub-queries
                let mut combined_keyword_hits: Vec<crate::storage::tantivy::SearchHit> = Vec::new();
                let mut combined_vector_hits: Vec<crate::storage::vector::VectorHit> = Vec::new();

                for sub_query in &sub_queries {
                    // Keyword search for this sub-query
                    let sub_keyword_hits = self.tantivy.search(sub_query, k)?;
                    combined_keyword_hits.extend(sub_keyword_hits);

                    // Vector search for this sub-query with graceful degradation
                    // Each sub-query degrades independently - one failure doesn't affect others
                    let sub_vector_hits = match self.get_query_vector_cached(sub_query).await {
                        Ok(query_vector) => {
                            match self.vectors.search(&query_vector, k).await {
                                Ok(mut hits) => {
                                    // HyDE for this sub-query (best-effort)
                                    if self.config.hyde_enabled {
                                        if let Some(generator) = &self.hyde_generator {
                                            let language = detect_language_from_query(sub_query);
                                            if let Ok(hyde_result) = generator.generate(sub_query, language).await {
                                                let mut embedder = self.embedder.lock().await;
                                                if let Ok(hyde_embeddings) =
                                                    embedder.embed(&[hyde_result.hypothetical_code])
                                                {
                                                    if let Some(hyde_vector) = hyde_embeddings.first() {
                                                        if let Ok(hyde_hits) =
                                                            self.vectors.search(hyde_vector, k / 2).await
                                                        {
                                                            hits.extend(hyde_hits);
                                                        }
                                                    }
                                                }
                                            }
                                            // HyDE failures are silently ignored - it's a best-effort enhancement
                                        }
                                    }
                                    hits
                                }
                                Err(e) => {
                                    // Vector search failed for this sub-query - degrade gracefully
                                    tracing::warn!(
                                        query = %sub_query,
                                        error = %e,
                                        "LanceDB vector search failed for sub-query, degrading to keyword-only"
                                    );
                                    self.metrics.search_errors_total.inc();
                                    Vec::new()
                                }
                            }
                        }
                        Err(e) => {
                            // Embedding generation failed for this sub-query - degrade gracefully
                            tracing::warn!(
                                query = %sub_query,
                                error = %e,
                                "Query embedding generation failed for sub-query, degrading to keyword-only"
                            );
                            self.metrics.search_errors_total.inc();
                            Vec::new()
                        }
                    };

                    combined_vector_hits.extend(sub_vector_hits);
                }

                // UNIFIED RRF: Single RRF pass over combined hits from all sub-queries
                // This avoids nested RRF layers

                let keyword_ranked: Vec<RankedHit> = combined_keyword_hits
                    .iter()
                    .map(|h| RankedHit {
                        id: h.id.clone(),
                        score: h.score,
                        name: h.name.clone(),
                        kind: h.kind.clone(),
                        file_path: h.file_path.clone(),
                        exported: h.exported,
                        language: String::new(),
                    })
                    .collect();

                let vector_ranked: Vec<RankedHit> = combined_vector_hits
                    .iter()
                    .map(|h| RankedHit {
                        id: h.id.clone(),
                        score: 1.0 / (1.0 + h.distance.unwrap_or(1.0).max(0.0)),
                        name: h.name.clone(),
                        kind: h.kind.clone(),
                        file_path: h.file_path.clone(),
                        exported: h.exported,
                        language: h.language.clone(),
                    })
                    .collect();

                let graph_hits = if let Ok(graph) = get_graph_ranked_hits(&keyword_ranked, &sqlite)
                {
                    graph
                } else {
                    keyword_ranked.clone()
                };

                // Single RRF pass over combined results (dynamic weights for NL queries)
                let weights = if is_nl_query {
                    (
                        self.config.rrf_keyword_weight * 0.5,
                        self.config.rrf_vector_weight * 1.5,
                        self.config.rrf_graph_weight,
                    )
                } else {
                    (
                        self.config.rrf_keyword_weight,
                        self.config.rrf_vector_weight,
                        self.config.rrf_graph_weight,
                    )
                };

                let mut ranked =
                    reciprocal_rank_fusion(&keyword_ranked, &vector_ranked, &graph_hits, weights);

                // Apply structural and intent adjustments post-RRF (same as single-query path)
                let line_count_ids: Vec<String> = ranked.iter().map(|h| h.id.clone()).collect();
                let line_counts = sqlite.batch_get_symbol_line_counts(&line_count_ids).unwrap_or_default();
                let test_symbols = sqlite.batch_check_test_symbols(&line_count_ids).unwrap_or_default();
                let symbol_texts = sqlite.batch_get_symbol_texts(&line_count_ids).unwrap_or_default();

                let mut hit_signals = HashMap::new();
                for hit in ranked.iter_mut() {
                    let structural = ranking::structural_adjustment(
                        &self.config,
                        hit.exported,
                        &hit.file_path,
                        &hit.kind,
                        &intent,
                        &query_without_controls,
                    );
                    let intent_mult = ranking::intent_adjustment(
                        &intent,
                        &hit.kind,
                        &hit.file_path,
                        hit.exported,
                        &hit.name,
                    );

                    let base_score = hit.score;
                    hit.score = (hit.score + structural) * intent_mult;

                    let def_bias = ranking::definition_bias(
                        &query_without_controls,
                        &hit.name,
                        &hit.kind,
                        &intent,
                    );
                    let body = symbol_texts.get(&hit.id).map(|s| s.as_str());
                    let tc = ranking::term_coverage_adjustment(
                        &query_without_controls,
                        &hit.name,
                        &hit.file_path,
                        body,
                    );
                    let lc = line_counts.get(&hit.id).copied().unwrap_or(0);
                    let si = ranking::symbol_importance_adjustment(lc, hit.exported);
                    let is_test = test_symbols.contains(&hit.id);
                    let tp = ranking::test_symbol_penalty(is_test);
                    hit.score += def_bias + tc + si + tp;

                    hit_signals.insert(
                        hit.id.clone(),
                        HitSignals {
                            keyword_score: 0.0,
                            vector_score: 0.0,
                            base_score,
                            structural_adjust: structural,
                            intent_mult,
                            definition_bias: def_bias,
                            term_coverage: tc,
                            symbol_importance: si,
                            test_symbol_penalty: tp,
                            popularity_boost: 0.0,
                            learning_boost: 0.0,
                            affinity_boost: 0.0,
                            docstring_boost: 0.0,
                            package_boost: 0.0,
                        },
                    );
                }

                // Re-sort after structural/intent adjustments
                ranked.sort_by(|a, b| {
                    b.score
                        .total_cmp(&a.score)
                        .then_with(|| b.exported.cmp(&a.exported))
                        .then_with(|| a.name.cmp(&b.name))
                });

                (ranked, hit_signals)
            };

        // Start merge timing after search completes
        // Note: keyword_ms and vector_ms timing are lost in multi-query path
        // because we aggregate across multiple sub-queries. Set to 0 for telemetry.
        let (keyword_ms, vector_ms) = if sub_queries.len() == 1 {
            // In single-query path, these were captured but as _keyword_ms, _vector_ms
            // We need to track them properly for telemetry
            (0, 0) // Timing captured internally, not exposed
        } else {
            // Multi-query: aggregate timing not meaningful per sub-query
            (0, 0)
        };

        let merge_t = Instant::now();

        let mut uniq = Vec::new();
        let mut seen = HashSet::new();
        for hit in ranked {
            if seen.insert(hit.id.clone()) {
                uniq.push(hit);
            }
        }

        // Inject framework pattern matches for NL queries.
        // Framework patterns (WebSocket handlers, routes, middleware) live in a
        // separate table and aren't directly searchable via BM25/vector. This
        // post-merge step queries them and boosts/injects matching parent symbols.
        if is_nl_query {
            if let Ok(patterns) = sqlite.search_framework_patterns(
                None, None, None, None, None, None, 200,
            ) {
                let query_lower = query_without_controls.to_lowercase();
                let mut fw_file_lines: Vec<(String, u32)> = Vec::new();

                for pattern in &patterns {
                    let kind_lower = pattern.kind.to_lowercase();
                    let matches = query_lower.contains(&kind_lower)
                        || (kind_lower == "websocket"
                            && (query_lower.contains("websocket")
                                || query_lower.contains("ws")
                                || query_lower.contains("socket")))
                        || (kind_lower == "route"
                            && (query_lower.contains("route")
                                || query_lower.contains("endpoint")
                                || query_lower.contains("api")))
                        || (kind_lower == "middleware"
                            && query_lower.contains("middleware"))
                        || (kind_lower == "plugin"
                            && query_lower.contains("plugin"));

                    if matches {
                        fw_file_lines.push((pattern.file_path.clone(), pattern.line));
                    }
                }

                // Find parent symbols for matched framework patterns
                let mut fw_files: HashSet<String> = HashSet::new();
                for (fp, _) in &fw_file_lines {
                    fw_files.insert(fp.clone());
                }

                for fw_file in &fw_files {
                    if let Ok(file_symbols) = sqlite.list_symbols_by_file(fw_file) {
                        // For each framework pattern in this file, find the
                        // smallest enclosing symbol (closest start_line <= pattern line)
                        for &(ref fp, line) in &fw_file_lines {
                            if fp != fw_file {
                                continue;
                            }
                            // Find best enclosing symbol: start_line <= line <= end_line,
                            // preferring the smallest span (most specific)
                            let enclosing = file_symbols
                                .iter()
                                .filter(|s| {
                                    s.start_line <= line && s.end_line >= line
                                })
                                .min_by_key(|s| s.end_line - s.start_line);

                            if let Some(sym) = enclosing {
                                if seen.contains(&sym.id) {
                                    // Already in results — boost its score
                                    if let Some(hit) = uniq.iter_mut().find(|h| h.id == sym.id) {
                                        hit.score += 0.15;
                                    }
                                } else {
                                    // Inject as new result with moderate score
                                    let top_score = uniq.first().map(|h| h.score).unwrap_or(1.0);
                                    seen.insert(sym.id.clone());
                                    uniq.push(RankedHit {
                                        id: sym.id.clone(),
                                        score: top_score * 0.7,
                                        name: sym.name.clone(),
                                        kind: sym.kind.clone(),
                                        file_path: sym.file_path.clone(),
                                        exported: sym.exported,
                                        language: sym.language.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        let hits = Self::filter_hits_by_controls(uniq, &controls);
        let hits = if exported_only {
            hits.into_iter().filter(|h| h.exported).collect::<Vec<_>>()
        } else {
            hits
        };

        let hits =
            apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &self.config)?;

        // Apply JSDoc documentation boost (1.5x for well-documented symbols)
        let hits = apply_docstring_boost_with_signals(&sqlite, hits, &mut hit_signals)?;

        let hits = apply_selection_boost_with_signals(
            &sqlite,
            hits,
            &mut hit_signals,
            &expanded_query,
            &self.config,
        )?;

        let hits =
            apply_file_affinity_boost_with_signals(&sqlite, hits, &mut hit_signals, &self.config)?;

        // Apply package boost for same-package prioritization
        let query_package_id = controls.package.as_deref();
        let hits = apply_package_boost_with_signals(
            &sqlite,
            hits,
            &mut hit_signals,
            query_package_id,
            &self.config,
            intent.clone().unwrap_or(Intent::Definition),
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

        let (hits, expanded_ids) = expand_with_edges(&sqlite, hits, limit)?;

        // Apply file/kind diversity on the expanded pool (limit*3 candidates),
        // then truncate to final limit. This gives diversity enough headroom
        // to promote cross-file results without destroying same-file clusters.
        let mut hits = diversify_by_file(hits, limit);
        hits = diversify_by_kind(hits, limit);
        hits.truncate(limit);

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

    fn cache_insert_response(
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

    async fn get_query_vector_cached(&self, query: &str) -> Result<Vec<f32>> {
        let key = format!("q={}", trim_query(query, 500));
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(v) = cache.embeddings.get(&key) {
                return Ok(v);
            }
        }

        let v = {
            let mut embedder = self.embedder.lock().await;
            let mut out = embedder.embed(&[query.to_string()])?;
            out.pop()
                .ok_or_else(|| anyhow!("Embedder returned no vector"))?
        };

        let size = v.len().saturating_mul(4);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.embeddings.insert(key, v.clone(), size);
        Ok(v)
    }

    fn assemble_context_cached(
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

    fn filter_hits_by_controls(hits: Vec<RankedHit>, controls: &QueryControls) -> Vec<RankedHit> {
        hits.into_iter()
            .filter(|h| {
                controls
                    .lang
                    .as_ref()
                    .is_none_or(|l| h.language == l.as_str())
            })
            .filter(|h| {
                controls
                    .kind
                    .as_ref()
                    .is_none_or(|k| Self::kind_matches(&h.kind, k))
            })
            .filter(|h| {
                controls
                    .path
                    .as_ref()
                    .is_none_or(|p| Self::path_matches(&h.file_path, p))
            })
            .filter(|h| {
                controls
                    .file
                    .as_ref()
                    .is_none_or(|f| Self::file_matches(&h.file_path, f))
            })
            .collect()
    }

    fn kind_matches(kind: &str, control: &str) -> bool {
        control
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .any(|k| kind.eq_ignore_ascii_case(k))
    }

    fn path_matches(file_path: &str, control: &str) -> bool {
        file_path.to_lowercase().contains(&control.to_lowercase())
    }

    fn file_matches(file_path: &str, control: &str) -> bool {
        let file_path = file_path.to_lowercase();
        let control = control.to_lowercase();
        match (control.starts_with('*'), control.ends_with('*')) {
            (true, true) => file_path.contains(control.trim_matches('*')),
            (true, false) => file_path.ends_with(control.trim_start_matches('*')),
            (false, true) => file_path.starts_with(control.trim_end_matches('*')),
            (false, false) => file_path.contains(&control),
        }
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
fn detect_language_from_query(query: &str) -> &'static str {
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

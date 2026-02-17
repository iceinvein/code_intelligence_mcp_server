//! Hybrid search execution: single-query and multi-query paths with RRF fusion.

use super::ranking::{self, get_graph_ranked_hits, rank_hits_with_signals, reciprocal_rank_fusion};
use super::query::{contains_code_snippet, Intent};
use super::{detect_language_from_query, HitSignals, RankedHit, Retriever};
use crate::storage::sqlite::SqliteStore;
use anyhow::Result;
use std::collections::HashMap;
use std::time::Instant;

/// Aggregated result from the hybrid search execution phase.
pub(super) struct HybridSearchResult {
    pub ranked: Vec<RankedHit>,
    pub hit_signals: HashMap<String, HitSignals>,
    pub vector_ranked_for_promotion: Vec<RankedHit>,
    pub keyword_ms: u64,
    pub vector_ms: u64,
}

/// Execute hybrid search across keyword (Tantivy) and vector (LanceDB) backends.
///
/// Dispatches to single-query or multi-query path based on `sub_queries` length.
/// Returns ranked hits with scoring signals and the raw vector results for
/// optional post-diversity promotion.
pub(super) async fn execute_hybrid_search(
    retriever: &Retriever,
    sqlite: &SqliteStore,
    query_without_controls: &str,
    sub_queries: &[String],
    intent: &Option<Intent>,
    is_nl_query: bool,
    limit: usize,
) -> Result<HybridSearchResult> {
    if sub_queries.len() == 1 {
        execute_single_query_search(
            retriever,
            sqlite,
            query_without_controls,
            &sub_queries[0],
            intent,
            is_nl_query,
            limit,
        )
        .await
    } else {
        execute_multi_query_search(
            retriever,
            sqlite,
            query_without_controls,
            sub_queries,
            intent,
            is_nl_query,
            limit,
        )
        .await
    }
}

/// Single-query search path: BM25 + vector + graph → RRF → structural scoring.
async fn execute_single_query_search(
    retriever: &Retriever,
    sqlite: &SqliteStore,
    query_without_controls: &str,
    search_query: &str,
    intent: &Option<Intent>,
    is_nl_query: bool,
    limit: usize,
) -> Result<HybridSearchResult> {
    let k = if contains_code_snippet(search_query) {
        retriever.config.vector_search_limit.max(limit).max(5)
    } else {
        retriever
            .config
            .vector_search_limit
            .max(limit * 3)
            .max(40)
    };

    let keyword_t = Instant::now();
    let keyword_hits = retriever.tantivy.search(search_query, k)?;
    let keyword_ms = keyword_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

    let vector_t = Instant::now();

    // Vector search with graceful degradation
    // Use raw query (pre-expansion) for vector search — synonym expansion
    // helps BM25 (more tokens) but hurts embeddings (noisy average of concepts).
    // The embedding model already captures synonyms through its training.
    let vector_query = query_without_controls;
    let (vector_hits, _vector_degraded) =
        match retriever.get_query_vector_cached(vector_query).await {
            Ok(query_vector) => match retriever.vectors.search(&query_vector, k).await {
                Ok(mut hits) => {
                    // HyDE: Add hypothetical document retrieval (best-effort)
                    if retriever.config.hyde_enabled {
                        if let Some(generator) = &retriever.hyde_generator {
                            let language = detect_language_from_query(search_query);
                            if let Ok(hyde_result) =
                                generator.generate(search_query, language).await
                            {
                                let mut embedder = retriever.embedder.lock().await;
                                if let Ok(hyde_embeddings) =
                                    embedder.embed(&[hyde_result.hypothetical_code])
                                {
                                    if let Some(hyde_vector) = hyde_embeddings.first() {
                                        if let Ok(mut hyde_hits) =
                                            retriever.vectors.search(hyde_vector, k / 2).await
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
                    retriever.metrics.search_errors_total.inc();
                    (Vec::new(), true)
                }
            },
            Err(e) => {
                // Embedding generation failed - degrade gracefully
                tracing::warn!(
                    query = %search_query,
                    error = %e,
                    "Query embedding generation failed, degrading to keyword-only search"
                );
                retriever.metrics.search_errors_total.inc();
                (Vec::new(), true)
            }
        };

    let vector_ms = vector_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

    // Use RRF if enabled, otherwise use existing score fusion
    if retriever.config.rrf_enabled {
        let keyword_ranked: Vec<RankedHit> = keyword_hits
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

        let vector_ranked: Vec<RankedHit> = vector_hits
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

        let graph_hits = if let Ok(graph) = get_graph_ranked_hits(&keyword_ranked, sqlite) {
            graph
        } else {
            keyword_ranked.clone()
        };

        // Apply RRF with dynamic weights based on query type.
        // NL queries get higher vector weight because BM25 often
        // matches irrelevant identifiers for conceptual queries.
        let weights = rrf_weights(&retriever.config, is_nl_query);

        let mut rrf_results =
            reciprocal_rank_fusion(&keyword_ranked, &vector_ranked, &graph_hits, weights);

        normalize_rrf_scores(&mut rrf_results);

        // Build lookup maps for original keyword/vector scores (for diagnostics)
        let kw_score_map: HashMap<&str, f32> = keyword_ranked
            .iter()
            .map(|h| (h.id.as_str(), h.score))
            .collect();
        let vec_score_map: HashMap<&str, f32> = vector_ranked
            .iter()
            .map(|h| (h.id.as_str(), h.score))
            .collect();

        let signals = apply_structural_scoring(
            &retriever.config,
            sqlite,
            &mut rrf_results,
            intent,
            query_without_controls,
            false,
            Some((&kw_score_map, &vec_score_map)),
        );

        rrf_results.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| b.exported.cmp(&a.exported))
                .then_with(|| a.name.cmp(&b.name))
        });

        Ok(HybridSearchResult {
            ranked: rrf_results,
            hit_signals: signals,
            vector_ranked_for_promotion: vector_ranked,
            keyword_ms,
            vector_ms,
        })
    } else {
        // Use existing score fusion (no vector promotion in non-RRF path)
        let (ranked, signals) = rank_hits_with_signals(
            &keyword_hits,
            &vector_hits,
            &retriever.config,
            intent,
            search_query,
        );
        Ok(HybridSearchResult {
            ranked,
            hit_signals: signals,
            vector_ranked_for_promotion: Vec::new(),
            keyword_ms,
            vector_ms,
        })
    }
}

/// Multi-query search path: loop over sub-queries, combine hits, single RRF pass.
async fn execute_multi_query_search(
    retriever: &Retriever,
    sqlite: &SqliteStore,
    query_without_controls: &str,
    sub_queries: &[String],
    intent: &Option<Intent>,
    is_nl_query: bool,
    limit: usize,
) -> Result<HybridSearchResult> {
    // Always use larger pool for multi-query (compound NL queries)
    let base_k = retriever
        .config
        .vector_search_limit
        .max(limit * 3)
        .max(40);
    // Cross-cutting intent queries (Error) need larger pools because
    // LLM descriptions dilute IDF for common terms like "error",
    // pushing specialized symbols (PathError) below normal k threshold.
    let k = if matches!(intent, Some(Intent::Error)) {
        base_k.max(500)
    } else {
        base_k
    };

    let mut combined_keyword_hits: Vec<crate::storage::tantivy::SearchHit> = Vec::new();
    let mut combined_vector_hits: Vec<crate::storage::vector::VectorHit> = Vec::new();

    // Vector search uses raw query (pre-expansion) — one embedding for
    // full user intent. BM25 still loops over expanded sub-queries.
    let vector_query = query_without_controls;
    let multi_vector_hits = match retriever.get_query_vector_cached(vector_query).await {
        Ok(query_vector) => match retriever.vectors.search(&query_vector, k).await {
            Ok(hits) => hits,
            Err(e) => {
                tracing::warn!(
                    query = %vector_query,
                    error = %e,
                    "LanceDB vector search failed for multi-query, degrading to keyword-only"
                );
                retriever.metrics.search_errors_total.inc();
                Vec::new()
            }
        },
        Err(e) => {
            tracing::warn!(
                query = %vector_query,
                error = %e,
                "Query embedding failed for multi-query, degrading to keyword-only"
            );
            retriever.metrics.search_errors_total.inc();
            Vec::new()
        }
    };
    combined_vector_hits.extend(multi_vector_hits);

    for sub_query in sub_queries {
        let sub_keyword_hits = retriever.tantivy.search(sub_query, k)?;
        combined_keyword_hits.extend(sub_keyword_hits);

        // HyDE per sub-query (best-effort)
        if retriever.config.hyde_enabled {
            if let Some(generator) = &retriever.hyde_generator {
                let language = detect_language_from_query(sub_query);
                if let Ok(hyde_result) = generator.generate(sub_query, language).await {
                    let mut embedder = retriever.embedder.lock().await;
                    if let Ok(hyde_embeddings) =
                        embedder.embed(&[hyde_result.hypothetical_code])
                    {
                        if let Some(hyde_vector) = hyde_embeddings.first() {
                            if let Ok(hyde_hits) =
                                retriever.vectors.search(hyde_vector, k / 2).await
                            {
                                combined_vector_hits.extend(hyde_hits);
                            }
                        }
                    }
                }
            }
        }
    }

    // UNIFIED RRF: Single RRF pass over combined hits from all sub-queries
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

    let graph_hits = if let Ok(graph) = get_graph_ranked_hits(&keyword_ranked, sqlite) {
        graph
    } else {
        keyword_ranked.clone()
    };

    let weights = rrf_weights(&retriever.config, is_nl_query);

    let mut ranked =
        reciprocal_rank_fusion(&keyword_ranked, &vector_ranked, &graph_hits, weights);

    normalize_rrf_scores(&mut ranked);

    let signals = apply_structural_scoring(
        &retriever.config,
        sqlite,
        &mut ranked,
        intent,
        query_without_controls,
        true,
        None,
    );

    ranked.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(HybridSearchResult {
        ranked,
        hit_signals: signals,
        vector_ranked_for_promotion: vector_ranked,
        keyword_ms: 0,
        vector_ms: 0,
    })
}

/// Compute RRF weights, boosting vector weight for natural language queries.
fn rrf_weights(config: &crate::config::Config, is_nl_query: bool) -> (f32, f32, f32) {
    if is_nl_query {
        (
            config.rrf_keyword_weight * 0.5,
            config.rrf_vector_weight * 1.5,
            config.rrf_graph_weight,
        )
    } else {
        (
            config.rrf_keyword_weight,
            config.rrf_vector_weight,
            config.rrf_graph_weight,
        )
    }
}

/// Normalize RRF scores to 0-10 range so search signal is competitive
/// with post-RRF additive adjustments (structural, term_coverage, etc.)
fn normalize_rrf_scores(results: &mut [RankedHit]) {
    let rrf_max = results
        .iter()
        .map(|h| h.score)
        .fold(f32::NEG_INFINITY, f32::max);
    let rrf_min = results
        .iter()
        .map(|h| h.score)
        .fold(f32::INFINITY, f32::min);
    let rrf_range = rrf_max - rrf_min;
    if rrf_range > 0.0 {
        let target_range = 10.0;
        for hit in results.iter_mut() {
            hit.score = ((hit.score - rrf_min) / rrf_range) * target_range;
        }
    } else if rrf_max > 0.0 {
        for hit in results.iter_mut() {
            hit.score = 5.0;
        }
    }
}

/// Apply structural and intent adjustments post-RRF.
///
/// `multi_query`: controls how intent_mult is applied. In single-query mode,
/// intent_mult multiplies the entire sum `(base + structural + def_bias + tc + si + tp)`.
/// In multi-query mode, intent_mult only multiplies `(base + structural)`, then
/// the other adjustments are added after.
///
/// When `kw_vec_scores` is provided (single-query path), records original keyword/vector
/// scores in signals. When `None` (multi-query path), records 0.0 for both.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn apply_structural_scoring(
    config: &crate::config::Config,
    sqlite: &SqliteStore,
    hits: &mut [RankedHit],
    intent: &Option<Intent>,
    query_without_controls: &str,
    multi_query: bool,
    kw_vec_scores: Option<(&HashMap<&str, f32>, &HashMap<&str, f32>)>,
) -> HashMap<String, HitSignals> {
    let ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();
    let line_counts = sqlite
        .batch_get_symbol_line_counts(&ids)
        .unwrap_or_default();
    let test_symbols = sqlite.batch_check_test_symbols(&ids).unwrap_or_default();
    let symbol_texts = sqlite.batch_get_symbol_texts(&ids).unwrap_or_default();

    let mut signals = HashMap::new();
    for hit in hits.iter_mut() {
        let structural = ranking::structural_adjustment(
            config,
            hit.exported,
            &hit.file_path,
            &hit.kind,
            intent,
            query_without_controls,
        );
        let intent_mult = ranking::intent_adjustment(
            intent,
            &hit.kind,
            &hit.file_path,
            hit.exported,
            &hit.name,
        );

        let base_score = hit.score;
        let def_bias = ranking::definition_bias(
            query_without_controls,
            &hit.name,
            &hit.kind,
            intent,
        );
        let body = symbol_texts.get(&hit.id).map(|s| s.as_str());
        let tc = ranking::term_coverage_adjustment(
            query_without_controls,
            &hit.name,
            &hit.file_path,
            body,
        );
        let lc = line_counts.get(&hit.id).copied().unwrap_or(0);
        let si = ranking::symbol_importance_adjustment(lc, hit.exported);
        let is_test = test_symbols.contains(&hit.id);
        let tp = ranking::test_symbol_penalty(is_test);

        if multi_query {
            // Multi-query: intent_mult only on base+structural, rest additive
            hit.score = (hit.score + structural) * intent_mult + def_bias + tc + si + tp;
        } else {
            // Single-query: intent_mult on entire sum
            hit.score = (hit.score + structural + def_bias + tc + si + tp) * intent_mult;
        }

        let (kw_score, vec_score) = if let Some((kw_map, vec_map)) = kw_vec_scores {
            (
                kw_map.get(hit.id.as_str()).copied().unwrap_or(0.0),
                vec_map.get(hit.id.as_str()).copied().unwrap_or(0.0),
            )
        } else {
            (0.0, 0.0)
        };

        signals.insert(
            hit.id.clone(),
            HitSignals {
                keyword_score: kw_score,
                vector_score: vec_score,
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

    signals
}

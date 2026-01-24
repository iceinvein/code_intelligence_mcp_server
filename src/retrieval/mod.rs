//! Retrieval module for code intelligence search

pub mod assembler;
mod cache;
pub mod hyde;
mod query;
mod ranking;

use crate::{
    config::Config,
    embeddings::Embedder,
    retrieval::assembler::{ContextAssembler, ContextItem},
    reranker::Reranker,
    storage::{
        sqlite::{SqliteStore, SymbolRow},
        tantivy::TantivyIndex,
        vector::LanceVectorTable,
    },
};
use anyhow::{anyhow, Result};
use cache::RetrieverCaches;
use query::{detect_intent, normalize_query, parse_query_controls, trim_query, Intent, QueryControls};
use ranking::{
    apply_file_affinity_boost_with_signals, apply_popularity_boost_with_signals, apply_selection_boost_with_signals, diversify_by_cluster, diversify_by_kind, expand_with_edges,
    rank_hits_with_signals, apply_reranker_scores, prepare_rerank_docs, should_rerank,
    reciprocal_rank_fusion, get_graph_ranked_hits,
};
use crate::retrieval::hyde::HypotheticalCodeGenerator;
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
    pub exported: bool,
    pub language: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub limit: usize,
    pub hits: Vec<RankedHit>,
    pub context: String,
    pub context_items: Vec<ContextItem>,
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
    pub popularity_boost: f32,
    pub learning_boost: f32,
    pub affinity_boost: f32,
    pub docstring_boost: f32,
}

#[derive(Clone)]
pub struct Retriever {
    config: Arc<Config>,
    db_path: std::path::PathBuf,
    tantivy: Arc<TantivyIndex>,
    vectors: Arc<LanceVectorTable>,
    embedder: Arc<AsyncMutex<Box<dyn Embedder + Send>>>,
    reranker: Option<Arc<dyn Reranker>>,
    hyde_generator: Option<HypotheticalCodeGenerator>,
    cache: Arc<Mutex<RetrieverCaches>>,
    cache_config_key: String,
}

impl Retriever {
    pub fn new(
        config: Arc<Config>,
        tantivy: Arc<TantivyIndex>,
        vectors: Arc<LanceVectorTable>,
        embedder: Arc<AsyncMutex<Box<dyn Embedder + Send>>>,
        reranker: Option<Arc<dyn Reranker>>,
        hyde_generator: Option<HypotheticalCodeGenerator>,
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
        }
    }

    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        exported_only: bool,
    ) -> Result<SearchResponse> {
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
                return Ok(resp);
            }
        }

        let (query_without_controls, controls) = parse_query_controls(query);

        if let Some(id) = &controls.id {
            if let Some(row) = sqlite.get_symbol_by_id(id)? {
                if exported_only && !row.exported {
                    return Ok(SearchResponse {
                        query: query.to_string(),
                        limit,
                        hits: vec![],
                        context: String::new(),
                        context_items: vec![],
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

                let (context, context_items) =
                    self.assemble_context_cached(&sqlite, std::slice::from_ref(&row), &[])?;

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

                let mut hit_signals = HashMap::new();
                hit_signals.insert(
                    hits[0].id.clone(),
                    HitSignals {
                        keyword_score: 0.0,
                        vector_score: 0.0,
                        base_score: 0.0,
                        structural_adjust: 0.0,
                        intent_mult: 1.0,
                        definition_bias: 0.0,
                        popularity_boost: 0.0,
                        learning_boost: 0.0,
                        affinity_boost: 0.0,
                        docstring_boost: 0.0,
                    },
                );

                let resp = SearchResponse {
                    query: query.to_string(),
                    limit,
                    hits,
                    context,
                    context_items,
                    hit_signals,
                };
                self.cache_insert_response(cache_key, resp.clone());
                return Ok(resp);
            }
        }

        // Intent Detection
        let intent = detect_intent(&query_without_controls);

        // Normalize and expand query
        let expanded_query = normalize_query(&query_without_controls);
        let search_query = &expanded_query;

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

                    let (context, context_items) =
                        self.assemble_context_cached(&sqlite, &rows, &[])?;

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
                        context_items,
                        hit_signals: HashMap::new(),
                    };
                    self.cache_insert_response(cache_key, resp.clone());
                    return Ok(resp);
                }
            }
        }

        let k = self.config.vector_search_limit.max(limit).max(5);
        let keyword_t = Instant::now();
        let keyword_hits = self.tantivy.search(search_query, k)?;
        let keyword_ms = keyword_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

        let vector_t = Instant::now();
        let query_vector = self.get_query_vector_cached(search_query).await?;
        let mut vector_hits = self.vectors.search(&query_vector, k).await?;

        // HyDE: Add hypothetical document retrieval
        if self.config.hyde_enabled {
            if let Some(generator) = &self.hyde_generator {
                // Detect language from query or default to TypeScript
                let language = detect_language_from_query(search_query);

                if let Ok(hyde_result) = generator.generate(search_query, language).await {
                    // Embed hypothetical code and search
                    let mut embedder = self.embedder.lock().await;
                    if let Ok(hyde_embeddings) = embedder.embed(&[hyde_result.hypothetical_code]) {
                        if let Some(hyde_vector) = hyde_embeddings.first() {
                            if let Ok(mut hyde_hits) = self.vectors.search(hyde_vector, k / 2).await {
                                // Combine HyDE hits with direct vector hits
                                vector_hits.append(&mut hyde_hits);
                            }
                        }
                    }
                }
            }
        }

        let vector_ms = vector_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

        let merge_t = Instant::now();

        // Use RRF if enabled, otherwise use existing score fusion
        let (ranked, mut hit_signals) = if self.config.rrf_enabled {
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
            let graph_hits = if let Ok(graph) = get_graph_ranked_hits(&keyword_ranked, &sqlite) {
                graph
            } else {
                keyword_ranked.clone()
            };

            // Apply RRF
            let weights = (
                self.config.rrf_keyword_weight,
                self.config.rrf_vector_weight,
                self.config.rrf_graph_weight,
            );

            let rrf_results = reciprocal_rank_fusion(
                &keyword_ranked,
                &vector_ranked,
                &graph_hits,
                weights,
            );

            // Generate signals for RRF results
            let mut signals = HashMap::new();
            for hit in &rrf_results {
                signals.insert(
                    hit.id.clone(),
                    HitSignals {
                        keyword_score: 0.0,  // RRF doesn't preserve raw scores
                        vector_score: 0.0,
                        base_score: hit.score,
                        structural_adjust: 0.0,
                        intent_mult: 1.0,
                        definition_bias: 0.0,
                        popularity_boost: 0.0,
                        learning_boost: 0.0,
                        affinity_boost: 0.0,
                        docstring_boost: 0.0,
                    },
                );
            }

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
        };

        let mut uniq = Vec::new();
        let mut seen = HashSet::new();
        for hit in ranked {
            if seen.insert(hit.id.clone()) {
                uniq.push(hit);
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

        let hits = apply_selection_boost_with_signals(
            &sqlite,
            hits,
            &mut hit_signals,
            &expanded_query,
            &self.config,
        )?;

        let hits = apply_file_affinity_boost_with_signals(
            &sqlite,
            hits,
            &mut hit_signals,
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
                if let Ok(rerank_scores) = reranker.rerank(search_query, &docs).await {
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

        hits = diversify_by_cluster(&sqlite, hits, limit);
        hits = diversify_by_kind(hits, limit);
        hits.truncate(limit);

        let (hits, expanded_ids) = expand_with_edges(&sqlite, hits, limit)?;

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

        let (context, context_items) = self.assemble_context_cached(&sqlite, &roots, &extra)?;

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

        let resp = SearchResponse {
            query: query.to_string(),
            limit,
            hits,
            context,
            context_items,
            hit_signals,
        };
        self.cache_insert_response(cache_key, resp.clone());
        Ok(resp)
    }

    fn cache_insert_response(&self, key: String, resp: SearchResponse) {
        let size = resp.context.len() + resp.context_items.iter().map(|i| i.tokens * 4).sum::<usize>();
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
    ) -> Result<(String, Vec<ContextItem>)> {
        let mut root_ids = roots.iter().map(|r| r.id.as_str()).collect::<Vec<_>>();
        root_ids.sort_unstable();
        let mut extra_ids = extra.iter().map(|r| r.id.as_str()).collect::<Vec<_>>();
        extra_ids.sort_unstable();

        let key = format!(
            "m=default|t={}|r={}|x={}",
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
        let v = assembler.assemble_context_with_items(store, roots, extra)?;
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
        Ok(assembler.format_context(&sqlite, symbols, &[], &[])?.0)
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

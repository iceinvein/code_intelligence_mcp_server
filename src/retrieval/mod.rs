pub mod assembler;

use crate::{
    config::Config,
    embeddings::Embedder,
    retrieval::assembler::{ContextAssembler, ContextItem},
    storage::{
        sqlite::{SqliteStore, SymbolRow},
        tantivy::{SearchHit as KeywordHit, TantivyIndex},
        vector::{LanceVectorTable, VectorHit},
    },
    text,
};
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet, VecDeque},
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
}

#[derive(Debug, Clone, Default)]
struct QueryControls {
    id: Option<String>,
    file: Option<String>,
    path: Option<String>,
    lang: Option<String>,
    kind: Option<String>,
}

#[derive(Debug, Clone)]
struct LruCache<V> {
    max_entries: usize,
    max_bytes: Option<usize>,
    used_bytes: usize,
    order: VecDeque<String>,
    entries: HashMap<String, (V, usize)>,
}

impl<V: Clone> LruCache<V> {
    fn get(&mut self, key: &str) -> Option<V> {
        let (v, _) = self.entries.get(key).cloned()?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_string());
        Some(v)
    }

    fn insert(&mut self, key: String, value: V, size_bytes: usize) {
        if self.entries.contains_key(&key) {
            let old = self.entries.insert(key.clone(), (value, size_bytes));
            if let Some((_, old_size)) = old {
                self.used_bytes = self.used_bytes.saturating_sub(old_size);
            }
            self.used_bytes = self.used_bytes.saturating_add(size_bytes);
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
            self.order.push_back(key);
        } else {
            self.entries.insert(key.clone(), (value, size_bytes));
            self.used_bytes = self.used_bytes.saturating_add(size_bytes);
            self.order.push_back(key);
        }

        while self.order.len() > self.max_entries {
            if let Some(oldest) = self.order.pop_front() {
                if let Some((_, sz)) = self.entries.remove(&oldest) {
                    self.used_bytes = self.used_bytes.saturating_sub(sz);
                }
            }
        }

        if let Some(max) = self.max_bytes {
            while self.used_bytes > max {
                if let Some(oldest) = self.order.pop_front() {
                    if let Some((_, sz)) = self.entries.remove(&oldest) {
                        self.used_bytes = self.used_bytes.saturating_sub(sz);
                    }
                } else {
                    break;
                }
            }
        }
    }

    fn clear(&mut self) {
        self.order.clear();
        self.entries.clear();
        self.used_bytes = 0;
    }
}

#[derive(Debug, Clone)]
struct RetrieverCaches {
    last_symbol_update_unix_s: Option<i64>,
    last_index_run_started_at_unix_s: Option<i64>,
    responses: LruCache<SearchResponse>,
    embeddings: LruCache<Vec<f32>>,
    contexts: LruCache<(String, Vec<ContextItem>)>,
}

#[derive(Clone)]
pub struct Retriever {
    config: Arc<Config>,
    db_path: std::path::PathBuf,
    tantivy: Arc<TantivyIndex>,
    vectors: Arc<LanceVectorTable>,
    embedder: Arc<AsyncMutex<Box<dyn Embedder + Send>>>,
    cache: Arc<Mutex<RetrieverCaches>>,
    cache_config_key: String,
}

impl Retriever {
    pub fn new(
        config: Arc<Config>,
        tantivy: Arc<TantivyIndex>,
        vectors: Arc<LanceVectorTable>,
        embedder: Arc<AsyncMutex<Box<dyn Embedder + Send>>>,
    ) -> Self {
        let cache = RetrieverCaches {
            last_symbol_update_unix_s: None,
            last_index_run_started_at_unix_s: None,
            responses: LruCache {
                max_entries: 64,
                max_bytes: None,
                used_bytes: 0,
                order: VecDeque::new(),
                entries: HashMap::new(),
            },
            embeddings: LruCache {
                max_entries: 256,
                max_bytes: Some(4 * 1024 * 1024),
                used_bytes: 0,
                order: VecDeque::new(),
                entries: HashMap::new(),
            },
            contexts: LruCache {
                max_entries: 64,
                max_bytes: Some(8 * 1024 * 1024),
                used_bytes: 0,
                order: VecDeque::new(),
                entries: HashMap::new(),
            },
        };
        let cache_config_key = format!(
            "b={}|k={}|ha={:.3}|vw={:.3}|kw={:.3}|eb={:.3}|ib={:.3}|tp={:.3}|pw={:.3}|pc={}",
            config.max_context_bytes,
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

        let (query_without_controls, controls) = Self::parse_query_controls(query);

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

        // 0. Intent Detection
        let intent = detect_intent(&query_without_controls);

        // Normalize and expand query (Tweak 3: Acronyms & Casing)
        let expanded_query = normalize_query(&query_without_controls);
        // Use expanded query for search, but keep original for logging/response if desired?
        // Actually, we usually want to search with the "better" query.
        let search_query = &expanded_query;

        if let Some(Intent::Callers(name)) = &intent {
            let targets = sqlite.search_symbols_by_exact_name(name, None, 5)?;
            if let Some(target) = targets.first() {
                // Found the symbol, now find who calls/references it
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
                                score: 1.0, // High confidence
                                name: row.name,
                                kind: row.kind,
                                file_path: row.file_path,
                                exported: row.exported,
                                language: row.language,
                            });
                        }
                    }
                }

                // If we found hits via graph, return them directly
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
        let vector_hits = self.vectors.search(&query_vector, k).await?;
        let vector_ms = vector_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

        let merge_t = Instant::now();
        let (ranked, mut hit_signals) = rank_hits_with_signals(
            &keyword_hits,
            &vector_hits,
            &self.config,
            &intent,
            search_query,
        );
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

        // sqlite already opened above
        let mut hits =
            apply_popularity_boost_with_signals(&sqlite, hits, &mut hit_signals, &self.config)?;
        hits = diversify_by_cluster(&sqlite, hits, limit);
        hits = diversify_by_kind(hits, limit);
        hits.truncate(limit); // Keep top results before expansion

        // Expansion step: fetch related symbols (callees/callers) for top hits
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
        let size = resp.context.len() + resp.context_items.iter().map(|i| i.bytes).sum::<usize>();
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
            "m=default|b={}|r={}|x={}",
            self.config.max_context_bytes,
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
        let size = v.0.len() + v.1.iter().map(|i| i.bytes).sum::<usize>();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.contexts.insert(key, v.clone(), size);
        Ok(v)
    }

    fn parse_query_controls(query: &str) -> (String, QueryControls) {
        let mut controls = QueryControls::default();
        let mut kept = Vec::new();
        for token in query.split_whitespace() {
            let Some((k, v)) = token.split_once(':') else {
                kept.push(token);
                continue;
            };
            let key = k.trim().to_lowercase();
            let value = v.trim().trim_matches('"').trim_matches('\'');
            if value.is_empty() {
                kept.push(token);
                continue;
            }
            match key.as_str() {
                "id" => controls.id = Some(value.to_string()),
                "file" => controls.file = Some(value.to_string()),
                "path" => controls.path = Some(value.to_string()),
                "lang" | "language" => controls.lang = Some(Self::normalize_lang(value)),
                "kind" => controls.kind = Some(value.to_string()),
                _ => kept.push(token),
            }
        }
        (kept.join(" "), controls)
    }

    fn normalize_lang(s: &str) -> String {
        match s.trim().to_lowercase().as_str() {
            "ts" | "tsx" | "typescript" => "typescript".to_string(),
            "js" | "jsx" | "javascript" => "javascript".to_string(),
            other => other.to_string(),
        }
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

#[cfg(test)]
fn rank_hits(
    keyword_hits: &[KeywordHit],
    vector_hits: &[VectorHit],
    config: &Config,
    intent: &Option<Intent>,
    query: &str,
) -> Vec<RankedHit> {
    rank_hits_with_signals(keyword_hits, vector_hits, config, intent, query).0
}

fn rank_hits_with_signals(
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
            if h.name.eq_ignore_ascii_case(q) && is_definition_kind(&h.kind) {
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
            if h.name.eq_ignore_ascii_case(q) && is_definition_kind(&h.kind) {
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

fn intent_adjustment(intent: &Option<Intent>, kind: &str, file_path: &str, exported: bool) -> f32 {
    // Tweak 1: Test Penalty (0.5x multiplier)
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
        Intent::Callers(_) => 1.0, // Should be handled by graph search, but if fallback occurs
        Intent::Test => 1.0,
    }
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
    // Tweak 2: Glue Code Filtering
    if file_path.ends_with("index.ts") || file_path.ends_with("index.tsx") {
        // "Rank them lowest".
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
            score += 2.0; // Significant boost for directory/file match
        }
    }

    score
}

#[cfg(test)]
fn apply_popularity_boost(
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

fn apply_popularity_boost_with_signals(
    sqlite: &SqliteStore,
    mut hits: Vec<RankedHit>,
    hit_signals: &mut HashMap<String, HitSignals>,
    config: &Config,
) -> Result<Vec<RankedHit>> {
    if hits.is_empty() || config.rank_popularity_weight == 0.0 || config.rank_popularity_cap == 0 {
        return Ok(hits);
    }

    for h in hits.iter_mut() {
        let count = sqlite.count_incoming_edges(&h.id).unwrap_or(0);
        let capped = count.min(config.rank_popularity_cap) as f32;
        let denom = config.rank_popularity_cap as f32;
        if denom <= 0.0 {
            continue;
        }
        let boost = config.rank_popularity_weight * (capped / denom);
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

fn diversify_by_cluster(
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

fn diversify_by_kind(hits: Vec<RankedHit>, limit: usize) -> Vec<RankedHit> {
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

fn is_definition_kind(kind: &str) -> bool {
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

fn expand_with_edges(
    sqlite: &SqliteStore,
    hits: Vec<RankedHit>,
    limit: usize,
) -> Result<(Vec<RankedHit>, HashSet<String>)> {
    if hits.is_empty() {
        return Ok((hits, HashSet::new()));
    }

    let mut out = hits.clone();
    let mut seen: HashSet<String> = hits.iter().map(|h| h.id.clone()).collect();
    let mut expanded_ids = HashSet::new();
    let expand_candidates = hits.iter().take(3).cloned().collect::<Vec<_>>();

    for h in expand_candidates {
        let (is_func, is_type) = match h.kind.as_str() {
            "function" | "method" => (true, false),
            "struct" | "enum" | "class" | "interface" | "trait" => (false, true),
            _ => (false, false),
        };

        if is_func {
            // Find callees (implementation details)
            let edges = sqlite.list_edges_from(&h.id, 5)?;
            for edge in edges {
                if edge.edge_type != "call" {
                    continue;
                }
                if seen.insert(edge.to_symbol_id.clone()) {
                    if let Some(row) = sqlite.get_symbol_by_id(&edge.to_symbol_id)? {
                        let evidence_boost =
                            (1.0 + (edge.evidence_count as f32).ln_1p() * 0.25).clamp(1.0, 1.75);
                        let resolution_multiplier = match edge.resolution.as_str() {
                            "local" => 1.0,
                            "import" => 0.9,
                            "heuristic" => 0.75,
                            _ => 0.8,
                        };
                        out.push(RankedHit {
                            id: row.id.clone(),
                            score: h.score
                                * 0.8
                                * edge.confidence
                                * evidence_boost
                                * resolution_multiplier,
                            name: row.name,
                            kind: row.kind,
                            file_path: row.file_path,
                            exported: row.exported,
                            language: row.language,
                        });
                        expanded_ids.insert(row.id);
                    }
                }
            }
        } else if is_type {
            // Find usages (references TO this symbol)
            let edges = sqlite.list_edges_to(&h.id, 5)?;
            for edge in edges {
                if edge.edge_type != "reference"
                    && edge.edge_type != "type"
                    && edge.edge_type != "extends"
                    && edge.edge_type != "implements"
                    && edge.edge_type != "alias"
                {
                    continue;
                }
                if seen.insert(edge.from_symbol_id.clone()) {
                    if let Some(row) = sqlite.get_symbol_by_id(&edge.from_symbol_id)? {
                        let evidence_boost =
                            (1.0 + (edge.evidence_count as f32).ln_1p() * 0.25).clamp(1.0, 1.75);
                        let resolution_multiplier = match edge.resolution.as_str() {
                            "local" => 1.0,
                            "import" => 0.9,
                            "heuristic" => 0.75,
                            _ => 0.8,
                        };
                        out.push(RankedHit {
                            id: row.id.clone(),
                            score: h.score
                                * 0.8
                                * edge.confidence
                                * evidence_boost
                                * resolution_multiplier,
                            name: row.name,
                            kind: row.kind,
                            file_path: row.file_path,
                            exported: row.exported,
                            language: row.language,
                        });
                        expanded_ids.insert(row.id);
                    }
                }
            }
        }
    }

    // Re-sort and truncate
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
    });
    if out.len() > limit {
        out.truncate(limit);
    }

    Ok((out, expanded_ids))
}

fn unix_now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

fn trim_query(s: &str, max_len: usize) -> String {
    let mut out = s.trim().to_string();
    if out.len() > max_len {
        out.truncate(max_len);
    }
    out
}

enum Intent {
    Callers(String),
    Definition,
    Schema,
    Test,
}

fn normalize_query(query: &str) -> String {
    let out = text::normalize_query_text(query);
    let mut final_parts = Vec::new();
    for part in out.split_whitespace() {
        final_parts.push(part.to_string());
        let lower = part.to_lowercase();
        match lower.as_str() {
            "and" | "or" | "not" => {}
            "db" => final_parts.push("database".to_string()),
            "auth" => final_parts.push("authentication".to_string()),
            "nav" => final_parts.push("navigation".to_string()),
            "config" => final_parts.push("configuration".to_string()),
            _ => {}
        }

        if lower.chars().all(|c| c.is_ascii_alphabetic()) && lower.len() >= 5 {
            for stem in text::simple_stems(&lower) {
                final_parts.push(stem);
            }
        }
    }

    final_parts.join(" ")
}

fn detect_intent(query: &str) -> Option<Intent> {
    let q = query.trim().to_lowercase();

    // Tweak 1: Enhanced Test Detection
    if q.contains("test") || q.contains("spec") || q.contains("verify") {
        return Some(Intent::Test);
    }

    // Definition keywords
    if q.contains("schema")
        || q.contains("model")
        || q.contains("db table")
        || q.contains("database")
        || q.contains("migration")
        || q.contains("entity")
        || q.split_whitespace().any(|w| w == "db")
    {
        return Some(Intent::Schema);
    }
    if q.contains("class")
        || q.contains("interface")
        || q.contains("struct")
        || q.contains("type")
        || q.contains("def")
    {
        return Some(Intent::Definition);
    }

    if let Some(s) = q.strip_prefix("who calls ") {
        return Some(Intent::Callers(s.trim().to_string()));
    }
    if let Some(s) = q.strip_prefix("callers of ") {
        return Some(Intent::Callers(s.trim().to_string()));
    }
    if let Some(s) = q.strip_prefix("references to ") {
        return Some(Intent::Callers(s.trim().to_string()));
    }
    if let Some(s) = q.strip_prefix("usages of ") {
        return Some(Intent::Callers(s.trim().to_string()));
    }
    if let Some(s) = q.strip_prefix("where is ") {
        if let Some(rest) = s.strip_suffix(" used") {
            return Some(Intent::Callers(rest.trim().to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_max(max: usize) -> Config {
        Config {
            db_path: std::path::PathBuf::from("db"),
            vector_db_path: std::path::PathBuf::from("vec"),
            tantivy_index_path: std::path::PathBuf::from("tantivy"),
            base_dir: std::path::PathBuf::from("."),
            embeddings_backend: crate::config::EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_model_url: None,
            embeddings_model_sha256: None,
            embeddings_auto_download: false,
            embeddings_model_repo: None,
            embeddings_model_revision: None,
            embeddings_model_hf_token: None,
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 8,
            vector_search_limit: 10,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 0.0,
            rank_index_file_boost: 0.0,
            rank_test_penalty: 0.0,
            rank_popularity_weight: 0.0,
            rank_popularity_cap: 0,
            index_patterns: vec![],
            exclude_patterns: vec![],
            watch_mode: false,
            watch_debounce_ms: 100,
            max_context_bytes: max,
            index_node_modules: false,
            repo_roots: vec![],
        }
    }

    #[test]
    fn assemble_context_enforces_max_bytes_with_utf8() {
        let config = Arc::new(cfg_with_max(60));
        let sym = SymbolRow {
            id: "id1".to_string(),
            file_path: "a.ts".to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: "alpha".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            end_line: 1,
            text: "export function alpha() { return \"你好\" }".to_string(),
        };
        let assembler = ContextAssembler::new(config.clone());
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();
        let (out, _) = assembler.format_context(&sqlite, &[sym], &[], &[]).unwrap();
        assert!(out.len() <= config.max_context_bytes);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn popularity_sort_is_deterministic_on_ties() {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();
        let mut cfg = cfg_with_max(10_000);
        cfg.rank_popularity_weight = 0.01;
        cfg.rank_popularity_cap = 10;

        let a = RankedHit {
            id: "a".to_string(),
            score: 1.0,
            name: "b".to_string(),
            kind: "function".to_string(),
            file_path: "b.ts".to_string(),
            exported: false,
            language: "typescript".to_string(),
        };
        let b = RankedHit {
            id: "b".to_string(),
            score: 1.0,
            name: "a".to_string(),
            kind: "function".to_string(),
            file_path: "a.ts".to_string(),
            exported: false,
            language: "typescript".to_string(),
        };

        let out1 = apply_popularity_boost(&sqlite, vec![a.clone(), b.clone()], &cfg).unwrap();
        let out2 = apply_popularity_boost(&sqlite, vec![a, b], &cfg).unwrap();
        assert_eq!(
            out1.iter().map(|h| &h.id).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        assert_eq!(
            out2.iter().map(|h| &h.id).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
    }

    #[test]
    fn diversify_by_cluster_limits_per_cluster_and_fills_from_deferred() {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        for id in ["a1", "a2", "a3"] {
            sqlite
                .upsert_symbol(&SymbolRow {
                    id: id.to_string(),
                    file_path: "a.ts".to_string(),
                    language: "typescript".to_string(),
                    kind: "function".to_string(),
                    name: id.to_string(),
                    exported: true,
                    start_byte: 0,
                    end_byte: 0,
                    start_line: 1,
                    end_line: 1,
                    text: format!("export function {id}() {{}}"),
                })
                .unwrap();
            sqlite
                .upsert_similarity_cluster(&crate::storage::sqlite::SimilarityClusterRow {
                    symbol_id: id.to_string(),
                    cluster_key: "k".to_string(),
                })
                .unwrap();
        }

        let hits = vec![
            RankedHit {
                id: "a1".to_string(),
                score: 3.0,
                name: "a1".to_string(),
                kind: "function".to_string(),
                file_path: "a.ts".to_string(),
                exported: true,
                language: "typescript".to_string(),
            },
            RankedHit {
                id: "a2".to_string(),
                score: 2.0,
                name: "a2".to_string(),
                kind: "function".to_string(),
                file_path: "a.ts".to_string(),
                exported: true,
                language: "typescript".to_string(),
            },
            RankedHit {
                id: "a3".to_string(),
                score: 1.0,
                name: "a3".to_string(),
                kind: "function".to_string(),
                file_path: "a.ts".to_string(),
                exported: true,
                language: "typescript".to_string(),
            },
        ];

        let out = diversify_by_cluster(&sqlite, hits, 2);
        assert_eq!(
            out.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["a1", "a2"]
        );

        let hits2 = vec![
            RankedHit {
                id: "a1".to_string(),
                score: 4.0,
                name: "a1".to_string(),
                kind: "function".to_string(),
                file_path: "a.ts".to_string(),
                exported: true,
                language: "typescript".to_string(),
            },
            RankedHit {
                id: "a2".to_string(),
                score: 3.0,
                name: "a2".to_string(),
                kind: "function".to_string(),
                file_path: "a.ts".to_string(),
                exported: true,
                language: "typescript".to_string(),
            },
            RankedHit {
                id: "a3".to_string(),
                score: 2.0,
                name: "a3".to_string(),
                kind: "function".to_string(),
                file_path: "a.ts".to_string(),
                exported: true,
                language: "typescript".to_string(),
            },
            RankedHit {
                id: "x".to_string(),
                score: 1.0,
                name: "x".to_string(),
                kind: "function".to_string(),
                file_path: "x.ts".to_string(),
                exported: true,
                language: "typescript".to_string(),
            },
        ];
        let out2 = diversify_by_cluster(&sqlite, hits2, 3);
        assert_eq!(
            out2.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["a1", "a2", "x"]
        );
    }

    #[test]
    fn expand_with_edges_finds_related_symbols() {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        // 1. Setup: main -> calls -> helper
        sqlite
            .upsert_symbol(&SymbolRow {
                id: "main".to_string(),
                file_path: "main.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "main".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 0,
                start_line: 1,
                end_line: 1,
                text: "function main() {}".to_string(),
            })
            .unwrap();
        sqlite
            .upsert_symbol(&SymbolRow {
                id: "helper".to_string(),
                file_path: "helper.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "helper".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 0,
                start_line: 1,
                end_line: 1,
                text: "function helper() {}".to_string(),
            })
            .unwrap();
        sqlite
            .upsert_edge(&crate::storage::sqlite::EdgeRow {
                from_symbol_id: "main".to_string(),
                to_symbol_id: "helper".to_string(),
                edge_type: "call".to_string(),
                at_file: None,
                at_line: None,
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();

        // 2. Setup: consumer -> references -> MyStruct
        sqlite
            .upsert_symbol(&SymbolRow {
                id: "struct1".to_string(),
                file_path: "struct.ts".to_string(),
                language: "typescript".to_string(),
                kind: "struct".to_string(),
                name: "MyStruct".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 0,
                start_line: 1,
                end_line: 1,
                text: "struct MyStruct {}".to_string(),
            })
            .unwrap();
        sqlite
            .upsert_symbol(&SymbolRow {
                id: "consumer".to_string(),
                file_path: "consumer.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "consumer".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 0,
                start_line: 1,
                end_line: 1,
                text: "function consumer() {}".to_string(),
            })
            .unwrap();
        sqlite
            .upsert_edge(&crate::storage::sqlite::EdgeRow {
                from_symbol_id: "consumer".to_string(),
                to_symbol_id: "struct1".to_string(),
                edge_type: "reference".to_string(),
                at_file: None,
                at_line: None,
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();

        // Test Case A: Expand Function (Outgoing Calls)
        let hits_func = vec![RankedHit {
            id: "main".to_string(),
            score: 1.0,
            name: "main".to_string(),
            kind: "function".to_string(),
            file_path: "main.ts".to_string(),
            exported: true,
            language: "typescript".to_string(),
        }];

        let (expanded_func, _) = expand_with_edges(&sqlite, hits_func, 10).unwrap();
        assert!(expanded_func.iter().any(|h| h.id == "helper"));

        // Test Case B: Expand Struct (Incoming References)
        let hits_struct = vec![RankedHit {
            id: "struct1".to_string(),
            score: 1.0,
            name: "MyStruct".to_string(),
            kind: "struct".to_string(),
            file_path: "struct.ts".to_string(),
            exported: true,
            language: "typescript".to_string(),
        }];

        let (expanded_struct, _) = expand_with_edges(&sqlite, hits_struct, 10).unwrap();
        assert!(expanded_struct.iter().any(|h| h.id == "consumer"));
    }

    #[test]
    fn schema_intent_boosts_schema_files() {
        let schema_hit = RankedHit {
            id: "1".to_string(),
            score: 0.5,
            name: "UserSchema".to_string(),
            kind: "struct".to_string(),
            file_path: "src/db/schema.rs".to_string(),
            exported: true,
            language: "rust".to_string(),
        };
        let db_infra_hit = RankedHit {
            id: "3".to_string(),
            score: 0.5,
            name: "init_db".to_string(),
            kind: "function".to_string(),
            file_path: "src/db/init.rs".to_string(),
            exported: true,
            language: "rust".to_string(),
        };
        let other_hit = RankedHit {
            id: "2".to_string(),
            score: 0.9,
            name: "Other".to_string(),
            kind: "function".to_string(),
            file_path: "src/other.rs".to_string(),
            exported: true,
            language: "rust".to_string(),
        };

        let intent = Some(Intent::Schema);

        let schema_boost = super::intent_adjustment(
            &intent,
            &schema_hit.kind,
            &schema_hit.file_path,
            schema_hit.exported,
        );
        let db_infra_boost = super::intent_adjustment(
            &intent,
            &db_infra_hit.kind,
            &db_infra_hit.file_path,
            db_infra_hit.exported,
        );
        let other_boost = super::intent_adjustment(
            &intent,
            &other_hit.kind,
            &other_hit.file_path,
            other_hit.exported,
        );

        assert_eq!(schema_boost, 75.0); // Explicit schema file gets max boost
        assert_eq!(db_infra_boost, 25.0); // Generic db file gets lower boost
        assert_eq!(other_boost, 0.5);
    }

    #[test]
    fn rank_hits_applies_definition_bias() {
        let cfg = cfg_with_max(1000);
        let query = "MyClass";
        let intent = Some(Intent::Definition); // or None, rank_hits handles it

        let exact = KeywordHit {
            id: "1".to_string(),
            score: 1.0,
            name: "MyClass".to_string(),
            kind: "class".to_string(),
            file_path: "src/my_class.ts".to_string(),
            exported: true,
        };

        let partial = KeywordHit {
            id: "2".to_string(),
            score: 1.0, // Same base score
            name: "MyClassHelper".to_string(),
            kind: "class".to_string(),
            file_path: "src/helper.ts".to_string(),
            exported: true,
        };

        let hits = rank_hits(&[exact.clone(), partial.clone()], &[], &cfg, &intent, query);

        // Exact match should have much higher score
        let h1 = hits.iter().find(|h| h.id == "1").unwrap();
        let h2 = hits.iter().find(|h| h.id == "2").unwrap();

        assert!(
            h1.score > h2.score + 5.0,
            "Exact match should be significantly boosted"
        );
    }

    #[test]
    fn rank_hits_applies_subdirectory_bias() {
        let cfg = cfg_with_max(1000);
        let query = "auth login"; // "auth" matches directory

        let hit_in_dir = KeywordHit {
            id: "1".to_string(),
            score: 1.0,
            name: "login".to_string(),
            kind: "function".to_string(),
            file_path: "src/auth/login.ts".to_string(), // Matches "auth"
            exported: true,
        };

        let hit_outside = KeywordHit {
            id: "2".to_string(),
            score: 1.0,
            name: "login".to_string(),
            kind: "function".to_string(),
            file_path: "src/utils/login.ts".to_string(),
            exported: true,
        };

        let hits = rank_hits(
            &[hit_in_dir.clone(), hit_outside.clone()],
            &[],
            &cfg,
            &None,
            query,
        );

        let h1 = hits.iter().find(|h| h.id == "1").unwrap();
        let h2 = hits.iter().find(|h| h.id == "2").unwrap();

        assert!(h1.score > h2.score, "Subdirectory match should be boosted");
    }

    #[test]
    fn intent_adjustment_applies_test_penalty() {
        let file_path = "src/foo.test.ts";

        // Case 1: No intent -> Penalty
        let mult = intent_adjustment(&None, "function", file_path, true);
        assert!(
            (mult - 0.5).abs() < f32::EPSILON,
            "Should penalize test files when intent is None"
        );

        // Case 2: Test intent -> No Penalty
        let mult_test = intent_adjustment(&Some(Intent::Test), "function", file_path, true);
        assert!(
            (mult_test - 1.0).abs() < f32::EPSILON,
            "Should NOT penalize test files when intent is Test"
        );
    }

    #[test]
    fn normalize_query_expands_acronyms_and_splits_camel_case() {
        assert_eq!(normalize_query("DBConnection"), "DB database Connection");
        assert_eq!(
            normalize_query("auth service"),
            "auth authentication service"
        );
        assert_eq!(normalize_query("nav bar"), "nav navigation bar");
        assert_eq!(normalize_query("db connection"), "db database connection");
        // Test combinations
        // "AuthDB" -> "Auth DB" -> "Auth" + "authentication", "DB" + "database"
        assert_eq!(normalize_query("AuthDB"), "Auth authentication DB database");
    }

    #[test]
    fn normalize_query_splits_digits_and_separators() {
        assert_eq!(normalize_query("HTTP2Server_v1"), "HTTP 2 Server v1");
        assert_eq!(normalize_query("foo/bar-baz"), "foo bar baz");
    }

    #[test]
    fn query_controls_strip_tokens_and_filter_hits() {
        let (q, controls) =
            Retriever::parse_query_controls("id:abc lang:ts kind:function file:*util* foo bar");
        assert_eq!(q, "foo bar");
        assert_eq!(controls.id.as_deref(), Some("abc"));
        assert_eq!(controls.lang.as_deref(), Some("typescript"));
        assert_eq!(controls.kind.as_deref(), Some("function"));
        assert_eq!(controls.file.as_deref(), Some("*util*"));

        let hits = vec![
            RankedHit {
                id: "1".to_string(),
                score: 1.0,
                name: "x".to_string(),
                kind: "function".to_string(),
                file_path: "src/util.ts".to_string(),
                exported: true,
                language: "typescript".to_string(),
            },
            RankedHit {
                id: "2".to_string(),
                score: 1.0,
                name: "y".to_string(),
                kind: "class".to_string(),
                file_path: "src/util.ts".to_string(),
                exported: true,
                language: "typescript".to_string(),
            },
            RankedHit {
                id: "3".to_string(),
                score: 1.0,
                name: "z".to_string(),
                kind: "function".to_string(),
                file_path: "src/other.rs".to_string(),
                exported: true,
                language: "rust".to_string(),
            },
        ];

        let filtered = Retriever::filter_hits_by_controls(hits, &controls);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "1");
    }

    #[test]
    fn diversify_by_kind_interleaves_results() {
        let def = RankedHit {
            id: "d".to_string(),
            score: 10.0,
            name: "d".to_string(),
            kind: "function".to_string(),
            file_path: "d.ts".to_string(),
            exported: true,
            language: "ts".to_string(),
        };
        let test = RankedHit {
            id: "t".to_string(),
            score: 9.0,
            name: "t".to_string(),
            kind: "function".to_string(),
            file_path: "d.test.ts".to_string(),
            exported: true,
            language: "ts".to_string(),
        };
        let usage = RankedHit {
            id: "u".to_string(),
            score: 8.0,
            name: "u".to_string(),
            kind: "call".to_string(),
            file_path: "u.ts".to_string(),
            exported: true,
            language: "ts".to_string(),
        };

        // Even if scores are ordered d, t, u, result should preserve them if limit allows
        let hits = vec![def.clone(), test.clone(), usage.clone()];
        let out = diversify_by_kind(hits, 3);
        assert_eq!(out.len(), 3);

        // If we have many definitions and limit 3, we should still try to include a test/usage
        let def2 = RankedHit {
            id: "d2".to_string(),
            score: 9.5,
            ..def.clone()
        };
        let def3 = RankedHit {
            id: "d3".to_string(),
            score: 9.2,
            ..def.clone()
        };

        let hits2 = vec![
            def.clone(),
            def2.clone(),
            def3.clone(),
            test.clone(),
            usage.clone(),
        ];
        let out2 = diversify_by_kind(hits2, 3);

        // Should contain d (def), t (test), u (usage/other) because we try to pick one from each
        // "others" bucket catches usage (kind="call" is not definition kind)

        let has_def = out2.iter().any(|h| h.id.starts_with("d"));
        let has_test = out2.iter().any(|h| h.id == "t");
        let has_usage = out2.iter().any(|h| h.id == "u");

        assert!(has_def);
        assert!(has_test);
        assert!(has_usage);
    }

    #[tokio::test]
    async fn search_reuses_query_embedding_across_cache_misses() {
        use crate::embeddings::hash::HashEmbedder;
        use crate::storage::{tantivy::TantivyIndex, vector::LanceDbStore};
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingEmbedder {
            inner: HashEmbedder,
            calls: Arc<AtomicUsize>,
        }

        impl Embedder for CountingEmbedder {
            fn dim(&self) -> usize {
                self.inner.dim()
            }

            fn embed(&mut self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.inner.embed(texts)
            }
        }

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base: PathBuf = std::env::temp_dir().join(format!("cimcp-cache-test-{nanos}"));
        std::fs::create_dir_all(&base).unwrap();

        let mut cfg = cfg_with_max(50_000);
        cfg.base_dir = base.clone();
        cfg.db_path = base.join("code-intel.db");
        cfg.vector_db_path = base.join("vectors");
        cfg.tantivy_index_path = base.join("tantivy");
        cfg.repo_roots = vec![base.clone()];

        let sqlite = SqliteStore::open(&cfg.db_path).unwrap();
        sqlite.init().unwrap();
        let sym = SymbolRow {
            id: "id1".to_string(),
            file_path: "src/a.ts".to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: "alpha".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            end_line: 1,
            text: "export function alpha() {}".to_string(),
        };
        sqlite.upsert_symbol(&sym).unwrap();

        let tantivy = Arc::new(TantivyIndex::open_or_create(&cfg.tantivy_index_path).unwrap());
        tantivy.upsert_symbol(&sym).unwrap();
        tantivy.commit().unwrap();

        let vec_store = LanceDbStore::connect(&cfg.vector_db_path).await.unwrap();
        let vectors = Arc::new(vec_store.open_or_create_table("symbols", 8).await.unwrap());

        let calls = Arc::new(AtomicUsize::new(0));
        let embedder: Box<dyn Embedder + Send> = Box::new(CountingEmbedder {
            inner: HashEmbedder::new(8),
            calls: calls.clone(),
        });
        let embedder = Arc::new(AsyncMutex::new(embedder));

        let retriever = Retriever::new(Arc::new(cfg), tantivy, vectors, embedder);

        let _ = retriever.search("alpha", 3, false).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let _ = retriever.search("alpha", 4, false).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

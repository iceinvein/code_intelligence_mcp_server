pub mod assembler;

use crate::{
    config::Config,
    embeddings::Embedder,
    retrieval::assembler::ContextAssembler,
    storage::{
        sqlite::{SqliteStore, SymbolRow},
        tantivy::{SearchHit as KeywordHit, TantivyIndex},
        vector::{LanceVectorTable, VectorHit},
    },
};
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};
use tokio::sync::Mutex;

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
}

#[derive(Clone)]
pub struct Retriever {
    config: Arc<Config>,
    db_path: std::path::PathBuf,
    tantivy: Arc<TantivyIndex>,
    vectors: Arc<LanceVectorTable>,
    embedder: Arc<Mutex<Box<dyn Embedder + Send>>>,
}

impl Retriever {
    pub fn new(
        config: Arc<Config>,
        tantivy: Arc<TantivyIndex>,
        vectors: Arc<LanceVectorTable>,
        embedder: Arc<Mutex<Box<dyn Embedder + Send>>>,
    ) -> Self {
        Self {
            db_path: config.db_path.clone(),
            config,
            tantivy,
            vectors,
            embedder,
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

        // 0. Intent Detection
        let intent = detect_intent(query);

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

                    let assembler = ContextAssembler::new(self.config.clone());
                    let context = assembler.assemble_context(&sqlite, &rows, &[])?;

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

                    return Ok(SearchResponse {
                        query: query.to_string(),
                        limit,
                        hits,
                        context,
                    });
                }
            }
        }

        let k = self.config.vector_search_limit.max(limit).max(5);
        let keyword_t = Instant::now();
        let keyword_hits = self.tantivy.search(query, k)?;
        let keyword_ms = keyword_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

        let vector_t = Instant::now();
        let query_vector = {
            let mut embedder = self.embedder.lock().await;
            let mut out = embedder.embed(&[query.to_string()])?;
            out.pop()
                .ok_or_else(|| anyhow!("Embedder returned no vector"))?
        };
        let vector_hits = self.vectors.search(&query_vector, k).await?;
        let vector_ms = vector_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

        let merge_t = Instant::now();
        let ranked = rank_hits(&keyword_hits, &vector_hits, &self.config, &intent);
        let mut uniq = Vec::new();
        let mut seen = HashSet::new();
        for hit in ranked {
            if seen.insert(hit.id.clone()) {
                uniq.push(hit);
            }
        }

        let hits = if exported_only {
            uniq.into_iter().filter(|h| h.exported).collect::<Vec<_>>()
        } else {
            uniq
        };

        // sqlite already opened above
        let mut hits = apply_popularity_boost(&sqlite, hits, &self.config)?;
        hits = diversify_by_cluster(&sqlite, hits, limit);
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

        let assembler = ContextAssembler::new(self.config.clone());
        let context = assembler.assemble_context(&sqlite, &roots, &extra)?;

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

        Ok(SearchResponse {
            query: query.to_string(),
            limit,
            hits,
            context,
        })
    }

    pub fn assemble_definitions(&self, symbols: &[SymbolRow]) -> Result<String> {
        let assembler = ContextAssembler::new(self.config.clone());
        assembler.format_context(symbols, &[], &[])
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

fn rank_hits(
    keyword_hits: &[KeywordHit],
    vector_hits: &[VectorHit],
    config: &Config,
    intent: &Option<Intent>,
) -> Vec<RankedHit> {
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

    let (vector_w, keyword_w) =
        normalize_pair(config.rank_vector_weight, config.rank_keyword_weight);

    for h in vector_hits {
        let v = vec_scores.get(&h.id).copied().unwrap_or(0.0);
        let v = if max_vec > 0.0 { v / max_vec } else { 0.0 };
        let kw = kw_scores.get(&h.id).copied().unwrap_or(0.0);
        let mut score = vector_w * v + keyword_w * kw;
        score += structural_adjustment(config, h.exported, &h.file_path);
        score *= intent_adjustment(intent, &h.kind, &h.file_path, h.exported);

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
        let mut score = vector_w * v + keyword_w * kw;
        score += structural_adjustment(config, h.exported, &h.file_path);
        score *= intent_adjustment(intent, &h.kind, &h.file_path, h.exported);

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
    out
}

fn intent_adjustment(intent: &Option<Intent>, kind: &str, file_path: &str, exported: bool) -> f32 {
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
            if path.contains("schema")
                || path.contains("model")
                || path.contains("entity")
                || path.contains("db/")
            {
                2.0
            } else {
                1.0
            }
        }
        Intent::Callers(_) => 1.0, // Should be handled by graph search, but if fallback occurs
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

fn structural_adjustment(config: &Config, exported: bool, file_path: &str) -> f32 {
    let mut score = 0.0;
    if exported {
        score += config.rank_exported_boost;
    }
    if file_path.contains(".test.") || file_path.contains("/test/") || file_path.contains("/tests/")
    {
        score -= config.rank_test_penalty;
    }
    if file_path.ends_with("index.ts") || file_path.ends_with("index.tsx") {
        score += config.rank_index_file_boost;
    }
    score
}

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
                        out.push(RankedHit {
                            id: row.id.clone(),
                            score: h.score * 0.8,
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
                if edge.edge_type != "reference" && edge.edge_type != "type" {
                    continue;
                }
                if seen.insert(edge.from_symbol_id.clone()) {
                    if let Some(row) = sqlite.get_symbol_by_id(&edge.from_symbol_id)? {
                        out.push(RankedHit {
                            id: row.id.clone(),
                            score: h.score * 0.8,
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
}

fn detect_intent(query: &str) -> Option<Intent> {
    let q = query.trim().to_lowercase();

    // Definition keywords
    if q.contains("schema") || q.contains("model") || q.contains("db table") {
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
        let out = assembler.format_context(&[sym], &[], &[]).unwrap();
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
}

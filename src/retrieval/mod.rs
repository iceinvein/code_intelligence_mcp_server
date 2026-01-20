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
    pub context_items: Vec<ContextItem>,
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

        // Normalize and expand query (Tweak 3: Acronyms & Casing)
        let expanded_query = normalize_query(query);
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

                    let assembler = ContextAssembler::new(self.config.clone());
                    let (context, context_items) =
                        assembler.assemble_context_with_items(&sqlite, &rows, &[])?;

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
                        context_items,
                    });
                }
            }
        }

        let k = self.config.vector_search_limit.max(limit).max(5);
        let keyword_t = Instant::now();
        let keyword_hits = self.tantivy.search(search_query, k)?;
        let keyword_ms = keyword_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

        let vector_t = Instant::now();
        let query_vector = {
            let mut embedder = self.embedder.lock().await;
            let mut out = embedder.embed(&[search_query.to_string()])?;
            out.pop()
                .ok_or_else(|| anyhow!("Embedder returned no vector"))?
        };
        let vector_hits = self.vectors.search(&query_vector, k).await?;
        let vector_ms = vector_t.elapsed().as_millis().min(u64::MAX as u128) as u64;

        let merge_t = Instant::now();
        let ranked = rank_hits(&keyword_hits, &vector_hits, &self.config, &intent, query);
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

        let assembler = ContextAssembler::new(self.config.clone());
        let (context, context_items) =
            assembler.assemble_context_with_items(&sqlite, &roots, &extra)?;

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
            context_items,
        })
    }

    pub fn assemble_definitions(&self, symbols: &[SymbolRow]) -> Result<String> {
        let assembler = ContextAssembler::new(self.config.clone());
        Ok(assembler.format_context(symbols, &[], &[])?.0)
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
    query: &str,
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
        score += structural_adjustment(config, h.exported, &h.file_path, intent, query);
        score *= intent_adjustment(intent, &h.kind, &h.file_path, h.exported);

        // Definition Bias
        if !matches!(intent, Some(Intent::Callers(_))) {
            let q = query.trim();
            if h.name.eq_ignore_ascii_case(q) && is_definition_kind(&h.kind) {
                score += 10.0;
            } else if h.name.to_lowercase().contains(&q.to_lowercase())
                && is_definition_kind(&h.kind)
            {
                score += 1.0;
            }
        }

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
        score += structural_adjustment(config, h.exported, &h.file_path, intent, query);
        score *= intent_adjustment(intent, &h.kind, &h.file_path, h.exported);

        // Definition Bias
        if !matches!(intent, Some(Intent::Callers(_))) {
            let q = query.trim();
            if h.name.eq_ignore_ascii_case(q) && is_definition_kind(&h.kind) {
                score += 10.0;
            } else if h.name.to_lowercase().contains(&q.to_lowercase())
                && is_definition_kind(&h.kind)
            {
                score += 1.0;
            }
        }

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
                if edge.edge_type != "reference"
                    && edge.edge_type != "extends"
                    && edge.edge_type != "implements"
                    && edge.edge_type != "alias"
                {
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
    Test,
}

fn normalize_query(query: &str) -> String {
    // 1. Split CamelCase
    let mut new_query = String::new();
    let chars: Vec<char> = query.chars().collect();

    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            if prev.is_lowercase() {
                new_query.push(' ');
            } else if i + 1 < chars.len() && chars[i + 1].is_lowercase() {
                // Handles DBConnection -> DB Connection
                new_query.push(' ');
            }
        }
        new_query.push(c);
    }

    // 2. Acronym expansion
    let mut final_parts = Vec::new();
    for part in new_query.split_whitespace() {
        final_parts.push(part.to_string());
        match part.to_lowercase().as_str() {
            "db" => final_parts.push("database".to_string()),
            "auth" => final_parts.push("authentication".to_string()),
            "nav" => final_parts.push("navigation".to_string()),
            "config" => final_parts.push("configuration".to_string()),
            _ => {}
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
        let (out, _) = assembler.format_context(&[sym], &[], &[]).unwrap();
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
}

//! Fast-path early exits for ID lookup and Callers intent queries.

use super::query::{trim_query, QueryControls};
use super::{RankedHit, Retriever, SearchResponse, SearchResponseWithSignals};
use crate::storage::sqlite::SqliteStore;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Handle direct symbol ID lookup (fast path: no search needed).
///
/// Returns `Some(response)` if the query has an `id:` control and the symbol exists.
/// Returns `None` to fall through to the normal search path.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_id_lookup(
    retriever: &Retriever,
    sqlite: &SqliteStore,
    controls: &QueryControls,
    query_without_controls: &str,
    query: &str,
    cache_key: String,
    limit: usize,
    exported_only: bool,
    started_at_unix_s: i64,
    started: Instant,
) -> Result<Option<SearchResponseWithSignals>> {
    let id = match &controls.id {
        Some(id) => id,
        None => return Ok(None),
    };

    let row = match sqlite.get_symbol_by_id(id)? {
        Some(row) => row,
        None => return Ok(None),
    };

    if exported_only && !row.exported {
        return Ok(Some(SearchResponseWithSignals {
            response: SearchResponse {
                query: query.to_string(),
                limit,
                hits: vec![],
                context: String::new(),
            },
            hit_signals: HashMap::new(),
        }));
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

    let (context, _context_items) = retriever.assemble_context_cached(
        sqlite,
        std::slice::from_ref(&row),
        &[],
        Some(query_without_controls),
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
        embedding_ms: 0,
        reranker_ms: 0,
        scoring_ms: 0,
        assembly_ms: 0,
        fusion_ms: 0,
        search_path: "direct_id".to_string(),
        cache_status: "miss".to_string(),
        subquery_count: 0,
        keyword_candidates: 0,
        vector_candidates: 0,
        fused_candidates: hits.len() as u64,
    };
    let _ = sqlite.insert_search_run(&run);

    let resp = SearchResponse {
        query: query.to_string(),
        limit,
        hits,
        context,
    };
    let result = SearchResponseWithSignals {
        response: resp,
        hit_signals: HashMap::new(),
    };
    retriever.cache_insert_response(
        cache_key,
        result.response.clone(),
        result.hit_signals.clone(),
        &[],
    );
    Ok(Some(result))
}

/// Handle `callers:FunctionName` intent (fast path: graph traversal, no search).
///
/// Returns `Some(response)` if the intent is Callers and matching edges exist.
/// Returns `None` to fall through to the normal search path.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_callers_intent(
    retriever: &Retriever,
    sqlite: &SqliteStore,
    target_name: &str,
    query_without_controls: &str,
    query: &str,
    cache_key: String,
    limit: usize,
    exported_only: bool,
    started_at_unix_s: i64,
    started: Instant,
) -> Result<Option<SearchResponseWithSignals>> {
    let targets = sqlite.search_symbols_by_exact_name(target_name, None, 5)?;
    let target = match targets.first() {
        Some(t) => t,
        None => return Ok(None),
    };

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

    if hits.is_empty() {
        return Ok(None);
    }

    hits.truncate(limit);
    let rows = hits
        .iter()
        .filter_map(|h| sqlite.get_symbol_by_id(&h.id).ok().flatten())
        .collect::<Vec<_>>();

    let (context, _context_items) =
        retriever.assemble_context_cached(sqlite, &rows, &[], Some(query_without_controls))?;

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
        embedding_ms: 0,
        reranker_ms: 0,
        scoring_ms: 0,
        assembly_ms: 0,
        fusion_ms: 0,
        search_path: "callers".to_string(),
        cache_status: "miss".to_string(),
        subquery_count: 0,
        keyword_candidates: 0,
        vector_candidates: 0,
        fused_candidates: hits.len() as u64,
    };
    let _ = sqlite.insert_search_run(&run);

    let resp = SearchResponse {
        query: query.to_string(),
        limit,
        hits,
        context,
    };
    let result = SearchResponseWithSignals {
        response: resp,
        hit_signals: HashMap::new(),
    };
    retriever.cache_insert_response(
        cache_key,
        result.response.clone(),
        result.hit_signals.clone(),
        &[],
    );
    Ok(Some(result))
}

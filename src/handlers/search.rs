//! Search-related MCP tool handlers

use crate::retrieval::{ContextMode, Retriever};
use crate::storage::sqlite::SqliteStore;
use crate::tools::*;
use serde_json::json;

use super::budget::{
    budget_array, budget_string_field, clamp_limit, insert_budgeted_array, DEFAULT_MAX_STRING_CHARS,
};
use super::AppState;

/// Default per-hit snippet line count for `context="snippets"`.
const SNIPPET_LINES: usize = 8;

/// Build the `next_step` payload that nudges the agent toward `hydrate_symbols`
/// when `search_code` returned hits in discovery-only mode.
///
/// Returns `None` when:
///   - `context_mode` is not `None` (bodies are already in the response).
///   - `hits` is empty (nothing to hydrate).
#[allow(dead_code)]
fn build_next_step(
    context_mode: ContextMode,
    hits: &[crate::retrieval::RankedHit],
) -> Option<serde_json::Value> {
    if !matches!(context_mode, ContextMode::None) {
        return None;
    }
    if hits.is_empty() {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut ids: Vec<String> = Vec::with_capacity(hits.len());
    for h in hits {
        if seen.insert(h.id.clone()) {
            ids.push(h.id.clone());
        }
    }
    Some(json!({
        "tool": "hydrate_symbols",
        "args": { "ids": ids },
        "reason": "Call hydrate_symbols with these IDs to fetch source bodies. Prefer this over grep/read for the symbols already located by search_code."
    }))
}

/// Handle search_code tool.
///
/// `context` defaults to `"none"` — the lean discovery response (hits + IDs).
/// For nontrivial questions prefer `investigate` (which runs the full chain
/// server-side); pass `context: "snippets"` for a per-hit preview, or
/// `context: "full"` for the legacy markdown bundle with graph expansion.
pub async fn handle_search_code(
    retriever: &Retriever,
    db_path: &camino::Utf8Path,
    tool: SearchCodeTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = clamp_limit(tool.limit, 5, 100);
    let exported_only = tool.exported_only.unwrap_or(false);
    let context_mode = ContextMode::from_str(tool.context.as_deref());

    let result = retriever
        .search(&tool.query, limit, exported_only, context_mode)
        .await?;
    let mut response = serde_json::to_value(&result.response)?;
    let hits_count = response
        .get("hits")
        .and_then(|v| v.as_array())
        .map(|v| v.len())
        .unwrap_or(0);
    response["hits_budget"] = json!({
        "total_count": hits_count,
        "returned_count": hits_count,
        "truncated": false,
    });

    match context_mode {
        ContextMode::None => {
            // Drop the empty context string entirely so callers don't see a
            // misleading empty field.
            if let Some(map) = response.as_object_mut() {
                map.remove("context");
                if let Some(ns) = build_next_step(ContextMode::None, &result.response.hits) {
                    map.insert("next_step".to_string(), ns);
                }
            }
        }
        ContextMode::Snippets => {
            if let Some(map) = response.as_object_mut() {
                map.remove("context");
            }
            attach_hit_snippets(db_path, &mut response, &result.response.hits)?;
        }
        ContextMode::Full => {
            budget_string_field(&mut response, "context", DEFAULT_MAX_STRING_CHARS);
        }
    }
    Ok(response)
}

/// Attach a compact per-hit `snippet` (first N body lines) to each entry in
/// `response["hits"]`. Used by `context="snippets"`.
fn attach_hit_snippets(
    db_path: &camino::Utf8Path,
    response: &mut serde_json::Value,
    hits: &[crate::retrieval::RankedHit],
) -> Result<(), anyhow::Error> {
    if hits.is_empty() {
        return Ok(());
    }
    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;
    let Some(arr) = response.get_mut("hits").and_then(|v| v.as_array_mut()) else {
        return Ok(());
    };
    for (i, slot) in arr.iter_mut().enumerate() {
        let Some(hit) = hits.get(i) else { break };
        let Some(row) = sqlite.get_symbol_by_id(&hit.id)? else {
            continue;
        };
        let snippet = compact_snippet(&row.text, SNIPPET_LINES);
        if let Some(obj) = slot.as_object_mut() {
            obj.insert("snippet".to_string(), json!(snippet));
        }
    }
    Ok(())
}

/// Take the first `max_lines` lines (trimmed of trailing whitespace) and
/// append a truncation marker when the body exceeds it.
fn compact_snippet(text: &str, max_lines: usize) -> String {
    let total = text.lines().count();
    let kept: Vec<&str> = text.lines().take(max_lines).map(|l| l.trim_end()).collect();
    let mut out = kept.join("\n");
    if total > max_lines {
        out.push_str(&format!("\n// ... {} more lines", total - max_lines));
    }
    out
}

/// Handle explain_search tool
pub async fn handle_explain_search(
    retriever: &Retriever,
    tool: ExplainSearchTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = clamp_limit(tool.limit, 10, 100);
    let exported_only = tool.exported_only.unwrap_or(false);
    let verbose = tool.verbose.unwrap_or(false);
    let include_display = tool.include_display.unwrap_or(false);

    let result = retriever
        .search(&tool.query, limit, exported_only, ContextMode::None)
        .await?;
    let resp = &result.response;
    let hit_signals = &result.hit_signals;

    // Build detailed breakdown with display formatting
    let mut results = Vec::new();
    for hit in &resp.hits {
        let signals = hit_signals.get(&hit.id);
        let mut breakdown = json!({
            "symbol_id": hit.id,
            "symbol_name": hit.name,
            "kind": hit.kind,
            "file_path": hit.file_path,
            "score": hit.score,
            "exported": hit.exported,
        });

        if let Some(sig) = signals {
            breakdown["score_breakdown"] = json!({
                "keyword_score": sig.keyword_score,
                "vector_score": sig.vector_score,
                "base_score": sig.base_score,
                "structural_adjust": sig.structural_adjust,
                "intent_multiplier": sig.intent_mult,
                "definition_bias": sig.definition_bias,
                "term_coverage": sig.term_coverage,
                "symbol_importance": sig.symbol_importance,
                "test_symbol_penalty": sig.test_symbol_penalty,
                "popularity_boost": sig.popularity_boost,
                "learning_boost": sig.learning_boost,
                "affinity_boost": sig.affinity_boost,
            });
        }

        if verbose {
            if let Some(sig) = signals {
                breakdown["signals"] = json!({
                    "test_file_penalty": sig.keyword_score < 0.0,
                    "glue_code_penalty": sig.structural_adjust < 0.0,
                    "export_boost": sig.definition_bias > 0.0,
                });
            }
        }

        results.push(breakdown);
    }

    let budgeted_results = budget_array(results, limit);
    let mut response = json!({
        "query": resp.query,
        "limit": resp.limit,
        "count": budgeted_results.returned_count,
    });
    insert_budgeted_array(&mut response, "results", budgeted_results)?;
    if include_display {
        let results = response
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        response["display"] = json!(format_scoring_breakdown(&resp.query, &results));
    }
    Ok(response)
}

/// Handle find_similar_code tool - semantic similarity search via embeddings
pub async fn handle_find_similar_code(
    state: &AppState,
    tool: FindSimilarCodeTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = clamp_limit(tool.limit, 20, 100);
    let threshold = tool.threshold.unwrap_or(0.5);
    let include_display = tool.include_display.unwrap_or(false);

    let sqlite = &state.sqlite;

    // Determine search vector: either from symbol_name or code_snippet
    let (query_vector, query_description) = if let Some(name) = &tool.symbol_name {
        // Find symbol and get its embedding
        let roots = sqlite.search_symbols_by_exact_name(name, tool.file_path.as_deref(), 1)?;
        let Some(root) = roots.first() else {
            return Ok(json!({
                "error": "SYMBOL_NOT_FOUND",
                "message": format!("Symbol '{}' not found", name),
                "results": [],
            }));
        };

        // Try to get embedding from LanceDB by symbol ID
        // If not found, fall back to embedding the symbol's text
        let vector = match state
            .retriever
            .get_vector_store()
            .get_embedding_by_id(&root.id)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    symbol_id = %root.id,
                    symbol_name = %root.name,
                    error = %e,
                    "Embedding not found in LanceDB, falling back to text embedding"
                );
                // Fall back to embedding the symbol's text
                state.retriever.embed_text(&root.text).await?
            }
        };
        (vector, name.clone())
    } else if let Some(snippet) = &tool.code_snippet {
        // Embed the code snippet
        let vector = state.retriever.embed_text(snippet).await?;
        let desc = if snippet.len() > 50 {
            format!("{}...", &snippet[..50])
        } else {
            snippet.clone()
        };
        (vector, desc)
    } else {
        return Ok(json!({
            "error": "INVALID_INPUT",
            "message": "Either symbol_name or code_snippet must be provided",
            "results": [],
        }));
    };

    // Search LanceDB for similar vectors (fetch more for threshold filtering)
    let similar = state
        .retriever
        .get_vector_store()
        .search(&query_vector, limit * 2)
        .await?;

    // Filter by threshold and fetch symbol details (dedup by ID — LanceDB may
    // contain duplicate records if concurrent embedding generation raced).
    let mut seen_ids = std::collections::HashSet::new();
    let mut results = Vec::new();
    for hit in similar.into_iter().take(limit * 2) {
        if !seen_ids.insert(hit.id.clone()) {
            continue;
        }
        let distance = hit.distance.unwrap_or(1.0);
        let similarity = 1.0 / (1.0 + distance); // Convert distance to similarity

        if similarity < threshold {
            continue;
        }

        if let Some(row) = sqlite.get_symbol_by_id(&hit.id)? {
            results.push(json!({
                "symbol_id": row.id,
                "symbol_name": row.name,
                "kind": row.kind,
                "file_path": row.file_path,
                "language": row.language,
                "similarity": similarity,
                "exported": row.exported,
            }));
        }
    }

    // Sort by similarity descending and limit
    results.sort_by(|a, b| {
        let sa = a.get("similarity").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let sb = b.get("similarity").and_then(|v| v.as_f64()).unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    let budgeted_results = budget_array(results, limit);

    let mut response = json!({
        "query": query_description,
        "threshold": threshold,
        "count": budgeted_results.returned_count,
    });
    insert_budgeted_array(&mut response, "results", budgeted_results)?;
    if include_display {
        let results = response
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        response["display"] = json!(format_similar_results(
            &query_description,
            threshold,
            &results
        ));
    }
    Ok(response)
}

fn format_similar_results(query: &str, threshold: f32, results: &[serde_json::Value]) -> String {
    let mut out = format!(
        "# Similar Code Results\n\n**Query:** `{}`\n**Threshold:** {:.0}%\n\n",
        query,
        threshold * 100.0
    );

    if results.is_empty() {
        out.push_str("*No similar code found above threshold*\n");
        return out;
    }

    out.push_str("| Rank | Symbol | File | Kind | Similarity |\n");
    out.push_str("|------|--------|------|------|------------|\n");

    for (i, r) in results.iter().enumerate() {
        let name = r.get("symbol_name").and_then(|v| v.as_str()).unwrap_or("?");
        let file = r.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
        let file_short = file.split('/').next_back().unwrap_or(file);
        let kind = r.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
        let sim = r.get("similarity").and_then(|v| v.as_f64()).unwrap_or(0.0);

        out.push_str(&format!(
            "| {} | **{}** | {} | {} | {:.1}% |\n",
            i + 1,
            name,
            file_short,
            kind,
            sim * 100.0
        ));
    }

    out
}

fn format_scoring_breakdown(query: &str, results: &[serde_json::Value]) -> String {
    let mut out = format!("# Search Scoring Breakdown\n\n**Query:** `{}`\n\n", query);
    out.push_str("| Rank | Symbol | File | Score | Key | Vec | Pop | Learn |\n");
    out.push_str("|------|--------|------|-------|-----|-----|-----|-------|\n");

    for (i, r) in results.iter().enumerate() {
        let score = r.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let name = r.get("symbol_name").and_then(|v| v.as_str()).unwrap_or("?");
        let file = r.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
        let file_short = file.split('/').next_back().unwrap_or(file);

        let (kw, vec, pop, lrn) = if let Some(bd) = r.get("score_breakdown") {
            (
                bd.get("keyword_score")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                bd.get("vector_score")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                bd.get("popularity_boost")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                bd.get("learning_boost")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
            )
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        out.push_str(&format!(
            "| {} | **{}** | {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |\n",
            i + 1,
            name,
            file_short,
            score,
            kw,
            vec,
            pop,
            lrn
        ));
    }

    out.push_str("\n*Scores: keyword, vector, popularity, learning boosts*\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::{ContextMode, RankedHit};

    fn hit(id: &str) -> RankedHit {
        RankedHit {
            id: id.to_string(),
            score: 1.0,
            name: "x".to_string(),
            kind: "function".to_string(),
            file_path: "src/x.rs".to_string(),
            exported: true,
            language: "rust".to_string(),
        }
    }

    #[test]
    fn next_step_emitted_for_context_none_with_hits() {
        let hits = vec![hit("sym_a"), hit("sym_b")];
        let v = build_next_step(ContextMode::None, &hits).expect("Some");
        assert_eq!(v["tool"], "hydrate_symbols");
        let ids = v["args"]["ids"].as_array().expect("ids array");
        let ids_vec: Vec<&str> = ids.iter().map(|x| x.as_str().unwrap()).collect();
        assert_eq!(ids_vec, vec!["sym_a", "sym_b"]);
        let reason = v["reason"].as_str().expect("reason str");
        assert!(reason.contains("hydrate_symbols"));
        assert!(reason.to_lowercase().contains("grep"));
    }

    #[test]
    fn next_step_omitted_for_context_snippets() {
        let hits = vec![hit("sym_a")];
        assert!(build_next_step(ContextMode::Snippets, &hits).is_none());
    }

    #[test]
    fn next_step_omitted_for_context_full() {
        let hits = vec![hit("sym_a")];
        assert!(build_next_step(ContextMode::Full, &hits).is_none());
    }

    #[test]
    fn next_step_omitted_for_empty_hits() {
        let hits: Vec<RankedHit> = vec![];
        assert!(build_next_step(ContextMode::None, &hits).is_none());
    }

    #[test]
    fn next_step_dedups_repeated_ids_defensively() {
        let hits = vec![hit("sym_a"), hit("sym_a"), hit("sym_b")];
        let v = build_next_step(ContextMode::None, &hits).expect("Some");
        let ids: Vec<&str> = v["args"]["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["sym_a", "sym_b"]);
    }

    #[test]
    fn handle_search_code_response_contract_for_none_with_hits() {
        // We do not exercise the full handler here (it requires SQLite + Tantivy
        // + LanceDB). Instead we simulate the response-shaping path the handler
        // performs for `context: "none"` and verify that `build_next_step`'s
        // output is the value the handler will insert under the `next_step` key.
        let hits = vec![hit("sym_a")];
        let mut response = json!({
            "hits": [{"id": "sym_a", "name": "x", "file": "src/x.rs", "line": 1}],
            "hits_budget": {"total_count": 1, "returned_count": 1, "truncated": false},
            "context": ""
        });
        if let Some(map) = response.as_object_mut() {
            map.remove("context");
            if let Some(ns) = build_next_step(ContextMode::None, &hits) {
                map.insert("next_step".to_string(), ns);
            }
        }
        assert!(response.get("context").is_none());
        let ns = response.get("next_step").expect("next_step set");
        assert_eq!(ns["tool"], "hydrate_symbols");
        assert_eq!(ns["args"]["ids"][0], "sym_a");
    }

    #[test]
    fn handle_search_code_response_contract_for_snippets_omits_next_step() {
        let hits = vec![hit("sym_a")];
        let mut response = json!({"hits": [], "hits_budget": {}});
        if let Some(map) = response.as_object_mut() {
            if let Some(ns) = build_next_step(ContextMode::Snippets, &hits) {
                map.insert("next_step".to_string(), ns);
            }
        }
        assert!(response.get("next_step").is_none());
    }
}

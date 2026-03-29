//! Search-related MCP tool handlers

use crate::retrieval::Retriever;
use crate::tools::*;
use serde_json::json;

use super::AppState;

/// Handle search_code tool
pub async fn handle_search_code(
    retriever: &Retriever,
    tool: SearchCodeTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(5).max(1) as usize;
    let exported_only = tool.exported_only.unwrap_or(false);

    let result = retriever.search(&tool.query, limit, exported_only).await?;
    // Return only the SearchResponse (without hit_signals) to reduce response size
    Ok(serde_json::to_value(result.response)?)
}

/// Handle explain_search tool
pub async fn handle_explain_search(
    retriever: &Retriever,
    tool: ExplainSearchTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(10).max(1) as usize;
    let exported_only = tool.exported_only.unwrap_or(false);
    let verbose = tool.verbose.unwrap_or(false);

    let result = retriever.search(&tool.query, limit, exported_only).await?;
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

    // Build display field with markdown table
    let display = format_scoring_breakdown(&resp.query, &results);

    Ok(json!({
        "query": resp.query,
        "limit": resp.limit,
        "count": results.len(),
        "results": results,
        "display": display,
    }))
}

/// Handle find_similar_code tool - semantic similarity search via embeddings
pub async fn handle_find_similar_code(
    state: &AppState,
    tool: FindSimilarCodeTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(20).clamp(1, 100) as usize;
    let threshold = tool.threshold.unwrap_or(0.5);

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

    // Filter by threshold and fetch symbol details
    let mut results = Vec::new();
    for hit in similar.into_iter().take(limit * 2) {
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
    results.truncate(limit);

    // Build display
    let display = format_similar_results(&query_description, threshold, &results);

    Ok(json!({
        "query": query_description,
        "threshold": threshold,
        "count": results.len(),
        "results": results,
        "display": display,
    }))
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

//! MCP tool handlers

use crate::graph::{build_call_hierarchy, build_dependency_graph, build_type_graph};
use crate::retrieval::assembler::FormatMode;
use crate::retrieval::Retriever;
use crate::storage::sqlite::SqliteStore;
use crate::tools::*;
use rust_mcp_sdk::schema::{CallToolError, CallToolRequestParams};
use serde::de::DeserializeOwned;
use serde_json::json;

pub use state::AppState;

mod state;

/// Parse tool arguments from MCP request
pub fn parse_tool_args<T: DeserializeOwned>(
    params: &CallToolRequestParams,
) -> std::result::Result<T, CallToolError> {
    let args = params.arguments.clone().unwrap_or_default();
    let args = serde_json::Value::Object(args);
    serde_json::from_value(args)
        .map_err(|err| CallToolError::invalid_arguments(&params.name, Some(err.to_string())))
}

/// Convert internal error to MCP tool error
pub fn tool_internal_error(err: anyhow::Error) -> CallToolError {
    CallToolError::from_message(err.to_string())
}

/// Extract a line containing the symbol name from text
pub fn extract_usage_line(text: &str, symbol_name: &str) -> Option<String> {
    for line in text.lines() {
        if line.contains(symbol_name) {
            let mut s = line.trim().to_string();
            if s.len() > 200 {
                s.truncate(200);
            }
            return Some(s);
        }
    }
    None
}

/// Handle refresh_index tool
pub async fn handle_refresh_index(
    state: &AppState,
    tool: RefreshIndexTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let stats = if let Some(files) = tool.files {
        let paths = files
            .into_iter()
            .map(|p| {
                state
                    .config
                    .normalize_path_to_base(std::path::Path::new(&p))
            })
            .collect::<Result<Vec<_>, _>>()?;
        state.indexer.index_paths(&paths).await
    } else {
        state.indexer.index_all().await
    }?;

    Ok(json!({
        "ok": true,
        "stats": stats,
    }))
}

/// Handle search_code tool
pub async fn handle_search_code(
    retriever: &Retriever,
    tool: SearchCodeTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(5).max(1) as usize;
    let exported_only = tool.exported_only.unwrap_or(false);

    let resp = retriever.search(&tool.query, limit, exported_only).await?;
    Ok(serde_json::to_value(resp)?)
}

/// Handle get_definition tool
pub async fn handle_get_definition(
    state: &AppState,
    tool: GetDefinitionTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(10).max(1) as usize;

    let sqlite = SqliteStore::open(&state.config.db_path)?;
    sqlite.init()?;

    let rows = sqlite.search_symbols_by_exact_name(&tool.symbol_name, tool.file.as_deref(), limit)?;

    let context = state.retriever.assemble_definitions(&rows)?;

    Ok(json!({
        "symbol_name": tool.symbol_name,
        "count": rows.len(),
        "definitions": rows,
        "context": context,
    }))
}

/// Handle get_file_symbols tool
pub fn handle_get_file_symbols(
    db_path: &std::path::Path,
    tool: GetFileSymbolsTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let exported_only = tool.exported_only.unwrap_or(false);

    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    let rows = sqlite.list_symbol_headers_by_file(&tool.file_path, exported_only)?;

    Ok(json!({
        "file_path": tool.file_path,
        "count": rows.len(),
        "symbols": rows,
    }))
}

/// Handle get_index_stats tool
pub fn handle_get_index_stats(
    state: &AppState,
) -> Result<serde_json::Value, anyhow::Error> {
    let sqlite = SqliteStore::open(&state.config.db_path)?;
    sqlite.init()?;

    let symbols = sqlite.count_symbols()?;
    let edges = sqlite.count_edges()?;
    let last_updated = sqlite.most_recent_symbol_update()?;
    let latest_index_run = sqlite.latest_index_run()?;
    let latest_search_run = sqlite.latest_search_run()?;

    Ok(json!({
        "base_dir": state.config.base_dir,
        "symbols": symbols,
        "edges": edges,
        "last_updated_unix_s": last_updated,
        "latest_index_run": latest_index_run,
        "latest_search_run": latest_search_run,
    }))
}

/// Handle hydrate_symbols tool
pub fn handle_hydrate_symbols(
    state: &AppState,
    tool: HydrateSymbolsTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let sqlite = SqliteStore::open(&state.config.db_path)?;
    sqlite.init()?;

    let mut rows = Vec::new();
    let mut missing = Vec::new();
    for id in tool.ids {
        match sqlite.get_symbol_by_id(&id)? {
            Some(row) => rows.push(row),
            None => missing.push(id),
        }
    }

    let mode = match tool.mode.as_deref() {
        Some("full") => FormatMode::Full,
        _ => FormatMode::Default,
    };

    let assembler = crate::retrieval::assembler::ContextAssembler::new(state.config.clone());
    let (context, context_items) = assembler.format_context_with_mode(&sqlite, &rows, &[], &[], mode)?;

    Ok(json!({
        "count": rows.len(),
        "missing_ids": missing,
        "context": context,
        "context_items": context_items,
    }))
}

/// Handle explore_dependency_graph tool
pub fn handle_explore_dependency_graph(
    db_path: &std::path::Path,
    tool: ExploreDependencyGraphTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(2) as usize;
    let limit = tool.limit.unwrap_or(200).max(1) as usize;
    let direction = tool.direction.unwrap_or_else(|| "downstream".to_string());

    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    let roots = sqlite.search_symbols_by_exact_name(&tool.symbol_name, None, 10)?;
    let root = roots.first().cloned();

    let Some(root) = root else {
        return Ok(json!({
            "symbol_name": tool.symbol_name,
            "direction": direction,
            "depth": depth,
            "nodes": [],
            "edges": [],
        }));
    };

    let graph = build_dependency_graph(&sqlite, &root, &direction, depth, limit)?;
    Ok(graph)
}

/// Handle get_similarity_cluster tool
pub fn handle_get_similarity_cluster(
    db_path: &std::path::Path,
    tool: GetSimilarityClusterTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(20).max(1) as usize;

    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    let roots = sqlite.search_symbols_by_exact_name(&tool.symbol_name, None, 10)?;
    let root = roots.first().cloned();

    let Some(root) = root else {
        return Ok(json!({
            "symbol_name": tool.symbol_name,
            "cluster_key": null,
            "count": 0,
            "symbols": [],
        }));
    };

    let cluster_key = sqlite.get_similarity_cluster_key(&root.id)?;

    let mut out = Vec::new();
    if let Some(key) = cluster_key.clone() {
        let rows = sqlite.list_symbols_in_cluster(&key, limit + 1)?;
        for (id, name) in rows {
            if id == root.id {
                continue;
            }
            if out.len() >= limit {
                break;
            }
            out.push(json!({ "id": id, "name": name }));
        }
    }

    Ok(json!({
        "symbol_name": root.name,
        "cluster_key": cluster_key,
        "count": out.len(),
        "symbols": out,
    }))
}

/// Handle find_references tool
pub fn handle_find_references(
    db_path: &std::path::Path,
    tool: FindReferencesTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(200).max(1) as usize;
    let reference_type = tool.reference_type.unwrap_or_else(|| "all".to_string());

    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    let roots = sqlite.search_symbols_by_exact_name(&tool.symbol_name, None, 20)?;

    let mut out = Vec::new();
    for root in roots {
        if out.len() >= limit {
            break;
        }
        let edges = sqlite.list_edges_to(&root.id, limit * 3)?;
        for e in edges {
            if out.len() >= limit {
                break;
            }
            if reference_type != "all" && reference_type != e.edge_type {
                continue;
            }
            let from = sqlite.get_symbol_by_id(&e.from_symbol_id)?;
            out.push(json!({
                "to_symbol_id": e.to_symbol_id,
                "to_symbol_name": root.name,
                "from_symbol_id": e.from_symbol_id,
                "from_symbol_name": from.as_ref().map(|s| s.name.clone()).unwrap_or_default(),
                "reference_type": e.edge_type,
                "at_file": e.at_file,
                "at_line": e.at_line,
            }));
        }
    }

    Ok(json!({
        "symbol_name": tool.symbol_name,
        "reference_type": reference_type,
        "count": out.len(),
        "references": out,
    }))
}

/// Handle get_usage_examples tool
pub fn handle_get_usage_examples(
    db_path: &std::path::Path,
    tool: GetUsageExamplesTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(20).max(1) as usize;

    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    let roots = sqlite.search_symbols_by_exact_name(&tool.symbol_name, None, 20)?;

    let mut examples = Vec::new();
    for root in roots {
        if examples.len() >= limit {
            break;
        }
        let stored = sqlite.list_usage_examples_for_symbol(&root.id, limit * 4)?;

        if !stored.is_empty() {
            for ex in stored {
                if examples.len() >= limit {
                    break;
                }
                let from_symbol_name = ex
                    .from_symbol_id
                    .as_ref()
                    .and_then(|id| sqlite.get_symbol_by_id(id).ok().flatten())
                    .map(|s| s.name)
                    .unwrap_or_default();
                examples.push(json!({
                    "reference_type": ex.example_type,
                    "from_file_path": ex.file_path,
                    "from_symbol_name": from_symbol_name,
                    "at_file": ex.file_path,
                    "at_line": ex.line,
                    "snippet": ex.snippet,
                }));
            }
            continue;
        }

        let edges = sqlite.list_edges_to(&root.id, limit * 4)?;
        for e in edges {
            if examples.len() >= limit {
                break;
            }
            if e.edge_type != "call" && e.edge_type != "import" && e.edge_type != "reference" {
                continue;
            }
            let from = sqlite.get_symbol_by_id(&e.from_symbol_id)?;
            let Some(from) = from else {
                continue;
            };
            let snippet = extract_usage_line(&from.text, &root.name).unwrap_or_default();
            examples.push(json!({
                "reference_type": e.edge_type,
                "from_file_path": from.file_path,
                "from_symbol_name": from.name,
                "at_file": e.at_file,
                "at_line": e.at_line,
                "snippet": snippet,
            }));
        }
    }

    Ok(json!({
        "symbol_name": tool.symbol_name,
        "count": examples.len(),
        "examples": examples,
    }))
}

/// Handle get_call_hierarchy tool
pub fn handle_get_call_hierarchy(
    db_path: &std::path::Path,
    tool: GetCallHierarchyTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(2) as usize;
    let limit = tool.limit.unwrap_or(200).max(1) as usize;
    let direction = tool.direction.unwrap_or_else(|| "callees".to_string());

    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    let roots = sqlite.search_symbols_by_exact_name(&tool.symbol_name, None, 10)?;
    let root = roots.first().cloned();

    let Some(root) = root else {
        return Ok(json!({
            "symbol_name": tool.symbol_name,
            "direction": direction,
            "depth": depth,
            "nodes": [],
            "edges": [],
        }));
    };

    let graph = build_call_hierarchy(&sqlite, &root, &direction, depth, limit)?;
    Ok(graph)
}

/// Handle get_type_graph tool
pub fn handle_get_type_graph(
    db_path: &std::path::Path,
    tool: GetTypeGraphTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(2) as usize;
    let limit = tool.limit.unwrap_or(200).max(1) as usize;

    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    let roots = sqlite.search_symbols_by_exact_name(&tool.symbol_name, None, 10)?;
    let root = roots.first().cloned();

    let Some(root) = root else {
        return Ok(json!({
            "symbol_name": tool.symbol_name,
            "depth": depth,
            "nodes": [],
            "edges": [],
        }));
    };

    let graph = build_type_graph(&sqlite, &root, depth, limit)?;
    Ok(graph)
}

/// Handle report_selection tool
pub async fn handle_report_selection(
    db_path: &std::path::Path,
    tool: ReportSelectionTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    // Normalize query (reuse logic from retrieval/query.rs)
    let normalized = tool.query.to_lowercase().trim().to_string();

    let row_id = sqlite.insert_query_selection(
        &tool.query,
        &normalized,
        &tool.selected_symbol_id,
        tool.position,
    )?;

    Ok(json!({
        "ok": true,
        "recorded": true,
        "selection_id": row_id,
        "query_normalized": normalized,
    }))
}

/// Handle explain_search tool - returns detailed scoring breakdown
pub async fn handle_explain_search(
    retriever: &Retriever,
    tool: ExplainSearchTool,
) -> Result<serde_json::Value, anyhow::Error> {
    use crate::retrieval::HitSignals;

    let limit = tool.limit.unwrap_or(10).max(1) as usize;
    let exported_only = tool.exported_only.unwrap_or(false);
    let verbose = tool.verbose.unwrap_or(false);

    let resp = retriever.search(&tool.query, limit, exported_only).await?;

    // Build detailed breakdown with display formatting
    let mut results = Vec::new();
    for hit in &resp.hits {
        let signals = resp.hit_signals.get(&hit.id);
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
    use crate::storage::vector::LanceVectorTable;

    let limit = tool.limit.unwrap_or(20).max(1).min(100) as usize;
    let threshold = tool.threshold.unwrap_or(0.5);

    let sqlite = SqliteStore::open(&state.config.db_path)?;
    sqlite.init()?;

    // Determine search vector: either from symbol_name or code_snippet
    let (query_vector, query_description) = if let Some(name) = &tool.symbol_name {
        // Find symbol and get its embedding
        let roots = sqlite.search_symbols_by_exact_name(
            name,
            tool.file_path.as_deref(),
            1
        )?;
        let Some(root) = roots.first() else {
            return Ok(json!({
                "error": "SYMBOL_NOT_FOUND",
                "message": format!("Symbol '{}' not found", name),
                "results": [],
            }));
        };

        // Get embedding from LanceDB by symbol ID
        let vector = state.retriever.get_vector_store().get_embedding_by_id(&root.id).await?;
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
    let mut out = format!("# Similar Code Results\n\n**Query:** `{}`\n**Threshold:** {:.0}%\n\n", query, threshold * 100.0);

    if results.is_empty() {
        out.push_str("*No similar code found above threshold*\n");
        return out;
    }

    out.push_str("| Rank | Symbol | File | Kind | Similarity |\n");
    out.push_str("|------|--------|------|------|------------|\n");

    for (i, r) in results.iter().enumerate() {
        let name = r.get("symbol_name").and_then(|v| v.as_str()).unwrap_or("?");
        let file = r.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
        let file_short = file.split('/').last().unwrap_or(file);
        let kind = r.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
        let sim = r.get("similarity").and_then(|v| v.as_f64()).unwrap_or(0.0);

        out.push_str(&format!(
            "| {} | **{}** | {} | {} | {:.1}% |\n",
            i + 1, name, file_short, kind, sim * 100.0
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
        let file_short = file.split('/').last().unwrap_or(file);

        let (kw, vec, pop, lrn) = if let Some(bd) = r.get("score_breakdown") {
            (
                bd.get("keyword_score").and_then(|v| v.as_f64()).unwrap_or(0.0),
                bd.get("vector_score").and_then(|v| v.as_f64()).unwrap_or(0.0),
                bd.get("popularity_boost").and_then(|v| v.as_f64()).unwrap_or(0.0),
                bd.get("learning_boost").and_then(|v| v.as_f64()).unwrap_or(0.0),
            )
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        out.push_str(&format!(
            "| {} | **{}** | {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |\n",
            i + 1, name, file_short, score, kw, vec, pop, lrn
        ));
    }

    out.push_str("\n*Scores: keyword, vector, popularity, learning boosts*\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_line_extracts_and_trims() {
        let text = "line1\n   call alpha();   \nline3";
        let got = extract_usage_line(text, "alpha").unwrap();
        assert_eq!(got, "call alpha();");
    }
}

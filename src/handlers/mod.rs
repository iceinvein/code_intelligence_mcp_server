//! MCP tool handlers

use crate::graph::{build_call_hierarchy, build_dependency_graph, build_type_graph};
use crate::path::{PathError, PathNormalizer, Utf8PathBuf};
use crate::retrieval::assembler::FormatMode;
use crate::retrieval::Retriever;
use crate::storage::sqlite::{SqliteStore, SymbolRow};
use crate::tools::*;
use rust_mcp_sdk::schema::{CallToolError, CallToolRequestParams};
use serde::de::DeserializeOwned;
use serde_json::json;

pub use state::AppState;

mod state;

/// Type alias for data flow trace results
type DataFlowTraceResult = Result<
    (
        Vec<(String, String, Vec<String>)>,
        Vec<(String, String, Vec<String>)>,
    ),
    anyhow::Error,
>;

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
///
/// Logs the error before converting to MCP error format for observability.
/// Preserves PathError context for helpful error messages.
pub fn tool_internal_error(err: anyhow::Error) -> CallToolError {
    // Check for PathError in the error chain for better context
    let message = if let Some(path_err) = err.downcast_ref::<PathError>() {
        path_err.to_string()
    } else {
        err.to_string()
    };

    tracing::error!(
        error = %err,
        "Handler error: converting to MCP error"
    );
    CallToolError::from_message(message)
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
    let normalizer = PathNormalizer::new(state.config.base_dir.clone());

    let stats = if let Some(files) = tool.files {
        let paths = files
            .into_iter()
            .map(|p| {
                // Convert to Utf8Path and validate it's within base
                let path_buf = std::path::PathBuf::from(&p);
                let utf8_path = Utf8PathBuf::from_path_buf(path_buf.clone())
                    .map_err(|_| PathError::NonUtf8 { path: path_buf })?;

                // Validate path is within base directory
                normalizer.validate_within_base(&utf8_path)?;

                Ok(utf8_path)
            })
            .collect::<Result<Vec<_>, anyhow::Error>>()?;

        // Pass Utf8PathBuf slice directly to pipeline API
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

    let result = retriever.search(&tool.query, limit, exported_only).await?;
    // Return only the SearchResponse (without hit_signals) to reduce response size
    Ok(serde_json::to_value(result.response)?)
}

/// Handle get_definition tool
pub async fn handle_get_definition(
    state: &AppState,
    tool: GetDefinitionTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(10).max(1) as usize;

    let sqlite = &state.sqlite;

    let rows =
        sqlite.search_symbols_by_exact_name(&tool.symbol_name, tool.file.as_deref(), limit)?;

    let context = state.retriever.assemble_definitions(&rows)?;

    // Check if disambiguation is needed (multiple symbols with same name in different files)
    let unique_files: std::collections::HashSet<&str> =
        rows.iter().map(|r| r.file_path.as_str()).collect();
    let needs_disambiguation = unique_files.len() > 1 && tool.file.is_none();

    let mut response = json!({
        "symbol_name": tool.symbol_name,
        "count": rows.len(),
        "definitions": rows,
        "context": context,
    });

    // Add disambiguation hints when multiple symbols exist in different files
    if needs_disambiguation {
        let file_paths: Vec<&str> = unique_files.into_iter().collect();
        response["disambiguation"] = json!({
            "hint": format!(
                "Multiple '{}' symbols found in {} files. Use 'file' parameter to disambiguate.",
                tool.symbol_name,
                file_paths.len()
            ),
            "available_files": file_paths,
        });
    }

    Ok(response)
}

/// Handle get_file_symbols tool
pub fn handle_get_file_symbols(
    state: &AppState,
    tool: GetFileSymbolsTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let exported_only = tool.exported_only.unwrap_or(false);

    tracing::debug!(
        file_path = %tool.file_path,
        exported_only = exported_only,
        "get_file_symbols called"
    );

    // Create path normalizer for validation
    let normalizer = PathNormalizer::new(state.config.base_dir.clone());

    // Convert to Utf8Path and validate
    let path_buf = std::path::PathBuf::from(&tool.file_path);
    let utf8_path = Utf8PathBuf::from_path_buf(path_buf)
        .map_err(|_| PathError::NonUtf8 {
            path: std::path::PathBuf::from(&tool.file_path),
        })?;

    // Get relative path to base (for database lookup)
    let file_path_normalized = normalizer
        .relative_to_base(&utf8_path)
        .map(|p| p.to_string())
        .unwrap_or_else(|_| tool.file_path.clone());

    tracing::debug!(
        original_path = %tool.file_path,
        normalized_path = %file_path_normalized,
        "Normalized file path"
    );

    let sqlite = &state.sqlite;

    let rows = sqlite.list_symbol_headers_by_file(&file_path_normalized, exported_only)?;

    if rows.is_empty() {
        tracing::warn!(
            file_path = %tool.file_path,
            exported_only = exported_only,
            "get_file_symbols returned no results - file may not be indexed or path may be incorrect"
        );
    }

    Ok(json!({
        "file_path": tool.file_path,
        "file_path_normalized": file_path_normalized,
        "count": rows.len(),
        "symbols": rows,
    }))
}

/// Handle get_index_stats tool
pub fn handle_get_index_stats(state: &AppState) -> Result<serde_json::Value, anyhow::Error> {
    let sqlite = &state.sqlite;

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
    let sqlite = &state.sqlite;

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
    let (context, context_items) =
        assembler.format_context_with_mode(sqlite, &rows, &[], &[], mode, None)?;

    Ok(json!({
        "count": rows.len(),
        "missing_ids": missing,
        "context": context,
        "context_items": context_items,
    }))
}

/// Handle explore_dependency_graph tool
pub fn handle_explore_dependency_graph(
    state: &AppState,
    tool: ExploreDependencyGraphTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(2) as usize;
    let limit = tool.limit.unwrap_or(200).max(1) as usize;
    let direction = tool.direction.unwrap_or_else(|| "downstream".to_string());

    let sqlite = &state.sqlite;

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

    let graph = build_dependency_graph(sqlite, &root, &direction, depth, limit)?;
    Ok(graph)
}

/// Handle get_similarity_cluster tool
pub fn handle_get_similarity_cluster(
    state: &AppState,
    tool: GetSimilarityClusterTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(20).max(1) as usize;

    let sqlite = &state.sqlite;

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
    state: &AppState,
    tool: FindReferencesTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(200).max(1) as usize;
    let reference_type = tool.reference_type.unwrap_or_else(|| "all".to_string());

    let sqlite = &state.sqlite;

    // Use file parameter for disambiguation if provided
    let roots = sqlite.search_symbols_by_exact_name(&tool.symbol_name, tool.file.as_deref(), 20)?;

    // Check for disambiguation needs
    let unique_files: std::collections::HashSet<&str> =
        roots.iter().map(|r| r.file_path.as_str()).collect();
    let needs_disambiguation = unique_files.len() > 1 && tool.file.is_none();

    let mut out = Vec::new();
    for root in &roots {
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
                "to_symbol_file": root.file_path,
                "from_symbol_id": e.from_symbol_id,
                "from_symbol_name": from.as_ref().map(|s| s.name.clone()).unwrap_or_default(),
                "from_symbol_file": from.as_ref().map(|s| s.file_path.clone()).unwrap_or_default(),
                "reference_type": e.edge_type,
                "at_file": e.at_file,
                "at_line": e.at_line,
            }));
        }
    }

    let mut response = json!({
        "symbol_name": tool.symbol_name,
        "reference_type": reference_type,
        "count": out.len(),
        "references": out,
    });

    // Add disambiguation hints when multiple symbols exist in different files
    if needs_disambiguation {
        let file_paths: Vec<&str> = unique_files.into_iter().collect();
        response["disambiguation"] = json!({
            "hint": format!(
                "Multiple '{}' symbols found in {} files. Results include references to all. Use 'file' parameter to filter to a specific symbol.",
                tool.symbol_name,
                file_paths.len()
            ),
            "available_files": file_paths,
        });
    }

    Ok(response)
}

/// Handle get_usage_examples tool
pub fn handle_get_usage_examples(
    state: &AppState,
    tool: GetUsageExamplesTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(20).max(1) as usize;

    let sqlite = &state.sqlite;

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
    state: &AppState,
    tool: GetCallHierarchyTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(2) as usize;
    let limit = tool.limit.unwrap_or(200).max(1) as usize;
    let direction = tool.direction.unwrap_or_else(|| "callees".to_string());

    let sqlite = &state.sqlite;

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

    let graph = build_call_hierarchy(sqlite, &root, &direction, depth, limit)?;
    Ok(graph)
}

/// Handle get_type_graph tool
pub fn handle_get_type_graph(
    state: &AppState,
    tool: GetTypeGraphTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(2) as usize;
    let limit = tool.limit.unwrap_or(200).max(1) as usize;

    let sqlite = &state.sqlite;

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

    let graph = build_type_graph(sqlite, &root, depth, limit)?;
    Ok(graph)
}

/// Handle report_selection tool
pub async fn handle_report_selection(
    state: &AppState,
    tool: ReportSelectionTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let sqlite = &state.sqlite;

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

/// Handle trace_data_flow tool - trace variable reads/writes through the codebase
pub fn handle_trace_data_flow(
    state: &AppState,
    tool: TraceDataFlowTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(3) as usize;
    let limit = tool.limit.unwrap_or(50).max(1) as usize;
    let direction = tool.direction.unwrap_or_else(|| "both".to_string());

    let sqlite = &state.sqlite;

    // Find the root symbol
    let roots =
        sqlite.search_symbols_by_exact_name(&tool.symbol_name, tool.file_path.as_deref(), 1)?;
    let Some(root) = roots.first() else {
        return Ok(json!({
            "symbol_name": tool.symbol_name,
            "error": "SYMBOL_NOT_FOUND",
            "message": format!("Symbol '{}' not found", tool.symbol_name),
            "flows": [],
        }));
    };

    // Trace data flow using edge traversal
    let (reads, writes) = trace_data_flow_edges(sqlite, &root.id, depth, limit, &direction)?;

    // Build flow items
    let mut flows = Vec::new();
    for (sym_id, flow_type, path) in &reads {
        if let Some(sym) = sqlite.get_symbol_by_id(sym_id)? {
            flows.push(json!({
                "symbol_id": sym.id,
                "symbol_name": sym.name,
                "kind": sym.kind,
                "file_path": sym.file_path,
                "line": sym.start_line,
                "flow_type": flow_type,
                "path": path,
            }));
        }
    }
    for (sym_id, flow_type, path) in &writes {
        if let Some(sym) = sqlite.get_symbol_by_id(sym_id)? {
            flows.push(json!({
                "symbol_id": sym.id,
                "symbol_name": sym.name,
                "kind": sym.kind,
                "file_path": sym.file_path,
                "line": sym.start_line,
                "flow_type": flow_type,
                "path": path,
            }));
        }
    }

    // Sort: writes first, then reads, each by file path
    flows.sort_by(|a, b| {
        let fa = a.get("flow_type").and_then(|v| v.as_str()).unwrap_or("");
        let fb = b.get("flow_type").and_then(|v| v.as_str()).unwrap_or("");
        match (fa, fb) {
            ("write", "read") => std::cmp::Ordering::Less,
            ("read", "write") => std::cmp::Ordering::Greater,
            _ => {
                let fa_path = a.get("file_path").and_then(|v| v.as_str());
                let fb_path = b.get("file_path").and_then(|v| v.as_str());
                fa_path.cmp(&fb_path)
            }
        }
    });
    flows.truncate(limit);

    // Build display
    let display = format_data_flow(root, &flows);

    Ok(json!({
        "symbol_name": root.name,
        "symbol_kind": root.kind,
        "file_path": root.file_path,
        "direction": direction,
        "depth": depth,
        "read_count": reads.len(),
        "write_count": writes.len(),
        "flows": flows,
        "display": display,
    }))
}

fn trace_data_flow_edges(
    sqlite: &SqliteStore,
    root_id: &str,
    depth: usize,
    limit: usize,
    direction: &str,
) -> DataFlowTraceResult {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut queue = Vec::new();

    // Start with root symbol
    queue.push((root_id.to_string(), vec![]));
    visited.insert(root_id.to_string());

    for _level in 0..depth {
        if reads.len() + writes.len() >= limit {
            break;
        }
        let mut next_queue = Vec::new();

        for (current_id, path) in queue.drain(..) {
            // Get outgoing edges
            let outgoing = sqlite.list_edges_from(&current_id, limit)?;

            for edge in outgoing {
                if reads.len() + writes.len() >= limit {
                    break;
                }

                // Infer data flow from edge type
                let flow_type = match edge.edge_type.as_str() {
                    "call" => "read",
                    "reference" => "read",
                    "extends" | "implements" => "read",
                    _ => continue,
                };

                let match_direction = match direction {
                    "reads" => flow_type == "read",
                    "writes" => flow_type == "write",
                    _ => true,
                };

                if !match_direction {
                    continue;
                }

                if visited.insert(edge.to_symbol_id.clone()) {
                    let mut new_path = path.clone();
                    new_path.push(edge.to_symbol_id.clone());

                    let target = if flow_type == "read" {
                        &mut reads
                    } else {
                        &mut writes
                    };
                    target.push((
                        edge.to_symbol_id.clone(),
                        flow_type.to_string(),
                        new_path.clone(),
                    ));
                    next_queue.push((edge.to_symbol_id, new_path));
                }
            }
        }
        queue = next_queue;
    }

    Ok((reads, writes))
}

fn format_data_flow(root: &SymbolRow, flows: &[serde_json::Value]) -> String {
    let mut out = format!("# Data Flow Trace: {}\n\n", root.name);
    out.push_str(&format!("**Kind:** {}\n", root.kind));
    out.push_str(&format!("**File:** `{}`\n\n", root.file_path));

    let read_count = flows
        .iter()
        .filter(|f| f.get("flow_type").and_then(|v| v.as_str()) == Some("read"))
        .count();
    let write_count = flows
        .iter()
        .filter(|f| f.get("flow_type").and_then(|v| v.as_str()) == Some("write"))
        .count();

    out.push_str(&format!(
        "**Reads:** {} | **Writes:** {}\n\n",
        read_count, write_count
    ));

    if flows.is_empty() {
        out.push_str("*No data flow found*\n");
        return out;
    }

    out.push_str("## Flow\n\n");
    for (i, flow) in flows.iter().enumerate() {
        let name = flow
            .get("symbol_name")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let kind = flow.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let file = flow.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let file_short = file.split('/').next_back().unwrap_or(file);
        let flow_type = flow.get("flow_type").and_then(|v| v.as_str()).unwrap_or("");

        let icon = match flow_type {
            "write" => "[WRITE]",
            "read" => "[READ]",
            _ => "[?]",
        };

        out.push_str(&format!(
            "{}. {} **{}** ({})\n   - {}:{}\n",
            i + 1,
            icon,
            name,
            kind,
            file_short,
            flow.get("line").and_then(|v| v.as_i64()).unwrap_or(0)
        ));
        out.push('\n');
    }

    out
}

/// Handle get_module_summary tool - list exported symbols with signatures
pub fn handle_get_module_summary(
    state: &AppState,
    tool: GetModuleSummaryTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let group_by_kind = tool.group_by_kind.unwrap_or(false);

    tracing::debug!(
        file_path = %tool.file_path,
        group_by_kind = group_by_kind,
        "get_module_summary called"
    );

    // Create path normalizer for validation
    let normalizer = PathNormalizer::new(state.config.base_dir.clone());

    // Convert to Utf8Path and validate
    let path_buf = std::path::PathBuf::from(&tool.file_path);
    let utf8_path = Utf8PathBuf::from_path_buf(path_buf)
        .map_err(|_| PathError::NonUtf8 {
            path: std::path::PathBuf::from(&tool.file_path),
        })?;

    // Get relative path to base (for database lookup)
    let file_path_normalized = normalizer
        .relative_to_base(&utf8_path)
        .map(|p| p.to_string())
        .unwrap_or_else(|_| tool.file_path.clone());

    tracing::debug!(
        original_path = %tool.file_path,
        normalized_path = %file_path_normalized,
        "Normalized file path"
    );

    let sqlite = &state.sqlite;

    // Get exported symbols only
    let symbols = sqlite.list_symbol_headers_by_file(&file_path_normalized, true)?;

    if symbols.is_empty() {
        tracing::warn!(
            file_path = %tool.file_path,
            normalized_path = %file_path_normalized,
            "get_module_summary returned no exports - file may not be indexed or path may be incorrect"
        );

        return Ok(json!({
            "file_path": tool.file_path,
            "file_path_normalized": file_path_normalized,
            "error": "NO_EXPORTS",
            "message": format!("No exported symbols found for '{}'", tool.file_path),
            "exports": [],
            "groups": [],
        }));
    }

    // Build export list with signatures
    let mut exports = Vec::new();
    for sym in &symbols {
        // Get full symbol for signature extraction
        if let Some(full) = sqlite.get_symbol_by_id(&sym.id)? {
            let sig = extract_signature(&full.text, &full.kind);
            exports.push(json!({
                "id": full.id,
                "name": full.name,
                "kind": full.kind,
                "signature": sig,
                "line": full.start_line,
                "language": full.language,
            }));
        }
    }

    // Group by kind if requested
    let groups = if group_by_kind {
        let mut grouped: std::collections::HashMap<String, Vec<serde_json::Value>> =
            std::collections::HashMap::new();
        for exp in &exports {
            let kind = exp
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or("unknown");
            grouped
                .entry(kind.to_string())
                .or_default()
                .push(exp.clone());
        }
        let mut group_vec: Vec<serde_json::Value> = grouped
            .into_iter()
            .map(|(k, v)| json!({ "kind": k, "exports": v, "count": v.len() }))
            .collect();
        group_vec.sort_by(|a, b| {
            let ka = a.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            let kb = b.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            ka.cmp(kb)
        });
        group_vec
    } else {
        vec![]
    };

    // Build display
    let display = format_module_summary(&tool.file_path, &exports, &groups);

    Ok(json!({
        "file_path": tool.file_path,
        "file_path_normalized": file_path_normalized,
        "export_count": exports.len(),
        "exports": exports,
        "groups": groups,
        "display": display,
    }))
}

/// Extract a clean signature from symbol text
fn extract_signature(text: &str, kind: &str) -> String {
    // Take first few lines, up to a reasonable length
    let mut sig_lines = Vec::new();
    let max_lines = match kind {
        "class" | "interface" | "struct" => 3,
        "function" | "method" => 2,
        _ => 1,
    };

    for (i, line) in text.lines().enumerate() {
        if i >= max_lines {
            break;
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            sig_lines.push(trimmed.to_string());
        }
    }

    let sig = sig_lines.join(" ");
    // Limit signature length
    if sig.len() > 200 {
        format!("{}...", &sig[..200])
    } else {
        sig
    }
}

/// Format module summary as markdown
fn format_module_summary(
    file_path: &str,
    exports: &[serde_json::Value],
    groups: &[serde_json::Value],
) -> String {
    let file_name = file_path.split('/').next_back().unwrap_or(file_path);
    let mut out = format!("# Module Summary: {}\n\n", file_name);
    out.push_str(&format!("**Exports:** {}\n\n", exports.len()));

    if !groups.is_empty() {
        // Grouped display
        for g in groups {
            let kind = g.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            let count = g.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
            out.push_str(&format!("## {} ({})\n\n", kind, count));

            if let Some(arr) = g.get("exports").and_then(|v| v.as_array()) {
                for exp in arr.iter().take(50) {
                    let name = exp.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                    let sig = exp.get("signature").and_then(|s| s.as_str()).unwrap_or("");
                    out.push_str(&format!("- `{}`: {}\n", name, sig));
                }
            }
            out.push('\n');
        }
    } else {
        // Flat display
        out.push_str("## Exports\n\n");
        for exp in exports.iter().take(50) {
            let name = exp.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let kind = exp.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            let sig = exp.get("signature").and_then(|s| s.as_str()).unwrap_or("");
            out.push_str(&format!("- **{}** ({})\n  - `{}`\n", name, kind, sig));
        }
    }

    if exports.len() > 50 {
        out.push_str(&format!(
            "\n*... and {} more exports*\n",
            exports.len() - 50
        ));
    }

    out
}

/// Handle summarize_file tool - generate file-level summary with symbol counts and purpose inference
pub fn handle_summarize_file(
    state: &AppState,
    tool: SummarizeFileTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let include_signatures = tool.include_signatures.unwrap_or(false);
    let verbose = tool.verbose.unwrap_or(false);

    let sqlite = &state.sqlite;

    // Get all symbols in file
    let symbols = sqlite.list_symbols_by_file(&tool.file_path)?;

    if symbols.is_empty() {
        return Ok(json!({
            "file_path": tool.file_path,
            "error": "FILE_NOT_FOUND",
            "message": format!("No indexed symbols found for '{}'", tool.file_path),
            "summary": null,
        }));
    }

    // Count by kind
    let mut counts_by_kind = std::collections::HashMap::new();
    for sym in &symbols {
        *counts_by_kind.entry(sym.kind.clone()).or_insert(0) += 1;
    }

    // Count exports
    let export_count = symbols.iter().filter(|s| s.exported).count();
    let internal_count = symbols.len() - export_count;

    // Detect language
    let language = symbols
        .first()
        .map(|s| s.language.clone())
        .unwrap_or_default();

    // Build export list if include_signatures
    let exports = if include_signatures {
        symbols
            .iter()
            .filter(|s| s.exported || verbose)
            .map(|s| {
                let sig = extract_signature_for_summary(&s.text, &s.kind);
                json!({
                    "name": s.name,
                    "kind": s.kind,
                    "exported": s.exported,
                    "signature": sig,
                    "line": s.start_line,
                })
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };

    // Detect file purpose
    let purpose = infer_file_purpose_for_summary(&symbols);

    // Build display
    let display = format_file_summary(
        &tool.file_path,
        &symbols,
        &counts_by_kind,
        export_count,
        &purpose,
    );

    Ok(json!({
        "file_path": tool.file_path,
        "language": language,
        "total_symbols": symbols.len(),
        "exported_symbols": export_count,
        "internal_symbols": internal_count,
        "counts_by_kind": counts_by_kind,
        "purpose": purpose,
        "exports": exports,
        "display": display,
    }))
}

/// Extract signature from symbol text for summarize_file
fn extract_signature_for_summary(text: &str, kind: &str) -> String {
    let first_line = text.lines().next().unwrap_or("");
    let sig = match kind {
        "function" | "method" => first_line
            .trim_start_matches("export ")
            .trim_start_matches("pub ")
            .trim()
            .to_string(),
        "class" | "interface" | "type" => first_line
            .trim_start_matches("export ")
            .trim_start_matches("pub ")
            .trim()
            .to_string(),
        _ => first_line.chars().take(100).collect::<String>(),
    };
    if sig.len() > 100 {
        format!("{}...", &sig[..97])
    } else {
        sig
    }
}

/// Infer file purpose from symbol composition
fn infer_file_purpose_for_summary(symbols: &[SymbolRow]) -> String {
    if symbols.is_empty() {
        return "Empty or unknown".to_string();
    }

    let kinds: std::collections::HashSet<_> = symbols.iter().map(|s| s.kind.as_str()).collect();
    let export_ratio = symbols.iter().filter(|s| s.exported).count() as f64 / symbols.len() as f64;

    let mut tags = Vec::new();

    if export_ratio > 0.8 {
        tags.push("module");
    } else if export_ratio > 0.0 {
        tags.push("mixed-exports");
    } else {
        tags.push("internal");
    }

    if kinds.contains("interface") || kinds.contains("type") {
        tags.push("type-defs");
    }
    if kinds.contains("function") || kinds.contains("method") {
        tags.push("functions");
    }
    if kinds.contains("class") {
        tags.push("classes");
    }

    tags.join(" | ")
}

/// Format file summary as markdown
fn format_file_summary(
    file_path: &str,
    symbols: &[SymbolRow],
    counts_by_kind: &std::collections::HashMap<String, usize>,
    export_count: usize,
    purpose: &str,
) -> String {
    let file_name = file_path.split('/').next_back().unwrap_or(file_path);
    let mut out = format!("# File Summary: {}\n\n", file_name);
    out.push_str(&format!("**Path:** `{}`\n", file_path));
    out.push_str(&format!("**Total Symbols:** {}\n", symbols.len()));
    out.push_str(&format!("**Exports:** {}\n", export_count));
    out.push_str(&format!("**Purpose:** {}\n\n", purpose));

    out.push_str("## Symbol Counts\n\n");
    let mut kinds: Vec<_> = counts_by_kind.iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(a.1));
    for (kind, count) in kinds {
        out.push_str(&format!("- **{}:** {}\n", kind, count));
    }

    if export_count > 0 && export_count < symbols.len() {
        out.push_str(&format!("\n## Top Exports ({})\n\n", export_count));
        for sym in symbols.iter().filter(|s| s.exported).take(10) {
            out.push_str(&format!("- `{}` ({})\n", sym.name, sym.kind));
        }
    }

    out
}

/// Handle find_affected_code tool - find code affected if a symbol changes
pub fn handle_find_affected_code(
    state: &AppState,
    tool: FindAffectedCodeTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(3) as usize;
    let limit = tool.limit.unwrap_or(100).max(1) as usize;
    let include_tests = tool.include_tests.unwrap_or(false);

    let sqlite = &state.sqlite;

    // Find the root symbol
    let roots =
        sqlite.search_symbols_by_exact_name(&tool.symbol_name, tool.file_path.as_deref(), 1)?;
    let Some(root) = roots.first() else {
        return Ok(json!({
            "symbol_name": tool.symbol_name,
            "error": "SYMBOL_NOT_FOUND",
            "message": format!("Symbol '{}' not found", tool.symbol_name),
            "affected": [],
        }));
    };

    // Use build_dependency_graph with "upstream" direction
    let graph_result = build_dependency_graph(sqlite, root, "upstream", depth, limit);

    let (affected, warning) = match graph_result {
        Ok(graph) => {
            let empty_nodes: Vec<serde_json::Value> = vec![];
            let nodes = graph
                .get("nodes")
                .and_then(|v| v.as_array())
                .unwrap_or(&empty_nodes);

            // Build affected list with impact info
            let mut affected_list = Vec::new();
            let mut file_counts: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();

            for node in nodes {
                let id = node.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if id == root.id {
                    continue;
                }

                let file_path = node.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                let exported = node
                    .get("exported")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Filter out tests if requested
                if !include_tests && is_test_file_for_affected(file_path) {
                    continue;
                }

                *file_counts.entry(file_path.to_string()).or_insert(0) += 1;

                affected_list.push(json!({
                    "symbol_id": id,
                    "symbol_name": node.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    "kind": node.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
                    "file_path": file_path,
                    "exported": exported,
                    "impact": if exported { "high" } else { "medium" },
                }));
            }

            // Sort by impact then by file
            affected_list.sort_by(|a, b| {
                let ia = a.get("impact").and_then(|v| v.as_str()).unwrap_or("");
                let ib = b.get("impact").and_then(|v| v.as_str()).unwrap_or("");
                match (ia, ib) {
                    ("high", "medium") => std::cmp::Ordering::Less,
                    ("medium", "high") => std::cmp::Ordering::Greater,
                    _ => {
                        let fa_path = a.get("file_path").and_then(|v| v.as_str());
                        let fb_path = b.get("file_path").and_then(|v| v.as_str());
                        fa_path.cmp(&fb_path)
                    }
                }
            });

            (affected_list, None)
        }
        Err(e) => (
            vec![],
            Some(format!("Could not complete full trace: {}", e)),
        ),
    };

    // Truncate to limit
    let affected = affected.into_iter().take(limit).collect::<Vec<_>>();

    // Build summary stats
    let affected_files = affected
        .iter()
        .map(|f| f.get("file_path").and_then(|v| v.as_str()).unwrap_or(""))
        .collect::<std::collections::HashSet<_>>()
        .len();

    // Build display
    let display = format_affected_code(root, &affected, affected_files);

    Ok(json!({
        "symbol_name": root.name,
        "symbol_kind": root.kind,
        "file_path": root.file_path,
        "depth": depth,
        "affected_count": affected.len(),
        "affected_files": affected_files,
        "affected": affected,
        "warning": warning,
        "display": display,
    }))
}

/// Check if a file path appears to be a test file
fn is_test_file_for_affected(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_test.ts")
        || lower.ends_with("_test.tsx")
        || lower.ends_with("_test.js")
        || lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.contains("/__tests__/")
        || lower.contains("/spec/")
}

/// Format affected code results as markdown
fn format_affected_code(
    root: &SymbolRow,
    affected: &[serde_json::Value],
    affected_files: usize,
) -> String {
    let mut out = format!("# Affected Code: {}\n\n", root.name);
    out.push_str(&format!("**Kind:** {}\n", root.kind));
    out.push_str(&format!("**File:** `{}`\n\n", root.file_path));

    out.push_str(&format!(
        "**Affected:** {} symbols in {} files\n\n",
        affected.len(),
        affected_files
    ));

    if affected.is_empty() {
        out.push_str("*No reverse dependencies found*\n");
        return out;
    }

    // Group by impact level
    let high_impact: Vec<_> = affected
        .iter()
        .filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("high"))
        .collect();
    let medium_impact: Vec<_> = affected
        .iter()
        .filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("medium"))
        .collect();

    if !high_impact.is_empty() {
        out.push_str("## [!] High Impact (Exported)\n\n");
        for a in high_impact.iter().take(20) {
            let name = a.get("symbol_name").and_then(|v| v.as_str()).unwrap_or("?");
            let kind = a.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let file = a.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let file_short = file.split('/').next_back().unwrap_or(file);
            out.push_str(&format!("- **{}** ({}) - `{}`\n", name, kind, file_short));
        }
        if high_impact.len() > 20 {
            out.push_str(&format!("*... and {} more*\n", high_impact.len() - 20));
        }
        out.push('\n');
    }

    if !medium_impact.is_empty() {
        out.push_str("## Medium Impact (Internal)\n\n");
        for a in medium_impact.iter().take(20) {
            let name = a.get("symbol_name").and_then(|v| v.as_str()).unwrap_or("?");
            let kind = a.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let file = a.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let file_short = file.split('/').next_back().unwrap_or(file);
            out.push_str(&format!("- **{}** ({}) - `{}`\n", name, kind, file_short));
        }
        if medium_impact.len() > 20 {
            out.push_str(&format!("*... and {} more*\n", medium_impact.len() - 20));
        }
    }

    out
}

/// Handle search_todos tool
pub fn handle_search_todos(
    state: &AppState,
    tool: SearchTodosTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(50).max(1) as usize;

    let sqlite = &state.sqlite;

    let todos = sqlite.search_todos(
        tool.query.as_deref(),
        tool.file_path.as_deref(),
        tool.kind.as_deref(),
        limit,
    )?;

    // Build display
    let display = format_todos(&todos);

    Ok(json!({
        "count": todos.len(),
        "todos": todos,
        "display": display,
    }))
}

/// Handle find_tests_for_symbol tool
pub fn handle_find_tests_for_symbol(
    state: &AppState,
    tool: FindTestsForSymbolTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let _limit = tool.limit.unwrap_or(20).max(1) as usize;

    let sqlite = &state.sqlite;

    // Find the symbol
    let roots =
        sqlite.search_symbols_by_exact_name(&tool.symbol_name, tool.file_path.as_deref(), 1)?;
    let Some(root) = roots.first() else {
        return Ok(json!({
            "symbol_name": tool.symbol_name,
            "error": "SYMBOL_NOT_FOUND",
            "message": format!("Symbol '{}' not found", tool.symbol_name),
            "test_files": [],
        }));
    };

    // Get test files for this symbol's source file
    let test_files = sqlite.get_tests_for_source(&root.file_path)?;

    // Get symbols with tests for more detail
    let symbols_with_tests = sqlite.get_symbols_with_tests(&root.file_path)?;

    // Build display
    let display = format_test_results(root, &test_files, &symbols_with_tests);

    Ok(json!({
        "symbol_name": root.name,
        "symbol_kind": root.kind,
        "source_file": root.file_path,
        "test_file_count": test_files.len(),
        "test_files": test_files,
        "symbols_with_tests": symbols_with_tests,
        "display": display,
    }))
}

fn format_todos(todos: &[crate::storage::sqlite::schema::TodoRow]) -> String {
    let mut out = String::from("# TODO Comments\n\n");

    if todos.is_empty() {
        out.push_str("*No TODOs found*\n");
        return out;
    }

    let mut by_file: std::collections::HashMap<&str, Vec<_>> = std::collections::HashMap::new();
    for todo in todos {
        by_file.entry(&todo.file_path).or_default().push(todo);
    }

    for (file, file_todos) in by_file {
        let file_name = file.split('/').next_back().unwrap_or(file);
        out.push_str(&format!("## {}\n\n", file_name));

        for todo in file_todos {
            let icon = match todo.kind.as_str() {
                "fixme" => "[FIXME]",
                _ => "[TODO]",
            };
            out.push_str(&format!(
                "{} {}:{} - {}\n",
                icon, file_name, todo.line, todo.text
            ));
        }
        out.push('\n');
    }

    out
}

fn format_test_results(
    symbol: &SymbolRow,
    test_files: &[String],
    symbols_with_tests: &[(String, String)],
) -> String {
    let mut out = format!("# Tests for: {}\n\n", symbol.name);
    out.push_str(&format!("**Kind:** {}\n", symbol.kind));
    out.push_str(&format!("**File:** `{}`\n\n", symbol.file_path));

    if test_files.is_empty() {
        out.push_str("*No test files found*\n");
        return out;
    }

    out.push_str(&format!("**Test Files:** {}\n\n", test_files.len()));

    out.push_str("## Test Files\n\n");
    for (i, test_file) in test_files.iter().enumerate() {
        let file_short = test_file.split('/').next_back().unwrap_or(test_file);
        out.push_str(&format!("{}. `{}`\n", i + 1, file_short));
    }

    if !symbols_with_tests.is_empty() {
        out.push_str("\n## Tested Symbols\n\n");
        for (symbol_name, test_path) in symbols_with_tests {
            let test_short = test_path.split('/').next_back().unwrap_or(test_path);
            out.push_str(&format!("- `{}` ({})\n", symbol_name, test_short));
        }
    }

    out
}

/// Handle search_decorators tool
pub fn handle_search_decorators(
    state: &AppState,
    tool: SearchDecoratorsTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(50).clamp(1, 500) as usize;

    let sqlite = &state.sqlite;

    let decorators = sqlite.search_decorators_by_name(
        tool.name.as_deref(),
        tool.decorator_type.as_deref(),
        limit,
    )?;

    let mut results = Vec::new();
    for dec in decorators {
        // Get symbol details for context
        let symbol = sqlite
            .get_symbol_by_id(&dec.symbol_id)?
            .ok_or_else(|| anyhow::anyhow!("Symbol not found: {}", dec.symbol_id))?;

        results.push(serde_json::json!({
            "symbol_id": dec.symbol_id,
            "symbol_name": symbol.name,
            "decorator_name": dec.name,
            "decorator_type": dec.decorator_type,
            "arguments": dec.arguments,
            "file_path": symbol.file_path,
            "line": dec.target_line,
            "language": symbol.language,
            "symbol_kind": symbol.kind,
        }));
    }

    // Build display
    let display = format_decorators(&results);

    Ok(serde_json::json!({
        "count": results.len(),
        "decorators": results,
        "display": display,
    }))
}

/// Format decorator search results as markdown
fn format_decorators(decorators: &[serde_json::Value]) -> String {
    let mut out = String::from("# Decorator Search Results\n\n");

    if decorators.is_empty() {
        out.push_str("*No decorators found*\n");
        return out;
    }

    // Group by decorator name
    let mut by_name: std::collections::HashMap<&str, Vec<_>> = std::collections::HashMap::new();
    for dec in decorators {
        let name = dec
            .get("decorator_name")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        by_name.entry(name).or_default().push(dec);
    }

    for (decorator_name, items) in by_name {
        out.push_str(&format!("## @{}\n\n", decorator_name));
        out.push_str(&format!("**Found:** {} times\n\n", items.len()));

        for dec in items.iter().take(20) {
            let symbol_name = dec
                .get("symbol_name")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let file_path = dec.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let file_short = file_path.split('/').next_back().unwrap_or(file_path);
            let line = dec.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
            let decorator_type = dec
                .get("decorator_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = dec.get("arguments").and_then(|v| v.as_str()).unwrap_or("");

            out.push_str(&format!(
                "- **{}** - `{}`:{}\n",
                symbol_name, file_short, line
            ));

            if !decorator_type.is_empty() {
                out.push_str(&format!("  - Type: `{}`\n", decorator_type));
            }
            if !arguments.is_empty() {
                let args_preview = if arguments.len() > 60 {
                    format!("{}...", &arguments[..60])
                } else {
                    arguments.to_string()
                };
                out.push_str(&format!("  - Args: `{}`\n", args_preview));
            }
        }
        out.push('\n');
    }

    out
}

/// Handle search_framework_patterns tool
pub fn handle_search_framework_patterns(
    state: &AppState,
    tool: SearchFrameworkPatternsTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(50).clamp(1, 500) as usize;

    let sqlite = &state.sqlite;

    let patterns = sqlite.search_framework_patterns(
        tool.framework.as_deref(),
        tool.kind.as_deref(),
        tool.http_method.as_deref(),
        tool.path.as_deref(),
        None, // name filter not exposed in tool yet
        None, // file_path filter not exposed in tool yet
        limit,
    )?;

    let mut results = Vec::new();
    for pattern in patterns {
        results.push(serde_json::json!({
            "id": pattern.id,
            "file_path": pattern.file_path,
            "line": pattern.line,
            "framework": pattern.framework,
            "kind": pattern.kind,
            "http_method": pattern.http_method,
            "path": pattern.path,
            "name": pattern.name,
            "handler": pattern.handler,
            "arguments": pattern.arguments,
            "parent_chain": pattern.parent_chain,
        }));
    }

    // Build display
    let display = format_framework_patterns(&results);

    Ok(serde_json::json!({
        "count": results.len(),
        "patterns": results,
        "display": display,
    }))
}

/// Format framework pattern search results as markdown
fn format_framework_patterns(patterns: &[serde_json::Value]) -> String {
    let mut out = String::from("# Framework Pattern Search Results\n\n");

    if patterns.is_empty() {
        out.push_str("*No framework patterns found*\n");
        return out;
    }

    // Group by framework and kind
    let mut by_framework: std::collections::HashMap<&str, Vec<_>> = std::collections::HashMap::new();
    for pattern in patterns {
        let framework = pattern
            .get("framework")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        by_framework.entry(framework).or_default().push(pattern);
    }

    for (framework, items) in by_framework {
        out.push_str(&format!("## {} Framework\n\n", framework));

        // Group by kind within framework
        let mut by_kind: std::collections::HashMap<&str, Vec<_>> = std::collections::HashMap::new();
        for item in items {
            let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown");
            by_kind.entry(kind).or_default().push(item);
        }

        for (kind, kind_items) in by_kind {
            out.push_str(&format!("### {} ({})\n\n", kind, kind_items.len()));

            for pattern in kind_items.iter().take(20) {
                let file_path = pattern.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                let file_short = file_path.split('/').next_back().unwrap_or(file_path);
                let line = pattern.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
                let http_method = pattern.get("http_method").and_then(|v| v.as_str());
                let path = pattern.get("path").and_then(|v| v.as_str());
                let name = pattern.get("name").and_then(|v| v.as_str());

                // Format based on pattern type
                let label = if let (Some(method), Some(route)) = (http_method, path) {
                    format!("{} {}", method, route)
                } else if let Some(n) = name {
                    n.to_string()
                } else if let Some(p) = path {
                    p.to_string()
                } else {
                    kind.to_string()
                };

                out.push_str(&format!("- **{}** - `{}`:{}\n", label, file_short, line));
            }
            out.push('\n');
        }
    }

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

    #[test]
    fn test_extract_signature_for_summary_function() {
        let text = "export async function fetchData() {\n  return true;\n}";
        let sig = extract_signature_for_summary(text, "function");
        assert_eq!(sig, "async function fetchData() {");
    }

    #[test]
    fn test_extract_signature_for_summary_class() {
        let text = "export class MyClass extends Base {\n  constructor() {}\n}";
        let sig = extract_signature_for_summary(text, "class");
        assert_eq!(sig, "class MyClass extends Base {");
    }

    #[test]
    fn test_extract_signature_for_summary_truncates() {
        let text = "export function this_is_a_very_long_function_name_that_exceeds_limit() {\n  return true;\n}";
        let sig = extract_signature_for_summary(text, "function");
        // First line is 82 chars, well under 100
        assert_eq!(
            sig,
            "function this_is_a_very_long_function_name_that_exceeds_limit() {"
        );
    }

    #[test]
    fn test_extract_signature_for_summary_long_line_truncates() {
        let long_sig = "pub fn ".to_string() + &"a".repeat(110) + "() {";
        let sig = extract_signature_for_summary(&long_sig, "function");
        assert!(sig.len() <= 101);
        assert!(sig.ends_with("..."));
    }

    #[test]
    fn test_infer_file_purpose_for_summary_module() {
        let symbols = vec![
            SymbolRow {
                id: "1".to_string(),
                file_path: "test.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "exportedFunc".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 10,
                start_line: 1,
                end_line: 2,
                text: "export function exportedFunc() {}".to_string(),
            },
            SymbolRow {
                id: "2".to_string(),
                file_path: "test.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "anotherExportedFunc".to_string(),
                exported: true,
                start_byte: 10,
                end_byte: 20,
                start_line: 2,
                end_line: 3,
                text: "export function anotherExportedFunc() {}".to_string(),
            },
        ];
        let purpose = infer_file_purpose_for_summary(&symbols);
        assert!(purpose.contains("module"));
        assert!(purpose.contains("functions"));
    }

    #[test]
    fn test_infer_file_purpose_for_summary_internal() {
        let symbols = vec![SymbolRow {
            id: "1".to_string(),
            file_path: "test.ts".to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: "internalFunc".to_string(),
            exported: false,
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            end_line: 2,
            text: "function internalFunc() {}".to_string(),
        }];
        let purpose = infer_file_purpose_for_summary(&symbols);
        assert!(purpose.contains("internal"));
    }

    #[test]
    fn test_infer_file_purpose_for_summary_classes() {
        let symbols = vec![SymbolRow {
            id: "1".to_string(),
            file_path: "test.ts".to_string(),
            language: "typescript".to_string(),
            kind: "class".to_string(),
            name: "MyClass".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            end_line: 2,
            text: "export class MyClass {}".to_string(),
        }];
        let purpose = infer_file_purpose_for_summary(&symbols);
        assert!(purpose.contains("module"));
        assert!(purpose.contains("classes"));
    }
}

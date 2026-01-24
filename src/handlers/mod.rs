//! MCP tool handlers

use crate::graph::{build_call_hierarchy, build_dependency_graph, build_type_graph};
use crate::retrieval::assembler::FormatMode;
use crate::retrieval::Retriever;
use crate::storage::sqlite::{SqliteStore, SymbolRow};
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

/// Handle trace_data_flow tool - trace variable reads/writes through the codebase
pub fn handle_trace_data_flow(
    db_path: &std::path::Path,
    tool: TraceDataFlowTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(3) as usize;
    let limit = tool.limit.unwrap_or(50).max(1) as usize;
    let direction = tool.direction.unwrap_or_else(|| "both".to_string());

    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    // Find the root symbol
    let roots = sqlite.search_symbols_by_exact_name(
        &tool.symbol_name,
        tool.file_path.as_deref(),
        1
    )?;
    let Some(root) = roots.first() else {
        return Ok(json!({
            "symbol_name": tool.symbol_name,
            "error": "SYMBOL_NOT_FOUND",
            "message": format!("Symbol '{}' not found", tool.symbol_name),
            "flows": [],
        }));
    };

    // Trace data flow using edge traversal
    let (reads, writes) = trace_data_flow_edges(&sqlite, &root.id, depth, limit, &direction)?;

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
    let display = format_data_flow(&root, &flows);

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
) -> Result<(Vec<(String, String, Vec<String>)>, Vec<(String, String, Vec<String>)>), anyhow::Error> {
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

                let match_direction = match direction.as_ref() {
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

                    let target = (flow_type == "read").then(|| &mut reads)
                        .unwrap_or(&mut writes);
                    target.push((edge.to_symbol_id.clone(), flow_type.to_string(), new_path.clone()));
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

    let read_count = flows.iter().filter(|f| f.get("flow_type").and_then(|v| v.as_str()) == Some("read")).count();
    let write_count = flows.iter().filter(|f| f.get("flow_type").and_then(|v| v.as_str()) == Some("write")).count();

    out.push_str(&format!("**Reads:** {} | **Writes:** {}\n\n", read_count, write_count));

    if flows.is_empty() {
        out.push_str("*No data flow found*\n");
        return out;
    }

    out.push_str("## Flow\n\n");
    for (i, flow) in flows.iter().enumerate() {
        let name = flow.get("symbol_name").and_then(|v| v.as_str()).unwrap_or("?");
        let kind = flow.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let file = flow.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let file_short = file.split('/').last().unwrap_or(file);
        let flow_type = flow.get("flow_type").and_then(|v| v.as_str()).unwrap_or("");

        let icon = match flow_type {
            "write" => "[WRITE]",
            "read" => "[READ]",
            _ => "[?]",
        };

        out.push_str(&format!(
            "{}. {} **{}** ({})\n   - {}:{}\n",
            i + 1, icon, name, kind, file_short,
            flow.get("line").and_then(|v| v.as_i64()).unwrap_or(0)
        ));
        out.push('\n');
    }

    out
}

/// Handle get_module_summary tool - list exported symbols with signatures
pub fn handle_get_module_summary(
    db_path: &std::path::Path,
    tool: GetModuleSummaryTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let group_by_kind = tool.group_by_kind.unwrap_or(false);

    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    // Get exported symbols only
    let symbols = sqlite.list_symbol_headers_by_file(&tool.file_path, true)?;

    if symbols.is_empty() {
        return Ok(json!({
            "file_path": tool.file_path,
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
            let kind = exp.get("kind").and_then(|k| k.as_str()).unwrap_or("unknown");
            grouped.entry(kind.to_string()).or_default().push(exp.clone());
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
    let file_name = file_path.split('/').last().unwrap_or(file_path);
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
        out.push_str(&format!("\n*... and {} more exports*\n", exports.len() - 50));
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
}

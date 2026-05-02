//! Graph-related MCP tool handlers
//!
//! Covers dependency graphs, call hierarchies, type graphs, similarity
//! clusters, and data-flow traces.

use super::AppState;
use crate::graph::{build_call_hierarchy, build_dependency_graph, build_type_graph};
use crate::storage::sqlite::{SqliteStore, SymbolRow};
use crate::tools::*;
use anyhow::Result;
use serde_json::json;

use super::budget::{budget_array, insert_budgeted_array, BudgetedArray};

/// Type alias for data flow trace results
type DataFlowTraceResult = Result<
    (
        Vec<(String, String, Vec<String>)>,
        Vec<(String, String, Vec<String>)>,
    ),
    anyhow::Error,
>;

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

    let graph = build_dependency_graph(sqlite, &root, &direction, depth, limit, None)?;
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
        // "__skipped__" is a sentinel for symbols that were deliberately not
        // embedded (file-kind, tiny private helpers, test symbols).  Listing
        // the cluster would return all such unrelated symbols.
        if key == "__skipped__" {
            return Ok(json!({
                "symbol_name": root.name,
                "cluster_key": null,
                "count": 0,
                "symbols": [],
            }));
        }
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
    let direction = tool.direction.as_deref().unwrap_or("both");

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
    let graph = build_type_graph(sqlite, &root, direction, depth, limit)?;
    Ok(graph)
}

/// Handle trace_data_flow tool - trace variable reads/writes through the codebase
pub fn handle_trace_data_flow(
    state: &AppState,
    tool: TraceDataFlowTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(3) as usize;
    let limit = tool.limit.unwrap_or(50).max(1) as usize;
    let direction = tool.direction.unwrap_or_else(|| "both".to_string());
    let include_display = tool.include_display.unwrap_or(false);

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
    let budgeted_flows = budget_array(flows, limit);
    let total_flow_count = budgeted_flows.total_count;
    let flows_truncated = budgeted_flows.truncated;
    let mut flows = budgeted_flows.items;

    // Inter-procedural expansion: for each call/async_call edge, get callee's data flow
    let inter_proc = tool.inter_procedural.unwrap_or(false);
    if inter_proc {
        for flow in &mut flows {
            let flow_type = flow.get("flow_type").and_then(|v| v.as_str()).unwrap_or("");
            if flow_type != "read" && flow_type != "async_read" {
                continue;
            }

            let sym_id = flow.get("symbol_id").and_then(|v| v.as_str()).unwrap_or("");
            let sym_kind = flow.get("kind").and_then(|v| v.as_str()).unwrap_or("");

            // Only expand functions/methods
            if sym_kind != "function" && sym_kind != "method" && sym_kind != "arrow_function" {
                continue;
            }

            // Get callee's direct data flow edges (limit 20)
            let callee_edges = sqlite.list_edges_from(sym_id, 20)?;
            let mut called_flows = Vec::new();

            for edge in callee_edges {
                let ft = match edge.edge_type.as_str() {
                    "reads" => "read",
                    "writes" => "write",
                    "call" => "read",
                    "async_call" => "async_read",
                    "spawn" => "spawn",
                    _ => continue,
                };

                if let Some(target) = sqlite.get_symbol_by_id(&edge.to_symbol_id)? {
                    called_flows.push(json!({
                        "symbol_name": target.name,
                        "symbol_id": target.id,
                        "kind": target.kind,
                        "flow_type": ft,
                        "file_path": target.file_path,
                        "line": target.start_line,
                    }));
                }
            }

            if !called_flows.is_empty() {
                flow.as_object_mut()
                    .unwrap()
                    .insert("called_flows".to_string(), json!(called_flows));
            }
        }
    }

    let read_count = flows
        .iter()
        .filter(|f| {
            let ft = f.get("flow_type").and_then(|v| v.as_str()).unwrap_or("");
            ft == "read" || ft == "async_read"
        })
        .count();
    let write_count = flows
        .iter()
        .filter(|f| f.get("flow_type").and_then(|v| v.as_str()) == Some("write"))
        .count();
    let spawn_count = flows
        .iter()
        .filter(|f| f.get("flow_type").and_then(|v| v.as_str()) == Some("spawn"))
        .count();
    let budgeted_flows = BudgetedArray {
        returned_count: flows.len(),
        items: flows,
        total_count: total_flow_count,
        truncated: flows_truncated,
    };
    let mut response = json!({
        "symbol_name": root.name,
        "symbol_kind": root.kind,
        "file_path": root.file_path,
        "direction": direction,
        "depth": depth,
        "read_count": read_count,
        "write_count": write_count,
        "spawn_count": spawn_count,
    });
    insert_budgeted_array(&mut response, "flows", budgeted_flows)?;
    if include_display {
        let flows = response
            .get("flows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        response["display"] = json!(format_data_flow(root, &flows));
    }
    Ok(response)
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
            // --- Outgoing edges: what does this symbol read/write/call? ---
            let outgoing = sqlite.list_edges_from(&current_id, limit)?;

            for edge in &outgoing {
                if reads.len() + writes.len() >= limit {
                    break;
                }

                // Map actual reads/writes edge types first; fall back to
                // call/reference as secondary data-flow signals.
                let flow_type = match edge.edge_type.as_str() {
                    "reads" => "read",
                    "writes" => "write",
                    "call" | "reference" => "read",
                    "async_call" => "async_read",
                    "spawn" => "spawn",
                    // "extends", "implements", "type", "alias" are structural, not data-flow
                    _ => continue,
                };

                let match_direction = match direction {
                    "reads" => flow_type == "read" || flow_type == "async_read",
                    "writes" => flow_type == "write",
                    _ => true,
                };

                if !match_direction {
                    continue;
                }

                if visited.insert(edge.to_symbol_id.clone()) {
                    let mut new_path = path.clone();
                    new_path.push(edge.to_symbol_id.clone());

                    let target = if flow_type == "write" {
                        &mut writes
                    } else {
                        &mut reads
                    };
                    target.push((
                        edge.to_symbol_id.clone(),
                        flow_type.to_string(),
                        new_path.clone(),
                    ));
                    next_queue.push((edge.to_symbol_id.clone(), new_path));
                }
            }

            // --- Incoming edges: who reads/writes/calls this symbol? ---
            let incoming = sqlite.list_edges_to(&current_id, limit)?;

            for edge in &incoming {
                if reads.len() + writes.len() >= limit {
                    break;
                }

                // For incoming edges the source is the actor performing the
                // read/write.  Skip "reference" to avoid noise (import
                // declarations, type aliases, etc.).
                let flow_type = match edge.edge_type.as_str() {
                    "reads" => "read",
                    "writes" => "write",
                    "call" => "read",
                    "async_call" => "async_read",
                    "spawn" => "spawn",
                    // "reference" skipped incoming to avoid noise from imports/type aliases
                    _ => continue,
                };

                let match_direction = match direction {
                    "reads" => flow_type == "read" || flow_type == "async_read",
                    "writes" => flow_type == "write",
                    _ => true,
                };

                if !match_direction {
                    continue;
                }

                // The node discovered is the *source* of the incoming edge.
                if visited.insert(edge.from_symbol_id.clone()) {
                    let mut new_path = path.clone();
                    new_path.push(edge.from_symbol_id.clone());

                    let target = if flow_type == "write" {
                        &mut writes
                    } else {
                        &mut reads
                    };
                    target.push((
                        edge.from_symbol_id.clone(),
                        flow_type.to_string(),
                        new_path.clone(),
                    ));
                    next_queue.push((edge.from_symbol_id.clone(), new_path));
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

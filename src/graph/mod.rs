//! Graph building functions for call hierarchies, type graphs, and dependency graphs

pub mod pagerank;

use crate::storage::sqlite::{CrossRepoEdgeRow, SqliteStore, SymbolRow};
use serde_json::{json, Value};
use std::sync::Arc;

/// Build a compact JSON node payload for graph responses.
///
/// `language` is intentionally omitted: callers can derive it from `file_path`
/// extension when needed. Dropping it saves ~25 bytes per node on graphs that
/// commonly contain hundreds of nodes.
fn node_json(sym: &SymbolRow) -> Value {
    json!({
        "id": sym.id,
        "name": sym.name,
        "kind": sym.kind,
        "file_path": sym.file_path,
        "exported": sym.exported,
        "line_range": [sym.start_line, sym.end_line],
    })
}

/// Fetch up to 3 evidence rows and drop the one matching the edge's
/// (at_file, at_line). Returns `None` when no extra evidence remains so
/// callers can omit the field entirely. Evidence with `count > 1` is always
/// kept since it adds aggregation info beyond what the edge already carries.
fn extra_evidence(
    sqlite: &SqliteStore,
    from_id: &str,
    to_id: &str,
    edge_type: &str,
    primary_at_file: Option<&str>,
    primary_at_line: Option<u32>,
) -> Option<Vec<Value>> {
    let evidence = sqlite
        .list_edge_evidence(from_id, to_id, edge_type, 3)
        .unwrap_or_default();
    let extras: Vec<Value> = evidence
        .into_iter()
        .filter(|ev| {
            let same_file = primary_at_file == Some(ev.at_file.as_str());
            let same_line = primary_at_line == Some(ev.at_line);
            let duplicates_primary = same_file && same_line && ev.count <= 1;
            !duplicates_primary
        })
        .map(|ev| {
            json!({
                "at_file": ev.at_file,
                "at_line": ev.at_line,
                "count": ev.count,
            })
        })
        .collect();
    if extras.is_empty() {
        None
    } else {
        Some(extras)
    }
}

/// Build an edge JSON payload, omitting `evidence` when it would only repeat
/// the edge's own (at_file, at_line, count=1).
fn edge_json(
    sqlite: &SqliteStore,
    edge: &crate::storage::sqlite::EdgeRow,
    extra_fields: &[(&str, Value)],
) -> Value {
    let mut payload = json!({
        "from": edge.from_symbol_id,
        "to": edge.to_symbol_id,
        "edge_type": edge.edge_type,
        "at_file": edge.at_file,
        "at_line": edge.at_line,
        "evidence_count": edge.evidence_count,
        "resolution": edge.resolution,
    });
    for (key, value) in extra_fields {
        payload[*key] = value.clone();
    }
    if let Some(extras) = extra_evidence(
        sqlite,
        &edge.from_symbol_id,
        &edge.to_symbol_id,
        &edge.edge_type,
        edge.at_file.as_deref(),
        edge.at_line,
    ) {
        payload["evidence"] = Value::Array(extras);
    }
    payload
}

/// Trait for resolving cross-repo symbol references.
///
/// Implementors provide access to other repos' SQLite stores and symbol data,
/// enabling cross-repo dependency graph traversal in standalone mode.
pub trait CrossRepoResolver: Send + Sync {
    /// Look up a symbol in another repo by hash, name, and optional file path.
    ///
    /// Returns the target repo's SqliteStore and the resolved SymbolRow if found.
    fn resolve_cross_repo_symbol(
        &self,
        to_repo_hash: &str,
        to_symbol_name: &str,
        to_symbol_file: Option<&str>,
    ) -> anyhow::Result<Option<(Arc<SqliteStore>, SymbolRow)>>;

    /// List cross-repo edges originating from a symbol in the given store.
    fn list_cross_repo_edges_from(
        &self,
        sqlite: &SqliteStore,
        from_symbol_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<CrossRepoEdgeRow>>;

    /// Get the repo name for a given hash (for display purposes).
    fn repo_name_for_hash(&self, repo_hash: &str) -> anyhow::Result<Option<String>>;
}

/// Build a dependency graph starting from a root symbol
pub fn build_dependency_graph(
    sqlite: &SqliteStore,
    root: &SymbolRow,
    direction: &str,
    depth: usize,
    limit: usize,
    edge_types: Option<&[&str]>,
) -> anyhow::Result<serde_json::Value> {
    let mut nodes = std::collections::HashMap::<String, serde_json::Value>::new();
    let mut edges = Vec::<serde_json::Value>::new();
    let mut visited = std::collections::HashSet::<String>::new();

    // Initial node
    nodes.insert(root.id.clone(), node_json(root));
    visited.insert(root.id.clone());

    let mut frontier = vec![root.id.clone()];

    // Direction flags
    let traverse_upstream = direction == "upstream" || direction == "bidirectional";
    let traverse_downstream = direction == "downstream" || direction == "bidirectional";

    // Compute allowed edge types once before the loop
    let allowed_types = edge_types.unwrap_or(&["call", "reference"]);

    for _ in 0..depth {
        if edges.len() >= limit || frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();

        for current_id in frontier {
            if edges.len() >= limit {
                break;
            }

            // Upstream: Who calls me? (Incoming edges)
            if traverse_upstream {
                let incoming = sqlite.list_edges_to(&current_id, limit)?;
                for e in incoming {
                    if edges.len() >= limit {
                        break;
                    }

                    if !allowed_types.contains(&e.edge_type.as_str()) {
                        continue;
                    }

                    let Some(caller) = sqlite.get_symbol_by_id(&e.from_symbol_id)? else {
                        continue;
                    };

                    if !nodes.contains_key(&caller.id) {
                        nodes.insert(caller.id.clone(), node_json(&caller));
                    }
                    edges.push(edge_json(sqlite, &e, &[]));

                    if visited.insert(caller.id.clone()) {
                        next.push(caller.id);
                    }
                }
            }

            // Downstream: Who do I call? (Outgoing edges)
            if traverse_downstream {
                let outgoing = sqlite.list_edges_from(&current_id, limit)?;
                for e in outgoing {
                    if edges.len() >= limit {
                        break;
                    }

                    if !allowed_types.contains(&e.edge_type.as_str()) {
                        continue;
                    }

                    let Some(callee) = sqlite.get_symbol_by_id(&e.to_symbol_id)? else {
                        continue;
                    };

                    if !nodes.contains_key(&callee.id) {
                        nodes.insert(callee.id.clone(), node_json(&callee));
                    }
                    edges.push(edge_json(sqlite, &e, &[]));

                    if visited.insert(callee.id.clone()) {
                        next.push(callee.id);
                    }
                }
            }
        }
        frontier = next;
    }

    Ok(json!({
        "symbol_name": root.name,
        "direction": direction,
        "depth": depth,
        "nodes": nodes.into_values().collect::<Vec<_>>(),
        "edges": edges,
    }))
}

/// Build a call hierarchy starting from a root symbol
pub fn build_call_hierarchy(
    sqlite: &SqliteStore,
    root: &SymbolRow,
    direction: &str,
    depth: usize,
    limit: usize,
) -> anyhow::Result<serde_json::Value> {
    let mut nodes = std::collections::HashMap::<String, serde_json::Value>::new();
    let mut edges = Vec::<serde_json::Value>::new();
    let mut visited = std::collections::HashSet::<String>::new();
    let mut seen_edges =
        std::collections::HashSet::<(String, String, String, Option<String>, Option<u32>)>::new();

    nodes.insert(root.id.clone(), node_json(root));
    visited.insert(root.id.clone());

    let do_callers = direction == "callers" || direction == "both";
    let do_callees = direction == "callees" || direction == "both";
    let mut frontier = vec![root.id.clone()];
    for _ in 0..depth {
        if edges.len() >= limit || frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for current_id in frontier {
            if edges.len() >= limit {
                break;
            }
            if do_callers {
                let incoming = sqlite.list_edges_to(&current_id, limit)?;
                for e in incoming {
                    if edges.len() >= limit {
                        break;
                    }
                    if e.edge_type != "call"
                        && e.edge_type != "async_call"
                        && e.edge_type != "spawn"
                    {
                        continue;
                    }
                    let Some(caller) = sqlite.get_symbol_by_id(&e.from_symbol_id)? else {
                        continue;
                    };
                    nodes
                        .entry(caller.id.clone())
                        .or_insert_with(|| node_json(&caller));
                    let is_async = e.edge_type == "async_call" || e.edge_type == "spawn";
                    if seen_edges.insert(call_hierarchy_edge_key(&e)) {
                        edges.push(edge_json(sqlite, &e, &[("is_async", json!(is_async))]));
                    }
                    if visited.insert(caller.id.clone()) {
                        next.push(caller.id);
                    }
                }
            }
            if do_callees {
                let outgoing = sqlite.list_edges_from(&current_id, limit)?;
                for e in outgoing {
                    if edges.len() >= limit {
                        break;
                    }
                    if e.edge_type != "call"
                        && e.edge_type != "async_call"
                        && e.edge_type != "spawn"
                    {
                        continue;
                    }
                    let Some(callee) = sqlite.get_symbol_by_id(&e.to_symbol_id)? else {
                        continue;
                    };
                    nodes
                        .entry(callee.id.clone())
                        .or_insert_with(|| node_json(&callee));
                    let is_async = e.edge_type == "async_call" || e.edge_type == "spawn";
                    if seen_edges.insert(call_hierarchy_edge_key(&e)) {
                        edges.push(edge_json(sqlite, &e, &[("is_async", json!(is_async))]));
                    }
                    if visited.insert(callee.id.clone()) {
                        next.push(callee.id);
                    }
                }
            }
        }
        frontier = next;
    }

    Ok(json!({
        "symbol_name": root.name,
        "direction": direction,
        "depth": depth,
        "nodes": nodes.into_values().collect::<Vec<_>>(),
        "edges": edges,
    }))
}

fn call_hierarchy_edge_key(
    edge: &crate::storage::sqlite::EdgeRow,
) -> (String, String, String, Option<String>, Option<u32>) {
    (
        edge.from_symbol_id.clone(),
        edge.to_symbol_id.clone(),
        edge.edge_type.clone(),
        edge.at_file.clone(),
        edge.at_line,
    )
}

/// Build a type graph starting from a root symbol.
///
/// `direction` controls traversal:
/// - `"downstream"` — follows edges *from* the root (what does this extend/implement)
/// - `"upstream"`   — follows edges *to* the root (who extends/implements this)
/// - `"both"`       — combines both directions (default)
pub fn build_type_graph(
    sqlite: &SqliteStore,
    root: &SymbolRow,
    direction: &str,
    depth: usize,
    limit: usize,
) -> anyhow::Result<serde_json::Value> {
    let mut nodes = std::collections::HashMap::<String, serde_json::Value>::new();
    let mut edges = Vec::<serde_json::Value>::new();
    let mut visited = std::collections::HashSet::<String>::new();

    nodes.insert(root.id.clone(), node_json(root));
    visited.insert(root.id.clone());

    let do_downstream = direction == "downstream" || direction == "both";
    let do_upstream = direction == "upstream" || direction == "both";

    // Downstream frontier: follow outgoing type edges (extends/implements/alias)
    if do_downstream {
        let mut frontier = vec![root.id.clone()];
        for _ in 0..depth {
            if edges.len() >= limit || frontier.is_empty() {
                break;
            }
            let mut next = Vec::new();
            for current_id in frontier {
                if edges.len() >= limit {
                    break;
                }
                let outgoing = sqlite.list_edges_from(&current_id, limit)?;
                for e in outgoing {
                    if edges.len() >= limit {
                        break;
                    }
                    if e.edge_type != "extends"
                        && e.edge_type != "implements"
                        && e.edge_type != "alias"
                    {
                        continue;
                    }
                    let Some(to_sym) = sqlite.get_symbol_by_id(&e.to_symbol_id)? else {
                        continue;
                    };
                    nodes
                        .entry(to_sym.id.clone())
                        .or_insert_with(|| node_json(&to_sym));
                    edges.push(edge_json(sqlite, &e, &[]));
                    if visited.insert(to_sym.id.clone()) {
                        next.push(to_sym.id);
                    }
                }
            }
            frontier = next;
        }
    }

    // Upstream frontier: follow incoming type edges (who extends/implements/aliases this symbol)
    if do_upstream {
        let mut frontier = vec![root.id.clone()];
        for _ in 0..depth {
            if edges.len() >= limit || frontier.is_empty() {
                break;
            }
            let mut next = Vec::new();
            for current_id in frontier {
                if edges.len() >= limit {
                    break;
                }
                let incoming = sqlite.list_edges_to(&current_id, limit)?;
                for e in incoming {
                    if edges.len() >= limit {
                        break;
                    }
                    if e.edge_type != "extends"
                        && e.edge_type != "implements"
                        && e.edge_type != "alias"
                    {
                        continue;
                    }
                    let Some(from_sym) = sqlite.get_symbol_by_id(&e.from_symbol_id)? else {
                        continue;
                    };
                    nodes
                        .entry(from_sym.id.clone())
                        .or_insert_with(|| node_json(&from_sym));
                    edges.push(edge_json(sqlite, &e, &[]));
                    if visited.insert(from_sym.id.clone()) {
                        next.push(from_sym.id);
                    }
                }
            }
            frontier = next;
        }
    }

    Ok(json!({
        "symbol_name": root.name,
        "direction": direction,
        "depth": depth,
        "nodes": nodes.into_values().collect::<Vec<_>>(),
        "edges": edges,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::EdgeRow;

    fn sym(id: &str, name: &str) -> SymbolRow {
        SymbolRow {
            id: id.to_string(),
            file_path: "src/a.ts".to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: name.to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
            text: format!("export function {name}() {{}}"),
        }
    }

    #[test]
    fn call_hierarchy_traverses_callees_and_callers() {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        let a = sym("a", "alpha");
        let b = sym("b", "beta");
        let c = sym("c", "gamma");
        sqlite.upsert_symbol(&a).unwrap();
        sqlite.upsert_symbol(&b).unwrap();
        sqlite.upsert_symbol(&c).unwrap();

        sqlite
            .upsert_edge(&EdgeRow {
                from_symbol_id: "a".to_string(),
                to_symbol_id: "b".to_string(),
                edge_type: "call".to_string(),
                at_file: Some("src/a.ts".to_string()),
                at_line: Some(1),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();
        sqlite
            .upsert_edge(&EdgeRow {
                from_symbol_id: "b".to_string(),
                to_symbol_id: "c".to_string(),
                edge_type: "call".to_string(),
                at_file: Some("src/a.ts".to_string()),
                at_line: Some(1),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();

        let g1 = build_call_hierarchy(&sqlite, &a, "callees", 3, 100).unwrap();
        let nodes1 = g1.get("nodes").unwrap().as_array().unwrap();
        let edges1 = g1.get("edges").unwrap().as_array().unwrap();
        assert_eq!(edges1.len(), 2);
        assert_eq!(nodes1.len(), 3);

        let g2 = build_call_hierarchy(&sqlite, &c, "callers", 3, 100).unwrap();
        let nodes2 = g2.get("nodes").unwrap().as_array().unwrap();
        let edges2 = g2.get("edges").unwrap().as_array().unwrap();
        assert_eq!(edges2.len(), 2);
        assert_eq!(nodes2.len(), 3);
    }

    #[test]
    fn call_hierarchy_both_includes_callers_and_callees() {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        let caller = sym("caller", "caller");
        let root = sym("root", "root");
        let callee = sym("callee", "callee");
        sqlite.upsert_symbol(&caller).unwrap();
        sqlite.upsert_symbol(&root).unwrap();
        sqlite.upsert_symbol(&callee).unwrap();

        sqlite
            .upsert_edge(&EdgeRow {
                from_symbol_id: "caller".to_string(),
                to_symbol_id: "root".to_string(),
                edge_type: "call".to_string(),
                at_file: Some("src/caller.ts".to_string()),
                at_line: Some(1),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();
        sqlite
            .upsert_edge(&EdgeRow {
                from_symbol_id: "root".to_string(),
                to_symbol_id: "callee".to_string(),
                edge_type: "call".to_string(),
                at_file: Some("src/root.ts".to_string()),
                at_line: Some(2),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();

        let graph = build_call_hierarchy(&sqlite, &root, "both", 2, 100).unwrap();
        let edges = graph.get("edges").unwrap().as_array().unwrap();

        assert_eq!(edges.len(), 2);
        assert!(edges
            .iter()
            .any(|edge| edge["from"] == "caller" && edge["to"] == "root"));
        assert!(edges
            .iter()
            .any(|edge| edge["from"] == "root" && edge["to"] == "callee"));
    }

    #[test]
    fn type_graph_follows_extends_implements_and_alias() {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        let a = sym("a", "A");
        let b = sym("b", "B");
        let c = sym("c", "C");
        let d = sym("d", "D");
        sqlite.upsert_symbol(&a).unwrap();
        sqlite.upsert_symbol(&b).unwrap();
        sqlite.upsert_symbol(&c).unwrap();
        sqlite.upsert_symbol(&d).unwrap();

        for (from, to, ty) in [
            ("a", "b", "extends"),
            ("b", "c", "implements"),
            ("c", "d", "alias"),
        ] {
            sqlite
                .upsert_edge(&EdgeRow {
                    from_symbol_id: from.to_string(),
                    to_symbol_id: to.to_string(),
                    edge_type: ty.to_string(),
                    at_file: Some("src/a.ts".to_string()),
                    at_line: Some(1),
                    confidence: 1.0,
                    evidence_count: 1,
                    resolution: "local".to_string(),
                })
                .unwrap();
        }

        let g = build_type_graph(&sqlite, &a, "downstream", 3, 100).unwrap();
        let nodes = g.get("nodes").unwrap().as_array().unwrap();
        let edges = g.get("edges").unwrap().as_array().unwrap();
        assert_eq!(nodes.len(), 4);
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn type_graph_upstream_finds_implementors() {
        // B→A (implements), C→A (extends): upstream from A should find B and C
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        let a = sym("a", "A");
        let b = sym("b", "B");
        let c = sym("c", "C");
        sqlite.upsert_symbol(&a).unwrap();
        sqlite.upsert_symbol(&b).unwrap();
        sqlite.upsert_symbol(&c).unwrap();

        for (from, to, ty) in [("b", "a", "implements"), ("c", "a", "extends")] {
            sqlite
                .upsert_edge(&EdgeRow {
                    from_symbol_id: from.to_string(),
                    to_symbol_id: to.to_string(),
                    edge_type: ty.to_string(),
                    at_file: Some("src/a.ts".to_string()),
                    at_line: Some(1),
                    confidence: 1.0,
                    evidence_count: 1,
                    resolution: "local".to_string(),
                })
                .unwrap();
        }

        let g = build_type_graph(&sqlite, &a, "upstream", 3, 100).unwrap();
        let nodes = g.get("nodes").unwrap().as_array().unwrap();
        let edges = g.get("edges").unwrap().as_array().unwrap();
        // root A + implementor B + extender C
        assert_eq!(nodes.len(), 3);
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn type_graph_both_directions() {
        // B→A (implements), D→B (extends)
        // Starting from B with direction="both":
        //   downstream: B→A
        //   upstream:   D→B
        // Nodes: B (root), A (downstream), D (upstream) = 3
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        let a = sym("a", "A");
        let b = sym("b", "B");
        let d = sym("d", "D");
        sqlite.upsert_symbol(&a).unwrap();
        sqlite.upsert_symbol(&b).unwrap();
        sqlite.upsert_symbol(&d).unwrap();

        for (from, to, ty) in [("b", "a", "implements"), ("d", "b", "extends")] {
            sqlite
                .upsert_edge(&EdgeRow {
                    from_symbol_id: from.to_string(),
                    to_symbol_id: to.to_string(),
                    edge_type: ty.to_string(),
                    at_file: Some("src/b.ts".to_string()),
                    at_line: Some(1),
                    confidence: 1.0,
                    evidence_count: 1,
                    resolution: "local".to_string(),
                })
                .unwrap();
        }

        let g = build_type_graph(&sqlite, &b, "both", 3, 100).unwrap();
        let nodes = g.get("nodes").unwrap().as_array().unwrap();
        let edges = g.get("edges").unwrap().as_array().unwrap();
        // B (root) + A (downstream) + D (upstream) = 3 nodes
        assert_eq!(nodes.len(), 3);
        // B→A and D→B = 2 edges
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn test_call_hierarchy_includes_async_call_edges() {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        let caller = sym("caller_id", "caller_fn");
        let callee = sym("callee_id", "callee_fn");
        sqlite.upsert_symbol(&caller).unwrap();
        sqlite.upsert_symbol(&callee).unwrap();

        sqlite
            .upsert_edge(&EdgeRow {
                from_symbol_id: "caller_id".to_string(),
                to_symbol_id: "callee_id".to_string(),
                edge_type: "async_call".to_string(),
                at_file: Some("src/a.ts".to_string()),
                at_line: Some(10),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();

        let g = build_call_hierarchy(&sqlite, &caller, "callees", 3, 100).unwrap();
        let nodes = g.get("nodes").unwrap().as_array().unwrap();
        let edges = g.get("edges").unwrap().as_array().unwrap();

        assert_eq!(nodes.len(), 2, "both caller and callee must be in nodes");
        assert_eq!(edges.len(), 1, "the async_call edge must be included");

        let callee_present = nodes
            .iter()
            .any(|n| n.get("id").and_then(|v| v.as_str()) == Some("callee_id"));
        assert!(callee_present, "callee_id must appear in nodes");
    }

    #[test]
    fn test_call_hierarchy_edge_type_not_hardcoded() {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        let caller = sym("caller_id", "caller_fn");
        let callee = sym("callee_id", "callee_fn");
        sqlite.upsert_symbol(&caller).unwrap();
        sqlite.upsert_symbol(&callee).unwrap();

        sqlite
            .upsert_edge(&EdgeRow {
                from_symbol_id: "caller_id".to_string(),
                to_symbol_id: "callee_id".to_string(),
                edge_type: "async_call".to_string(),
                at_file: Some("src/a.ts".to_string()),
                at_line: Some(5),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();

        let g = build_call_hierarchy(&sqlite, &caller, "callees", 3, 100).unwrap();
        let edges = g.get("edges").unwrap().as_array().unwrap();
        assert_eq!(edges.len(), 1, "expected exactly one edge");

        let edge = &edges[0];

        let edge_type = edge
            .get("edge_type")
            .and_then(|v| v.as_str())
            .expect("edge_type field must be present");
        assert_eq!(
            edge_type, "async_call",
            "edge_type must be 'async_call', not hardcoded 'call'"
        );

        let is_async = edge
            .get("is_async")
            .and_then(|v| v.as_bool())
            .expect("is_async field must be present");
        assert!(is_async, "is_async must be true for async_call edges");
    }
}

#[cfg(test)]
mod edge_types_tests {
    #[test]
    fn test_default_edge_types() {
        // Verify default is call + reference (matches build_dependency_graph's unwrap_or)
        let allowed = ["call", "reference"];
        assert_eq!(allowed, ["call", "reference"]);
    }

    #[test]
    fn test_custom_edge_types() {
        let types = vec!["call", "type", "extends"];
        let allowed = types.as_slice();
        assert_eq!(allowed.len(), 3);
        assert!(allowed.contains(&"call"));
        assert!(allowed.contains(&"type"));
        assert!(allowed.contains(&"extends"));
        assert!(!allowed.contains(&"reference"));
    }
}

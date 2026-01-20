//! Graph building functions for call hierarchies, type graphs, and dependency graphs

use crate::storage::sqlite::{SqliteStore, SymbolRow};
use serde_json::json;

/// Build a dependency graph starting from a root symbol
pub fn build_dependency_graph(
    sqlite: &SqliteStore,
    root: &SymbolRow,
    direction: &str,
    depth: usize,
    limit: usize,
) -> anyhow::Result<serde_json::Value> {
    let mut nodes = std::collections::HashMap::<String, serde_json::Value>::new();
    let mut edges = Vec::<serde_json::Value>::new();
    let mut visited = std::collections::HashSet::<String>::new();

    // Initial node
    nodes.insert(
        root.id.clone(),
        json!({
            "id": root.id,
            "name": root.name,
            "kind": root.kind,
            "file_path": root.file_path,
            "language": root.language,
            "exported": root.exported,
            "line_range": [root.start_line, root.end_line],
        }),
    );
    visited.insert(root.id.clone());

    let mut frontier = vec![root.id.clone()];

    // Direction flags
    let traverse_upstream = direction == "upstream" || direction == "bidirectional";
    let traverse_downstream = direction == "downstream" || direction == "bidirectional";

    for _ in 0..depth {
        if edges.len() >= limit {
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

                    // Filter edge types? "call" is primary. "reference" maybe?
                    if e.edge_type != "call" && e.edge_type != "reference" {
                        continue;
                    }

                    let Some(caller) = sqlite.get_symbol_by_id(&e.from_symbol_id)? else {
                        continue;
                    };

                    // Add node if new
                    if !nodes.contains_key(&caller.id) {
                        nodes.insert(
                            caller.id.clone(),
                            json!({
                                "id": caller.id,
                                "name": caller.name,
                                "kind": caller.kind,
                                "file_path": caller.file_path,
                                "language": caller.language,
                                "exported": caller.exported,
                                "line_range": [caller.start_line, caller.end_line],
                            }),
                        );
                    }

                    // Add edge
                    let evidence = sqlite
                        .list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, &e.edge_type, 3)
                        .unwrap_or_default();
                    edges.push(json!({
                        "from": e.from_symbol_id,
                        "to": e.to_symbol_id,
                        "edge_type": e.edge_type,
                        "at_file": e.at_file,
                        "at_line": e.at_line,
                        "evidence_count": e.evidence_count,
                        "resolution": e.resolution,
                        "evidence": evidence.into_iter().map(|ev| json!({
                            "at_file": ev.at_file,
                            "at_line": ev.at_line,
                            "count": ev.count,
                        })).collect::<Vec<_>>(),
                    }));

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

                    if e.edge_type != "call" && e.edge_type != "reference" {
                        continue;
                    }

                    let Some(callee) = sqlite.get_symbol_by_id(&e.to_symbol_id)? else {
                        continue;
                    };

                    if !nodes.contains_key(&callee.id) {
                        nodes.insert(
                            callee.id.clone(),
                            json!({
                                "id": callee.id,
                                "name": callee.name,
                                "kind": callee.kind,
                                "file_path": callee.file_path,
                                "language": callee.language,
                                "exported": callee.exported,
                                "line_range": [callee.start_line, callee.end_line],
                            }),
                        );
                    }

                    edges.push(json!({
                        "from": e.from_symbol_id,
                        "to": e.to_symbol_id,
                        "edge_type": e.edge_type,
                        "at_file": e.at_file,
                        "at_line": e.at_line,
                        "evidence_count": e.evidence_count,
                        "resolution": e.resolution,
                        "evidence": sqlite
                            .list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, &e.edge_type, 3)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|ev| json!({
                                "at_file": ev.at_file,
                                "at_line": ev.at_line,
                                "count": ev.count,
                            }))
                            .collect::<Vec<_>>(),
                    }));

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

    nodes.insert(
        root.id.clone(),
        json!({
            "id": root.id,
            "name": root.name,
            "kind": root.kind,
            "file_path": root.file_path,
            "language": root.language,
            "exported": root.exported,
            "line_range": [root.start_line, root.end_line],
        }),
    );
    visited.insert(root.id.clone());

    let mut frontier = vec![root.id.clone()];
    for _ in 0..depth {
        if edges.len() >= limit {
            break;
        }
        let mut next = Vec::new();
        for current_id in frontier {
            if edges.len() >= limit {
                break;
            }
            if direction == "callers" {
                let incoming = sqlite.list_edges_to(&current_id, limit)?;
                for e in incoming {
                    if edges.len() >= limit {
                        break;
                    }
                    if e.edge_type != "call" {
                        continue;
                    }
                    let Some(caller) = sqlite.get_symbol_by_id(&e.from_symbol_id)? else {
                        continue;
                    };
                    nodes.entry(caller.id.clone()).or_insert_with(|| {
                        json!({
                            "id": caller.id,
                            "name": caller.name,
                            "kind": caller.kind,
                            "file_path": caller.file_path,
                            "language": caller.language,
                            "exported": caller.exported,
                            "line_range": [caller.start_line, caller.end_line],
                        })
                    });
                    edges.push(json!({
                        "from": e.from_symbol_id,
                        "to": e.to_symbol_id,
                        "edge_type": "call",
                        "at_file": e.at_file,
                        "at_line": e.at_line,
                        "evidence_count": e.evidence_count,
                        "resolution": e.resolution,
                        "evidence": sqlite
                            .list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, "call", 3)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|ev| json!({
                                "at_file": ev.at_file,
                                "at_line": ev.at_line,
                                "count": ev.count,
                            }))
                            .collect::<Vec<_>>(),
                    }));
                    if visited.insert(caller.id.clone()) {
                        next.push(caller.id);
                    }
                }
            } else {
                let outgoing = sqlite.list_edges_from(&current_id, limit)?;
                for e in outgoing {
                    if edges.len() >= limit {
                        break;
                    }
                    if e.edge_type != "call" {
                        continue;
                    }
                    let Some(callee) = sqlite.get_symbol_by_id(&e.to_symbol_id)? else {
                        continue;
                    };
                    nodes.entry(callee.id.clone()).or_insert_with(|| {
                        json!({
                            "id": callee.id,
                            "name": callee.name,
                            "kind": callee.kind,
                            "file_path": callee.file_path,
                            "language": callee.language,
                            "exported": callee.exported,
                            "line_range": [callee.start_line, callee.end_line],
                        })
                    });
                    edges.push(json!({
                        "from": e.from_symbol_id,
                        "to": e.to_symbol_id,
                        "edge_type": "call",
                        "at_file": e.at_file,
                        "at_line": e.at_line,
                        "evidence_count": e.evidence_count,
                        "resolution": e.resolution,
                        "evidence": sqlite
                            .list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, "call", 3)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|ev| json!({
                                "at_file": ev.at_file,
                                "at_line": ev.at_line,
                                "count": ev.count,
                            }))
                            .collect::<Vec<_>>(),
                    }));
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

/// Build a type graph starting from a root symbol
pub fn build_type_graph(
    sqlite: &SqliteStore,
    root: &SymbolRow,
    depth: usize,
    limit: usize,
) -> anyhow::Result<serde_json::Value> {
    let mut nodes = std::collections::HashMap::<String, serde_json::Value>::new();
    let mut edges = Vec::<serde_json::Value>::new();
    let mut visited = std::collections::HashSet::<String>::new();

    nodes.insert(
        root.id.clone(),
        json!({
            "id": root.id,
            "name": root.name,
            "kind": root.kind,
            "file_path": root.file_path,
            "language": root.language,
            "exported": root.exported,
            "line_range": [root.start_line, root.end_line],
        }),
    );
    visited.insert(root.id.clone());

    let mut frontier = vec![root.id.clone()];
    for _ in 0..depth {
        if edges.len() >= limit {
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
                if e.edge_type != "extends" && e.edge_type != "implements" && e.edge_type != "alias"
                {
                    continue;
                }
                let Some(to_sym) = sqlite.get_symbol_by_id(&e.to_symbol_id)? else {
                    continue;
                };
                nodes.entry(to_sym.id.clone()).or_insert_with(|| {
                    json!({
                        "id": to_sym.id,
                        "name": to_sym.name,
                        "kind": to_sym.kind,
                        "file_path": to_sym.file_path,
                        "language": to_sym.language,
                        "exported": to_sym.exported,
                        "line_range": [to_sym.start_line, to_sym.end_line],
                    })
                });
                edges.push(json!({
                    "from": e.from_symbol_id,
                    "to": e.to_symbol_id,
                    "edge_type": e.edge_type,
                    "at_file": e.at_file,
                    "at_line": e.at_line,
                    "evidence_count": e.evidence_count,
                    "resolution": e.resolution,
                    "evidence": sqlite
                        .list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, &e.edge_type, 3)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|ev| json!({
                            "at_file": ev.at_file,
                            "at_line": ev.at_line,
                            "count": ev.count,
                        }))
                        .collect::<Vec<_>>(),
                }));
                if visited.insert(to_sym.id.clone()) {
                    next.push(to_sym.id);
                }
            }
        }
        frontier = next;
    }

    Ok(json!({
        "symbol_name": root.name,
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

        let g = build_type_graph(&sqlite, &a, 3, 100).unwrap();
        let nodes = g.get("nodes").unwrap().as_array().unwrap();
        let edges = g.get("edges").unwrap().as_array().unwrap();
        assert_eq!(nodes.len(), 4);
        assert_eq!(edges.len(), 3);
    }
}

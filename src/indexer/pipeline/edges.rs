use std::collections::{HashMap, HashSet};

use crate::indexer::extract::symbol::Import;
use crate::storage::sqlite::{EdgeEvidenceRow, EdgeRow, SymbolRow};

use super::parsing::{
    extract_callee_names, extract_identifiers, identifier_evidence, parse_type_relations,
};
use super::utils::{build_import_map, resolve_imported_symbol_id};

pub fn upsert_name_mapping(name_to_id: &mut HashMap<String, String>, row: &SymbolRow) {
    if let Some(existing) = name_to_id.get(&row.name) {
        if row.exported && existing != &row.id {
            name_to_id.insert(row.name.clone(), row.id.clone());
        }
        return;
    }
    name_to_id.insert(row.name.clone(), row.id.clone());
}

pub fn extract_edges_for_symbol(
    row: &SymbolRow,
    name_to_id: &HashMap<String, String>,
    imports: &[Import],
    type_edges: &[(String, String)],
) -> Vec<(EdgeRow, Vec<EdgeEvidenceRow>)> {
    let mut out: Vec<(EdgeRow, Vec<EdgeEvidenceRow>)> = Vec::new();
    let mut used_edges: HashSet<(String, String)> = HashSet::new();
    let confidence_for = |edge_type: &str| match edge_type {
        "call" => 1.0,
        "reference" => 0.8,
        "type" => 0.9,
        "extends" | "implements" | "alias" => 0.95,
        _ => 0.7,
    };
    let evidence_for = |name: &str| identifier_evidence(&row.text, name, row.start_line);

    // Map import alias/name to Import struct for fast lookup
    let import_map = build_import_map(imports);

    for callee in extract_callee_names(&row.text) {
        let (to_id, resolution) = if let Some(local_id) = name_to_id.get(&callee) {
            if local_id == &row.id {
                continue;
            }
            (Some(local_id.clone()), "local")
        } else if let Some(imp) = import_map.get(callee.as_str()) {
            // Resolve import
            (resolve_imported_symbol_id(&row.file_path, imp), "import")
        } else {
            (None, "unknown")
        };

        let Some(to_id) = to_id else {
            continue;
        };

        if !used_edges.insert(("call".to_string(), to_id.clone())) {
            continue;
        }
        let (count, at_line, evidence_rows) = evidence_for(&callee);
        out.push((
            EdgeRow {
                from_symbol_id: row.id.clone(),
                to_symbol_id: to_id.clone(),
                edge_type: "call".to_string(),
                at_file: Some(row.file_path.clone()),
                at_line: Some(at_line),
                confidence: confidence_for("call"),
                evidence_count: count,
                resolution: resolution.to_string(),
            },
            evidence_rows
                .into_iter()
                .map(|(line, c)| EdgeEvidenceRow {
                    from_symbol_id: row.id.clone(),
                    to_symbol_id: to_id.clone(),
                    edge_type: "call".to_string(),
                    at_file: row.file_path.clone(),
                    at_line: line,
                    count: c,
                })
                .collect(),
        ));
    }

    // Handle extends/implements
    if row.kind == "class" || row.kind == "interface" || row.kind == "type_alias" {
        let (extends, implements, aliases) = parse_type_relations(&row.text);

        let mut handle_relation = |name: String, rel_type: &str| {
            let (to_id, resolution) = if let Some(local_id) = name_to_id.get(&name) {
                if local_id == &row.id {
                    return;
                }
                (Some(local_id.clone()), "local")
            } else if let Some(imp) = import_map.get(name.as_str()) {
                (resolve_imported_symbol_id(&row.file_path, imp), "import")
            } else {
                (None, "unknown")
            };

            if let Some(id) = to_id {
                if used_edges.insert((rel_type.to_string(), id.clone())) {
                    let (count, at_line, evidence_rows) = evidence_for(&name);
                    out.push((
                        EdgeRow {
                            from_symbol_id: row.id.clone(),
                            to_symbol_id: id.clone(),
                            edge_type: rel_type.to_string(),
                            at_file: Some(row.file_path.clone()),
                            at_line: Some(at_line),
                            confidence: confidence_for(rel_type),
                            evidence_count: count,
                            resolution: resolution.to_string(),
                        },
                        evidence_rows
                            .into_iter()
                            .map(|(line, c)| EdgeEvidenceRow {
                                from_symbol_id: row.id.clone(),
                                to_symbol_id: id.clone(),
                                edge_type: rel_type.to_string(),
                                at_file: row.file_path.clone(),
                                at_line: line,
                                count: c,
                            })
                            .collect(),
                    ));
                }
            }
        };

        for name in extends {
            handle_relation(name, "extends");
        }
        for name in implements {
            handle_relation(name, "implements");
        }
        for name in aliases {
            handle_relation(name, "alias");
        }
    }

    // References
    let mut refs_added = 0usize;
    for ident in extract_identifiers(&row.text) {
        if refs_added >= 20 {
            break;
        }
        if ident == row.name {
            continue;
        }

        let (to_id, resolution) = if let Some(local_id) = name_to_id.get(&ident) {
            if local_id == &row.id {
                continue;
            }
            (Some(local_id.clone()), "local")
        } else if let Some(imp) = import_map.get(ident.as_str()) {
            (resolve_imported_symbol_id(&row.file_path, imp), "import")
        } else {
            (None, "unknown")
        };

        if let Some(id) = to_id {
            if used_edges.insert(("reference".to_string(), id.clone())) {
                let (count, at_line, evidence_rows) = evidence_for(&ident);
                out.push((
                    EdgeRow {
                        from_symbol_id: row.id.clone(),
                        to_symbol_id: id.clone(),
                        edge_type: "reference".to_string(),
                        at_file: Some(row.file_path.clone()),
                        at_line: Some(at_line),
                        confidence: confidence_for("reference"),
                        evidence_count: count,
                        resolution: resolution.to_string(),
                    },
                    evidence_rows
                        .into_iter()
                        .map(|(line, c)| EdgeEvidenceRow {
                            from_symbol_id: row.id.clone(),
                            to_symbol_id: id.clone(),
                            edge_type: "reference".to_string(),
                            at_file: row.file_path.clone(),
                            at_line: line,
                            count: c,
                        })
                        .collect(),
                ));
            }
        }
        refs_added += 1;
    }

    // Add type edges
    for (parent_name, type_name) in type_edges {
        if parent_name == &row.name {
            // Resolve type_name
            let (to_id, resolution) = if let Some(local_id) = name_to_id.get(type_name) {
                if local_id == &row.id {
                    continue;
                }
                (Some(local_id.clone()), "local")
            } else if let Some(imp) = import_map.get(type_name.as_str()) {
                (resolve_imported_symbol_id(&row.file_path, imp), "import")
            } else {
                (None, "unknown")
            };

            if let Some(id) = to_id {
                if used_edges.insert(("type".to_string(), id.clone())) {
                    let (count, at_line, evidence_rows) = evidence_for(type_name);
                    out.push((
                        EdgeRow {
                            from_symbol_id: row.id.clone(),
                            to_symbol_id: id.clone(),
                            edge_type: "type".to_string(),
                            at_file: Some(row.file_path.clone()),
                            at_line: Some(at_line),
                            confidence: confidence_for("type"),
                            evidence_count: count,
                            resolution: resolution.to_string(),
                        },
                        evidence_rows
                            .into_iter()
                            .map(|(line, c)| EdgeEvidenceRow {
                                from_symbol_id: row.id.clone(),
                                to_symbol_id: id.clone(),
                                edge_type: "type".to_string(),
                                at_file: row.file_path.clone(),
                                at_line: line,
                                count: c,
                            })
                            .collect(),
                    ));
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::pipeline::utils::stable_symbol_id;

    fn symbol(id: &str, name: &str, kind: &str, text: &str) -> SymbolRow {
        SymbolRow {
            id: id.to_string(),
            file_path: "src/a.ts".to_string(),
            language: "typescript".to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
            text: text.to_string(),
        }
    }

    #[test]
    fn extracts_call_import_and_reference_edges() {
        let row = symbol(
            "id_a",
            "a",
            "function",
            "import { b } from './b';\nexport function a(){ b(); c(); }",
        );
        let mut name_to_id = HashMap::new();
        name_to_id.insert("c".to_string(), "id_c".to_string());

        let imports = vec![Import {
            name: "b".to_string(),
            source: "./b".to_string(),
            alias: None,
        }];
        let type_edges = vec![];

        let edges = extract_edges_for_symbol(&row, &name_to_id, &imports, &type_edges);

        let expected_b_id = stable_symbol_id("src/b.ts", "b", 0);

        assert!(edges
            .iter()
            .any(|(e, _)| { e.edge_type == "call" && e.to_symbol_id == expected_b_id }));

        assert!(edges
            .iter()
            .any(|(e, _)| e.edge_type == "call" && e.to_symbol_id == "id_c"));

        assert!(edges
            .iter()
            .any(|(e, _)| { e.edge_type == "reference" && e.to_symbol_id == expected_b_id }));
    }
}

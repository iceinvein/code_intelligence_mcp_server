use std::collections::{HashMap, HashSet};

use crate::indexer::extract::symbol::{DataFlowEdge, DataFlowType, Import};
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

/// Resolution context for edge creation
struct ResolutionContext<'a> {
    from_file_path: &'a str,
    from_package_id: Option<String>,
    row_name: &'a str,
    get_package_fn: Option<&'a PackageLookupFn>,
    id_to_symbol: &'a HashMap<String, &'a SymbolRow>,
}

/// Compute resolution for an edge to a target symbol
fn compute_resolution_for_target(ctx: &ResolutionContext, to_id: &str, was_import: bool) -> String {
    if let Some(to_symbol) = ctx.id_to_symbol.get(to_id) {
        let to_package_id = get_package_for_symbol(ctx.get_package_fn, &to_symbol.file_path);

        let resolution = determine_edge_resolution(
            ctx.from_file_path,
            &to_symbol.file_path,
            &ctx.from_package_id,
            &to_package_id,
            was_import,
        );

        // Log cross-package edges at DEBUG level
        if resolution == "cross-package" || resolution == "cross-package-import" {
            if let (Some(from_pkg), Some(to_pkg)) = (&ctx.from_package_id, &to_package_id) {
                tracing::debug!(
                    from = %ctx.row_name,
                    to = %to_symbol.name,
                    from_package = %from_pkg,
                    to_package = %to_pkg,
                    from_file = %ctx.from_file_path,
                    to_file = %to_symbol.file_path,
                    resolution = %resolution,
                    "Cross-package edge detected"
                );
            }
        }

        resolution
    } else {
        // Target symbol not in current batch (external import)
        if was_import {
            "import".to_string()
        } else {
            "unknown".to_string()
        }
    }
}

/// Package lookup function type for resolving symbol package membership
///
/// This is a boxed function pointer to allow capturing state (like db_path)
/// in the closure for package lookup during edge extraction.
pub type PackageLookupFn = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Helper to create None package lookup
pub fn no_package_lookup(_: &str) -> Option<String> {
    None
}

/// Get the package ID for a symbol's file path.
///
/// This function attempts to find which package contains a given symbol
/// by looking up the package for the symbol's file path.
///
/// # Arguments
///
/// * `get_package_fn` - Optional reference to function that returns package_id for a file_path
/// * `symbol_file_path` - The file path of the symbol
///
/// # Returns
///
/// * `Some(package_id)` if the file belongs to a package
/// * `None` if the file is not in any package
fn get_package_for_symbol(
    get_package_fn: Option<&PackageLookupFn>,
    symbol_file_path: &str,
) -> Option<String> {
    get_package_fn.and_then(|f| f(symbol_file_path))
}

/// Determine the resolution type for an edge based on package membership.
///
/// Resolution types:
/// - "local": Same file
/// - "package": Same package, different file
/// - "cross-package": Different package
/// - "import": External import (no package)
/// - "unknown": Cannot determine
///
/// # Arguments
///
/// * `from_file_path` - File path of the source symbol
/// * `to_file_path` - File path of the target symbol
/// * `from_package_id` - Package ID of source symbol (if any)
/// * `to_package_id` - Package ID of target symbol (if any)
/// * `was_import` - Whether the edge was resolved via import
///
/// # Returns
///
/// Resolution type string
fn determine_edge_resolution(
    from_file_path: &str,
    to_file_path: &str,
    from_package_id: &Option<String>,
    to_package_id: &Option<String>,
    was_import: bool,
) -> String {
    // If same file, always local
    if from_file_path == to_file_path {
        return "local".to_string();
    }

    // If resolved via import, keep that marker
    if was_import {
        // But we can still add package context
        if let (Some(from_pkg), Some(to_pkg)) = (from_package_id, to_package_id) {
            if from_pkg == to_pkg {
                return "package-import".to_string();
            } else {
                return "cross-package-import".to_string();
            }
        }
        return "import".to_string();
    }

    // Both in same package
    if let (Some(from_pkg), Some(to_pkg)) = (from_package_id, to_package_id) {
        if from_pkg == to_pkg {
            return "package".to_string();
        } else {
            return "cross-package".to_string();
        }
    }

    // One or both not in any package
    "unknown".to_string()
}

pub fn extract_edges_for_symbol(
    row: &SymbolRow,
    name_to_id: &HashMap<String, String>,
    id_to_symbol: &HashMap<String, &SymbolRow>,
    imports: &[Import],
    type_edges: &[(String, String)],
    dataflow_edges: &[DataFlowEdge],
    get_package_fn: Option<&PackageLookupFn>,
) -> Vec<(EdgeRow, Vec<EdgeEvidenceRow>)> {
    let mut out: Vec<(EdgeRow, Vec<EdgeEvidenceRow>)> = Vec::new();
    let mut used_edges: HashSet<(String, String)> = HashSet::new();
    let confidence_for = |edge_type: &str| match edge_type {
        "call" => 1.0,
        "reference" => 0.8,
        "type" => 0.9,
        "extends" | "implements" | "alias" => 0.95,
        "reads" | "writes" => 0.7,
        _ => 0.7,
    };
    let evidence_for = |name: &str| identifier_evidence(&row.text, name, row.start_line);

    // Map import alias/name to Import struct for fast lookup
    let import_map = build_import_map(imports);

    // Get package for source symbol
    let from_package_id = get_package_for_symbol(get_package_fn, &row.file_path);

    // Create resolution context
    let resolution_ctx = ResolutionContext {
        from_file_path: &row.file_path,
        from_package_id,
        row_name: &row.name,
        get_package_fn,
        id_to_symbol,
    };

    for callee in extract_callee_names(&row.text) {
        let (to_id, was_import) = if let Some(local_id) = name_to_id.get(&callee) {
            if local_id == &row.id {
                continue;
            }
            (Some(local_id.clone()), false)
        } else if let Some(imp) = import_map.get(callee.as_str()) {
            // Resolve import
            (resolve_imported_symbol_id(&row.file_path, imp), true)
        } else {
            (None, false)
        };

        let Some(to_id) = to_id else {
            continue;
        };

        if !used_edges.insert(("call".to_string(), to_id.clone())) {
            continue;
        }

        let resolution = compute_resolution_for_target(&resolution_ctx, &to_id, was_import);
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
                resolution,
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

        for name in extends {
            let (to_id, was_import) = if let Some(local_id) = name_to_id.get(&name) {
                if local_id == &row.id {
                    continue;
                }
                (Some(local_id.clone()), false)
            } else if let Some(imp) = import_map.get(name.as_str()) {
                (resolve_imported_symbol_id(&row.file_path, imp), true)
            } else {
                (None, false)
            };

            if let Some(id) = to_id {
                if used_edges.insert(("extends".to_string(), id.clone())) {
                    let resolution =
                        compute_resolution_for_target(&resolution_ctx, &id, was_import);
                    let (count, at_line, evidence_rows) = evidence_for(&name);
                    out.push((
                        EdgeRow {
                            from_symbol_id: row.id.clone(),
                            to_symbol_id: id.clone(),
                            edge_type: "extends".to_string(),
                            at_file: Some(row.file_path.clone()),
                            at_line: Some(at_line),
                            confidence: confidence_for("extends"),
                            evidence_count: count,
                            resolution,
                        },
                        evidence_rows
                            .into_iter()
                            .map(|(line, c)| EdgeEvidenceRow {
                                from_symbol_id: row.id.clone(),
                                to_symbol_id: id.clone(),
                                edge_type: "extends".to_string(),
                                at_file: row.file_path.clone(),
                                at_line: line,
                                count: c,
                            })
                            .collect(),
                    ));
                }
            }
        }

        for name in implements {
            let (to_id, was_import) = if let Some(local_id) = name_to_id.get(&name) {
                if local_id == &row.id {
                    continue;
                }
                (Some(local_id.clone()), false)
            } else if let Some(imp) = import_map.get(name.as_str()) {
                (resolve_imported_symbol_id(&row.file_path, imp), true)
            } else {
                (None, false)
            };

            if let Some(id) = to_id {
                if used_edges.insert(("implements".to_string(), id.clone())) {
                    let resolution =
                        compute_resolution_for_target(&resolution_ctx, &id, was_import);
                    let (count, at_line, evidence_rows) = evidence_for(&name);
                    out.push((
                        EdgeRow {
                            from_symbol_id: row.id.clone(),
                            to_symbol_id: id.clone(),
                            edge_type: "implements".to_string(),
                            at_file: Some(row.file_path.clone()),
                            at_line: Some(at_line),
                            confidence: confidence_for("implements"),
                            evidence_count: count,
                            resolution,
                        },
                        evidence_rows
                            .into_iter()
                            .map(|(line, c)| EdgeEvidenceRow {
                                from_symbol_id: row.id.clone(),
                                to_symbol_id: id.clone(),
                                edge_type: "implements".to_string(),
                                at_file: row.file_path.clone(),
                                at_line: line,
                                count: c,
                            })
                            .collect(),
                    ));
                }
            }
        }

        for name in aliases {
            let (to_id, was_import) = if let Some(local_id) = name_to_id.get(&name) {
                if local_id == &row.id {
                    continue;
                }
                (Some(local_id.clone()), false)
            } else if let Some(imp) = import_map.get(name.as_str()) {
                (resolve_imported_symbol_id(&row.file_path, imp), true)
            } else {
                (None, false)
            };

            if let Some(id) = to_id {
                if used_edges.insert(("alias".to_string(), id.clone())) {
                    let resolution =
                        compute_resolution_for_target(&resolution_ctx, &id, was_import);
                    let (count, at_line, evidence_rows) = evidence_for(&name);
                    out.push((
                        EdgeRow {
                            from_symbol_id: row.id.clone(),
                            to_symbol_id: id.clone(),
                            edge_type: "alias".to_string(),
                            at_file: Some(row.file_path.clone()),
                            at_line: Some(at_line),
                            confidence: confidence_for("alias"),
                            evidence_count: count,
                            resolution,
                        },
                        evidence_rows
                            .into_iter()
                            .map(|(line, c)| EdgeEvidenceRow {
                                from_symbol_id: row.id.clone(),
                                to_symbol_id: id.clone(),
                                edge_type: "alias".to_string(),
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

    // References
    let mut refs_added = 0usize;
    for ident in extract_identifiers(&row.text) {
        if refs_added >= 20 {
            break;
        }
        if ident == row.name {
            continue;
        }

        let (to_id, was_import) = if let Some(local_id) = name_to_id.get(&ident) {
            if local_id == &row.id {
                continue;
            }
            (Some(local_id.clone()), false)
        } else if let Some(imp) = import_map.get(ident.as_str()) {
            (resolve_imported_symbol_id(&row.file_path, imp), true)
        } else {
            (None, false)
        };

        if let Some(id) = to_id {
            if used_edges.insert(("reference".to_string(), id.clone())) {
                let resolution = compute_resolution_for_target(&resolution_ctx, &id, was_import);
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
                        resolution,
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
            let (to_id, was_import) = if let Some(local_id) = name_to_id.get(type_name) {
                if local_id == &row.id {
                    continue;
                }
                (Some(local_id.clone()), false)
            } else if let Some(imp) = import_map.get(type_name.as_str()) {
                (resolve_imported_symbol_id(&row.file_path, imp), true)
            } else {
                (None, false)
            };

            if let Some(id) = to_id {
                if used_edges.insert(("type".to_string(), id.clone())) {
                    let resolution =
                        compute_resolution_for_target(&resolution_ctx, &id, was_import);
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
                            resolution,
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

    // Handle data flow edges
    for dfe in dataflow_edges {
        // Resolve from_symbol to actual symbol ID
        let (to_id, was_import) = if let Some(local_id) = name_to_id.get(&dfe.from_symbol) {
            if local_id == &row.id {
                continue;
            }
            (Some(local_id.clone()), false)
        } else if let Some(imp) = import_map.get(dfe.from_symbol.as_str()) {
            (resolve_imported_symbol_id(&row.file_path, imp), true)
        } else {
            // For data flow edges, we might not have a symbol ID yet
            // Skip edges to unknown symbols for now
            continue;
        };

        if let Some(id) = to_id {
            let edge_type = match dfe.flow_type {
                DataFlowType::Reads => "reads",
                DataFlowType::Writes => "writes",
            };

            // Skip if we already have this edge type to this target
            if !used_edges.insert((edge_type.to_string(), id.clone())) {
                continue;
            }

            let resolution = compute_resolution_for_target(&resolution_ctx, &id, was_import);

            out.push((
                EdgeRow {
                    from_symbol_id: row.id.clone(),
                    to_symbol_id: id,
                    edge_type: edge_type.to_string(),
                    at_file: Some(row.file_path.clone()),
                    at_line: Some(dfe.at_line),
                    confidence: 0.7,
                    evidence_count: 1,
                    resolution,
                },
                vec![], // No evidence for data flow edges yet
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::pipeline::utils::stable_symbol_id;

    fn symbol(id: &str, name: &str, kind: &str, text: &str, file_path: &str) -> SymbolRow {
        SymbolRow {
            id: id.to_string(),
            file_path: file_path.to_string(),
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
            "src/a.ts",
        );
        let mut name_to_id = HashMap::new();
        name_to_id.insert("c".to_string(), "id_c".to_string());

        let symbol_c = symbol("id_c", "c", "function", "function c(){}", "src/a.ts");
        let mut id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();
        id_to_symbol.insert("id_c".to_string(), &symbol_c);

        let imports = vec![Import {
            name: "b".to_string(),
            source: "./b".to_string(),
            alias: None,
        }];
        let type_edges = vec![];
        let dataflow_edges = vec![];

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &imports,
            &type_edges,
            &dataflow_edges,
            None,
        );

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

    #[test]
    fn test_cross_package_edge_resolution() {
        // Create symbols in different packages
        let row = symbol(
            "id_a",
            "a",
            "function",
            "import { b } from '../utils/b';\nexport function a(){ b(); c(); }",
            "packages/core/src/a.ts",
        );
        let mut name_to_id = HashMap::new();
        name_to_id.insert("c".to_string(), "id_c".to_string());

        // Symbol c is in the same package (also in packages/core)
        let symbol_c = symbol(
            "id_c",
            "c",
            "function",
            "function c(){}",
            "packages/core/src/c.ts",
        );
        let mut id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();
        id_to_symbol.insert("id_c".to_string(), &symbol_c);

        let imports = vec![Import {
            name: "b".to_string(),
            source: "../utils/b".to_string(),
            alias: None,
        }];
        let type_edges = vec![];
        let dataflow_edges = vec![];

        // Mock package lookup function
        // files starting with "packages/core" are in "pkg-core"
        // files starting with "packages/utils" are in "pkg-utils"
        fn get_package_impl(file_path: &str) -> Option<String> {
            if file_path.starts_with("packages/core") {
                Some("pkg-core".to_string())
            } else if file_path.starts_with("packages/utils") {
                Some("pkg-utils".to_string())
            } else {
                None
            }
        }
        let get_package_fn: PackageLookupFn = Box::new(get_package_impl);

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &imports,
            &type_edges,
            &dataflow_edges,
            Some(&get_package_fn),
        );

        // Find the edge to symbol c (same package, different file)
        let edge_to_c = edges
            .iter()
            .find(|(e, _)| e.to_symbol_id == "id_c" && e.edge_type == "call");

        assert!(edge_to_c.is_some());
        // Should be "package" resolution (same package, different file)
        assert_eq!(edge_to_c.unwrap().0.resolution, "package");

        // Find the edge to symbol b (same package, different file via import)
        // Import ../utils/b from packages/core/src/a.ts resolves to packages/core/utils/b.ts
        let expected_b_id = stable_symbol_id("packages/core/utils/b.ts", "b", 0);
        let edge_to_b = edges
            .iter()
            .find(|(e, _)| e.to_symbol_id == expected_b_id && e.edge_type == "call");

        assert!(edge_to_b.is_some());
        // Should be "import" resolution (via import statement, same package)
        assert_eq!(edge_to_b.unwrap().0.resolution, "import");
    }

    #[test]
    fn test_same_file_resolution() {
        // Same file should always be "local"
        let row = symbol(
            "id_a",
            "a",
            "function",
            "export function a(){ b(); }",
            "src/a.ts",
        );
        let mut name_to_id = HashMap::new();
        name_to_id.insert("b".to_string(), "id_b".to_string());

        let symbol_b = symbol("id_b", "b", "function", "function b(){}", "src/a.ts");
        let mut id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();
        id_to_symbol.insert("id_b".to_string(), &symbol_b);

        let imports = vec![];
        let type_edges = vec![];
        let dataflow_edges = vec![];

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &imports,
            &type_edges,
            &dataflow_edges,
            None,
        );

        let edge_to_b = edges
            .iter()
            .find(|(e, _)| e.to_symbol_id == "id_b" && e.edge_type == "call");

        assert!(edge_to_b.is_some());
        // Same file should be "local"
        assert_eq!(edge_to_b.unwrap().0.resolution, "local");
    }

    #[test]
    fn test_cross_package_with_import_resolution() {
        // Test cross-package edge where both symbols are in the batch
        let row = symbol(
            "id_a",
            "a",
            "function",
            "import { b } from '../utils/b';\nexport function a(){ b(); }",
            "packages/core/src/a.ts",
        );
        let mut name_to_id = HashMap::new();
        name_to_id.insert("b".to_string(), "id_b".to_string());

        // Symbol b is in a different package (packages/utils)
        let symbol_b = symbol(
            "id_b",
            "b",
            "function",
            "export function b(){}",
            "packages/utils/src/b.ts",
        );
        let mut id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();
        id_to_symbol.insert("id_b".to_string(), &symbol_b);

        let imports = vec![Import {
            name: "b".to_string(),
            source: "../utils/b".to_string(),
            alias: None,
        }];
        let type_edges = vec![];
        let dataflow_edges = vec![];

        fn get_package_impl2(file_path: &str) -> Option<String> {
            if file_path.starts_with("packages/core") {
                Some("pkg-core".to_string())
            } else if file_path.starts_with("packages/utils") {
                Some("pkg-utils".to_string())
            } else {
                None
            }
        }
        let get_package_fn: PackageLookupFn = Box::new(get_package_impl2);

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &imports,
            &type_edges,
            &dataflow_edges,
            Some(&get_package_fn),
        );

        let edge_to_b = edges
            .iter()
            .find(|(e, _)| e.to_symbol_id == "id_b" && e.edge_type == "call");

        assert!(edge_to_b.is_some());
        // Cross-package (local reference, different packages) should be "cross-package"
        assert_eq!(edge_to_b.unwrap().0.resolution, "cross-package");
    }
}

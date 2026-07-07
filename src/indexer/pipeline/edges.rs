use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::indexer::extract::symbol::{DataFlowEdge, DataFlowType, Import};
use crate::storage::sqlite::{EdgeEvidenceRow, EdgeRow, SymbolRow};

use super::parsing::{
    extract_calls, extract_identifiers, identifier_evidence, parse_type_relations,
};
use super::utils::{
    build_import_map, resolve_imported_symbol_id, resolve_imported_symbol_id_with_db,
};

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
struct ResolutionContext<'a, 'f> {
    from_file_path: &'a str,
    from_package_id: Option<String>,
    row_name: &'a str,
    get_package_fn: Option<&'a PackageLookupFn<'f>>,
    id_to_symbol: &'a HashMap<String, &'a SymbolRow>,
}

/// Compute resolution for an edge to a target symbol
fn compute_resolution_for_target(
    ctx: &ResolutionContext<'_, '_>,
    to_id: &str,
    was_import: bool,
) -> String {
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

/// Package lookup function type for resolving symbol package membership.
///
/// Boxed so it can capture borrowed state (e.g. a pooled SQLite `&Connection`)
/// for the duration of a single `extract_edges_for_symbol` call. The `'a`
/// lifetime parameter lets the closure hold non-`'static` references; we don't
/// require `Send`/`Sync` because the closure is constructed per-thread inside
/// the parallel parse loop and never crosses threads.
pub type PackageLookupFn<'a> = Box<dyn Fn(&str) -> Option<String> + 'a>;

/// Helper to create None package lookup
pub fn no_package_lookup(_: &str) -> Option<String> {
    None
}

/// Resolve a `receiver.method()` call to the target symbol id.
///
/// When `method` isn't locally defined and isn't directly imported into
/// the calling file, this looks up `method` as ANY symbol (exported or
/// not) in the file that `receiver` was imported from. That catches
/// class methods like `SessionManager.createSession` -- the class is the
/// imported binding, the method is a non-exported member, and the
/// standard `resolve_imported_symbol_id_with_db` would reject it for
/// not being a top-level export.
fn resolve_method_on_receiver(
    conn: Option<&Connection>,
    from_file: &str,
    receiver: &str,
    method: &str,
    import_map: &HashMap<&str, &Import>,
) -> Option<String> {
    use crate::storage::sqlite::queries;

    let conn = conn?;
    let receiver_import = import_map.get(receiver)?;
    let target_path = super::utils::resolve_path(from_file, &receiver_import.source).or(None)?;

    // Try the canonical target path, then common alternatives (.tsx,
    // /index.ts, /index.tsx). For each, accept the first matching
    // symbol regardless of `exported` -- class methods carry the call
    // edge on the class, not the file's top-level export list.
    let mut candidate_paths = vec![target_path.clone()];
    candidate_paths.extend(alternative_import_paths(&target_path));

    for path in candidate_paths {
        let Ok(results) =
            queries::symbols::search_symbols_by_exact_name(conn, method, Some(&path), 1)
        else {
            continue;
        };
        if let Some(symbol) = results.into_iter().next() {
            return Some(symbol.id);
        }
    }
    None
}

/// Mirror of utils::alternative_import_paths kept private here to avoid
/// re-exporting through utils.
fn alternative_import_paths(base_path: &str) -> Vec<String> {
    let mut alternatives = Vec::new();
    if let Some(dir_path) = base_path.strip_suffix(".ts") {
        alternatives.push(format!("{}x", base_path));
        alternatives.push(format!("{}/index.ts", dir_path));
        alternatives.push(format!("{}/index.tsx", dir_path));
    } else if let Some(ts_path) = base_path.strip_suffix('x') {
        if ts_path.ends_with(".ts") {
            alternatives.push(ts_path.to_string());
        }
    } else if !base_path.contains('.') {
        alternatives.push(format!("{}.ts", base_path));
        alternatives.push(format!("{}.tsx", base_path));
        alternatives.push(format!("{}/index.ts", base_path));
        alternatives.push(format!("{}/index.tsx", base_path));
    }
    alternatives
}

/// Get the package ID for a symbol's file path.
fn get_package_for_symbol(
    get_package_fn: Option<&PackageLookupFn<'_>>,
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

#[allow(clippy::too_many_arguments)]
pub fn extract_edges_for_symbol(
    row: &SymbolRow,
    name_to_id: &HashMap<String, String>,
    id_to_symbol: &HashMap<String, &SymbolRow>,
    imports: &[Import],
    type_edges: &[(String, String)],
    dataflow_edges: &[DataFlowEdge],
    get_package_fn: Option<&PackageLookupFn<'_>>,
    conn: Option<&Connection>,
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

    // Helper to resolve import with DB or path-based fallback
    let resolve_import = |file_path: &str, imp: &Import| -> Option<String> {
        if let Some(c) = conn {
            resolve_imported_symbol_id_with_db(file_path, imp, c)
        } else {
            resolve_imported_symbol_id(file_path, imp)
        }
    };

    for call in extract_calls(&row.text) {
        let callee = call.method.clone();
        let (to_id, was_import) = if let Some(local_id) = name_to_id.get(&callee) {
            if local_id == &row.id {
                continue;
            }
            (Some(local_id.clone()), false)
        } else if let Some(imp) = import_map.get(callee.as_str()) {
            // Resolve import using DB when available, path-based otherwise
            (resolve_import(&row.file_path, imp), true)
        } else if let Some(receiver) = call.receiver.as_deref() {
            // `receiver.callee(...)` where `callee` is not locally defined and
            // not directly imported. Resolve via the receiver's import: if
            // `receiver` is imported from "./session-manager", look up
            // `callee` as ANY symbol (method, function, const) in that file.
            // resolve_imported_symbol_id_with_db only matches `exported=true`
            // symbols, which excludes class methods like
            // `SessionManager.createSession` -- those carry the method on the
            // class, not as a top-level export.
            (
                resolve_method_on_receiver(conn, &row.file_path, receiver, &callee, &import_map),
                true,
            )
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
                (resolve_import(&row.file_path, imp), true)
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
                (resolve_import(&row.file_path, imp), true)
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
                (resolve_import(&row.file_path, imp), true)
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
            (resolve_import(&row.file_path, imp), true)
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
                // The budget caps emitted reference EDGES. Counting every
                // scanned identifier (the old behavior) let locals/builtins
                // early in a body burn the budget before later imported
                // identifiers (e.g. drizzle schema tables) were reached.
                refs_added += 1;
            }
        }
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
                (resolve_import(&row.file_path, imp), true)
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
        // Detect async boundary prefixes before resolution
        let (async_kind, actual_from) = if let Some(rest) = dfe.from_symbol.strip_prefix("await:") {
            (Some("async_call"), rest.to_string())
        } else if let Some(rest) = dfe.from_symbol.strip_prefix("spawn:") {
            (Some("spawn"), rest.to_string())
        } else {
            (None, dfe.from_symbol.clone())
        };

        // Resolve actual_from to actual symbol ID
        let (to_id, was_import) = if let Some(local_id) = name_to_id.get(&actual_from) {
            if local_id == &row.id {
                continue;
            }
            (Some(local_id.clone()), false)
        } else if let Some(imp) = import_map.get(actual_from.as_str()) {
            (resolve_import(&row.file_path, imp), true)
        } else if let Some(ref scope) = dfe.scope {
            // Local variable — create synthetic ID for scope-aware tracking.
            // SQLite FK enforcement is OFF during batch writes, so synthetic IDs
            // without corresponding symbol rows are safe.
            let synthetic_id = format!("local:{}#{}::{}", row.file_path, scope, actual_from);
            (Some(synthetic_id), false)
        } else if async_kind.is_some() {
            // Async edge without scope — create a synthetic boundary ID
            let synthetic_id = format!("async:{}#{}", row.file_path, actual_from);
            (Some(synthetic_id), false)
        } else {
            // No scope information available — cannot create a useful edge
            continue;
        };

        if let Some(id) = to_id {
            let edge_type = if let Some(ak) = async_kind {
                ak
            } else {
                match dfe.flow_type {
                    DataFlowType::Reads => "reads",
                    DataFlowType::Writes => "writes",
                }
            };

            let confidence = if async_kind.is_some() { 0.9 } else { 0.7 };

            // Skip if we already have this edge type to this target
            if !used_edges.insert((edge_type.to_string(), id.clone())) {
                continue;
            }

            let resolution = if id.starts_with("local:") {
                // Synthetic local-variable ID — no real symbol to look up
                "local-variable".to_string()
            } else if id.starts_with("async:") {
                // Synthetic async-boundary ID — no real symbol to look up
                "async-boundary".to_string()
            } else {
                compute_resolution_for_target(&resolution_ctx, &id, was_import)
            };

            out.push((
                EdgeRow {
                    from_symbol_id: row.id.clone(),
                    to_symbol_id: id,
                    edge_type: edge_type.to_string(),
                    at_file: Some(row.file_path.clone()),
                    at_line: Some(dfe.at_line),
                    confidence,
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

    /// Regression: when the call is `receiver.method()` and `method` is
    /// not directly imported but `receiver` is, the resolver must look up
    /// `method` as ANY symbol (not just exported) in the file that
    /// `receiver` was imported from. This catches class methods like
    /// `sessionManager.createSession` where createSession lives as a
    /// method on the SessionManager class (not as a top-level export).
    #[test]
    fn resolve_method_on_receiver_resolves_class_method_through_import() {
        use crate::storage::sqlite::queries;
        use crate::storage::sqlite::schema::SCHEMA_SQL;
        use rusqlite::Connection;

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();

        // Seed: session-manager.ts has a non-exported createSession method.
        let target_id = "real_create_session_id";
        let create_session = symbol(
            target_id,
            "createSession",
            "function",
            "createSession(cwd: string) { ... }",
            "src/main/session-manager.ts",
        );
        let mut sm_row = create_session.clone();
        sm_row.exported = false; // method, not top-level export
        queries::symbols::batch_upsert_symbols(&conn, std::slice::from_ref(&sm_row)).unwrap();

        // The calling file imports `sessionManager` (the instance), then
        // invokes its method `createSession`.
        let row = symbol(
            "ipc_id",
            "registerIpcHandlers",
            "function",
            "export function registerIpcHandlers() {\n  return sessionManager.createSession(cwd);\n}",
            "src/main/ipc-handlers.ts",
        );
        let name_to_id = HashMap::new();
        let id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();
        let imports = vec![Import {
            name: "sessionManager".to_string(),
            source: "./session-manager".to_string(),
            alias: None,
        }];

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &imports,
            &[],
            &[],
            None,
            Some(&conn),
        );

        let call_edge = edges
            .iter()
            .find(|(e, _)| e.edge_type == "call" && e.to_symbol_id == target_id);
        assert!(
            call_edge.is_some(),
            "expected a call edge to src/main/session-manager.ts::createSession; got edges: {:?}",
            edges
                .iter()
                .map(|(e, _)| (e.edge_type.clone(), e.to_symbol_id.clone()))
                .collect::<Vec<_>>()
        );
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
            None, // No sqlite in tests
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
    fn reference_budget_counts_emitted_edges_not_scanned_identifiers() {
        // A drizzle-style handler references an imported schema table late in
        // a body full of locals. The old cap burned the 20-identifier budget
        // on unresolvable names before reaching the import, so the table
        // never got a reference edge (real miss: desktopAuthExchangeCodes in
        // wolfmax's desktop-auth.ts, docked by judges in every trace answer).
        let locals: String = (0..25)
            .map(|i| format!("  const local{i} = other{i};\n"))
            .collect();
        // No import line: a symbol's text is the handler body only; file-top
        // imports arrive via the imports parameter.
        let text = format!(
            "export function handler() {{\n{locals}  db.insert(exchangeCodes).values({{}});\n}}"
        );
        let row = symbol("id_h", "handler", "function", &text, "src/x.ts");
        let imports = vec![Import {
            name: "exchangeCodes".to_string(),
            source: "./schema".to_string(),
            alias: None,
        }];

        let edges = extract_edges_for_symbol(
            &row,
            &HashMap::new(),
            &HashMap::new(),
            &imports,
            &[],
            &[],
            None,
            None,
        );

        let expected = stable_symbol_id("src/schema.ts", "exchangeCodes", 0);
        assert!(
            edges
                .iter()
                .any(|(e, _)| e.edge_type == "reference" && e.to_symbol_id == expected),
            "imported identifier used late in the body must still get a reference edge; got: {:?}",
            edges
                .iter()
                .map(|(e, _)| (e.edge_type.clone(), e.to_symbol_id.clone()))
                .collect::<Vec<_>>()
        );
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
            None, // No sqlite in tests
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
            None, // No sqlite in tests
        );

        let edge_to_b = edges
            .iter()
            .find(|(e, _)| e.to_symbol_id == "id_b" && e.edge_type == "call");

        assert!(edge_to_b.is_some());
        // Same file should be "local"
        assert_eq!(edge_to_b.unwrap().0.resolution, "local");
    }

    #[test]
    fn test_dataflow_local_variable_creates_synthetic_edge() {
        // Function symbol that contains a local variable "x"
        let row = symbol(
            "fn-1",
            "process",
            "function",
            "fn process() { let x = foo(); }",
            "src/main.rs",
        );
        // Peer function "foo" that is resolvable via name_to_id
        let foo = symbol(
            "fn-2",
            "foo",
            "function",
            "fn foo() -> i32 { 42 }",
            "src/main.rs",
        );

        let mut name_to_id = HashMap::new();
        upsert_name_mapping(&mut name_to_id, &foo);
        // "x" is intentionally absent — it is a local variable

        let mut id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();
        id_to_symbol.insert("fn-2".to_string(), &foo);

        let dataflow_edges = vec![
            DataFlowEdge {
                from_symbol: "foo".to_string(), // Resolvable via name_to_id
                to_symbol: "process".to_string(),
                flow_type: DataFlowType::Reads,
                at_line: 1,
                scope: Some("process".to_string()),
            },
            DataFlowEdge {
                from_symbol: "x".to_string(), // Local variable — not in name_to_id
                to_symbol: "process".to_string(),
                flow_type: DataFlowType::Writes,
                at_line: 1,
                scope: Some("process".to_string()),
            },
        ];

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &[], // no imports
            &[], // no type edges
            &dataflow_edges,
            None, // no package lookup
            None, // no sqlite
        );

        // The "foo" reads edge must resolve to fn-2 with normal resolution
        let foo_edge = edges
            .iter()
            .find(|(e, _)| e.to_symbol_id == "fn-2" && e.edge_type == "reads");
        assert!(foo_edge.is_some(), "Expected a 'reads' edge to fn-2 (foo)");
        assert_eq!(
            foo_edge.unwrap().0.resolution,
            "local",
            "foo is in the same file, so resolution must be 'local'"
        );

        // The "x" writes edge must use a synthetic ID and resolution = "local-variable"
        let expected_synthetic_id = "local:src/main.rs#process::x";
        let x_edge = edges
            .iter()
            .find(|(e, _)| e.to_symbol_id == expected_synthetic_id && e.edge_type == "writes");
        assert!(
            x_edge.is_some(),
            "Expected a 'writes' edge with synthetic ID '{expected_synthetic_id}'"
        );
        assert_eq!(
            x_edge.unwrap().0.resolution,
            "local-variable",
            "Synthetic local variable edge must have resolution 'local-variable'"
        );
    }

    #[test]
    fn test_dataflow_local_variable_no_scope_drops_edge() {
        // When scope is None and the symbol is unknown, the edge must be dropped
        let row = symbol(
            "fn-3",
            "compute",
            "function",
            "fn compute() { let z = bar(); }",
            "src/lib.rs",
        );

        let name_to_id = HashMap::new(); // "bar" and "z" are both absent
        let id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();

        let dataflow_edges = vec![DataFlowEdge {
            from_symbol: "z".to_string(), // Local variable with NO scope
            to_symbol: "compute".to_string(),
            flow_type: DataFlowType::Writes,
            at_line: 1,
            scope: None, // No scope — edge must be silently dropped
        }];

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &[],
            &[],
            &dataflow_edges,
            None,
            None,
        );

        assert!(
            edges.is_empty(),
            "Edge with unknown symbol and no scope must be dropped"
        );
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
            None, // No sqlite in tests
        );

        let edge_to_b = edges
            .iter()
            .find(|(e, _)| e.to_symbol_id == "id_b" && e.edge_type == "call");

        assert!(edge_to_b.is_some());
        // Cross-package (local reference, different packages) should be "cross-package"
        assert_eq!(edge_to_b.unwrap().0.resolution, "cross-package");
    }

    #[test]
    fn test_async_await_prefix_creates_async_call_edge() {
        let row = symbol(
            "fn-caller",
            "fetch_user",
            "function",
            "async fn fetch_user() { let data = fetch_data().await; }",
            "src/api.rs",
        );

        let callee = symbol(
            "fn-callee",
            "fetch_data",
            "function",
            "async fn fetch_data() -> Vec<u8> { vec![] }",
            "src/api.rs",
        );

        let mut name_to_id = HashMap::new();
        upsert_name_mapping(&mut name_to_id, &callee);

        let mut id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();
        id_to_symbol.insert("fn-callee".to_string(), &callee);

        let dataflow_edges = vec![DataFlowEdge {
            from_symbol: "await:fetch_data".to_string(),
            to_symbol: "fetch_user".to_string(),
            flow_type: DataFlowType::Reads,
            at_line: 2,
            scope: Some("fetch_user".to_string()),
        }];

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &[],
            &[],
            &dataflow_edges,
            None,
            None,
        );

        let async_edge = edges
            .iter()
            .find(|(e, _)| e.edge_type == "async_call" && e.to_symbol_id == "fn-callee");

        assert!(
            async_edge.is_some(),
            "Expected an async_call edge to fn-callee, got: {edges:?}"
        );
        let (edge, _) = async_edge.unwrap();
        assert!(
            (edge.confidence - 0.9_f32).abs() < f32::EPSILON,
            "async_call confidence must be 0.9, got {}",
            edge.confidence
        );
    }

    #[test]
    fn test_spawn_prefix_creates_spawn_edge() {
        let row = symbol(
            "fn-spawner",
            "start_worker",
            "function",
            "fn start_worker() { tokio::spawn(worker()); }",
            "src/tasks.rs",
        );

        let worker = symbol(
            "fn-worker",
            "worker",
            "function",
            "async fn worker() {}",
            "src/tasks.rs",
        );

        let mut name_to_id = HashMap::new();
        upsert_name_mapping(&mut name_to_id, &worker);

        let mut id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();
        id_to_symbol.insert("fn-worker".to_string(), &worker);

        let dataflow_edges = vec![DataFlowEdge {
            from_symbol: "spawn:worker".to_string(),
            to_symbol: "start_worker".to_string(),
            flow_type: DataFlowType::Reads,
            at_line: 1,
            scope: Some("start_worker".to_string()),
        }];

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &[],
            &[],
            &dataflow_edges,
            None,
            None,
        );

        let spawn_edge = edges
            .iter()
            .find(|(e, _)| e.edge_type == "spawn" && e.to_symbol_id == "fn-worker");

        assert!(
            spawn_edge.is_some(),
            "Expected a spawn edge to fn-worker, got: {edges:?}"
        );
        let (edge, _) = spawn_edge.unwrap();
        assert!(
            (edge.confidence - 0.9_f32).abs() < f32::EPSILON,
            "spawn confidence must be 0.9, got {}",
            edge.confidence
        );
    }

    #[test]
    fn test_no_prefix_creates_reads_writes_edge() {
        let row = symbol(
            "fn-main",
            "process",
            "function",
            "fn process() { let x = helper(); }",
            "src/lib.rs",
        );

        let helper = symbol(
            "fn-helper",
            "helper",
            "function",
            "fn helper() -> i32 { 0 }",
            "src/lib.rs",
        );

        let mut name_to_id = HashMap::new();
        upsert_name_mapping(&mut name_to_id, &helper);

        let mut id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();
        id_to_symbol.insert("fn-helper".to_string(), &helper);

        let dataflow_edges = vec![DataFlowEdge {
            from_symbol: "helper".to_string(),
            to_symbol: "process".to_string(),
            flow_type: DataFlowType::Reads,
            at_line: 1,
            scope: Some("process".to_string()),
        }];

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &[],
            &[],
            &dataflow_edges,
            None,
            None,
        );

        let reads_edge = edges
            .iter()
            .find(|(e, _)| e.to_symbol_id == "fn-helper" && e.edge_type == "reads");

        assert!(
            reads_edge.is_some(),
            "Expected a reads edge to fn-helper, got: {edges:?}"
        );
        let (edge, _) = reads_edge.unwrap();
        assert!(
            (edge.confidence - 0.7_f32).abs() < f32::EPSILON,
            "reads confidence must be 0.7, got {}",
            edge.confidence
        );

        assert!(
            !edges
                .iter()
                .any(|(e, _)| e.edge_type == "async_call" || e.edge_type == "spawn"),
            "Unexpected async edge in plain reads scenario"
        );
    }

    #[test]
    fn test_async_edge_without_scope_gets_synthetic_id() {
        let row = symbol(
            "fn-entry",
            "run",
            "function",
            "async fn run() { unknown_func().await; }",
            "src/main.rs",
        );

        let name_to_id = HashMap::new();
        let id_to_symbol: HashMap<String, &SymbolRow> = HashMap::new();

        let dataflow_edges = vec![DataFlowEdge {
            from_symbol: "await:unknown_func".to_string(),
            to_symbol: "run".to_string(),
            flow_type: DataFlowType::Reads,
            at_line: 1,
            scope: None,
        }];

        let edges = extract_edges_for_symbol(
            &row,
            &name_to_id,
            &id_to_symbol,
            &[],
            &[],
            &dataflow_edges,
            None,
            None,
        );

        assert!(
            !edges.is_empty(),
            "async edge with unknown callee must not be dropped"
        );

        let async_edge = edges.iter().find(|(e, _)| e.edge_type == "async_call");

        assert!(
            async_edge.is_some(),
            "Expected an async_call edge, got: {edges:?}"
        );
        let (edge, _) = async_edge.unwrap();

        assert!(
            edge.to_symbol_id.starts_with("async:"),
            "to_symbol_id must start with 'async:', got '{}'",
            edge.to_symbol_id
        );

        assert_eq!(
            edge.resolution, "async-boundary",
            "resolution must be 'async-boundary', got '{}'",
            edge.resolution
        );
    }
}

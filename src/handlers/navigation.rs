//! Navigation-related MCP tool handlers
//!
//! Covers symbol lookup, reference finding, usage examples, and file/module
//! summarization tools.

use std::collections::HashSet;

use anyhow::Result;
use serde_json::json;

use crate::external_index::provider::{merged_references_to_internal_symbol, MergedReference};
use crate::path::{PathError, PathNormalizer, Utf8PathBuf};
use crate::retrieval::assembler::FormatMode;
use crate::storage::sqlite::{SqliteStore, SymbolIdentityRow, SymbolRow};
use crate::tools::*;

use super::budget::{
    budget_array, budget_string_field, clamp_limit, insert_budgeted_array, DEFAULT_MAX_STRING_CHARS,
};
use super::symbol_resolution::{candidate_values, resolve_symbol, SymbolResolution};
use super::{extract_usage_line, AppState};

#[cfg(test)]
mod usage_examples_tests {
    use super::*;
    use crate::storage::sqlite::{EdgeRow, SqliteStore};

    fn sym(id: &str, file: &str, name: &str, text: &str) -> SymbolRow {
        SymbolRow {
            id: id.into(),
            file_path: file.into(),
            language: "rust".into(),
            kind: "function".into(),
            name: name.into(),
            exported: true,
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            end_line: 3,
            text: text.into(),
        }
    }

    fn call_edge(from: &str, to: &str, at_file: &str, at_line: u32) -> EdgeRow {
        EdgeRow {
            from_symbol_id: from.into(),
            to_symbol_id: to.into(),
            edge_type: "call".into(),
            at_file: Some(at_file.into()),
            at_line: Some(at_line),
            confidence: 1.0,
            evidence_count: 1,
            resolution: "exact".into(),
        }
    }

    #[test]
    fn collect_usage_examples_scopes_to_file_when_given() {
        let store = SqliteStore::open_in_memory().unwrap();
        store.init().unwrap();

        // Two distinct `foo` symbols in different files, each with one caller.
        for s in [
            sym("foo_a", "a.rs", "foo", "fn foo() {}"),
            sym("foo_b", "b.rs", "foo", "fn foo() {}"),
            sym("caller_a", "ca.rs", "callerA", "fn callerA() { foo() }"),
            sym("caller_b", "cb.rs", "callerB", "fn callerB() { foo() }"),
        ] {
            store.upsert_symbol(&s).unwrap();
        }
        store
            .upsert_edge(&call_edge("caller_a", "foo_a", "ca.rs", 1))
            .unwrap();
        store
            .upsert_edge(&call_edge("caller_b", "foo_b", "cb.rs", 1))
            .unwrap();

        // Scoped to a.rs: only caller_a's example surfaces.
        let scoped = collect_usage_examples(&store, "foo", Some("a.rs"), 20).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0]["from_symbol_name"], "callerA");

        // Unscoped (None): both callers surface.
        let all = collect_usage_examples(&store, "foo", None, 20).unwrap();
        assert_eq!(all.len(), 2);
    }
}

#[cfg(test)]
mod find_references_tests {
    use super::*;
    use std::path::Path;

    use crate::external_index::importer::import_external_index;
    use crate::external_index::provider::{MergedReference, ReferenceSource};
    use crate::storage::sqlite::{EdgeRow, SqliteStore, SymbolRow};

    fn merged_reference(source: ReferenceSource) -> MergedReference {
        MergedReference {
            to_symbol_id: "target_internal".to_string(),
            from_symbol_id: Some("caller_internal".to_string()),
            from_external_symbol_id: match source {
                ReferenceSource::Native => None,
                ReferenceSource::External => Some("external-caller".to_string()),
            },
            from_symbol_name: match source {
                ReferenceSource::Native => Some("caller".to_string()),
                ReferenceSource::External => None,
            },
            from_symbol_file: match source {
                ReferenceSource::Native => Some("src/caller.ts".to_string()),
                ReferenceSource::External => None,
            },
            reference_type: "call".to_string(),
            at_file: Some("src/caller.ts".to_string()),
            at_line: Some(2),
            at_column: match source {
                ReferenceSource::Native => None,
                ReferenceSource::External => Some(12),
            },
            at_end_line: match source {
                ReferenceSource::Native => None,
                ReferenceSource::External => Some(2),
            },
            at_end_column: match source {
                ReferenceSource::Native => None,
                ReferenceSource::External => Some(20),
            },
            source,
            confidence: match source {
                ReferenceSource::Native => 0.75,
                ReferenceSource::External => 1.0,
            },
            external_index_id: match source {
                ReferenceSource::Native => None,
                ReferenceSource::External => Some("external:fixture".to_string()),
            },
            provenance: match source {
                ReferenceSource::Native => None,
                ReferenceSource::External => Some("fixture".to_string()),
            },
            metadata_json: match source {
                ReferenceSource::Native => None,
                ReferenceSource::External => Some("{}".to_string()),
            },
        }
    }

    fn store_with_symbols() -> SqliteStore {
        let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
        store.init().expect("init sqlite");
        insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
        insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);
        store
    }

    fn insert_symbol(
        store: &SqliteStore,
        id: &str,
        file_path: &str,
        name: &str,
        start_line: u32,
        end_line: u32,
    ) {
        store
            .upsert_symbol(&SymbolRow {
                id: id.to_string(),
                file_path: file_path.to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: name.to_string(),
                exported: false,
                start_byte: 0,
                end_byte: 60,
                start_line,
                end_line,
                text: format!("function {name}() {{}}"),
            })
            .expect("insert symbol");
    }

    fn insert_edge(
        store: &SqliteStore,
        from_symbol_id: &str,
        to_symbol_id: &str,
        edge_type: &str,
        at_file: &str,
        at_line: u32,
        confidence: f32,
    ) {
        store
            .upsert_edge(&EdgeRow {
                from_symbol_id: from_symbol_id.to_string(),
                to_symbol_id: to_symbol_id.to_string(),
                edge_type: edge_type.to_string(),
                at_file: Some(at_file.to_string()),
                at_line: Some(at_line),
                confidence,
                evidence_count: 1,
                resolution: "resolved".to_string(),
            })
            .expect("insert edge");
    }

    #[test]
    fn response_includes_imported_external_reference_provenance() {
        let store = store_with_symbols();
        let report = import_external_index(
            &store,
            "/fixture/repo",
            Path::new("tests/fixtures/external_index/typescript-normalized.json"),
        )
        .expect("import external index");

        let response = collect_find_references_response(
            &store,
            "target".to_string(),
            None,
            Some("call".to_string()),
            Some(20),
        )
        .expect("find references response");

        assert_eq!(response["symbol_name"], "target");
        assert_eq!(response["reference_type"], "call");
        assert_eq!(response["count"], 1);
        let reference = &response["references"][0];
        assert_eq!(reference["to_symbol_id"], "target_internal");
        assert_eq!(reference["reference_type"], "call");
        assert_eq!(reference["at_file"], "src/caller.ts");
        assert_eq!(reference["at_line"], 2);
        assert_eq!(reference["source"], "external");
        assert_eq!(reference["confidence"], 1.0);
        assert_eq!(reference["external_index_id"], report.index_id);
        assert_eq!(reference["provenance"], "fixture");
        assert_eq!(reference["metadata_json"], "{}");
    }

    #[test]
    fn response_keeps_native_fallback_old_fields_with_source() {
        let store = store_with_symbols();
        insert_edge(
            &store,
            "caller_internal",
            "target_internal",
            "call",
            "src/caller.ts",
            2,
            0.75,
        );

        let response = collect_find_references_response(
            &store,
            "target".to_string(),
            None,
            Some("call".to_string()),
            Some(20),
        )
        .expect("find references response");

        assert_eq!(response["count"], 1);
        let reference = &response["references"][0];
        assert_eq!(reference["to_symbol_id"], "target_internal");
        assert_eq!(reference["from_symbol_id"], "caller_internal");
        assert_eq!(reference["from_symbol_name"], "caller");
        assert_eq!(reference["from_symbol_file"], "src/caller.ts");
        assert_eq!(reference["reference_type"], "call");
        assert_eq!(reference["at_file"], "src/caller.ts");
        assert_eq!(reference["at_line"], 2);
        assert_eq!(reference["source"], "native");
        assert_eq!(reference["confidence"], 0.75);
        assert!(reference["external_index_id"].is_null());
        assert!(reference["provenance"].is_null());
        assert!(reference["metadata_json"].is_null());
    }

    #[test]
    fn response_preserves_targets_and_disambiguation_for_same_name_roots() {
        let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
        store.init().expect("init sqlite");
        insert_symbol(&store, "target_a", "src/a.ts", "target", 1, 3);
        insert_symbol(&store, "target_b", "src/b.ts", "target", 1, 3);
        insert_symbol(&store, "caller_a", "src/caller_a.ts", "callerA", 1, 4);
        insert_symbol(&store, "caller_b", "src/caller_b.ts", "callerB", 1, 4);
        insert_edge(
            &store,
            "caller_a",
            "target_a",
            "call",
            "src/caller_a.ts",
            2,
            1.0,
        );
        insert_edge(
            &store,
            "caller_b",
            "target_b",
            "call",
            "src/caller_b.ts",
            2,
            1.0,
        );

        let response = collect_find_references_response(
            &store,
            "target".to_string(),
            None,
            Some("call".to_string()),
            Some(20),
        )
        .expect("find references response");

        assert_eq!(response["count"], 2);
        assert_eq!(response["resolution"], "ambiguous");
        assert_eq!(response["logical_count"], 2);
        assert_eq!(response["targets"].as_array().expect("targets").len(), 2);
        assert!(response["disambiguation"].is_object());
        assert_eq!(
            response["disambiguation"]["candidates"]
                .as_array()
                .expect("candidates")
                .len(),
            2
        );
    }

    #[test]
    fn formats_external_reference_with_provenance_overlay_fields() {
        let value = format_find_reference(merged_reference(ReferenceSource::External));

        assert_eq!(value["to_symbol_id"], "target_internal");
        assert_eq!(value["from_symbol_id"], "caller_internal");
        assert_eq!(value["from_symbol_name"], "");
        assert_eq!(value["from_symbol_file"], "");
        assert_eq!(value["reference_type"], "call");
        assert_eq!(value["at_file"], "src/caller.ts");
        assert_eq!(value["at_line"], 2);
        assert_eq!(value["source"], "external");
        assert_eq!(value["confidence"], 1.0);
        assert_eq!(value["external_index_id"], "external:fixture");
        assert_eq!(value["provenance"], "fixture");
        assert_eq!(value["metadata_json"], "{}");
        assert_eq!(value["from_external_symbol_id"], "external-caller");
        assert_eq!(value["at_column"], 12);
        assert_eq!(value["at_end_line"], 2);
        assert_eq!(value["at_end_column"], 20);
    }

    #[test]
    fn formats_external_reference_without_internal_caller_with_legacy_string_fallback() {
        let mut reference = merged_reference(ReferenceSource::External);
        reference.from_symbol_id = None;

        let value = format_find_reference(reference);

        assert_eq!(value["from_symbol_id"], "");
        assert_eq!(value["from_symbol_name"], "");
        assert_eq!(value["from_symbol_file"], "");
        assert_eq!(value["source"], "external");
        assert_eq!(value["external_index_id"], "external:fixture");
        assert_eq!(value["provenance"], "fixture");
        assert_eq!(value["metadata_json"], "{}");
        assert_eq!(value["from_external_symbol_id"], "external-caller");
    }

    #[test]
    fn formats_native_reference_with_backward_compatible_old_fields() {
        let value = format_find_reference(merged_reference(ReferenceSource::Native));

        assert_eq!(value["to_symbol_id"], "target_internal");
        assert_eq!(value["from_symbol_id"], "caller_internal");
        assert_eq!(value["from_symbol_name"], "caller");
        assert_eq!(value["from_symbol_file"], "src/caller.ts");
        assert_eq!(value["reference_type"], "call");
        assert_eq!(value["at_file"], "src/caller.ts");
        assert_eq!(value["at_line"], 2);
        assert_eq!(value["source"], "native");
        assert_eq!(value["confidence"], 0.75);
        assert!(value["external_index_id"].is_null());
        assert!(value["provenance"].is_null());
        assert!(value["metadata_json"].is_null());
        assert!(value["from_external_symbol_id"].is_null());
        assert!(value["at_column"].is_null());
        assert!(value["at_end_line"].is_null());
        assert!(value["at_end_column"].is_null());
    }
}

/// Handle get_definition tool
pub async fn handle_get_definition(
    state: &AppState,
    tool: GetDefinitionTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = clamp_limit(tool.limit, 10, 100);

    let sqlite = &state.sqlite;

    let resolved = resolve_symbol(sqlite, &tool.symbol_name, tool.file.as_deref(), limit)?;
    let resolution = resolved.state();
    let logical_count = resolved.logical_count();
    let rows = resolved
        .groups()
        .into_iter()
        .flat_map(|group| group.occurrences.iter().cloned())
        .take(limit)
        .collect::<Vec<_>>();

    let symbol_ids = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
    let identities = sqlite.get_symbol_identities(&symbol_ids)?;
    let context = state.retriever.assemble_definitions(&rows)?;

    // Multiple occurrences of one logical overload/partial-declaration set are
    // exact. Distinct logical identities require owner/file disambiguation.
    let needs_disambiguation = matches!(resolved, SymbolResolution::Ambiguous(_));

    let mut response = json!({
        "symbol_name": tool.symbol_name,
        "count": rows.len(),
        "logical_count": logical_count,
        "resolution": resolution,
        "definitions": symbol_summaries(&rows, &identities),
        "context": context,
    });
    budget_string_field(&mut response, "context", DEFAULT_MAX_STRING_CHARS);

    if needs_disambiguation {
        let groups = match &resolved {
            SymbolResolution::Ambiguous(groups) => groups.as_slice(),
            _ => &[],
        };
        response["disambiguation"] = json!({
            "hint": format!(
                "Multiple logical '{}' symbols found. Use an owner-qualified name and/or the 'file' parameter to disambiguate.",
                tool.symbol_name,
            ),
            "candidates": candidate_values(groups),
        });
    }

    Ok(response)
}

fn symbol_summaries(
    rows: &[SymbolRow],
    identities: &std::collections::HashMap<String, SymbolIdentityRow>,
) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| {
            let identity = identities.get(&row.id);
            json!({
                "id": row.id,
                "occurrence_id": row.id,
                "logical_id": identity.map(|value| value.logical_id.as_str()).unwrap_or(row.id.as_str()),
                "qualified_name": identity.map(|value| value.qualified_name.as_str()).unwrap_or(row.name.as_str()),
                "signature": identity.map(|value| value.signature.as_str()).unwrap_or(""),
                "occurrence_discriminator": identity.map(|value| value.occurrence_discriminator.as_str()).unwrap_or("legacy"),
                "is_canonical": identity.map(|value| value.is_canonical).unwrap_or(true),
                "file_path": row.file_path,
                "language": row.language,
                "kind": row.kind,
                "name": row.name,
                "exported": row.exported,
                "start_byte": row.start_byte,
                "end_byte": row.end_byte,
                "start_line": row.start_line,
                "end_line": row.end_line,
            })
        })
        .collect()
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
    let utf8_path = Utf8PathBuf::from_path_buf(path_buf).map_err(|_| PathError::NonUtf8 {
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

    // Drop per-row file_path (it equals file_path_normalized for every row,
    // and the response already carries it once at the top level).
    let symbols: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "language": r.language,
                "kind": r.kind,
                "name": r.name,
                "exported": r.exported,
                "start_byte": r.start_byte,
                "end_byte": r.end_byte,
                "start_line": r.start_line,
                "end_line": r.end_line,
            })
        })
        .collect();

    let mut response = json!({
        "file_path": tool.file_path,
        "count": rows.len(),
        "symbols": symbols,
    });
    if file_path_normalized != tool.file_path {
        response["file_path_normalized"] = json!(file_path_normalized);
    }
    Ok(response)
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

    let verbose = tool.verbose.unwrap_or(false);
    let assembler = crate::retrieval::assembler::ContextAssembler::new(state.config.clone());
    let (context, context_items) =
        assembler.format_context_with_mode(sqlite, &rows, &[], &[], mode, None)?;

    let mut response = json!({
        "count": rows.len(),
        "missing_ids": missing,
        "context": context,
    });
    budget_string_field(&mut response, "context", DEFAULT_MAX_STRING_CHARS);
    if verbose {
        response["context_items"] = serde_json::to_value(&context_items)?;
    }
    Ok(response)
}

/// Handle find_references tool
pub fn handle_find_references(
    state: &AppState,
    tool: FindReferencesTool,
) -> Result<serde_json::Value, anyhow::Error> {
    collect_find_references_response(
        state.sqlite.as_ref(),
        tool.symbol_name,
        tool.file,
        tool.reference_type,
        tool.limit,
    )
}

fn collect_find_references_response(
    sqlite: &SqliteStore,
    symbol_name: String,
    file: Option<String>,
    requested_reference_type: Option<String>,
    requested_limit: Option<u32>,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = clamp_limit(requested_limit, 200, 500);
    let reference_type = requested_reference_type.unwrap_or_else(|| "all".to_string());
    let resolved = resolve_symbol(sqlite, &symbol_name, file.as_deref(), 100)?;
    let resolution = resolved.state();
    let logical_count = resolved.logical_count();
    let roots = resolved
        .groups()
        .into_iter()
        .flat_map(|group| group.occurrences.iter())
        .take(100)
        .collect::<Vec<_>>();
    let needs_disambiguation = matches!(resolved, SymbolResolution::Ambiguous(_));

    let mut out = Vec::new();
    let mut targets: Vec<serde_json::Value> = Vec::new();
    let mut seen_targets: HashSet<String> = HashSet::new();
    for root in &roots {
        if out.len() >= limit {
            break;
        }
        if seen_targets.insert(root.id.clone()) {
            targets.push(json!({
                "symbol_id": root.id,
                "symbol_name": root.name,
                "file_path": root.file_path,
            }));
        }
        let remaining_limit = limit.saturating_sub(out.len());
        let relationship_filter = if reference_type.eq_ignore_ascii_case("all") {
            None
        } else {
            Some(reference_type.as_str())
        };
        let references = merged_references_to_internal_symbol(
            sqlite,
            &root.id,
            relationship_filter,
            remaining_limit,
        )?;
        for reference in references {
            out.push(format_find_reference(reference));
        }
    }

    let budgeted_references = budget_array(out, limit);
    let mut response = json!({
        "symbol_name": symbol_name,
        "reference_type": reference_type,
        "resolution": resolution,
        "logical_count": logical_count,
        "count": budgeted_references.returned_count,
        "targets": targets,
    });
    insert_budgeted_array(&mut response, "references", budgeted_references)?;

    // Add disambiguation hints when multiple logical symbols exist.
    if needs_disambiguation {
        let groups = match &resolved {
            SymbolResolution::Ambiguous(groups) => groups.as_slice(),
            _ => &[],
        };
        response["disambiguation"] = json!({
            "hint": format!(
                "Multiple logical '{}' symbols found ({} candidates). Results include references to all. Use an owner-qualified name and/or file parameter to disambiguate.",
                symbol_name,
                logical_count
            ),
            "candidates": candidate_values(groups),
        });
    }

    Ok(response)
}

fn format_find_reference(reference: MergedReference) -> serde_json::Value {
    json!({
        "to_symbol_id": reference.to_symbol_id,
        "from_symbol_id": reference.from_symbol_id.unwrap_or_default(),
        "from_symbol_name": reference.from_symbol_name.unwrap_or_default(),
        "from_symbol_file": reference.from_symbol_file.unwrap_or_default(),
        "reference_type": reference.reference_type,
        "at_file": reference.at_file,
        "at_line": reference.at_line,
        "source": reference.source,
        "confidence": reference.confidence,
        "external_index_id": reference.external_index_id,
        "provenance": reference.provenance,
        "metadata_json": reference.metadata_json,
        "from_external_symbol_id": reference.from_external_symbol_id,
        "at_column": reference.at_column,
        "at_end_line": reference.at_end_line,
        "at_end_column": reference.at_end_column,
    })
}

/// Handle get_usage_examples tool
pub fn handle_get_usage_examples(
    state: &AppState,
    tool: GetUsageExamplesTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = clamp_limit(tool.limit, 20, 100);

    // When a file is given, scope examples to the symbol defined there so a
    // common name does not pull in call sites of an unrelated same-named symbol.
    let normalized_file = tool.file.as_ref().map(|f| {
        let normalizer = PathNormalizer::new(state.config.base_dir.clone());
        normalizer
            .relative_to_base(&Utf8PathBuf::from(f.as_str()))
            .map(|p| p.to_string())
            .unwrap_or_else(|_| f.clone())
    });

    let examples = collect_usage_examples(
        &state.sqlite,
        &tool.symbol_name,
        normalized_file.as_deref(),
        limit,
    )?;

    let budgeted_examples = budget_array(examples, limit);
    let mut response = json!({
        "symbol_name": tool.symbol_name,
        "count": budgeted_examples.returned_count,
    });
    insert_budgeted_array(&mut response, "examples", budgeted_examples)?;
    Ok(response)
}

/// Gather usage examples for `symbol_name`, optionally scoped to symbols defined
/// in `file` (base-relative). Prefers stored usage examples; falls back to
/// incoming call/import/reference edges. Returns the un-budgeted example list.
fn collect_usage_examples(
    sqlite: &SqliteStore,
    symbol_name: &str,
    file: Option<&str>,
    limit: usize,
) -> Result<Vec<serde_json::Value>> {
    let roots = sqlite.search_symbols_by_exact_name(symbol_name, file, 20)?;

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

    Ok(examples)
}

/// Handle get_module_summary tool
pub fn handle_get_module_summary(
    state: &AppState,
    tool: GetModuleSummaryTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let group_by_kind = tool.group_by_kind.unwrap_or(false);
    let include_display = tool.include_display.unwrap_or(false);
    let requested_file_path = tool.file_path.clone();

    tracing::debug!(
        file_path = %tool.file_path,
        group_by_kind = group_by_kind,
        "get_module_summary called"
    );

    // Create path normalizer for validation
    let normalizer = PathNormalizer::new(state.config.base_dir.clone());

    // Convert to Utf8Path and validate
    let path_buf = std::path::PathBuf::from(&tool.file_path);
    let utf8_path = Utf8PathBuf::from_path_buf(path_buf).map_err(|_| PathError::NonUtf8 {
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

    // When grouping is requested, the `groups` field carries each export
    // (categorized by kind), so we omit the flat `exports` list to avoid
    // shipping every export twice. When not grouping, we omit `groups` entirely.
    let groups: Vec<serde_json::Value> = if group_by_kind {
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

    let display_string = if include_display {
        Some(format_module_summary(
            &requested_file_path,
            &exports,
            &groups,
        ))
    } else {
        None
    };

    let mut response = json!({
        "file_path": tool.file_path,
        "file_path_normalized": file_path_normalized,
        "export_count": exports.len(),
    });
    if group_by_kind {
        response["groups"] = json!(groups);
    } else {
        response["exports"] = json!(exports);
    }
    if let Some(display) = display_string {
        response["display"] = json!(display);
    }
    Ok(response)
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
    let include_display = tool.include_display.unwrap_or(false);
    let requested_file_path = tool.file_path.clone();

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

    let mut response = json!({
        "file_path": tool.file_path,
        "language": language,
        "total_symbols": symbols.len(),
        "exported_symbols": export_count,
        "internal_symbols": internal_count,
        "counts_by_kind": counts_by_kind,
        "purpose": purpose,
        "exports": exports,
    });
    if include_display {
        response["display"] = json!(format_file_summary(
            &requested_file_path,
            &symbols,
            &counts_by_kind,
            export_count,
            &purpose,
        ));
    }
    Ok(response)
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

    let kinds: HashSet<_> = symbols.iter().map(|s| s.kind.as_str()).collect();
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

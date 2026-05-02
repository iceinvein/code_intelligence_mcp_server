//! Navigation-related MCP tool handlers
//!
//! Covers symbol lookup, reference finding, usage examples, and file/module
//! summarization tools.

use std::collections::HashSet;

use anyhow::Result;
use serde_json::json;

use crate::path::{PathError, PathNormalizer, Utf8PathBuf};
use crate::retrieval::assembler::FormatMode;
use crate::storage::sqlite::SymbolRow;
use crate::tools::*;

use super::budget::{budget_array, insert_budgeted_array};
use super::{extract_usage_line, AppState};

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
    let unique_files: HashSet<&str> = rows.iter().map(|r| r.file_path.as_str()).collect();
    let needs_disambiguation = unique_files.len() > 1 && tool.file.is_none();

    let mut response = json!({
        "symbol_name": tool.symbol_name,
        "count": rows.len(),
        "definitions": symbol_summaries(&rows),
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

fn symbol_summaries(rows: &[SymbolRow]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| {
            json!({
                "id": row.id,
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

    Ok(json!({
        "file_path": tool.file_path,
        "file_path_normalized": file_path_normalized,
        "count": rows.len(),
        "symbols": rows,
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
    let unique_files: HashSet<&str> = roots.iter().map(|r| r.file_path.as_str()).collect();
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

    let budgeted_references = budget_array(out, limit);
    let mut response = json!({
        "symbol_name": tool.symbol_name,
        "reference_type": reference_type,
        "count": budgeted_references.returned_count,
    });
    insert_budgeted_array(&mut response, "references", budgeted_references)?;

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

    let budgeted_examples = budget_array(examples, limit);
    let mut response = json!({
        "symbol_name": tool.symbol_name,
        "count": budgeted_examples.returned_count,
    });
    insert_budgeted_array(&mut response, "examples", budgeted_examples)?;
    Ok(response)
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

    let mut response = json!({
        "file_path": tool.file_path,
        "file_path_normalized": file_path_normalized,
        "export_count": exports.len(),
        "exports": exports,
        "groups": groups,
    });
    if include_display {
        response["display"] = json!(format_module_summary(&requested_file_path, &exports, &groups));
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

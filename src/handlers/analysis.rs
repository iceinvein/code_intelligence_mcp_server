//! Analysis-related MCP tool handlers.
//!
//! Contains handlers for code analysis tools: affected code and tests.

use crate::external_index::provider::{merged_references_to_internal_symbol, ReferenceSource};
use crate::graph::build_dependency_graph;
use crate::storage::sqlite::SymbolRow;
use crate::tools::*;
use serde_json::json;

use super::framework_routes::route_exposures_for_symbol;
use super::AppState;

use super::budget::{budget_array, clamp_limit, insert_budgeted_array};

pub fn handle_find_affected_code(
    state: &AppState,
    tool: FindAffectedCodeTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.map(|d| (d as usize).clamp(1, 10)).unwrap_or(3);
    let limit = clamp_limit(tool.limit, 100, 200);
    let include_tests = tool.include_tests.unwrap_or(false);
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
            "affected": [],
        }));
    };

    // Convert edge_types from Vec<String> to Option<&[&str]>
    let edge_type_strs: Option<Vec<&str>> = tool
        .edge_types
        .as_ref()
        .map(|v| v.iter().map(|s| s.as_str()).collect());
    let edge_types_slice: Option<&[&str]> = edge_type_strs.as_deref();

    // Use build_dependency_graph with "upstream" direction
    let graph_result =
        build_dependency_graph(sqlite, root, "upstream", depth, limit, edge_types_slice);

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

                // Severity scoring: depth (40%) + export (30%) + in-degree (30%)
                let in_degree = sqlite.count_incoming_edges(id).unwrap_or(0);
                let depth_score = 8.0_f64; // Direct callers get high depth score
                let export_score = if exported { 10.0 } else { 4.0 };
                let indegree_score = ((in_degree as f64).ln().max(0.0) * 3.0 + 1.0).min(10.0);

                let severity = ((depth_score * 0.4 + export_score * 0.3 + indegree_score * 0.3)
                    as u8)
                    .clamp(1, 10);
                let impact_level = match severity {
                    8..=10 => "critical",
                    5..=7 => "high",
                    _ => "medium",
                };

                let route_exposure = sqlite
                    .get_symbol_by_id(id)?
                    .map(|row| route_exposures_for_symbol(sqlite, &row, 20))
                    .transpose()?
                    .unwrap_or_default();

                let mut affected_entry = json!({
                    "symbol_id": id,
                    "symbol_name": node.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    "kind": node.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
                    "file_path": file_path,
                    "exported": exported,
                    "severity": severity,
                    "impact": impact_level,
                    "in_degree": in_degree,
                });
                if !route_exposure.is_empty() {
                    affected_entry["route_exposure"] = json!(route_exposure);
                }
                affected_list.push(affected_entry);
            }

            append_external_affected_entries(
                sqlite,
                root,
                &mut affected_list,
                limit,
                include_tests,
                edge_types_slice,
            )?;

            // Sort by severity descending
            affected_list.sort_by(|a, b| {
                let sa = a.get("severity").and_then(|v| v.as_u64()).unwrap_or(0);
                let sb = b.get("severity").and_then(|v| v.as_u64()).unwrap_or(0);
                sb.cmp(&sa)
            });

            (affected_list, None)
        }
        Err(e) => (
            vec![],
            Some(format!("Could not complete full trace: {}", e)),
        ),
    };

    let budgeted_affected = budget_array(affected, limit);
    let affected_total_count = budgeted_affected.total_count;
    let affected_truncated = budgeted_affected.truncated;
    let affected = budgeted_affected.items;

    // Build summary stats
    let affected_files = affected
        .iter()
        .map(|f| f.get("file_path").and_then(|v| v.as_str()).unwrap_or(""))
        .collect::<std::collections::HashSet<_>>()
        .len();

    let mut response = json!({
        "symbol_name": root.name,
        "symbol_kind": root.kind,
        "file_path": root.file_path,
        "depth": depth,
        "affected_count": affected.len(),
        "affected_files": affected_files,
        "severity_breakdown": {
            "critical": affected.iter().filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("critical")).count(),
            "high": affected.iter().filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("high")).count(),
            "medium": affected.iter().filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("medium")).count(),
        },
        "warning": warning,
    });
    insert_budgeted_array(
        &mut response,
        "affected",
        super::budget::BudgetedArray {
            returned_count: affected.len(),
            items: affected,
            total_count: affected_total_count,
            truncated: affected_truncated,
        },
    )?;
    if include_display {
        let affected = response
            .get("affected")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        response["display"] = json!(format_affected_code(root, &affected, affected_files));
    }
    Ok(response)
}

fn append_external_affected_entries(
    sqlite: &crate::storage::sqlite::SqliteStore,
    root: &SymbolRow,
    affected_list: &mut Vec<serde_json::Value>,
    limit: usize,
    include_tests: bool,
    edge_types: Option<&[&str]>,
) -> Result<(), anyhow::Error> {
    let mut seen = affected_list
        .iter()
        .filter_map(|entry| {
            entry
                .get("symbol_id")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .collect::<std::collections::HashSet<_>>();
    seen.insert(root.id.clone());

    for relationship in external_affected_relationships(edge_types) {
        let references =
            merged_references_to_internal_symbol(sqlite, &root.id, Some(relationship), limit)?;
        for reference in references
            .into_iter()
            .filter(|reference| reference.source == ReferenceSource::External)
        {
            let Some(from_symbol_id) = reference.from_symbol_id.as_deref() else {
                continue;
            };
            if !seen.insert(from_symbol_id.to_string()) {
                continue;
            }
            let Some(row) = sqlite.get_symbol_by_id(from_symbol_id)? else {
                continue;
            };
            if !include_tests && is_test_file_for_affected(&row.file_path) {
                continue;
            }

            let in_degree = sqlite.count_incoming_edges(&row.id).unwrap_or(0);
            let depth_score = 8.0_f64;
            let export_score = if row.exported { 10.0 } else { 4.0 };
            let indegree_score = ((in_degree as f64).ln().max(0.0) * 3.0 + 1.0).min(10.0);
            let severity = ((depth_score * 0.4 + export_score * 0.3 + indegree_score * 0.3) as u8)
                .clamp(1, 10);
            let impact_level = match severity {
                8..=10 => "critical",
                5..=7 => "high",
                _ => "medium",
            };
            let route_exposure = route_exposures_for_symbol(sqlite, &row, 20)?;
            let mut affected_entry = json!({
                "symbol_id": row.id,
                "symbol_name": row.name,
                "kind": row.kind,
                "file_path": row.file_path,
                "exported": row.exported,
                "severity": severity,
                "impact": impact_level,
                "in_degree": in_degree,
                "source": "external",
                "confidence": reference.confidence,
                "external_index_id": reference.external_index_id,
                "provenance": reference.provenance,
                "metadata_json": reference.metadata_json,
                "reference_type": reference.reference_type,
                "at_file": reference.at_file,
                "at_line": reference.at_line,
            });
            if !route_exposure.is_empty() {
                affected_entry["route_exposure"] = json!(route_exposure);
            }
            affected_list.push(affected_entry);
        }
    }

    Ok(())
}

fn external_affected_relationships(edge_types: Option<&[&str]>) -> Vec<&'static str> {
    let Some(edge_types) = edge_types else {
        return vec!["call", "reference"];
    };
    let mut relationships = Vec::new();
    if edge_types.iter().any(|edge_type| *edge_type == "call") {
        relationships.push("call");
    }
    if edge_types.iter().any(|edge_type| *edge_type == "reference") {
        relationships.push("reference");
    }
    relationships
}

/// Check if a file path appears to be a test file.
///
/// Delegates to the canonical implementation in `crate::classify`.
fn is_test_file_for_affected(path: &str) -> bool {
    crate::classify::is_test_file(path)
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

    // Group by severity level
    let critical: Vec<_> = affected
        .iter()
        .filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("critical"))
        .collect();
    let high: Vec<_> = affected
        .iter()
        .filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("high"))
        .collect();
    let medium: Vec<_> = affected
        .iter()
        .filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("medium"))
        .collect();

    fn format_group(out: &mut String, items: &[&serde_json::Value], max_display: usize) {
        for a in items.iter().take(max_display) {
            let name = a.get("symbol_name").and_then(|v| v.as_str()).unwrap_or("?");
            let kind = a.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let file = a.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let file_short = file.split('/').next_back().unwrap_or(file);
            let severity = a.get("severity").and_then(|v| v.as_u64()).unwrap_or(0);
            out.push_str(&format!(
                "- **{}** ({}) [severity={}] - `{}`\n",
                name, kind, severity, file_short
            ));
        }
        if items.len() > max_display {
            out.push_str(&format!("*... and {} more*\n", items.len() - max_display));
        }
        out.push('\n');
    }

    if !critical.is_empty() {
        out.push_str(&format!(
            "## [!!!] Critical Impact ({} symbols)\n\n",
            critical.len()
        ));
        format_group(&mut out, &critical, 20);
    }

    if !high.is_empty() {
        out.push_str(&format!("## [!] High Impact ({} symbols)\n\n", high.len()));
        format_group(&mut out, &high, 20);
    }

    if !medium.is_empty() {
        out.push_str(&format!("## Medium Impact ({} symbols)\n\n", medium.len()));
        format_group(&mut out, &medium, 20);
    }

    out
}

/// Handle find_tests_for_symbol tool
pub fn handle_find_tests_for_symbol(
    state: &AppState,
    tool: FindTestsForSymbolTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = clamp_limit(tool.limit, 20, 100);
    let include_display = tool.include_display.unwrap_or(false);

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

    // Use call-graph edges to find which test functions actually call the target symbol.
    let tests_for_symbol = if !test_files.is_empty() {
        sqlite
            .find_test_symbols_calling(&test_files, &root.id, limit)?
            .into_iter()
            .map(|(id, name, file_path, line, edge_type)| {
                json!({
                    "test_id": id,
                    "test_name": name,
                    "test_file": file_path,
                    "line": line,
                    "edge_type": edge_type,
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    // Get symbols with tests for more detail
    let symbols_with_tests = sqlite.get_symbols_with_tests(&root.file_path)?;

    let budgeted_test_files = budget_array(test_files, limit);
    let budgeted_symbols_with_tests = budget_array(symbols_with_tests, limit);
    let display = include_display.then(|| {
        format_test_results(
            root,
            &budgeted_test_files.items,
            &budgeted_symbols_with_tests.items,
        )
    });

    let follow_up = if budgeted_test_files.items.is_empty() {
        "No test_links rows resolved for this symbol's source file. \
            The repository may genuinely have no dedicated unit test, or the \
            test file may live outside the inferred path. If the question \
            demands certainty, fall back to Glob/Grep on `*.test.*` / `*.spec.*` \
            once."
    } else {
        "`test_files` is the verified answer: these paths were resolved via \
            test_links (path-pattern inference at index time) and confirmed \
            against the symbols table. Do not Read the files to verify their \
            existence -- they are guaranteed indexed and on disk. \
            `tests_for_symbol` lists the specific test functions that call \
            the target symbol via call-graph edges. Cite `test_files[0]` \
            directly without further investigation; do not re-run ask_code or \
            investigate with rephrased prompts."
    };

    let mut response = json!({
        "symbol_name": root.name,
        "symbol_kind": root.kind,
        "source_file": root.file_path,
        "test_file_count": budgeted_test_files.returned_count,
        "tests_for_symbol": tests_for_symbol,
        "follow_up": follow_up,
    });
    insert_budgeted_array(&mut response, "test_files", budgeted_test_files)?;
    insert_budgeted_array(
        &mut response,
        "symbols_with_tests",
        budgeted_symbols_with_tests,
    )?;
    if include_display {
        response["display"] = json!(display.unwrap_or_default());
    }
    Ok(response)
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

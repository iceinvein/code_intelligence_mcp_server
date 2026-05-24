//! Analysis-related MCP tool handlers.
//!
//! Contains handlers for code analysis tools: affected code, dead code,
//! duplicates, TODOs, tests, decorators, framework patterns, stale descriptions,
//! undocumented symbols, impact prediction, and context bundle assembly.

use crate::graph::build_dependency_graph;
use crate::storage::sqlite::SymbolRow;
use crate::tools::*;
use serde_json::json;

use super::framework_routes::route_exposures_for_symbol;
use super::AppState;

// Re-import the other handlers called by handle_get_context_bundle
use super::budget::{
    budget_array, budget_string_field, insert_budgeted_array, DEFAULT_MAX_STRING_CHARS,
};
use super::{
    handle_find_similar_code, handle_get_call_hierarchy, handle_get_definition, handle_search_code,
};

pub fn handle_find_affected_code(
    state: &AppState,
    tool: FindAffectedCodeTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(3) as usize;
    let limit = tool.limit.unwrap_or(100).max(1) as usize;
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

/// Handle search_todos tool
pub fn handle_search_todos(
    state: &AppState,
    tool: SearchTodosTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(50).max(1) as usize;
    let include_display = tool.include_display.unwrap_or(false);

    let sqlite = &state.sqlite;

    let todos = sqlite.search_todos(
        tool.query.as_deref(),
        tool.file_path.as_deref(),
        tool.kind.as_deref(),
        limit,
    )?;

    let budgeted_todos = budget_array(todos, limit);
    let display = include_display.then(|| format_todos(&budgeted_todos.items));
    let mut response = json!({
        "count": budgeted_todos.returned_count,
    });
    insert_budgeted_array(&mut response, "todos", budgeted_todos)?;
    if include_display {
        response["display"] = json!(display.unwrap_or_default());
    }
    Ok(response)
}

/// Handle find_tests_for_symbol tool
pub fn handle_find_tests_for_symbol(
    state: &AppState,
    tool: FindTestsForSymbolTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(20).max(1) as usize;
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

    let mut response = json!({
        "symbol_name": root.name,
        "symbol_kind": root.kind,
        "source_file": root.file_path,
        "test_file_count": budgeted_test_files.returned_count,
        "tests_for_symbol": tests_for_symbol,
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

fn format_todos(todos: &[crate::storage::sqlite::schema::TodoRow]) -> String {
    let mut out = String::from("# TODO Comments\n\n");

    if todos.is_empty() {
        out.push_str("*No TODOs found*\n");
        return out;
    }

    let mut by_file: std::collections::HashMap<&str, Vec<_>> = std::collections::HashMap::new();
    for todo in todos {
        by_file.entry(&todo.file_path).or_default().push(todo);
    }

    for (file, file_todos) in by_file {
        let file_name = file.split('/').next_back().unwrap_or(file);
        out.push_str(&format!("## {}\n\n", file_name));

        for todo in file_todos {
            let icon = match todo.kind.as_str() {
                "fixme" => "[FIXME]",
                _ => "[TODO]",
            };
            out.push_str(&format!(
                "{} {}:{} - {}\n",
                icon, file_name, todo.line, todo.text
            ));
        }
        out.push('\n');
    }

    out
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

/// Handle search_decorators tool
pub fn handle_search_decorators(
    state: &AppState,
    tool: SearchDecoratorsTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(50).clamp(1, 500) as usize;
    let include_display = tool.include_display.unwrap_or(false);

    let sqlite = &state.sqlite;

    let decorators = sqlite.search_decorators_by_name(
        tool.name.as_deref(),
        tool.decorator_type.as_deref(),
        limit,
    )?;

    let mut results = Vec::new();
    for dec in decorators {
        // Get symbol details for context
        let symbol = sqlite
            .get_symbol_by_id(&dec.symbol_id)?
            .ok_or_else(|| anyhow::anyhow!("Symbol not found: {}", dec.symbol_id))?;

        results.push(serde_json::json!({
            "symbol_id": dec.symbol_id,
            "symbol_name": symbol.name,
            "decorator_name": dec.name,
            "decorator_type": dec.decorator_type,
            "arguments": dec.arguments,
            "file_path": symbol.file_path,
            "line": dec.target_line,
            "language": symbol.language,
            "symbol_kind": symbol.kind,
        }));
    }

    let budgeted_decorators = budget_array(results, limit);
    let display = include_display.then(|| format_decorators(&budgeted_decorators.items));
    let mut response = serde_json::json!({
        "count": budgeted_decorators.returned_count,
    });
    insert_budgeted_array(&mut response, "decorators", budgeted_decorators)?;
    if include_display {
        response["display"] = json!(display.unwrap_or_default());
    }
    Ok(response)
}

/// Format decorator search results as markdown
fn format_decorators(decorators: &[serde_json::Value]) -> String {
    let mut out = String::from("# Decorator Search Results\n\n");

    if decorators.is_empty() {
        out.push_str("*No decorators found*\n");
        return out;
    }

    // Group by decorator name
    let mut by_name: std::collections::HashMap<&str, Vec<_>> = std::collections::HashMap::new();
    for dec in decorators {
        let name = dec
            .get("decorator_name")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        by_name.entry(name).or_default().push(dec);
    }

    for (decorator_name, items) in by_name {
        out.push_str(&format!("## @{}\n\n", decorator_name));
        out.push_str(&format!("**Found:** {} times\n\n", items.len()));

        for dec in items.iter().take(20) {
            let symbol_name = dec
                .get("symbol_name")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let file_path = dec.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let file_short = file_path.split('/').next_back().unwrap_or(file_path);
            let line = dec.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
            let decorator_type = dec
                .get("decorator_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = dec.get("arguments").and_then(|v| v.as_str()).unwrap_or("");

            out.push_str(&format!(
                "- **{}** - `{}`:{}\n",
                symbol_name, file_short, line
            ));

            if !decorator_type.is_empty() {
                out.push_str(&format!("  - Type: `{}`\n", decorator_type));
            }
            if !arguments.is_empty() {
                let args_preview = if arguments.len() > 60 {
                    format!("{}...", &arguments[..60])
                } else {
                    arguments.to_string()
                };
                out.push_str(&format!("  - Args: `{}`\n", args_preview));
            }
        }
        out.push('\n');
    }

    out
}

/// Handle search_framework_patterns tool
pub fn handle_search_framework_patterns(
    state: &AppState,
    tool: SearchFrameworkPatternsTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(50).clamp(1, 500) as usize;
    let include_display = tool.include_display.unwrap_or(false);

    let sqlite = &state.sqlite;

    let patterns = sqlite.search_framework_patterns(
        tool.framework.as_deref(),
        tool.kind.as_deref(),
        tool.http_method.as_deref(),
        tool.path.as_deref(),
        None, // name filter not exposed in tool yet
        None, // file_path filter not exposed in tool yet
        limit,
    )?;

    let mut results = Vec::new();
    for pattern in patterns {
        results.push(serde_json::json!({
            "id": pattern.id,
            "file_path": pattern.file_path,
            "line": pattern.line,
            "framework": pattern.framework,
            "kind": pattern.kind,
            "http_method": pattern.http_method,
            "path": pattern.path,
            "name": pattern.name,
            "handler": pattern.handler,
            "arguments": pattern.arguments,
            "parent_chain": pattern.parent_chain,
        }));
    }

    let budgeted_patterns = budget_array(results, limit);
    let display = include_display.then(|| format_framework_patterns(&budgeted_patterns.items));
    let mut response = serde_json::json!({
        "count": budgeted_patterns.returned_count,
    });
    insert_budgeted_array(&mut response, "patterns", budgeted_patterns)?;
    if include_display {
        response["display"] = json!(display.unwrap_or_default());
    }
    Ok(response)
}

/// Format framework pattern search results as markdown
fn format_framework_patterns(patterns: &[serde_json::Value]) -> String {
    let mut out = String::from("# Framework Pattern Search Results\n\n");

    if patterns.is_empty() {
        out.push_str("*No framework patterns found*\n");
        return out;
    }

    // Group by framework and kind
    let mut by_framework: std::collections::HashMap<&str, Vec<_>> =
        std::collections::HashMap::new();
    for pattern in patterns {
        let framework = pattern
            .get("framework")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        by_framework.entry(framework).or_default().push(pattern);
    }

    for (framework, items) in by_framework {
        out.push_str(&format!("## {} Framework\n\n", framework));

        // Group by kind within framework
        let mut by_kind: std::collections::HashMap<&str, Vec<_>> = std::collections::HashMap::new();
        for item in items {
            let kind = item
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            by_kind.entry(kind).or_default().push(item);
        }

        for (kind, kind_items) in by_kind {
            out.push_str(&format!("### {} ({})\n\n", kind, kind_items.len()));

            for pattern in kind_items.iter().take(20) {
                let file_path = pattern
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let file_short = file_path.split('/').next_back().unwrap_or(file_path);
                let line = pattern.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
                let http_method = pattern.get("http_method").and_then(|v| v.as_str());
                let path = pattern.get("path").and_then(|v| v.as_str());
                let name = pattern.get("name").and_then(|v| v.as_str());

                // Format based on pattern type
                let label = if let (Some(method), Some(route)) = (http_method, path) {
                    format!("{} {}", method, route)
                } else if let Some(n) = name {
                    n.to_string()
                } else if let Some(p) = path {
                    p.to_string()
                } else {
                    kind.to_string()
                };

                out.push_str(&format!("- **{}** - `{}`:{}\n", label, file_short, line));
            }
            out.push('\n');
        }
    }

    out
}

/// Handle find_dead_code tool - find unused symbols with zero incoming references
pub fn handle_find_dead_code(
    state: &AppState,
    tool: FindDeadCodeTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(50).max(1) as usize;
    let include_tests = tool.include_tests.unwrap_or(false);
    let include_display = tool.include_display.unwrap_or(false);

    let sqlite = &state.sqlite;

    let dead_symbols = sqlite.find_dead_symbols(
        tool.file_path.as_deref(),
        tool.language.as_deref(),
        tool.kind.as_deref(),
        include_tests,
        limit,
    )?;

    // Classify into high priority (exported) and medium priority (private)
    let mut high_priority = Vec::new();
    let mut medium_priority = Vec::new();

    let mut dead_files = std::collections::HashSet::new();
    let mut dead_symbol_entries = Vec::new();

    for sym in &dead_symbols {
        dead_files.insert(sym.file_path.clone());

        // priority is derivable from `exported` (true => high, false => medium),
        // so we don't ship it in the per-symbol entry.
        let entry = json!({
            "symbol_name": sym.name,
            "symbol_id": sym.id,
            "kind": sym.kind,
            "file_path": sym.file_path,
            "line": sym.start_line,
            "exported": sym.exported,
            "language": sym.language,
        });

        if sym.exported {
            high_priority.push(entry.clone());
        } else {
            medium_priority.push(entry.clone());
        }

        dead_symbol_entries.push(entry);
    }

    let budgeted_dead_symbols = budget_array(dead_symbol_entries, limit);
    let display = include_display.then(|| format_dead_code(&high_priority, &medium_priority));
    // dead_symbol_count is omitted: the array's `dead_symbols_budget.total_count`
    // already carries the same number.
    let mut response = json!({
        "dead_files": dead_files.len(),
        "high_priority_count": high_priority.len(),
        "medium_priority_count": medium_priority.len(),
    });
    insert_budgeted_array(&mut response, "dead_symbols", budgeted_dead_symbols)?;
    if include_display {
        response["display"] = json!(display.unwrap_or_default());
    }
    Ok(response)
}

/// Format dead code results as markdown
fn format_dead_code(
    high_priority: &[serde_json::Value],
    medium_priority: &[serde_json::Value],
) -> String {
    let total = high_priority.len() + medium_priority.len();
    let mut out = String::from("# Dead Code Analysis\n\n");
    out.push_str(&format!(
        "**Total unused symbols:** {} ({} high priority, {} medium priority)\n\n",
        total,
        high_priority.len(),
        medium_priority.len()
    ));

    if total == 0 {
        out.push_str("*No dead code found*\n");
        return out;
    }

    if !high_priority.is_empty() {
        out.push_str("## High Priority (exported but unused)\n\n");
        for sym in high_priority.iter().take(30) {
            let name = sym
                .get("symbol_name")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let kind = sym.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let file = sym.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let file_short = file.split('/').next_back().unwrap_or(file);
            let line = sym.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
            out.push_str(&format!(
                "- **{}** ({}) - `{}`:{}\n",
                name, kind, file_short, line
            ));
        }
        if high_priority.len() > 30 {
            out.push_str(&format!("*... and {} more*\n", high_priority.len() - 30));
        }
        out.push('\n');
    }

    if !medium_priority.is_empty() {
        out.push_str("## Medium Priority (private unused)\n\n");
        for sym in medium_priority.iter().take(30) {
            let name = sym
                .get("symbol_name")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let kind = sym.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let file = sym.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let file_short = file.split('/').next_back().unwrap_or(file);
            let line = sym.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
            out.push_str(&format!(
                "- **{}** ({}) - `{}`:{}\n",
                name, kind, file_short, line
            ));
        }
        if medium_priority.len() > 30 {
            out.push_str(&format!("*... and {} more*\n", medium_priority.len() - 30));
        }
    }

    out
}

/// Handle find_duplicates tool - find groups of semantically similar symbols
pub fn handle_find_duplicates(
    state: &AppState,
    tool: FindDuplicatesTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(50).max(1) as usize;
    let include_display = tool.include_display.unwrap_or(false);
    let sqlite = &state.sqlite;

    let clusters = sqlite.list_duplicate_clusters(limit)?;

    let mut groups = Vec::new();
    let mut total_symbols = 0usize;

    for (cluster_key, _member_count) in &clusters {
        let members = sqlite.list_cluster_members_with_details(cluster_key)?;

        // Apply filters
        let filtered: Vec<_> = members
            .into_iter()
            .filter(|m| {
                if let Some(ref kind_filter) = tool.kind {
                    if !m.kind.eq_ignore_ascii_case(kind_filter) {
                        return false;
                    }
                }
                if let Some(ref path_filter) = tool.file_path {
                    if !m.file_path.contains(path_filter.as_str()) {
                        return false;
                    }
                }
                true
            })
            .collect();

        // After filtering, skip clusters with fewer than 2 members
        if filtered.len() < 2 {
            continue;
        }

        total_symbols += filtered.len();

        let member_entries: Vec<serde_json::Value> = filtered
            .iter()
            .map(|m| {
                json!({
                    "name": m.name,
                    "file_path": m.file_path,
                    "kind": m.kind,
                    "start_line": m.start_line,
                    "end_line": m.end_line,
                    "exported": m.exported,
                    "symbol_id": m.symbol_id,
                })
            })
            .collect();

        let kind_label = if filtered.iter().all(|m| m.kind == filtered[0].kind) {
            format!("{}s", filtered[0].kind)
        } else {
            "symbols".to_string()
        };
        let suggestion = format!(
            "These {} {} share the same embedding cluster, suggesting high semantic similarity. Consider consolidating.",
            filtered.len(),
            kind_label,
        );

        groups.push(json!({
            "cluster_key": cluster_key,
            "member_count": filtered.len(),
            "members": member_entries,
            "suggestion": suggestion,
        }));
    }

    let budgeted_groups = budget_array(groups, limit);
    let display = include_display.then(|| format_duplicates(&budgeted_groups.items, total_symbols));
    let mut response = json!({
        "duplicate_group_count": budgeted_groups.returned_count,
        "total_duplicate_symbols": total_symbols,
    });
    insert_budgeted_array(&mut response, "groups", budgeted_groups)?;
    if include_display {
        response["display"] = json!(display.unwrap_or_default());
    }
    Ok(response)
}

/// Format duplicate detection results as markdown
fn format_duplicates(groups: &[serde_json::Value], total_symbols: usize) -> String {
    let mut out = String::from("# Semantic Duplicate Detection\n\n");
    out.push_str(&format!(
        "**Found {} duplicate groups ({} total symbols)**\n\n",
        groups.len(),
        total_symbols,
    ));

    if groups.is_empty() {
        out.push_str("*No semantic duplicates found*\n");
        return out;
    }

    for (i, group) in groups.iter().enumerate().take(30) {
        let member_count = group
            .get("member_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        out.push_str(&format!(
            "## Group {} ({} members)\n\n",
            i + 1,
            member_count
        ));

        if let Some(members) = group.get("members").and_then(|v| v.as_array()) {
            for m in members {
                let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let kind = m.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                let file = m.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                let file_short = file.split('/').next_back().unwrap_or(file);
                let line = m.get("start_line").and_then(|v| v.as_u64()).unwrap_or(0);
                out.push_str(&format!(
                    "- **{}** ({}) - `{}`:{}\n",
                    name, kind, file_short, line
                ));
            }
        }

        if let Some(suggestion) = group.get("suggestion").and_then(|v| v.as_str()) {
            out.push_str(&format!("\n> {}\n", suggestion));
        }
        out.push('\n');
    }

    out
}

/// Handle find_stale_descriptions tool
pub fn handle_find_stale_descriptions(
    state: &AppState,
    tool: FindStaleDescriptionsTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(100).max(1) as usize;
    let include_display = tool.include_display.unwrap_or(false);
    let sqlite = &state.sqlite;

    let rows = sqlite.list_descriptions_with_symbol_data(tool.file_path.as_deref(), limit)?;

    let mut stale_entries = Vec::new();
    let mut checked = 0usize;

    for row in &rows {
        checked += 1;
        let current_hash = crate::llm::compute_content_hash(&row.name, &row.kind, &row.text);
        if current_hash != row.content_hash {
            stale_entries.push(json!({
                "symbol_id": row.symbol_id,
                "name": row.name,
                "kind": row.kind,
                "file_path": row.file_path,
                "stored_hash": row.content_hash,
                "current_hash": current_hash,
                "stale_description": row.description,
            }));
        }
    }

    let budgeted_stale = budget_array(stale_entries, limit);
    let mut response = json!({
        "checked": checked,
        "stale_count": budgeted_stale.returned_count,
    });
    insert_budgeted_array(&mut response, "stale_symbols", budgeted_stale)?;
    if include_display {
        let mut display = String::from("# Stale Description Analysis\n\n");
        display.push_str(&format!(
            "**Checked:** {} descriptions | **Stale:** {}\n\n",
            checked,
            response
                .get("stale_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        ));
        if response
            .get("stale_symbols")
            .and_then(|v| v.as_array())
            .map(|v| v.is_empty())
            .unwrap_or(true)
        {
            display.push_str("*All descriptions are up to date.*\n");
        } else if let Some(entries) = response.get("stale_symbols").and_then(|v| v.as_array()) {
            for (i, entry) in entries.iter().enumerate() {
                display.push_str(&format!(
                    "{}. **{}** `{}` - `{}`\n",
                    i + 1,
                    entry["name"].as_str().unwrap_or("?"),
                    entry["kind"].as_str().unwrap_or("?"),
                    entry["file_path"].as_str().unwrap_or("?"),
                ));
            }
        }
        response["display"] = json!(display);
    }
    Ok(response)
}

/// Handle find_undocumented_symbols tool
pub fn handle_find_undocumented_symbols(
    state: &AppState,
    tool: FindUndocumentedSymbolsTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(100).max(1) as usize;
    let min_lines = tool.min_lines.unwrap_or(3);
    let exported_only = tool.exported_only.unwrap_or(false);
    let include_display = tool.include_display.unwrap_or(false);
    let sqlite = &state.sqlite;

    let symbols = sqlite.find_undocumented_symbols_filtered(
        min_lines,
        exported_only,
        tool.file_path.as_deref(),
        limit,
    )?;

    let mut entries = Vec::new();
    for sym in &symbols {
        entries.push(json!({
            "symbol_id": sym.id,
            "name": sym.name,
            "kind": sym.kind,
            "file_path": sym.file_path,
            "exported": sym.exported,
            "line_count": sym.line_count,
        }));
    }

    let budgeted_symbols = budget_array(entries, limit);
    let mut response = json!({
        "undocumented_count": budgeted_symbols.returned_count,
    });
    insert_budgeted_array(&mut response, "symbols", budgeted_symbols)?;
    if include_display {
        let mut display = String::from("# Undocumented Symbols\n\n");
        display.push_str(&format!(
            "**Found:** {} symbols without descriptions\n\n",
            symbols.len()
        ));
        if symbols.is_empty() {
            display.push_str("*All symbols have descriptions.*\n");
        } else {
            let exported_count = symbols.iter().filter(|s| s.exported).count();
            let private_count = symbols.len() - exported_count;
            display.push_str(&format!(
                "**Exported:** {} | **Private:** {}\n\n",
                exported_count, private_count
            ));
            if let Some(entries) = response.get("symbols").and_then(|v| v.as_array()) {
                for (i, entry) in entries.iter().enumerate() {
                    let marker = if entry["exported"].as_bool().unwrap_or(false) {
                        "pub"
                    } else {
                        "priv"
                    };
                    display.push_str(&format!(
                        "{}. [{}] **{}** `{}` - `{}` ({} lines)\n",
                        i + 1,
                        marker,
                        entry["name"].as_str().unwrap_or("?"),
                        entry["kind"].as_str().unwrap_or("?"),
                        entry["file_path"].as_str().unwrap_or("?"),
                        entry["line_count"].as_u64().unwrap_or(0),
                    ));
                }
            }
        }
        response["display"] = json!(display);
    }
    Ok(response)
}

pub fn handle_predict_impact(
    state: &AppState,
    tool: PredictImpactTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(20).clamp(1, 200) as usize;
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
            "predictions": [],
        }));
    };

    // --- Phase 1: Structural impact (reuse find_affected_code logic) ---
    let graph_result = build_dependency_graph(sqlite, root, "upstream", 3, limit * 2, None);

    let mut structural_impacts: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();

    if let Ok(graph) = graph_result {
        let empty_nodes: Vec<serde_json::Value> = vec![];
        let nodes = graph
            .get("nodes")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty_nodes);

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

            if !include_tests && is_test_file_for_affected(file_path) {
                continue;
            }

            // Severity scoring (same as find_affected_code)
            let in_degree = sqlite.count_incoming_edges(id).unwrap_or(0);
            let depth_score = 8.0_f64;
            let export_score = if exported { 10.0 } else { 4.0 };
            let indegree_score = ((in_degree as f64).ln().max(0.0) * 3.0 + 1.0).min(10.0);

            let severity = ((depth_score * 0.4 + export_score * 0.3 + indegree_score * 0.3) as u8)
                .clamp(1, 10);

            // Normalize structural score to 0.0-1.0 range
            let structural_score = severity as f64 / 10.0;

            let route_exposure = sqlite
                .get_symbol_by_id(id)?
                .map(|row| route_exposures_for_symbol(sqlite, &row, 20))
                .transpose()?
                .unwrap_or_default();

            let mut impact = json!({
                "symbol_id": id,
                "symbol_name": node.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                "kind": node.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
                "file_path": file_path,
                "exported": exported,
                "structural_score": structural_score,
                "in_degree": in_degree,
            });
            if !route_exposure.is_empty() {
                impact["route_exposure"] = json!(route_exposure);
            }

            structural_impacts.insert(id.to_string(), impact);
        }
    }

    // --- Phase 2: Co-change impact ---
    // Build co-change matrix on the fly for the current repo
    let co_change_result = crate::indexer::package::cochange::build_co_change_matrix(
        &state.config.base_dir,
        sqlite,
        500,
    );

    let co_change_stats = match &co_change_result {
        Ok(stats) => Some(json!({
            "commits_walked": stats.commits_walked,
            "commits_skipped": stats.commits_skipped,
            "pairs_recorded": stats.pairs_recorded,
        })),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to build co-change matrix");
            None
        }
    };

    // Look up co-changes for the file containing the target symbol
    let mut cochange_impacts: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();

    if co_change_result.is_ok() {
        if let Ok(co_changes) = sqlite.get_co_changes_for_file(&root.file_path, limit * 2) {
            for cc in &co_changes {
                // Get the "other" file (the one that isn't the root symbol's file)
                let other_file = if cc.file_a == root.file_path {
                    &cc.file_b
                } else {
                    &cc.file_a
                };

                if !include_tests && is_test_file_for_affected(other_file) {
                    continue;
                }

                // Use file path as key for co-change impacts
                let entry = cochange_impacts
                    .entry(other_file.to_string())
                    .or_insert(0.0);
                if (cc.confidence as f64) > *entry {
                    *entry = cc.confidence as f64;
                }
            }
        }
    }

    // --- Phase 3: Merge structural + co-change scores ---
    let mut merged: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();

    // Add structural impacts
    for (id, val) in &structural_impacts {
        let file_path = val.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let structural_score = val
            .get("structural_score")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let cochange_confidence = cochange_impacts.get(file_path).copied().unwrap_or(0.0);

        let merged_score = structural_score * 0.6 + cochange_confidence * 0.4;
        let source = if cochange_confidence > 0.0 {
            "both"
        } else {
            "structural"
        };

        let mut prediction = json!({
            "symbol_id": val.get("symbol_id"),
            "symbol_name": val.get("symbol_name"),
            "kind": val.get("kind"),
            "file_path": file_path,
            "exported": val.get("exported"),
            "structural_score": structural_score,
            "cochange_confidence": cochange_confidence,
            "merged_score": merged_score,
            "source": source,
            "in_degree": val.get("in_degree"),
        });
        if let Some(route_exposure) = val.get("route_exposure") {
            prediction["route_exposure"] = route_exposure.clone();
        }

        merged.insert(id.clone(), prediction);
    }

    // Add co-change-only impacts (files not found via structural analysis).
    // We omit symbol_id/symbol_name/kind/exported/in_degree here rather than
    // shipping explicit nulls — these are file-level signals with no symbol.
    for (file_path, confidence) in &cochange_impacts {
        let already_covered = merged
            .values()
            .any(|v| v.get("file_path").and_then(|p| p.as_str()) == Some(file_path.as_str()));

        if !already_covered {
            let merged_score = confidence * 0.4; // No structural component
            let key = format!("cochange:{}", file_path);
            merged.insert(
                key,
                json!({
                    "file_path": file_path,
                    "structural_score": 0.0,
                    "cochange_confidence": confidence,
                    "merged_score": merged_score,
                    "source": "cochange",
                }),
            );
        }
    }

    // Sort by merged_score descending
    let mut predictions: Vec<serde_json::Value> = merged.into_values().collect();
    predictions.sort_by(|a, b| {
        let sa = a
            .get("merged_score")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let sb = b
            .get("merged_score")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });

    let budgeted_predictions = budget_array(predictions, limit);
    let predictions_total_count = budgeted_predictions.total_count;
    let predictions_truncated = budgeted_predictions.truncated;
    let predictions = budgeted_predictions.items;

    // Build summary
    let structural_count = predictions
        .iter()
        .filter(|p| p.get("source").and_then(|v| v.as_str()) == Some("structural"))
        .count();
    let cochange_count = predictions
        .iter()
        .filter(|p| p.get("source").and_then(|v| v.as_str()) == Some("cochange"))
        .count();
    let both_count = predictions
        .iter()
        .filter(|p| p.get("source").and_then(|v| v.as_str()) == Some("both"))
        .count();

    let affected_files: std::collections::HashSet<String> = predictions
        .iter()
        .filter_map(|p| p.get("file_path").and_then(|v| v.as_str()))
        .map(ToString::to_string)
        .collect();
    let affected_file_count = affected_files.len();

    let mut response = json!({
        "symbol_name": root.name,
        "symbol_kind": root.kind,
        "file_path": root.file_path,
        "prediction_count": predictions.len(),
        "affected_files": affected_file_count,
        "source_breakdown": {
            "structural": structural_count,
            "cochange": cochange_count,
            "both": both_count,
        },
        "co_change_stats": co_change_stats,
    });
    insert_budgeted_array(
        &mut response,
        "predictions",
        super::budget::BudgetedArray {
            returned_count: predictions.len(),
            items: predictions,
            total_count: predictions_total_count,
            truncated: predictions_truncated,
        },
    )?;
    if include_display {
        let predictions = response
            .get("predictions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        response["display"] = json!(format_predict_impact(
            root,
            &predictions,
            affected_file_count
        ));
    }
    Ok(response)
}

fn format_predict_impact(
    root: &SymbolRow,
    predictions: &[serde_json::Value],
    affected_files: usize,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "# Impact Prediction: {} ({})",
        root.name, root.kind
    ));
    lines.push(format!("File: {}", root.file_path));
    lines.push(format!(
        "Predictions: {} | Affected files: {}",
        predictions.len(),
        affected_files
    ));
    lines.push(String::new());

    for (i, pred) in predictions.iter().enumerate() {
        let name = pred
            .get("symbol_name")
            .and_then(|v| v.as_str())
            .unwrap_or("(file-level)");
        let file = pred
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let score = pred
            .get("merged_score")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let source = pred.get("source").and_then(|v| v.as_str()).unwrap_or("?");

        lines.push(format!(
            "{}. {} ({}) — score: {:.2} [{}]",
            i + 1,
            name,
            file,
            score,
            source,
        ));
    }

    lines.join("\n")
}

/// Handle get_context_bundle tool — assembles a multi-section context bundle for a task description.
///
/// Pipeline:
/// 1. `search_code(task)` → seed symbols (top N, default 3)
/// 2. For each seed: get_definition, get_call_hierarchy, find_tests_for_symbol,
///    find_similar_code, find_affected_code
/// 3. Assemble into unified markdown context with token budget
pub async fn handle_get_context_bundle(
    state: &AppState,
    tool: GetContextBundleTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let start = std::time::Instant::now();
    let seed_limit = tool.seed_limit.unwrap_or(3).clamp(1, 10) as usize;
    let max_tokens = tool.max_tokens;
    let include_raw_sections = tool.include_raw_sections.unwrap_or(false);

    // Determine which sections to include
    let all_sections = vec![
        "definitions".to_string(),
        "call_chain".to_string(),
        "tests".to_string(),
        "similar".to_string(),
        "affected".to_string(),
    ];
    let sections = tool.sections.unwrap_or(all_sections);
    let include_definitions = sections.iter().any(|s| s == "definitions");
    let include_call_chain = sections.iter().any(|s| s == "call_chain");
    let include_tests = sections.iter().any(|s| s == "tests");
    let include_similar = sections.iter().any(|s| s == "similar");
    let include_affected = sections.iter().any(|s| s == "affected");

    // Step 1: Search for seed symbols. context_bundle wants source code for
    // each seed, so request snippets — full markdown would balloon the bundle.
    let search_result = handle_search_code(
        &state.retriever,
        &state.config.db_path,
        SearchCodeTool {
            query: tool.task.clone(),
            limit: Some(seed_limit as u32),
            exported_only: None,
            context: Some("snippets".to_string()),
        },
    )
    .await?;

    // Extract seed symbol names from search results
    let seed_symbols: Vec<String> = search_result
        .get("hits")
        .or_else(|| search_result.get("results"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Also extract file paths for disambiguation
    let seed_files: Vec<Option<String>> = search_result
        .get("hits")
        .or_else(|| search_result.get("results"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|r| {
                    r.get("file_path")
                        .and_then(|f| f.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    // Step 2: Gather sections for each seed symbol
    let mut definitions_section = Vec::new();
    let mut call_chain_section = Vec::new();
    let mut tests_section = Vec::new();
    let mut similar_section = Vec::new();
    let mut affected_section = Vec::new();

    for (i, symbol_name) in seed_symbols.iter().enumerate() {
        let file_hint = seed_files.get(i).and_then(|f| f.clone());

        // Definitions
        if include_definitions {
            let def_result = handle_get_definition(
                state,
                GetDefinitionTool {
                    symbol_name: symbol_name.clone(),
                    file: file_hint.clone(),
                    limit: Some(1),
                },
            )
            .await;
            match def_result {
                Ok(val) => definitions_section.push(val),
                Err(e) => {
                    tracing::debug!(symbol = %symbol_name, error = %e, "context_bundle: definition lookup failed")
                }
            }
        }

        // Call hierarchy
        if include_call_chain {
            let call_result = handle_get_call_hierarchy(
                state,
                GetCallHierarchyTool {
                    symbol_name: symbol_name.clone(),
                    direction: Some("both".to_string()),
                    depth: Some(2),
                    limit: Some(20),
                },
            );
            match call_result {
                Ok(val) => call_chain_section.push(val),
                Err(e) => {
                    tracing::debug!(symbol = %symbol_name, error = %e, "context_bundle: call hierarchy failed")
                }
            }
        }

        // Tests
        if include_tests {
            let tests_result = handle_find_tests_for_symbol(
                state,
                FindTestsForSymbolTool {
                    symbol_name: symbol_name.clone(),
                    file_path: file_hint.clone(),
                    limit: Some(5),
                    include_display: Some(true),
                },
            );
            match tests_result {
                Ok(val) => tests_section.push(val),
                Err(e) => {
                    tracing::debug!(symbol = %symbol_name, error = %e, "context_bundle: test lookup failed")
                }
            }
        }

        // Similar code
        if include_similar {
            let similar_result = handle_find_similar_code(
                state,
                FindSimilarCodeTool {
                    symbol_name: Some(symbol_name.clone()),
                    code_snippet: None,
                    file_path: file_hint.clone(),
                    limit: Some(5),
                    threshold: None,
                    include_display: None,
                },
            )
            .await;
            match similar_result {
                Ok(val) => similar_section.push(val),
                Err(e) => {
                    tracing::debug!(symbol = %symbol_name, error = %e, "context_bundle: similar code failed")
                }
            }
        }

        // Affected code
        if include_affected {
            let affected_result = handle_find_affected_code(
                state,
                FindAffectedCodeTool {
                    symbol_name: symbol_name.clone(),
                    file_path: file_hint,
                    depth: Some(2),
                    limit: Some(10),
                    include_tests: Some(false),
                    edge_types: None,
                    include_display: None,
                },
            );
            match affected_result {
                Ok(val) => affected_section.push(val),
                Err(e) => {
                    tracing::debug!(symbol = %symbol_name, error = %e, "context_bundle: affected code failed")
                }
            }
        }
    }

    // Step 3: Assemble context markdown string
    let mut context = String::new();

    if include_definitions && !definitions_section.is_empty() {
        context.push_str("## Definitions\n\n");
        for def in &definitions_section {
            if let Some(ctx) = def.get("context").and_then(|v| v.as_str()) {
                context.push_str(ctx);
                context.push_str("\n\n");
            }
        }
    }

    if include_call_chain && !call_chain_section.is_empty() {
        context.push_str("## Call Chain\n\n");
        for call in &call_chain_section {
            let sym = call
                .get("symbol_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            context.push_str(&format!("### {}\n\n", sym));

            if let Some(nodes) = call.get("nodes").and_then(|v| v.as_array()) {
                for node in nodes.iter().take(10) {
                    let name = node.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let kind = node.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
                    let file = node
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    context.push_str(&format!("- `{}` ({}) in `{}`\n", name, kind, file));
                }
            }
            context.push('\n');
        }
    }

    if include_tests && !tests_section.is_empty() {
        context.push_str("## Tests\n\n");
        for test in &tests_section {
            if let Some(display) = test.get("display").and_then(|v| v.as_str()) {
                context.push_str(display);
                context.push_str("\n\n");
            }
        }
    }

    if include_similar && !similar_section.is_empty() {
        context.push_str("## Similar Code\n\n");
        for sim in &similar_section {
            let query_desc = sim
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            context.push_str(&format!("### Similar to: {}\n\n", query_desc));

            if let Some(results) = sim.get("results").and_then(|v| v.as_array()) {
                for r in results.iter().take(5) {
                    let name = r
                        .get("symbol_name")
                        .or_else(|| r.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let file = r.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
                    let score = r.get("similarity").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    context.push_str(&format!(
                        "- `{}` in `{}` (similarity: {:.2})\n",
                        name, file, score
                    ));
                }
            }
            context.push('\n');
        }
    }

    if include_affected && !affected_section.is_empty() {
        context.push_str("## Affected Code\n\n");
        for aff in &affected_section {
            let sym = aff
                .get("symbol_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            context.push_str(&format!("### Affected by changes to: {}\n\n", sym));

            if let Some(affected) = aff.get("affected").and_then(|v| v.as_array()) {
                for a in affected.iter().take(10) {
                    let name = a
                        .get("symbol_name")
                        .or_else(|| a.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let file = a.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
                    context.push_str(&format!("- `{}` in `{}`\n", name, file));
                }
            }
            context.push('\n');
        }
    }

    // Apply max_tokens truncation (estimate: 1 token ~= 4 chars)
    if let Some(max_tok) = max_tokens {
        let max_chars = (max_tok as usize) * 4;
        if context.len() > max_chars {
            // Find a safe UTF-8 char boundary at or before max_chars
            let safe_boundary = context
                .char_indices()
                .take_while(|(i, _)| *i <= max_chars)
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            context.truncate(safe_boundary);
            context.push_str("\n\n... [truncated to token budget]");
        }
    }

    let token_count = context.len() / 4;
    let assembly_ms = start.elapsed().as_millis() as u64;

    let mut response = json!({
        "task": tool.task,
        "seed_symbols": seed_symbols,
        "section_counts": {
            "definitions": definitions_section.len(),
            "call_chain": call_chain_section.len(),
            "tests": tests_section.len(),
            "similar": similar_section.len(),
            "affected": affected_section.len(),
        },
        "context": context,
        "token_count": token_count,
        "assembly_ms": assembly_ms,
    });
    budget_string_field(&mut response, "context", DEFAULT_MAX_STRING_CHARS);

    if include_raw_sections {
        response["raw_sections"] = json!({
            "definitions": definitions_section,
            "call_chain": call_chain_section,
            "tests": tests_section,
            "similar": similar_section,
            "affected": affected_section,
        });
    }

    Ok(response)
}

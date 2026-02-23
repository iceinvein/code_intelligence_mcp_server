# Dead Code Detection & Change Impact Analysis — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `find_dead_code` MCP tool to identify unused symbols, and enhance the existing `find_affected_code` tool with severity scoring and configurable edge types.

**Architecture:** Dead code detection uses a single SQL query (`LEFT JOIN edges ON id = to_symbol_id WHERE to_symbol_id IS NULL`) with entry-point exclusion via framework_patterns table. Impact analysis enhances the existing handler by adding severity scoring (depth + export + in-degree) and an `edge_types` parameter passed through to `build_dependency_graph`.

**Tech Stack:** Rust, SQLite, tree-sitter (existing)

---

### Task 1: Add `find_dead_symbols` SQL query

**Files:**
- Modify: `src/storage/sqlite/queries/edges.rs` — add new query function
- Modify: `src/storage/sqlite/mod.rs` — expose via SqliteStore

**Changes:**

Add to `src/storage/sqlite/queries/edges.rs`:

```rust
pub fn find_dead_symbols(
    conn: &Connection,
    file_path: Option<&str>,
    language: Option<&str>,
    kind: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    // Build WHERE clause dynamically
    let mut conditions = vec![
        // Core: no incoming edges
        "e.to_symbol_id IS NULL".to_string(),
        // Exclude structural kinds
        "s.kind NOT IN ('file', 'module', 'impl')".to_string(),
        // Exclude main entry points
        "s.name != 'main'".to_string(),
        // Exclude framework entry points (route handlers, controllers, middleware)
        "NOT EXISTS (SELECT 1 FROM framework_patterns fp WHERE fp.name = s.name AND fp.file_path = s.file_path)".to_string(),
    ];

    if !include_tests {
        conditions.push(
            "s.file_path NOT LIKE '%test%' AND s.file_path NOT LIKE '%.test.%' AND s.file_path NOT LIKE '%.spec.%'".to_string(),
        );
    }

    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1;

    if let Some(fp) = file_path {
        conditions.push(format!("s.file_path = ?{}", param_idx));
        params_vec.push(Box::new(fp.to_string()));
        param_idx += 1;
    }
    if let Some(lang) = language {
        conditions.push(format!("s.language = ?{}", param_idx));
        params_vec.push(Box::new(lang.to_string()));
        param_idx += 1;
    }
    if let Some(k) = kind {
        conditions.push(format!("s.kind = ?{}", param_idx));
        params_vec.push(Box::new(k.to_string()));
        param_idx += 1;
    }

    let _ = param_idx; // suppress unused warning

    conditions.push(format!("1=1")); // no-op tail for simpler join
    let where_clause = conditions.join(" AND ");

    let sql = format!(
        r#"
SELECT s.id, s.file_path, s.language, s.kind, s.name, s.exported,
       s.start_byte, s.end_byte, s.start_line, s.end_line, s.text, s.updated_at
FROM symbols s
LEFT JOIN edges e ON s.id = e.to_symbol_id
WHERE {}
ORDER BY s.exported DESC, s.file_path ASC, s.start_line ASC
LIMIT ?
"#,
        where_clause
    );

    params_vec.push(Box::new(limit as i64));
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_refs.as_slice())?;
    let mut out = Vec::new();

    while let Some(row) = rows.next()? {
        out.push(SymbolRow {
            id: row.get(0)?,
            file_path: row.get(1)?,
            language: row.get(2)?,
            kind: row.get(3)?,
            name: row.get(4)?,
            exported: row.get(5)?,
            start_byte: row.get(6)?,
            end_byte: row.get(7)?,
            start_line: row.get::<_, Option<i64>>(8)?.and_then(|v| u32::try_from(v).ok()),
            end_line: row.get::<_, Option<i64>>(9)?.and_then(|v| u32::try_from(v).ok()),
            text: row.get(10)?,
            updated_at: row.get(11)?,
        });
    }
    Ok(out)
}
```

Add to `src/storage/sqlite/mod.rs`:

```rust
pub fn find_dead_symbols(
    &self,
    file_path: Option<&str>,
    language: Option<&str>,
    kind: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    let conn = self.read()?;
    queries::edges::find_dead_symbols(&conn, file_path, language, kind, include_tests, limit)
}
```

### Task 2: Add `find_dead_code` MCP tool definition + handler

**Files:**
- Modify: `src/tools/mod.rs` — add FindDeadCodeTool struct
- Modify: `src/handlers/mod.rs` — add handle_find_dead_code function
- Modify: `src/server/mod.rs` — add routing

**Changes in `src/tools/mod.rs`:**

Add after the SearchTodosTool definition:

```rust
#[macros::mcp_tool(
    name = "find_dead_code",
    description = "Find unused symbols (functions, classes, types) with zero incoming references. Identifies dead code that can be safely removed."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindDeadCodeTool {
    /// Scope to specific file path
    pub file_path: Option<String>,
    /// Filter by language (e.g., "rust", "typescript")
    pub language: Option<String>,
    /// Filter by kind (e.g., "function", "class", "struct")
    pub kind: Option<String>,
    /// Include test symbols (default false)
    pub include_tests: Option<bool>,
    /// Maximum number of results (default 50)
    pub limit: Option<u32>,
}
```

**Changes in `src/handlers/mod.rs`:**

Add handler function:

```rust
pub fn handle_find_dead_code(
    state: &AppState,
    tool: FindDeadCodeTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(50).max(1) as usize;
    let include_tests = tool.include_tests.unwrap_or(false);
    let sqlite = &state.sqlite;

    let dead_symbols = sqlite.find_dead_symbols(
        tool.file_path.as_deref(),
        tool.language.as_deref(),
        tool.kind.as_deref(),
        include_tests,
        limit,
    )?;

    // Classify by priority
    let mut high_priority = Vec::new();
    let mut medium_priority = Vec::new();
    let mut file_set = std::collections::HashSet::new();

    for sym in &dead_symbols {
        file_set.insert(sym.file_path.clone());

        let entry = json!({
            "symbol_name": sym.name,
            "symbol_id": sym.id,
            "kind": sym.kind,
            "file_path": sym.file_path,
            "line": sym.start_line,
            "exported": sym.exported,
            "priority": if sym.exported { "high" } else { "medium" },
            "language": sym.language,
        });

        if sym.exported {
            high_priority.push(entry);
        } else {
            medium_priority.push(entry);
        }
    }

    // Build display
    let display = format_dead_code(&high_priority, &medium_priority, file_set.len());

    Ok(json!({
        "dead_symbol_count": dead_symbols.len(),
        "dead_files": file_set.len(),
        "high_priority_count": high_priority.len(),
        "medium_priority_count": medium_priority.len(),
        "dead_symbols": dead_symbols.iter().map(|s| json!({
            "symbol_name": s.name,
            "symbol_id": s.id,
            "kind": s.kind,
            "file_path": s.file_path,
            "line": s.start_line,
            "exported": s.exported,
            "priority": if s.exported { "high" } else { "medium" },
            "language": s.language,
        })).collect::<Vec<_>>(),
        "display": display,
    }))
}

fn format_dead_code(
    high: &[serde_json::Value],
    medium: &[serde_json::Value],
    file_count: usize,
) -> String {
    let total = high.len() + medium.len();
    let mut out = format!("# Dead Code Report\n\n");
    out.push_str(&format!(
        "**Found {} unused symbols across {} files**\n\n",
        total, file_count
    ));

    if !high.is_empty() {
        out.push_str(&format!("## High Priority ({} exported but unused)\n\n", high.len()));
        for s in high {
            let name = s.get("symbol_name").and_then(|v| v.as_str()).unwrap_or("");
            let kind = s.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let path = s.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let line = s.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
            out.push_str(&format!("- `{}` ({}) — `{}:{}`\n", name, kind, path, line));
        }
        out.push('\n');
    }

    if !medium.is_empty() {
        out.push_str(&format!("## Medium Priority ({} private unused)\n\n", medium.len()));
        for s in medium {
            let name = s.get("symbol_name").and_then(|v| v.as_str()).unwrap_or("");
            let kind = s.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let path = s.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let line = s.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
            out.push_str(&format!("- `{}` ({}) — `{}:{}`\n", name, kind, path, line));
        }
    }

    if total == 0 {
        out.push_str("*No dead code found!*\n");
    }

    out
}
```

**Changes in `src/server/mod.rs`:**

Add to tool list: `FindDeadCodeTool::tool(),`

Add routing case:
```rust
"find_dead_code" => {
    let tool: FindDeadCodeTool = parse_tool_args(&params)?;
    let result = handle_find_dead_code(state, tool)
        .map_err(tool_internal_error)?;
    Ok(CallToolResult::text_content(vec![
        serde_json::to_string_pretty(&result)
            .unwrap_or_default()
            .into(),
    ]))
}
```

### Task 3: Add `edge_types` parameter to `build_dependency_graph`

**Files:**
- Modify: `src/graph/mod.rs` — add `edge_types` parameter to `build_dependency_graph`
- Modify: `src/handlers/mod.rs` — update call site in `handle_find_affected_code`

**Changes in `src/graph/mod.rs`:**

Change function signature from:
```rust
pub fn build_dependency_graph(
    sqlite: &SqliteStore,
    root: &SymbolRow,
    direction: &str,
    depth: usize,
    limit: usize,
) -> anyhow::Result<serde_json::Value>
```
to:
```rust
pub fn build_dependency_graph(
    sqlite: &SqliteStore,
    root: &SymbolRow,
    direction: &str,
    depth: usize,
    limit: usize,
    edge_types: Option<&[&str]>,
) -> anyhow::Result<serde_json::Value>
```

Replace the hardcoded edge type filter (line ~61):
```rust
// Before:
if e.edge_type != "call" && e.edge_type != "reference" {
    continue;
}

// After:
let allowed = edge_types.unwrap_or(&["call", "reference"]);
if !allowed.contains(&e.edge_type.as_str()) {
    continue;
}
```

Apply the same change for the downstream traversal block (around line ~100).

Update ALL existing callers of `build_dependency_graph` to pass `None` as the last argument (keeping existing behavior). Callers:
- `src/handlers/mod.rs` in `handle_find_affected_code` — pass `None` initially (will be enhanced in Task 4)
- `src/handlers/mod.rs` in `handle_explore_dependency_graph` — pass `None`

### Task 4: Add severity scoring + edge_types to `find_affected_code`

**Files:**
- Modify: `src/tools/mod.rs` — add `edge_types` field to FindAffectedCodeTool
- Modify: `src/handlers/mod.rs` — add severity scoring to handle_find_affected_code

**Changes in `src/tools/mod.rs`:**

Add field to FindAffectedCodeTool:
```rust
pub struct FindAffectedCodeTool {
    pub symbol_name: String,
    pub file_path: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
    pub include_tests: Option<bool>,
    /// Filter by edge types (default: ["call", "reference"]). Options: call, reference, type, extends, implements, alias
    pub edge_types: Option<Vec<String>>,
}
```

**Changes in `src/handlers/mod.rs`:**

In `handle_find_affected_code`, after computing `affected_list`:

1. Pass `edge_types` through to `build_dependency_graph`:
```rust
let edge_type_strs: Option<Vec<&str>> = tool.edge_types.as_ref()
    .map(|v| v.iter().map(|s| s.as_str()).collect());
let edge_types_slice: Option<&[&str]> = edge_type_strs.as_deref();

let graph_result = build_dependency_graph(sqlite, root, "upstream", depth, limit, edge_types_slice);
```

2. Add severity scoring to each affected symbol. After the loop that builds `affected_list`, add:
```rust
// Compute severity for each affected symbol
for item in &mut affected_list {
    let sym_id = item.get("symbol_id").and_then(|v| v.as_str()).unwrap_or("");
    let exported = item.get("exported").and_then(|v| v.as_bool()).unwrap_or(false);

    // Get in-degree for this affected symbol
    let in_degree = sqlite.count_incoming_edges(sym_id).unwrap_or(0) as f64;

    // Depth from root (approximate: direct callers are depth 1)
    // For now use a simple heuristic based on graph position
    let depth_score = 8.0_f64; // Direct callers get high depth score
    let export_score = if exported { 10.0 } else { 4.0 };
    let indegree_score = (in_degree.ln().max(0.0) * 3.0 + 1.0).min(10.0);

    let severity = ((depth_score * 0.4 + export_score * 0.3 + indegree_score * 0.3) as u8).min(10).max(1);
    let impact_level = match severity {
        8..=10 => "critical",
        5..=7 => "high",
        _ => "medium",
    };

    item.as_object_mut().unwrap().insert("severity".to_string(), json!(severity));
    item.as_object_mut().unwrap().insert("impact".to_string(), json!(impact_level));
    item.as_object_mut().unwrap().insert("in_degree".to_string(), json!(in_degree as u64));
}
```

3. Sort by severity descending instead of just impact string:
```rust
affected_list.sort_by(|a, b| {
    let sa = a.get("severity").and_then(|v| v.as_u64()).unwrap_or(0);
    let sb = b.get("severity").and_then(|v| v.as_u64()).unwrap_or(0);
    sb.cmp(&sa) // Higher severity first
});
```

4. Add `severity_breakdown` to response:
```rust
let critical = affected.iter().filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("critical")).count();
let high = affected.iter().filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("high")).count();
let medium = affected.iter().filter(|a| a.get("impact").and_then(|v| v.as_str()) == Some("medium")).count();

// In the response json!, add:
"severity_breakdown": { "critical": critical, "high": high, "medium": medium },
```

5. Update `format_affected_code` to use severity-based grouping (critical/high/medium instead of just high/medium).

### Task 5: Tests for dead code detection

**Files:**
- Modify: `src/storage/sqlite/queries/edges.rs` — add unit tests
- Modify: `src/handlers/mod.rs` — add handler tests

**Tests to add in `edges.rs`:**

```rust
#[cfg(test)]
mod dead_code_tests {
    use super::*;
    use crate::storage::sqlite::test_helpers::*;

    #[test]
    fn test_find_dead_symbols_returns_unreferenced() {
        // Setup: create symbols A, B, C. Create edge A->B (B is alive).
        // C has no incoming edges → dead.
        // Assert: C is returned, B is not.
    }

    #[test]
    fn test_find_dead_symbols_excludes_file_and_module_kinds() {
        // Setup: create file symbol with no incoming edges
        // Assert: not returned (file/module excluded)
    }

    #[test]
    fn test_find_dead_symbols_excludes_framework_entry_points() {
        // Setup: create symbol + framework_pattern row for it
        // Assert: not returned (framework entry point excluded)
    }

    #[test]
    fn test_find_dead_symbols_filters_by_file_path() {
        // Setup: dead symbols in two files
        // Assert: filtering by file_path returns only matching file
    }

    #[test]
    fn test_find_dead_symbols_exported_priority() {
        // Setup: dead exported symbol + dead private symbol
        // Assert: exported comes first (ORDER BY exported DESC)
    }
}
```

### Task 6: Tests for enhanced impact analysis

**Files:**
- Modify: `src/handlers/mod.rs` or relevant test file

**Tests:**

```rust
#[test]
fn test_severity_scoring_exported_high() {
    // Exported symbol with high in-degree → severity 8-10
}

#[test]
fn test_severity_scoring_private_low_indegree() {
    // Private symbol with in-degree 1 → severity 1-4
}

#[test]
fn test_edge_types_parameter_filters_graph() {
    // Pass edge_types=["call"] → only call edges traversed
    // Pass edge_types=["call", "type"] → both types traversed
}
```

### Task 7: Build + test verification

Run: `EMBEDDINGS_BACKEND=hash cargo test`
Expected: All tests pass

Run: `cargo build --release`
Expected: Clean release build

Verify: `cargo test --lib find_dead` to run dead code specific tests.

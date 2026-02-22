# Tier 1 Depth Improvements Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix three broken/incomplete MCP tools — `trace_data_flow`, `get_type_graph`, and `find_tests_for_symbol` — so they use the existing graph infrastructure correctly.

**Architecture:** Each fix is isolated to 1-2 files. Fix 1 rewires the data flow handler to use `reads`/`writes` edge types + bidirectional traversal. Fix 2 adds a `direction` parameter to the type graph builder and uses `list_edges_to()` for upstream traversal. Fix 3 enriches test mapping with call-graph analysis from test symbols to target symbols.

**Tech Stack:** Rust, SQLite (rusqlite), serde_json, in-memory SQLite for tests.

**Design doc:** `docs/plans/2026-02-22-tier1-depth-improvements-design.md`

---

### Task 1: Fix `trace_data_flow` — Wire reads/writes edges + bidirectional traversal

**Files:**
- Modify: `src/handlers/mod.rs:958-1029` (function `trace_data_flow_edges`)

**Step 1: Write the failing test**

Add a test at the bottom of `src/handlers/mod.rs` (or in a new `src/handlers/tests.rs` — but since `trace_data_flow_edges` is a private function, test it where it's defined). The cleanest approach: add a `#[cfg(test)]` module at the end of `src/handlers/mod.rs`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::{SqliteStore, EdgeRow};

    fn make_sqlite() -> SqliteStore {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();
        sqlite
    }

    fn sym(id: &str, name: &str, file: &str) -> crate::storage::sqlite::SymbolRow {
        crate::storage::sqlite::SymbolRow {
            id: id.to_string(),
            file_path: file.to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: name.to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 100,
            start_line: 1,
            end_line: 5,
            text: format!("function {name}() {{}}"),
        }
    }

    fn edge(from: &str, to: &str, edge_type: &str) -> EdgeRow {
        EdgeRow {
            from_symbol_id: from.to_string(),
            to_symbol_id: to.to_string(),
            edge_type: edge_type.to_string(),
            at_file: Some("src/a.ts".to_string()),
            at_line: Some(1),
            confidence: 0.7,
            evidence_count: 1,
            resolution: "local".to_string(),
        }
    }

    #[test]
    fn trace_data_flow_follows_reads_and_writes_edges() {
        let sqlite = make_sqlite();
        sqlite.upsert_symbol(&sym("root", "processData", "src/a.ts")).unwrap();
        sqlite.upsert_symbol(&sym("reader", "readConfig", "src/b.ts")).unwrap();
        sqlite.upsert_symbol(&sym("writer", "writeOutput", "src/c.ts")).unwrap();

        // root reads from reader, root writes to writer
        sqlite.upsert_edge(&edge("root", "reader", "reads")).unwrap();
        sqlite.upsert_edge(&edge("root", "writer", "writes")).unwrap();

        let (reads, writes) = trace_data_flow_edges(&sqlite, "root", 2, 50, "both").unwrap();

        assert!(!reads.is_empty(), "Should find reads edges. Got: {:?}", reads);
        assert!(!writes.is_empty(), "Should find writes edges. Got: {:?}", writes);

        // Check that the reads contain reader
        assert!(reads.iter().any(|(id, _, _)| id == "reader"), "reads should contain 'reader'");
        // Check that the writes contain writer
        assert!(writes.iter().any(|(id, _, _)| id == "writer"), "writes should contain 'writer'");
    }

    #[test]
    fn trace_data_flow_incoming_edges() {
        let sqlite = make_sqlite();
        sqlite.upsert_symbol(&sym("target", "config", "src/a.ts")).unwrap();
        sqlite.upsert_symbol(&sym("caller", "initApp", "src/b.ts")).unwrap();

        // caller reads target (incoming read edge to target)
        sqlite.upsert_edge(&edge("caller", "target", "reads")).unwrap();

        // Tracing "target" should find that "caller" reads it via incoming edges
        let (reads, _writes) = trace_data_flow_edges(&sqlite, "target", 2, 50, "both").unwrap();

        assert!(reads.iter().any(|(id, _, _)| id == "caller"),
            "Should find incoming reader 'caller'. Got: {:?}", reads);
    }

    #[test]
    fn trace_data_flow_direction_filter() {
        let sqlite = make_sqlite();
        sqlite.upsert_symbol(&sym("root", "process", "src/a.ts")).unwrap();
        sqlite.upsert_symbol(&sym("r", "reader", "src/b.ts")).unwrap();
        sqlite.upsert_symbol(&sym("w", "writer", "src/c.ts")).unwrap();

        sqlite.upsert_edge(&edge("root", "r", "reads")).unwrap();
        sqlite.upsert_edge(&edge("root", "w", "writes")).unwrap();

        // Only reads
        let (reads, writes) = trace_data_flow_edges(&sqlite, "root", 2, 50, "reads").unwrap();
        assert!(!reads.is_empty(), "reads direction should return reads");
        assert!(writes.is_empty(), "reads direction should not return writes");

        // Only writes
        let (reads, writes) = trace_data_flow_edges(&sqlite, "root", 2, 50, "writes").unwrap();
        assert!(reads.is_empty(), "writes direction should not return reads");
        assert!(!writes.is_empty(), "writes direction should return writes");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib handlers::tests::trace_data_flow_follows_reads_and_writes_edges -- --nocapture 2>&1 | tail -20`

Expected: FAIL — reads and writes are empty because `_ => continue` skips `"reads"` and `"writes"` edge types.

**Step 3: Implement the fix**

Replace `trace_data_flow_edges` in `src/handlers/mod.rs:958-1029` with:

```rust
fn trace_data_flow_edges(
    sqlite: &SqliteStore,
    root_id: &str,
    depth: usize,
    limit: usize,
    direction: &str,
) -> DataFlowTraceResult {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut queue = Vec::new();

    queue.push((root_id.to_string(), vec![]));
    visited.insert(root_id.to_string());

    for _level in 0..depth {
        if reads.len() + writes.len() >= limit {
            break;
        }
        let mut next_queue = Vec::new();

        for (current_id, path) in queue.drain(..) {
            if reads.len() + writes.len() >= limit {
                break;
            }

            // Outgoing edges: what does this symbol read/write/call
            let outgoing = sqlite.list_edges_from(&current_id, limit)?;
            for edge in &outgoing {
                if reads.len() + writes.len() >= limit {
                    break;
                }
                let flow_type = match edge.edge_type.as_str() {
                    "reads" => "read",
                    "writes" => "write",
                    "call" | "reference" => "read",
                    _ => continue,
                };
                let match_direction = match direction {
                    "reads" => flow_type == "read",
                    "writes" => flow_type == "write",
                    _ => true,
                };
                if !match_direction {
                    continue;
                }
                if visited.insert(edge.to_symbol_id.clone()) {
                    let mut new_path = path.clone();
                    new_path.push(edge.to_symbol_id.clone());
                    let target = if flow_type == "read" {
                        &mut reads
                    } else {
                        &mut writes
                    };
                    target.push((
                        edge.to_symbol_id.clone(),
                        flow_type.to_string(),
                        new_path.clone(),
                    ));
                    next_queue.push((edge.to_symbol_id.clone(), new_path));
                }
            }

            // Incoming edges: who reads/writes/calls this symbol
            let incoming = sqlite.list_edges_to(&current_id, limit)?;
            for edge in &incoming {
                if reads.len() + writes.len() >= limit {
                    break;
                }
                let flow_type = match edge.edge_type.as_str() {
                    "reads" => "read",   // someone reads me
                    "writes" => "write", // someone writes to me
                    "call" => "read",    // someone calls me (implicit read)
                    _ => continue,
                };
                let match_direction = match direction {
                    "reads" => flow_type == "read",
                    "writes" => flow_type == "write",
                    _ => true,
                };
                if !match_direction {
                    continue;
                }
                if visited.insert(edge.from_symbol_id.clone()) {
                    let mut new_path = path.clone();
                    new_path.push(edge.from_symbol_id.clone());
                    let target = if flow_type == "read" {
                        &mut reads
                    } else {
                        &mut writes
                    };
                    target.push((
                        edge.from_symbol_id.clone(),
                        flow_type.to_string(),
                        new_path.clone(),
                    ));
                    next_queue.push((edge.from_symbol_id.clone(), new_path));
                }
            }
        }
        queue = next_queue;
    }

    Ok((reads, writes))
}
```

**Step 4: Run all three tests to verify they pass**

Run: `cargo test --lib handlers::tests -- --nocapture 2>&1 | tail -20`

Expected: 3 tests PASS

**Step 5: Run full test suite for regressions**

Run: `EMBEDDINGS_BACKEND=hash cargo test 2>&1 | tail -10`

Expected: All tests pass

**Step 6: Commit**

```bash
git add src/handlers/mod.rs
git commit -m "fix: wire trace_data_flow to actual reads/writes edges with bidirectional traversal"
```

---

### Task 2: Bidirectional type graph — add `direction` parameter to tool struct

**Files:**
- Modify: `src/tools/mod.rs:83-87` (add `direction` field to `GetTypeGraphTool`)

**Step 1: Add direction field**

Change `GetTypeGraphTool` at `src/tools/mod.rs:83-87`:

```rust
pub struct GetTypeGraphTool {
    pub symbol_name: String,
    /// Direction of traversal: "downstream" (what does this extend/implement), "upstream" (who extends/implements this), or "both" (default)
    pub direction: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
}
```

**Step 2: Verify it compiles**

Run: `cargo check 2>&1 | tail -10`

Expected: Compile error in `handle_get_type_graph` because `build_type_graph` doesn't accept `direction` yet. This is expected — we'll fix it in the next task.

**Step 3: Commit (partial — will compile after Task 3)**

Do not commit yet — wait for Task 3.

---

### Task 3: Bidirectional type graph — implement upstream traversal in `build_type_graph`

**Files:**
- Modify: `src/graph/mod.rs:322-412` (function `build_type_graph`)
- Modify: `src/handlers/mod.rs:535-558` (function `handle_get_type_graph`, pass direction)

**Step 1: Write the failing test**

Add to existing `#[cfg(test)] mod tests` in `src/graph/mod.rs` (after line 523):

```rust
#[test]
fn type_graph_upstream_finds_implementors() {
    let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
    sqlite.init().unwrap();

    // B implements A, C extends A
    let a = sym("a", "BaseInterface");
    let b = sym("b", "ImplB");
    let c = sym("c", "SubclassC");
    sqlite.upsert_symbol(&a).unwrap();
    sqlite.upsert_symbol(&b).unwrap();
    sqlite.upsert_symbol(&c).unwrap();

    // Edges go FROM implementor TO interface (b->a, c->a)
    for (from, to, ty) in [
        ("b", "a", "implements"),
        ("c", "a", "extends"),
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

    // Upstream from A should find B and C
    let g = build_type_graph(&sqlite, &a, "upstream", 3, 100).unwrap();
    let nodes = g.get("nodes").unwrap().as_array().unwrap();
    let edges = g.get("edges").unwrap().as_array().unwrap();
    assert_eq!(nodes.len(), 3, "Should have A, B, C. Got: {:?}", nodes);
    assert_eq!(edges.len(), 2, "Should have 2 edges. Got: {:?}", edges);
}

#[test]
fn type_graph_both_directions() {
    let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
    sqlite.init().unwrap();

    // Chain: D extends B, B implements A
    let a = sym("a", "Root");
    let b = sym("b", "Middle");
    let d = sym("d", "Leaf");
    sqlite.upsert_symbol(&a).unwrap();
    sqlite.upsert_symbol(&b).unwrap();
    sqlite.upsert_symbol(&d).unwrap();

    sqlite.upsert_edge(&EdgeRow {
        from_symbol_id: "b".to_string(),
        to_symbol_id: "a".to_string(),
        edge_type: "implements".to_string(),
        at_file: Some("src/a.ts".to_string()),
        at_line: Some(1),
        confidence: 1.0,
        evidence_count: 1,
        resolution: "local".to_string(),
    }).unwrap();
    sqlite.upsert_edge(&EdgeRow {
        from_symbol_id: "d".to_string(),
        to_symbol_id: "b".to_string(),
        edge_type: "extends".to_string(),
        at_file: Some("src/a.ts".to_string()),
        at_line: Some(1),
        confidence: 1.0,
        evidence_count: 1,
        resolution: "local".to_string(),
    }).unwrap();

    // "both" from B should find A (downstream via implements) and D (upstream via extends)
    let g = build_type_graph(&sqlite, &b, "both", 3, 100).unwrap();
    let nodes = g.get("nodes").unwrap().as_array().unwrap();
    assert_eq!(nodes.len(), 3, "Should find B, A, and D. Got: {:?}", nodes);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib graph::tests::type_graph_upstream_finds_implementors -- --nocapture 2>&1 | tail -20`

Expected: Compile error — `build_type_graph` doesn't accept `direction` parameter yet.

**Step 3: Implement bidirectional `build_type_graph`**

Replace `build_type_graph` in `src/graph/mod.rs:322-412` with:

```rust
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

    let do_downstream = direction == "downstream" || direction == "both";
    let do_upstream = direction == "upstream" || direction == "both";

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

            let type_edge_types = ["extends", "implements", "alias"];

            // Downstream: what does current extend/implement?
            if do_downstream {
                let outgoing = sqlite.list_edges_from(&current_id, limit)?;
                for e in outgoing {
                    if edges.len() >= limit {
                        break;
                    }
                    if !type_edge_types.contains(&e.edge_type.as_str()) {
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

            // Upstream: who extends/implements current?
            if do_upstream {
                let incoming = sqlite.list_edges_to(&current_id, limit)?;
                for e in incoming {
                    if edges.len() >= limit {
                        break;
                    }
                    if !type_edge_types.contains(&e.edge_type.as_str()) {
                        continue;
                    }
                    let Some(from_sym) = sqlite.get_symbol_by_id(&e.from_symbol_id)? else {
                        continue;
                    };
                    nodes.entry(from_sym.id.clone()).or_insert_with(|| {
                        json!({
                            "id": from_sym.id,
                            "name": from_sym.name,
                            "kind": from_sym.kind,
                            "file_path": from_sym.file_path,
                            "language": from_sym.language,
                            "exported": from_sym.exported,
                            "line_range": [from_sym.start_line, from_sym.end_line],
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
                    if visited.insert(from_sym.id.clone()) {
                        next.push(from_sym.id);
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
```

**Step 4: Update `handle_get_type_graph` to pass direction**

In `src/handlers/mod.rs:535-558`, change the `build_type_graph` call:

```rust
pub fn handle_get_type_graph(
    state: &AppState,
    tool: GetTypeGraphTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let depth = tool.depth.unwrap_or(2) as usize;
    let limit = tool.limit.unwrap_or(200).max(1) as usize;
    let direction = tool.direction.as_deref().unwrap_or("both");

    let sqlite = &state.sqlite;

    let roots = sqlite.search_symbols_by_exact_name(&tool.symbol_name, None, 10)?;
    let root = roots.first().cloned();

    let Some(root) = root else {
        return Ok(json!({
            "symbol_name": tool.symbol_name,
            "depth": depth,
            "nodes": [],
            "edges": [],
        }));
    };

    let graph = build_type_graph(sqlite, &root, direction, depth, limit)?;
    Ok(graph)
}
```

**Step 5: Update existing test call site**

The existing test at `src/graph/mod.rs:518` calls `build_type_graph(&sqlite, &a, 3, 100)` — update to `build_type_graph(&sqlite, &a, "downstream", 3, 100)` so it continues testing downstream behavior.

**Step 6: Run all graph tests**

Run: `cargo test --lib graph::tests -- --nocapture 2>&1 | tail -20`

Expected: All 4 tests pass (2 existing + 2 new)

**Step 7: Run full test suite**

Run: `EMBEDDINGS_BACKEND=hash cargo test 2>&1 | tail -10`

Expected: All tests pass

**Step 8: Commit**

```bash
git add src/tools/mod.rs src/graph/mod.rs src/handlers/mod.rs
git commit -m "feat: add bidirectional type graph traversal with direction parameter"
```

---

### Task 4: Call-graph-based test mapping — add SQL query

**Files:**
- Modify: `src/storage/sqlite/queries/tests.rs` (add new query function)
- Modify: `src/storage/sqlite/mod.rs` (expose on `SqliteStore`)

**Step 1: Write the SQL query function**

Add to `src/storage/sqlite/queries/tests.rs` after `get_symbols_with_tests` (after line 188):

```rust
/// Find test symbols that have call/reference edges to a target symbol
pub fn find_test_symbols_calling(
    conn: &Connection,
    test_file_paths: &[String],
    target_symbol_id: &str,
    limit: usize,
) -> Result<Vec<(String, String, String, u32, String)>> {
    if test_file_paths.is_empty() {
        return Ok(Vec::new());
    }

    // Build placeholders for IN clause
    let placeholders: Vec<String> = (0..test_file_paths.len())
        .map(|i| format!("?{}", i + 1))
        .collect();
    let in_clause = placeholders.join(", ");

    let target_param = test_file_paths.len() + 1;
    let limit_param = test_file_paths.len() + 2;

    let sql = format!(
        r#"
SELECT s.id, s.name, s.file_path, s.start_line, e.edge_type
FROM symbols s
JOIN edges e ON e.from_symbol_id = s.id
WHERE s.file_path IN ({in_clause})
  AND e.to_symbol_id = ?{target_param}
  AND e.edge_type IN ('call', 'reference')
ORDER BY s.file_path, s.start_line
LIMIT ?{limit_param}
"#
    );

    let mut stmt = conn.prepare(&sql).context("Failed to prepare find_test_symbols_calling")?;

    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    for path in test_file_paths {
        params_vec.push(Box::new(path.clone()));
    }
    params_vec.push(Box::new(target_symbol_id.to_string()));
    params_vec.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let mut rows = stmt.query(param_refs.as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push((
            row.get::<_, String>(0)?,  // symbol id
            row.get::<_, String>(1)?,  // symbol name
            row.get::<_, String>(2)?,  // file path
            row.get::<_, i64>(3)? as u32, // start line
            row.get::<_, String>(4)?,  // edge type
        ));
    }
    Ok(out)
}
```

**Step 2: Expose on SqliteStore**

Add to `src/storage/sqlite/mod.rs` after the `get_symbols_with_tests` method (around line 435):

```rust
pub fn find_test_symbols_calling(
    &self,
    test_file_paths: &[String],
    target_symbol_id: &str,
    limit: usize,
) -> Result<Vec<(String, String, String, u32, String)>> {
    let conn = self.read()?;
    queries::tests::find_test_symbols_calling(&conn, test_file_paths, target_symbol_id, limit)
}
```

**Step 3: Write the test**

Add to the `#[cfg(test)]` module in `src/handlers/mod.rs` (from Task 1):

```rust
#[test]
fn find_tests_for_symbol_returns_calling_test_functions() {
    let sqlite = make_sqlite();

    // Source symbol
    sqlite.upsert_symbol(&sym("auth_fn", "authenticate", "src/auth.ts")).unwrap();

    // Test symbols in test file
    sqlite.upsert_symbol(&sym("test1", "should_reject_invalid", "src/auth.test.ts")).unwrap();
    sqlite.upsert_symbol(&sym("test2", "should_accept_valid", "src/auth.test.ts")).unwrap();
    sqlite.upsert_symbol(&sym("test3", "unrelated_test", "src/auth.test.ts")).unwrap();

    // test1 calls authenticate, test2 calls authenticate, test3 does not
    sqlite.upsert_edge(&edge("test1", "auth_fn", "call")).unwrap();
    sqlite.upsert_edge(&edge("test2", "auth_fn", "call")).unwrap();

    let test_files = vec!["src/auth.test.ts".to_string()];
    let results = sqlite.find_test_symbols_calling(&test_files, "auth_fn", 20).unwrap();

    assert_eq!(results.len(), 2, "Should find 2 test functions calling authenticate. Got: {:?}", results);
    let names: Vec<&str> = results.iter().map(|(_, name, _, _, _)| name.as_str()).collect();
    assert!(names.contains(&"should_reject_invalid"));
    assert!(names.contains(&"should_accept_valid"));
    assert!(!names.contains(&"unrelated_test"));
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib handlers::tests::find_tests_for_symbol_returns_calling_test_functions -- --nocapture 2>&1 | tail -20`

Expected: PASS

**Step 5: Commit**

```bash
git add src/storage/sqlite/queries/tests.rs src/storage/sqlite/mod.rs src/handlers/mod.rs
git commit -m "feat: add SQL query for call-graph-based test symbol lookup"
```

---

### Task 5: Call-graph-based test mapping — enrich the handler

**Files:**
- Modify: `src/handlers/mod.rs:1670-1708` (function `handle_find_tests_for_symbol`)

**Step 1: Implement the enriched handler**

Replace `handle_find_tests_for_symbol` at `src/handlers/mod.rs:1670-1708`:

```rust
pub fn handle_find_tests_for_symbol(
    state: &AppState,
    tool: FindTestsForSymbolTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(20).max(1) as usize;

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

    // Get test files for this symbol's source file (existing heuristic)
    let test_files = sqlite.get_tests_for_source(&root.file_path)?;

    // Call-graph enrichment: find specific test functions that call/reference this symbol
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

    // Legacy: symbols with tests (file-level)
    let symbols_with_tests = sqlite.get_symbols_with_tests(&root.file_path)?;

    // Build display
    let display = format_test_results(root, &test_files, &symbols_with_tests);

    Ok(json!({
        "symbol_name": root.name,
        "symbol_kind": root.kind,
        "source_file": root.file_path,
        "test_file_count": test_files.len(),
        "test_files": test_files,
        "tests_for_symbol": tests_for_symbol,
        "symbols_with_tests": symbols_with_tests,
        "display": display,
    }))
}
```

**Step 2: Verify compilation**

Run: `cargo check 2>&1 | tail -10`

Expected: Compiles clean

**Step 3: Run full test suite**

Run: `EMBEDDINGS_BACKEND=hash cargo test 2>&1 | tail -10`

Expected: All tests pass

**Step 4: Commit**

```bash
git add src/handlers/mod.rs
git commit -m "feat: enrich find_tests_for_symbol with call-graph-based test function detection"
```

---

### Task 6: Build release binary and manual verification

**Files:** None (verification only)

**Step 1: Build release binary**

Run: `cargo build --release 2>&1 | tail -5`

Expected: Compiles successfully

**Step 2: Run full test suite one more time**

Run: `EMBEDDINGS_BACKEND=hash cargo test 2>&1 | tail -10`

Expected: All tests pass

**Step 3: Verify all three fixes work via MCP tools (manual)**

After restarting the MCP server, test:

1. `trace_data_flow` on a TypeScript symbol that has reads/writes edges — should now show actual data flow instead of only call/reference
2. `get_type_graph` with `direction: "upstream"` on an interface — should find implementors
3. `find_tests_for_symbol` on a function with tests — should include `tests_for_symbol` with specific test function names

**Step 4: Final commit (if any adjustments)**

```bash
# Only if manual testing reveals issues to fix
```

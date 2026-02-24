# Async Edge Exposure & Inter-Procedural Data Flow — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix async boundary edges being silently dropped, expose them in call hierarchy and data flow handlers, and add 1-level inter-procedural data flow expansion.

**Architecture:** Parse `await:`/`spawn:` prefixes in `edges.rs` pipeline to store as `async_call`/`spawn` edge types. Update `build_call_hierarchy` to include these edge types. Update `trace_data_flow` to map them. Add `inter_procedural` parameter for 1-level callee expansion.

**Tech Stack:** Rust, SQLite

---

### Task 1: Parse async prefixes in edges.rs pipeline

**Files:**
- Modify: `src/indexer/pipeline/edges.rs:493-546`

**Changes:**

In the data flow edge processing loop (line 494: `for dfe in dataflow_edges`), BEFORE the existing resolution logic (line 496), add prefix detection:

```rust
// Handle data flow edges
for dfe in dataflow_edges {
    // Detect async boundary prefixes
    let (async_kind, actual_from) = if let Some(rest) = dfe.from_symbol.strip_prefix("await:") {
        (Some("async_call"), rest.to_string())
    } else if let Some(rest) = dfe.from_symbol.strip_prefix("spawn:") {
        (Some("spawn"), rest.to_string())
    } else {
        (None, dfe.from_symbol.clone())
    };

    // Resolve actual_from (instead of dfe.from_symbol) to symbol ID
    let (to_id, was_import) = if let Some(local_id) = name_to_id.get(&actual_from) {
        // ... existing logic using actual_from
    } else if let Some(imp) = import_map.get(actual_from.as_str()) {
        // ... existing logic using actual_from
    } else if let Some(ref scope) = dfe.scope {
        let synthetic_id = format!("local:{}#{}::{}", row.file_path, scope, actual_from);
        (Some(synthetic_id), false)
    } else if async_kind.is_some() {
        // Async edges without scope still get a synthetic ID (the callee name itself is meaningful)
        let synthetic_id = format!("async:{}#{}", row.file_path, actual_from);
        (Some(synthetic_id), false)
    } else {
        continue;
    };

    if let Some(id) = to_id {
        // Use async_kind for edge_type if present, otherwise reads/writes
        let edge_type = if let Some(ak) = async_kind {
            ak
        } else {
            match dfe.flow_type {
                DataFlowType::Reads => "reads",
                DataFlowType::Writes => "writes",
            }
        };

        // ... rest of edge creation (skip-if-duplicate, resolution, push to out)
        // Use confidence 0.9 for async edges, 0.7 for data flow
        let confidence = if async_kind.is_some() { 0.9 } else { 0.7 };
    }
}
```

Key points:
- `actual_from` is the callee name with prefix stripped
- `async_kind` determines whether to use `"async_call"` or `"spawn"` as edge_type
- Async edges without scope get a synthetic `async:` ID (so they're not silently dropped)
- The `to_symbol_id` in the EdgeRow is `id` (the resolved callee), `from_symbol_id` is `row.id` (the enclosing function)

### Task 2: Include async edges in build_call_hierarchy

**Files:**
- Modify: `src/graph/mod.rs:224,244,250,270,290,296`

**Changes:**

1. Replace the edge type filter (line 224 for callers, line 270 for callees):

```rust
// Before (line 224):
if e.edge_type != "call" {
    continue;
}

// After:
if e.edge_type != "call" && e.edge_type != "async_call" && e.edge_type != "spawn" {
    continue;
}
```

Apply same change at line 270 (callees direction).

2. Use actual edge type in response instead of hardcoded "call" (lines 244 and 290):

```rust
// Before (line 244):
"edge_type": "call",

// After:
"edge_type": &e.edge_type,
"is_async": e.edge_type == "async_call" || e.edge_type == "spawn",
```

Apply same change at line 290.

3. Pass actual edge type to `list_edge_evidence` (lines 250 and 296):

```rust
// Before (line 250):
.list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, "call", 3)

// After:
.list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, &e.edge_type, 3)
```

Apply same change at line 296.

### Task 3: Map async edges in trace_data_flow

**Files:**
- Modify: `src/handlers/mod.rs:992-998,1039-1044`

**Changes:**

In `trace_data_flow_edges()`, extend the edge type mapping in BOTH outgoing (line 992) and incoming (line 1039) blocks:

```rust
// Outgoing (line 992):
let flow_type = match edge.edge_type.as_str() {
    "reads" => "read",
    "writes" => "write",
    "call" | "reference" => "read",
    "async_call" => "async_read",
    "spawn" => "spawn",
    _ => continue,
};

// Incoming (line 1039):
let flow_type = match edge.edge_type.as_str() {
    "reads" => "read",
    "writes" => "write",
    "call" => "read",
    "async_call" => "async_read",
    "spawn" => "spawn",
    _ => continue,
};
```

Also update the direction filter (lines 1000-1003 and 1047-1050) to include async types:

```rust
let match_direction = match direction {
    "reads" => flow_type == "read" || flow_type == "async_read",
    "writes" => flow_type == "write",
    _ => true,
};
```

Update the response counts (around line 952) to include async_read and spawn:

```rust
"read_count": flows.iter().filter(|f| {
    let ft = f.get("flow_type").and_then(|v| v.as_str()).unwrap_or("");
    ft == "read" || ft == "async_read"
}).count(),
"spawn_count": flows.iter().filter(|f| f.get("flow_type").and_then(|v| v.as_str()) == Some("spawn")).count(),
```

### Task 4: Add inter_procedural parameter to TraceDataFlowTool

**Files:**
- Modify: `src/tools/mod.rs` — add field to TraceDataFlowTool
- Modify: `src/handlers/mod.rs` — add expansion logic to handle_trace_data_flow

**Changes in tools/mod.rs:**

Add field to TraceDataFlowTool:
```rust
/// Enable 1-level inter-procedural expansion into called functions (default false)
pub inter_procedural: Option<bool>,
```

**Changes in handlers/mod.rs:**

After building the `flows` array (around line 940), if `inter_procedural` is true, expand call edges:

```rust
let inter_proc = tool.inter_procedural.unwrap_or(false);

// ... existing flow building ...

// Inter-procedural expansion: for each call/async_call edge, get callee's data flow
if inter_proc {
    for flow in &mut flows {
        let flow_type = flow.get("flow_type").and_then(|v| v.as_str()).unwrap_or("");
        if flow_type != "read" && flow_type != "async_read" {
            continue;
        }

        let sym_id = flow.get("symbol_id").and_then(|v| v.as_str()).unwrap_or("");
        let sym_kind = flow.get("kind").and_then(|v| v.as_str()).unwrap_or("");

        // Only expand functions/methods
        if sym_kind != "function" && sym_kind != "method" && sym_kind != "arrow_function" {
            continue;
        }

        // Get callee's direct data flow (depth=1, limit=20)
        let callee_edges = sqlite.list_edges_from(sym_id, 20)?;
        let mut called_flows = Vec::new();

        for edge in callee_edges {
            let ft = match edge.edge_type.as_str() {
                "reads" => "read",
                "writes" => "write",
                "call" => "read",
                "async_call" => "async_read",
                "spawn" => "spawn",
                _ => continue,
            };

            if let Some(target) = sqlite.get_symbol_by_id(&edge.to_symbol_id)? {
                called_flows.push(json!({
                    "symbol_name": target.name,
                    "symbol_id": target.id,
                    "kind": target.kind,
                    "flow_type": ft,
                    "file_path": target.file_path,
                    "line": target.start_line,
                }));
            }
        }

        if !called_flows.is_empty() {
            flow.as_object_mut().unwrap().insert(
                "called_flows".to_string(),
                json!(called_flows),
            );
        }
    }
}
```

### Task 5: Tests for async edge storage

**Files:**
- Modify: `src/indexer/pipeline/edges.rs` — add tests in `#[cfg(test)]` module

**Tests:**

```rust
#[test]
fn test_async_await_prefix_creates_async_call_edge() {
    // Create DataFlowEdge with from_symbol = "await:fetch_data"
    // Verify edge stored with edge_type = "async_call"
}

#[test]
fn test_spawn_prefix_creates_spawn_edge() {
    // Create DataFlowEdge with from_symbol = "spawn:tokio::spawn"
    // Verify edge stored with edge_type = "spawn"
}

#[test]
fn test_no_prefix_creates_reads_writes_edge() {
    // Create normal DataFlowEdge (no prefix)
    // Verify edge_type is "reads" or "writes"
}
```

### Task 6: Tests for call hierarchy async edges

**Files:**
- Modify: `src/graph/mod.rs` — add tests

**Tests:**

```rust
#[test]
fn test_call_hierarchy_includes_async_call_edges() {
    // Verify async_call edges are not filtered out
}

#[test]
fn test_call_hierarchy_edge_type_not_hardcoded() {
    // Verify response includes actual edge_type, not always "call"
}
```

### Task 7: Tests for inter-procedural expansion

**Files:**
- Modify: `src/handlers/mod.rs` — add test

**Tests:**

```rust
#[test]
fn test_inter_procedural_false_no_called_flows() {
    // Verify called_flows is not present when inter_procedural=false
}
```

### Task 8: Build + test verification

Run: `EMBEDDINGS_BACKEND=hash cargo test`
Run: `cargo build --release`
Verify all tests pass and release build is clean.

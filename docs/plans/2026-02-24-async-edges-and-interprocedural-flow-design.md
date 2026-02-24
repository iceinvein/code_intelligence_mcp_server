# Async Edge Exposure & Inter-Procedural Data Flow — Design

**Goal:** Fix async boundary edges being silently dropped in the pipeline, expose them in `get_call_hierarchy` and `trace_data_flow`, and add 1-level inter-procedural data flow expansion.

**Architecture:** Three pipeline fixes (edges.rs prefix parsing, call hierarchy filter, trace_data_flow mapping) plus one handler enhancement (inter-procedural expansion). No schema changes — uses existing VARCHAR edge_type column.

**Tech Stack:** Rust, SQLite

---

## Feature 1: Fix Async Edge Storage

### Problem

Extractors emit `DataFlowEdge { from_symbol: "spawn:tokio::spawn", ... }` but `edges.rs` tries to resolve `"spawn:tokio::spawn"` in `name_to_id`, fails, and drops the edge. All `await:` and `spawn:` prefixed edges are silently lost.

### Solution

In `src/indexer/pipeline/edges.rs`, in the data flow edge processing loop:

1. Detect `await:` and `spawn:` prefixes on `dfe.from_symbol`
2. Strip the prefix to get the actual callee name (e.g., `"spawn:tokio::spawn"` → `"tokio::spawn"`)
3. Resolve the callee in `name_to_id` or import_map
4. Store with `edge_type = "async_call"` (for `await:`) or `edge_type = "spawn"` (for `spawn:`) instead of the default reads/writes mapping
5. If callee can't be resolved, create synthetic ID using scope (same as local variables)
6. The `to_symbol` field is resolved normally (it's the function being awaited/spawned)

### Edge type mapping

| Prefix | Stored edge_type | Confidence |
|--------|-----------------|------------|
| `await:` | `async_call` | 0.9 |
| `spawn:` | `spawn` | 0.9 |
| (none) | `reads` or `writes` | 0.7 |

---

## Feature 2: Expose Async Edges in Call Hierarchy

### Problem

`build_call_hierarchy` in `src/graph/mod.rs` filters `if e.edge_type != "call" { continue; }` — async_call and spawn edges are skipped. Also hardcodes `"edge_type": "call"` in output.

### Solution

1. Change edge filter to include async types: `["call", "async_call", "spawn"].contains(&e.edge_type.as_str())`
2. Use actual `e.edge_type` in response JSON instead of hardcoded `"call"`
3. Add `is_async: bool` convenience field to each edge in output (`true` for async_call and spawn)

---

## Feature 3: Expose Async Edges in Data Flow

### Problem

`trace_data_flow` handler maps `"reads"` → "read", `"writes"` → "write", `"call"` → "read". It skips unrecognized edge types including `"async_call"` and `"spawn"`.

### Solution

Add mappings in `trace_data_flow_edges()`:
- `"async_call"` → flow_type = `"async_read"` (awaited call is a data flow read with async semantics)
- `"spawn"` → flow_type = `"spawn"` (fire-and-forget, different from read)

---

## Feature 4: Inter-Procedural Data Flow (1-level)

### Problem

`trace_data_flow("process")` only returns data flow within `process`. When `process` calls `validate(input)`, the flow through `validate`'s body is invisible.

### Solution

Add `inter_procedural: Option<bool>` parameter to `TraceDataFlowTool` (default false).

When enabled, after the initial BFS traversal:
1. For each "call" or "async_call" edge where `to_symbol` is a function/method, query that function's data flow edges (reads/writes only, depth=1)
2. Attach results as a `called_flows` array nested under the calling flow entry
3. Only expand 1 level deep regardless of the `depth` parameter
4. Skip expansion for symbols already visited (prevent cycles)

### Response format addition

```json
{
  "flows": [
    {
      "symbol_name": "validate",
      "flow_type": "read",
      "edge_type": "call",
      "file_path": "src/api.rs",
      "line": 42,
      "called_flows": [
        {
          "symbol_name": "email_regex",
          "flow_type": "read",
          "file_path": "src/validation.rs",
          "line": 15
        }
      ]
    }
  ]
}
```

`called_flows` is only present when `inter_procedural = true` and the target is a function with data flow edges.

---

## Testing

- Async edge storage: unit tests in edges.rs verifying prefix parsing and correct edge_type
- Call hierarchy: test that async_call/spawn edges appear in output with correct edge_type
- Data flow: test that async edge types map to correct flow_types
- Inter-procedural: test that called_flows array is populated for call edges

## Out of Scope

- Multi-level inter-procedural expansion (depth > 1)
- Channel send/recv edges (Go-specific, low ROI)
- Async edge visualization in search results

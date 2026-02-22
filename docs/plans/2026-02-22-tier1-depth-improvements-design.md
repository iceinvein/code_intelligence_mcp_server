# Tier 1 Depth Improvements Design

Date: 2026-02-22

## Context

The code intelligence MCP server has three tools with broken or incomplete core functionality. These tools have existing infrastructure (edge types, SQL queries, graph traversal) but the wiring is wrong. Fixing them is the highest-ROI depth work available.

Constructor call detection (`new Foo()`) was originally included as Fix 4 but verified to already work correctly — the byte scanner skips `new` (stopword) and captures `Foo(` as expected.

## Fix 1: Wire `trace_data_flow` to actual read/write edges

### Problem

`trace_data_flow_edges` in `src/handlers/mod.rs:958-1029` discards the only edge types that carry real data flow information. The match at line 990-994:

```rust
let flow_type = match edge.edge_type.as_str() {
    "call" => "read",
    "reference" => "read",
    "extends" | "implements" => "read",
    _ => continue,  // <-- discards "reads" and "writes" edges
};
```

The TypeScript extractor (`src/indexer/extract/typescript.rs:592+`) produces `reads` and `writes` edges via `extract_dataflow_from_function_body`, and these are stored in the DB via `src/indexer/pipeline/edges.rs:494-536`. But the handler never queries them.

Additionally, the handler only calls `list_edges_from()` (outgoing edges). For write tracing, incoming edges are needed — "who writes to this variable?" requires `list_edges_to()`.

### Solution

1. Map `"reads"` -> `"read"` and `"writes"` -> `"write"` in the edge type match
2. Add bidirectional traversal:
   - **Outgoing** edges: what does this symbol read/write/call (current behavior, extended)
   - **Incoming** edges via `list_edges_to()`: who reads/writes/calls this symbol
3. Keep `call`/`reference` as secondary data flow signals (implicit reads)
4. Deduplicate by `visited` set across both directions

### Scope

~30 lines changed in `src/handlers/mod.rs`, `trace_data_flow_edges` function only.

## Fix 2: Bidirectional type graph with `direction` parameter

### Problem

`build_type_graph` in `src/graph/mod.rs:322-412` only calls `list_edges_from()`, traversing downstream (`extends`/`implements`/`alias` edges from root outward). This means "who implements this interface?" or "what classes extend this base class?" is unanswerable.

`list_edges_to()` already exists in `src/storage/sqlite/queries/edges.rs:219` but is never called from the type graph builder.

### Solution

1. Add `direction: Option<String>` to `GetTypeGraphTool` in `src/tools/mod.rs`:
   - `"downstream"` — current behavior (what does root extend/implement?)
   - `"upstream"` — reverse (who extends/implements root?)
   - `"both"` — merge both directions (default)
2. In `build_type_graph`, accept direction parameter:
   - `"downstream"`: `list_edges_from()` filtered to `extends`/`implements`/`alias` (current code)
   - `"upstream"`: `list_edges_to()` filtered to same edge types
   - `"both"`: run both passes, merge nodes/edges, deduplicate by visited set
3. Edge entries in the response already include `from`/`to` fields, so direction is implicit in the data

### Scope

~40 lines in `src/graph/mod.rs`, ~5 lines in `src/tools/mod.rs`.

## Fix 3: Call-graph-based test mapping

### Problem

`handle_find_tests_for_symbol` in `src/handlers/mod.rs:1670-1708` returns test *files* for a source *file* via filename heuristic. It has no symbol-level granularity — every symbol in a source file gets the same test files listed.

Additional bugs:
- `_limit` (line 1674) is computed but never applied (underscore prefix = unused)

### Solution

1. After finding test files via existing heuristic, query all symbols in those test files from SQLite
2. For each test symbol, check `list_edges_from(test_symbol_id)` for `call`/`reference` edges whose `to_symbol_id` matches the queried symbol's ID
3. Return a new `tests_for_symbol` array containing the specific test functions that reference the target
4. Apply the `limit` parameter to the results
5. Keep existing `test_files` field as fallback for when edges are sparse

### New SQL query

Query symbols in test files and their outgoing edges to the target symbol:

```sql
SELECT s.id, s.name, s.file_path, s.start_line, e.edge_type
FROM symbols s
JOIN edges e ON e.from_symbol_id = s.id
WHERE s.file_path IN (/* test files */)
  AND e.to_symbol_id = ?target_symbol_id
  AND e.edge_type IN ('call', 'reference')
ORDER BY s.file_path, s.start_line
```

### New response shape

```json
{
  "symbol_name": "authenticate",
  "symbol_kind": "function",
  "source_file": "src/auth.ts",
  "test_file_count": 1,
  "test_files": ["src/auth.test.ts"],
  "tests_for_symbol": [
    {
      "test_name": "should_reject_invalid_token",
      "test_file": "src/auth.test.ts",
      "line": 42,
      "edge_type": "call"
    }
  ],
  "display": "..."
}
```

### Scope

~50 lines in `src/handlers/mod.rs`, ~20 lines new query in `src/storage/sqlite/queries/tests.rs`.

## Files touched

| File | Changes |
|------|---------|
| `src/handlers/mod.rs` | Fix 1: rewrite `trace_data_flow_edges`. Fix 3: enrich `handle_find_tests_for_symbol` |
| `src/graph/mod.rs` | Fix 2: add direction param to `build_type_graph` |
| `src/tools/mod.rs` | Fix 2: add `direction` field to `GetTypeGraphTool` |
| `src/storage/sqlite/queries/tests.rs` | Fix 3: new query for test symbols with edges to target |
| `src/storage/sqlite/mod.rs` | Fix 3: expose new query method on `SqliteStore` |

## Testing strategy

- Unit tests for each fix using the existing test infrastructure (in-memory SQLite, mock symbols/edges)
- Integration test: `EMBEDDINGS_BACKEND=hash cargo test` to verify no regressions
- Manual verification via MCP tools on this codebase (dogfood)

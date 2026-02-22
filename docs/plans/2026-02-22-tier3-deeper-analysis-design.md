# Tier 3: Deeper Analysis — Design

**Goal:** Improve cross-symbol analysis quality by tracking scope-aware locals, async boundaries, and inter-procedural data flow. This makes `trace_data_flow`, `get_call_hierarchy`, and `explore_dependency_graph` more useful for real-world debugging and refactoring tasks.

**Architecture:** Extend existing extractors and edge processing pipeline. Requires minor data model additions (scope field on DataFlowEdge, async edge metadata). No new storage tables needed — edges table and existing `edge_type` column handle new variants.

**Tech Stack:** Rust, tree-sitter, SQLite (edges table)

---

## Priority Order (by ROI)

### 1. Scope-Aware Symbol Resolution (HIGH ROI)

**Problem:** Data flow edges for local variables are silently dropped. When `extract_python_dataflow` emits `DataFlowEdge { from_symbol: "result", to_symbol: "process_data" }`, the `edges.rs` pipeline tries to resolve "result" in the `name_to_id` map. Since locals aren't symbols, the edge is dropped. This means `trace_data_flow` can't track variable flow through function bodies.

**Solution:** Add a `scope` field to DataFlowEdge:
- `scope: Option<String>` — the enclosing function/method name
- In `edges.rs`, when resolving `from_symbol` fails in `name_to_id`, create a synthetic local ID: `<file_path>#<scope>.<local_name>`
- Store edges with synthetic IDs in the edges table
- `trace_data_flow` handler already resolves by name — extend to check locals within scope

**Impact:** All 7 language extractors that emit data flow edges (TS, Rust, Go, Python, Java, C, C++) benefit immediately. No extractor changes needed — just pipeline changes in `edges.rs` and query changes in handlers.

**Estimated tasks:** 4-5 (data model, edge processing, query handler, tests)

### 2. Async Boundary Tracking (MEDIUM-HIGH ROI)

**Problem:** `get_call_hierarchy` doesn't distinguish sync calls from async boundaries (`.await`, `tokio::spawn`, `Promise.all`, goroutine `go func()`). This matters for performance analysis and debugging.

**Solution:** Add async-related edge types:
- `edge_type = "async_call"` for `.await` expressions
- `edge_type = "spawn"` for task spawning (`tokio::spawn`, `go func()`, `Promise.all`, `asyncio.create_task`)
- `edge_type = "channel_send"` / `channel_recv"` for channel operations

**Detection (per language):**
- **Rust:** `await_expression` wrapping a call → async_call. `call_expression` where callee contains "spawn" → spawn.
- **TypeScript/JavaScript:** `await_expression` → async_call. `Promise.all/race/allSettled` → spawn.
- **Python:** `await` expression → async_call. `asyncio.create_task/gather` → spawn.
- **Go:** `go_statement` → spawn. Channel `<-` operator → channel_send/recv.

**Impact:** `get_call_hierarchy` can annotate edges with async boundary info. Useful for distributed tracing and concurrency debugging.

**Estimated tasks:** 6-8 (one per language + edge storage + handler update)

### 3. Inter-Procedural Data Flow — Shallow (MEDIUM ROI, HIGH complexity)

**Problem:** `trace_data_flow` only tracks within a single function. When `fn process(input: Data)` calls `validate(input)`, the flow from `process.input` to `validate`'s parameter isn't tracked.

**Solution (shallow, 1 level):** At query time, when `trace_data_flow` finds `process reads validate`, follow into `validate`'s body to find what `validate` reads/writes. This is a query-time join, not index-time.

**Approach:**
1. `trace_data_flow("process", depth=1)` returns data flow within process
2. For each "reads" edge to another function, recurse into that function's data flow
3. Cap at depth=1 to prevent explosion

**Impact:** Enables "what does this function touch transitively?" queries. Useful for impact analysis.

**Estimated tasks:** 3-4 (handler changes + depth parameter + tests)

### 4. Generic Type Parameterization (LOW-MEDIUM ROI)

**Problem:** Type edges lose generic parameters. `HashMap<String, User>` creates a type edge to "HashMap" but not to "String" or "User". This means `get_type_graph` misses transitive type dependencies.

**Solution:** Extract type arguments from generic types:
- Rust: `generic_type` → `type_identifier` + `type_arguments`
- TypeScript: `generic_type` → `type_identifier` + `type_arguments`
- Java: `generic_type` → `type_identifier` + `type_arguments`
- Go: N/A (no generics in type edges currently)

**Impact:** `get_type_graph` returns richer graphs. Moderate ROI because most users search by concrete types, not by "what uses HashMap".

**Estimated tasks:** 4-5 (one per language + tests)

### 5. Closure/Lambda Capture Tracking (LOW ROI, HIGH complexity)

**Problem:** Closures capture variables from enclosing scope. This creates implicit data flow that isn't tracked.

**Recommendation:** DEFER. The complexity of correctly identifying captures across all languages (Rust's explicit `move`, JS/Python's implicit capture, Go's goroutine captures) is high. ROI is low because most searches don't require capture tracking.

**Estimated tasks:** 8-10 (deferred)

## Out of Scope

- Full inter-procedural analysis (depth > 1) — query-time explosion risk
- Effect systems / purity analysis — too language-specific
- Lifetime tracking for Rust — tree-sitter can't validate borrow checker
- Control flow graphs — too granular for search use case

## Data Model Changes

```rust
// DataFlowEdge — add scope field
pub struct DataFlowEdge {
    pub from_symbol: String,
    pub to_symbol: String,
    pub flow_type: DataFlowType,
    pub at_line: u32,
    pub scope: Option<String>,  // NEW: enclosing function name
}

// New edge types (string constants, no enum change needed)
// "async_call", "spawn", "channel_send", "channel_recv"
// Stored in edges.edge_type column (already VARCHAR)
```

## Testing

Unit tests per feature. Integration: build release, index a polyglot repo, verify `trace_data_flow` and `get_call_hierarchy` return async/local edges.

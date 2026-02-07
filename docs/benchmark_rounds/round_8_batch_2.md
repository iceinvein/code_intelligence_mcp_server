# Round 8 - Batch 2

| # | Query | CI | Augment | Winner | Pattern |
|---|-------|-----|---------|--------|---------|
| 6 | MCP server tool request handling | 6 | 9 | Augment | Missing body text |
| 7 | WebSocket handler | 3 | 5 | Augment | Keyword mismatch |
| 8 | SQLite schema tables init | 6 | 9 | Augment | Missing body text |
| 9 | Error handling graceful degradation | 4 | 8 | Augment | Single-file flooding |
| 10 | JSON serialization response formatting | 4 | 7 | Augment | Keyword mismatch |

## Per-Query Notes

### Q6: "How does the MCP server handle incoming tool requests?"
- **CI top-3:** handlers/mod.rs:tool_internal_error, metrics/server.rs:spawn_metrics_server, handlers/mod.rs:handle_search_code
- **Augment top-3:** server/mod.rs (handle_call_tool_request dispatch), main.rs (server setup), tools/mod.rs (tool definitions)
- **CI miss:** `server/mod.rs` with `handle_call_tool_request` and the match-based dispatch -- the core answer to the query -- was absent from CI results entirely
- **CI hit:** Found `tool_internal_error` and handler functions, but missed the routing/dispatch layer which is the key answer

### Q7: "How does the WebSocket handler work?"
- **CI top-3:** handlers/mod.rs:tool_internal_error, handlers/mod.rs:handle_get_index_stats, elysia.rs:extracts_websocket (test)
- **Augment top-3:** handlers/mod.rs (various handler code), web_ui.rs, server/mod.rs
- **CI miss:** This project has no WebSocket handler -- CI returned a test function `extracts_websocket` from elysia.rs which only tests extraction of WS patterns; both tools struggled since there's no real WS handler
- **CI hit:** Found the elysia websocket extraction test, but top-2 results were completely irrelevant (tool_internal_error, get_index_stats)

### Q8: "SQLite database schema tables initialization"
- **CI top-3:** sqlite/operations.rs:SqliteStore, sqlite/operations.rs:open, sqlite/mod.rs:schema (module re-export)
- **Augment top-3:** sqlite/schema.rs (full SCHEMA_SQL with all CREATE TABLE statements), sqlite/operations.rs (open + init), sqlite/mod.rs
- **CI miss:** `schema.rs` with the actual `SCHEMA_SQL` constant containing all CREATE TABLE statements -- the primary answer. CI found `init()` which calls it but not the SQL itself
- **CI hit:** Found `SqliteStore`, `open()`, `init()`, and `clear_all()` -- all relevant but the actual schema definition was missing

### Q9: "Error handling and graceful degradation"
- **CI top-3:** handlers/mod.rs:tool_internal_error, handlers/mod.rs:parse_tool_args, handlers/mod.rs:handle_refresh_index
- **Augment top-3:** handlers/mod.rs:tool_internal_error, retrieval/mod.rs (graceful degradation from vector to keyword), main.rs (error mapping)
- **CI miss:** `retrieval/mod.rs` graceful degradation logic (vector search fallback to keyword-only), `reranker/mod.rs` fallback behavior, `pipeline/parallel.rs` retry logic
- **CI hit:** Found `tool_internal_error` but then flooded with handler functions from the same file rather than cross-cutting error patterns

### Q10: "JSON serialization and response formatting"
- **CI top-3:** assembler/formatting.rs:format_section_header, assembler/formatting.rs:format_symbol_with_docstring, assembler/formatting.rs:smart_truncate
- **Augment top-3:** handlers/mod.rs (json! macro response building), web_ui.rs (Json responses), server/mod.rs (serde_json::to_string_pretty)
- **CI miss:** `server/mod.rs` response formatting with `serde_json::to_string_pretty` + `CallToolResult::text_content`, `handlers/mod.rs` `json!()` macro patterns, tool structs with `#[derive(Serialize, Deserialize)]`
- **CI hit:** Found formatting functions but these are context assembly formatters, not JSON serialization or MCP response formatting -- semantic mismatch

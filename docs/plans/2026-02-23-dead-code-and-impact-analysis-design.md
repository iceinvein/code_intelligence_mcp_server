# Dead Code Detection & Change Impact Analysis — Design

**Goal:** Add a `find_dead_code` MCP tool to identify unused symbols, and enhance the existing `find_affected_code` tool with severity scoring and configurable edge types.

**Architecture:** Both features build on existing SQLite edge infrastructure (`count_incoming_edges`, `list_edges_to`, `build_dependency_graph`). Dead code detection is a single SQL query with entry-point exclusion logic. Impact analysis extends the existing handler with severity scoring.

**Tech Stack:** Rust, SQLite

---

## Feature 1: Dead Code Detection

### New MCP Tool: `find_dead_code`

```rust
pub struct FindDeadCodeTool {
    pub file_path: Option<String>,    // Scope to specific file
    pub language: Option<String>,     // Filter by language
    pub kind: Option<String>,         // Filter by kind (function, class, etc.)
    pub include_tests: Option<bool>,  // Include test symbols (default false)
    pub limit: Option<u32>,           // Max results (default 50)
}
```

### Detection Logic

1. SQL query: find all symbols with zero incoming edges (`id NOT IN (SELECT DISTINCT to_symbol_id FROM edges)`)
2. Exclude symbols that are inherently "used" (entry points):
   - `kind = "file"` or `kind = "module"` — always roots
   - Symbols named `main` — program entry points
   - Symbols with `framework_patterns` entries — route handlers, controllers, middleware
   - Test symbols (unless `include_tests = true`)
3. Classify priority: `exported = true` → "high", `exported = false` → "medium"
4. Apply optional filters: `file_path`, `language`, `kind`
5. Group output by file for readability

### Output Format

```json
{
  "dead_symbol_count": 12,
  "dead_files": 5,
  "high_priority_count": 3,
  "medium_priority_count": 9,
  "excluded_entry_points": 4,
  "dead_symbols": [
    {
      "symbol_name": "unused_helper",
      "kind": "function",
      "file_path": "src/utils.rs",
      "line": 42,
      "exported": false,
      "priority": "medium",
      "language": "rust"
    }
  ],
  "display": "..."
}
```

### Entry Point Exclusion

Symbols excluded from dead code reporting:
- `kind IN ("file", "module")` — structural, not callable
- `name = "main"` — program entry points
- Symbols in `framework_patterns` table — HTTP handlers, middleware, controllers, etc.
- Test symbols (configurable)
- `impl` blocks — structural containers

---

## Feature 2: Enhanced `find_affected_code`

### Extended Tool Parameters

```rust
pub struct FindAffectedCodeTool {
    pub symbol_name: String,
    pub file_path: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
    pub include_tests: Option<bool>,
    pub edge_types: Option<Vec<String>>,  // NEW: default ["call", "reference"]
}
```

### Severity Scoring

Each affected symbol gets a `severity` field (1-10):

| Factor | Weight | Logic |
|--------|--------|-------|
| **Depth** | 40% | Direct callers score higher than transitive |
| **Export status** | 30% | Exported affected symbol = API breakage risk |
| **In-degree** | 30% | High in-degree = critical hub, changes cascade further |

Severity formula:
```
depth_score = max(1.0, 10.0 - (depth_from_root * 2.0))
export_score = if exported { 10.0 } else { 4.0 }
indegree_score = min(10.0, (in_degree as f64).ln().max(0.0) * 3.0 + 1.0)
severity = (depth_score * 0.4 + export_score * 0.3 + indegree_score * 0.3).round() as u8
```

Impact categories:
- `"critical"` — severity 8-10
- `"high"` — severity 5-7
- `"medium"` — severity 1-4

### Edge Type Filtering

Currently hardcoded to `call` + `reference` in `build_dependency_graph`. Pass `edge_types` parameter through so callers can include `type`, `extends`, `implements` edges for broader impact analysis.

### Enhanced Output

```json
{
  "symbol_name": "process_data",
  "affected_count": 15,
  "affected_files": 8,
  "severity_breakdown": {
    "critical": 2,
    "high": 5,
    "medium": 8
  },
  "affected": [
    {
      "symbol_name": "handle_request",
      "kind": "function",
      "file_path": "src/api.rs",
      "exported": true,
      "severity": 9,
      "impact": "critical",
      "depth": 1,
      "in_degree": 12
    }
  ],
  "display": "..."
}
```

---

## Testing

- Dead code: unit tests with known unused symbols in test fixtures
- Impact analysis: unit tests verifying severity scoring math + edge type filtering
- Both: integration tests via `scripts/test_local.sh` with dummy workspace

## Out of Scope

- Transitive dead code detection (all callers are themselves dead)
- Multi-file change-set input (compose via multiple tool calls)
- Dead code auto-removal suggestions

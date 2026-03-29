# Architecture Review — Code Intelligence MCP Server

**Date**: 2026-03-29
**Codebase size**: 62,027 lines of Rust across 129 files
**Modules**: 19 top-level modules in `lib.rs`

---

## Architecture Overview

The system is a single-binary Rust server implementing the Model Context Protocol (MCP) for code intelligence. It has two deployment modes (embedded stdio, standalone HTTP) and a clear three-layer architecture:

```
┌─────────────────────────────────────────────────────────┐
│  Transport Layer                                         │
│  server/ (MCP protocol, dispatch)                        │
│  main.rs (wiring, bootstrap)                             │
├─────────────────────────────────────────────────────────┤
│  Application Layer                                       │
│  handlers/ (tool implementations)                        │
│  retrieval/ (search pipeline)                            │
│  indexer/ (indexing pipeline)                             │
│  graph/ (call hierarchy, type graph, dependency graph)   │
├─────────────────────────────────────────────────────────┤
│  Infrastructure Layer                                    │
│  storage/ (sqlite, tantivy, lancedb, cache)              │
│  embeddings/ (Embedder trait + backends)                  │
│  llm/ (LlmGenerator trait + llamacpp)                    │
│  reranker/ (Reranker trait + llamacpp)                    │
│  config/, path/, logging/, metrics/, leader/, registry/  │
└─────────────────────────────────────────────────────────┘
```

Data flows through two pipelines:
- **Write path**: `indexer/` — scan → parse (tree-sitter) → extract symbols → generate embeddings → generate LLM descriptions → store (SQLite + Tantivy + LanceDB)
- **Read path**: `retrieval/` — query normalization → hybrid search (BM25 + vector) → RRF fusion → structural scoring → edge expansion → diversification → reranking → context assembly

---

## Dependency Graph

```
                          ┌────────┐
                          │  main  │ (wiring point, imports everything)
                          └───┬────┘
                              │
              ┌───────────────┼──────────────┐
              ▼               ▼              ▼
         ┌─────────┐   ┌──────────┐   ┌──────────┐
         │ server   │   │ web_ui   │   │ session  │
         └────┬─────┘   └────┬─────┘   └──────────┘
              │              │              │
              ▼              │              ▼
         ┌──────────┐       │         config, embeddings
         │ handlers  │◀─────┘
         └─────┬─────┘
               │
    ┌──────────┼─────────────────┐
    ▼          ▼                 ▼
┌──────┐  ┌───────────┐  ┌──────────┐
│graph │  │ retrieval  │  │ indexer   │
└──┬───┘  └─────┬──────┘  └────┬─────┘
   │            │              │
   │     ┌──────┼──────┐      │
   │     ▼      ▼      ▼      ▼
   │  config  text  embeddings llm
   │     │              │
   ▼     ▼              ▼
┌────────────────────────────────┐
│          storage               │
│  sqlite / tantivy / lancedb    │
└────────────────────────────────┘
        │           │
        ▼           ▼
      path       (leaf)
```

### Dependency Cycles (3 detected)

**1. storage ↔ indexer** (type coupling)
- `storage/sqlite/mod.rs:12` re-exports `crate::indexer::extract::symbol::TodoEntry`
- `storage/sqlite/queries/docstrings.rs:4` imports `crate::indexer::extract::symbol::JSDocEntry`
- `storage/sqlite/queries/todos.rs:4` imports `crate::indexer::extract::symbol::{TodoEntry, TodoKind}`
- Meanwhile, `indexer/` imports heavily from `storage/`

**2. storage ↔ text** (bidirectional logic)
- `storage/tantivy.rs:3` imports `crate::text` (text processing for BM25 indexing)
- `text.rs:1128` imports `crate::storage::sqlite::SymbolRow` (text generation needs symbol data)

**3. indexer → retrieval** (cross-concern)
- `indexer/pipeline/mod.rs:64-67` calls `crate::retrieval::ranking::score::is_test_file` and `is_test_symbol`
- The write path reaches into the read path for scoring utilities

---

## Pattern Inventory

| Pattern | Where | Consistency |
|---------|-------|-------------|
| **Facade** | `AppState` (handlers/state.rs:15) — holds refs to all subsystems | Single usage, clean |
| **Strategy** | `Embedder` trait (embeddings/mod.rs:18), `LlmGenerator` (llm/mod.rs:37), `Reranker` (reranker/mod.rs:13) | Consistent across ML backends |
| **Decorator** | `TruncatingEmbedder` wraps `Embedder` (embeddings/mod.rs:139), `DeferredEmbedder` (embeddings/mod.rs:66) | Well-applied |
| **Pipeline** | Indexer pipeline (indexer/pipeline/), Retrieval pipeline (retrieval/) | Two distinct pipelines, well-decomposed internally |
| **Leader Election** | File-lock based (leader.rs) — multi-instance coordination | Single usage, pragmatic |
| **Connection Pool** | Custom `SqlitePool` (storage/sqlite/pool.rs) with RAII guards | Correct, simple |
| **Deferred Loading** | `DeferredEmbedder` for non-blocking startup with graceful degradation | Elegant pattern |
| **Dispatch Table** | String-match dispatch in `server/mod.rs:173` — 31 match arms | Heavy boilerplate (see Smells) |
| **Extractor** | Per-language files in `indexer/extract/` (29 extractors) | Consistent, easy to extend |

---

## Architectural Health Assessment

| Dimension | Rating | Notes |
|-----------|--------|-------|
| **Dependency direction** | adequate | Generally flows downward (handler → retrieval → storage). Marred by 3 cycles. |
| **Module cohesion** | adequate | Most modules have clear single responsibilities. `handlers/mod.rs` and `text.rs` are exceptions. |
| **Coupling** | adequate | Concrete storage types used everywhere (no storage traits). ML backends have good trait abstraction. |
| **Boundary clarity** | weak-adequate | `pub(super)` and `pub(crate)` used in retrieval (good). Storage leaks indexer types. Handlers glob-import tools. |
| **Pattern consistency** | strong | ML backends consistently use trait + factory. Language extractors follow uniform pattern. |
| **Abstraction quality** | adequate | Trait abstractions earn their keep (Embedder, Reranker, LlmGenerator). No unnecessary abstractions. Missing storage abstraction is intentional simplicity but limits testability. |

---

## Strengths

1. **Retrieval submodule decomposition is excellent.** `hybrid.rs`, `ranking/` (6 files), `assembler/`, `query.rs`, `fast_paths.rs`, `cache.rs`, `framework_patterns.rs`, `postprocess.rs`, `hyde/` — each has a clear responsibility within the search pipeline. This module handles the most complex logic in the system and is well-structured for its complexity.

2. **ML backend abstraction via traits is clean and consistent.** `Embedder`, `Reranker`, `LlmGenerator` — three traits, each with a factory function, each with swappable backends. The `DeferredEmbedder` decorator pattern for non-blocking startup with graceful degradation to BM25-only is particularly elegant.

3. **Language extractor pattern scales well.** 29 extractors in `indexer/extract/`, each in its own file, each isolated. Adding a new language is documented and mechanical. The extractors share utilities via `framework_utils.rs` without tight coupling.

4. **Path module is a model of centralized infrastructure.** `camino`-based UTF-8 typed paths, `PathNormalizer` for all operations, security validation against path escaping, comprehensive parameterized tests. This prevents a whole class of path-handling bugs.

5. **Benchmark discipline is exceptional.** The search quality benchmark system (97+ self-rounds, 1044+ wolfmax rounds) with round-over-round tracking, per-query scoring, and documented operational lessons in CLAUDE.md shows rare engineering discipline. This is the kind of infrastructure most teams never build.

6. **Leader election pattern is pragmatic.** File-lock based leader/follower prevents duplicate indexing across MCP instances. Simple, correct, appropriate for the use case.

7. **Config module handles complexity well.** 60+ config knobs with layered precedence (env → TOML → defaults), structured TOML parsing, sensible defaults. At 1566 lines it's large but the complexity is inherent.

---

## Smell Inventory

### Critical

**S1: `handlers/mod.rs` is a 3911-line god file**
- All 23+ MCP tool handlers live in a single file
- Contains `handle_refresh_index`, `handle_search_code`, `handle_get_definition`, `handle_search_across_repos`, `handle_get_context_bundle`, etc.
- Makes navigation, code review, and parallel work painful
- File: `src/handlers/mod.rs`

**S2: `server/mod.rs` dispatch has 31 identical boilerplate blocks**
- Each match arm: parse args → call handler → `serde_json::to_string_pretty` → `CallToolResult::text_content`
- 31 repetitions of the same 7-line pattern with only the type, handler name, and fallback string varying
- File: `src/server/mod.rs:173-450+`

### Warning

**S3: Dependency cycle — storage ↔ indexer (type coupling)**
- Storage re-exports and imports types from `indexer::extract::symbol` (`TodoEntry`, `JSDocEntry`, `TodoKind`)
- These are domain types that should live in a shared location
- Files: `storage/sqlite/mod.rs:12`, `storage/sqlite/queries/docstrings.rs:4`, `storage/sqlite/queries/todos.rs:4`

**S4: Dependency cycle — storage ↔ text**
- Tantivy indexing calls text processing functions
- Text processing needs `SymbolRow` for description generation
- Files: `storage/tantivy.rs:3`, `text.rs:1128`

**S5: Indexer depends on retrieval scoring utilities**
- `indexer/pipeline/mod.rs:64-67` calls `crate::retrieval::ranking::score::is_test_file/is_test_symbol`
- The write path shouldn't reach into the read path

**S6: `text.rs` is a 1509-line utility bag**
- Mixes: string processing, stemming, NL description generation, concept tag extraction, morphological variants, synonym expansion, comment stripping
- No submodule structure despite handling 6+ distinct concerns

**S7: No storage trait abstraction**
- `SqliteStore`, `TantivyIndex`, `LanceVectorTable` used as concrete types throughout
- Cannot swap storage implementations for testing without real backends
- `EMBEDDINGS_BACKEND=hash` exists for embeddings but no equivalent for storage

### Info

**S8: `main.rs` has duplicated bootstrap logic (860 lines)**
- `run_embedded()` and `run_standalone()` duplicate embedder creation, config parsing, and service wiring
- Could share a builder/factory pattern

**S9: `SqliteStore` uses `unsafe impl Send + Sync`**
- `RwLock<Connection>` is correct but the safety comment acknowledges it's working around rusqlite's `!Sync` constraint
- `SqlitePool` exists alongside `SqliteStore` — two different concurrency strategies for the same DB

**S10: Config has 60+ env vars parsed individually**
- Each env var is parsed with its own `env::var()` + parsing + default block
- Could use a derive macro or structured parsing (e.g., `envy` crate)

---

## Priority Ordering

| Priority | Smell | Impact | Effort | Rationale |
|----------|-------|--------|--------|-----------|
| **High** | S1 (handlers god file) | High — blocks parallel work, makes review painful | Medium — mechanical split into per-domain files | Biggest win per effort. Every code change touches this file. |
| **High** | S2 (dispatch boilerplate) | Medium — 31 copy-paste blocks | Low — macro or generic dispatch helper | Quick win, eliminates a class of copy-paste bugs |
| **Medium** | S3 (storage↔indexer cycle) | Medium — blocks clean module extraction | Low — move `TodoEntry`, `JSDocEntry`, `TodoKind` to a shared `types` module | Type-only cycle, clean fix |
| **Medium** | S5 (indexer→retrieval) | Low-Medium — cross-concern coupling | Low — move `is_test_file`/`is_test_symbol` to shared utility | 2 function moves |
| **Medium** | S6 (text.rs bag) | Medium — hard to navigate, mixed concerns | Medium — split into submodules | Requires deciding new boundaries |
| **Low** | S4 (storage↔text) | Low — tightly coupled by nature (BM25 indexing needs text processing) | Medium — would need a callback/trait pattern | May not be worth fixing — the coupling is inherent to BM25 indexing |
| **Low** | S8 (main.rs duplication) | Low — only changes on new features | Medium — builder pattern | Bootstrap code changes rarely |
| **Low** | S7 (no storage traits) | Low — `EMBEDDINGS_BACKEND=hash` covers most testing needs | High — would need traits + test doubles for 3 storage backends | Over-engineering risk; current approach works |

---

## Prescriptions

### P1: Split `handlers/mod.rs` into per-domain files

Move handler functions into submodules grouped by domain:

```
handlers/
├── mod.rs          (AppState, parse_tool_args, tool_internal_error, extract_usage_line)
├── state.rs        (AppState struct — already exists)
├── search.rs       (handle_search_code, handle_explain_search, handle_find_similar_code)
├── navigation.rs   (handle_get_definition, handle_find_references, handle_get_call_hierarchy, ...)
├── index.rs        (handle_refresh_index, handle_get_index_stats)
├── graph.rs        (handle_explore_dependency_graph, handle_get_type_graph, handle_trace_data_flow, ...)
├── analysis.rs     (handle_find_dead_code, handle_find_duplicates, handle_predict_impact, ...)
├── cross_repo.rs   (handle_search_across_repos, handle_explore_cross_repo_deps)
└── learning.rs     (handle_report_selection, handle_report_file_access)
```

Each submodule imports `AppState` from `state.rs` and the relevant tool types from `tools/`. No new public API — just internal reorganization.

**Pattern**: Module decomposition. **Risk**: Low — mechanical refactor. **Impact**: ~3911 lines split into ~8 files of ~400-500 lines each.

### P2: Eliminate dispatch boilerplate with a macro

Replace the 31 identical match arms in `server/mod.rs` with a declarative macro:

```rust
macro_rules! dispatch_tool {
    ($state:expr, $params:expr, sync $tool:ty => $handler:expr) => {{
        let tool: $tool = parse_tool_args(&$params)?;
        let result = $handler($state, tool).map_err(tool_internal_error)?;
        Ok(CallToolResult::text_content(vec![
            serde_json::to_string_pretty(&result)
                .unwrap_or_else(|_| "{\"ok\":true}".to_string()).into(),
        ]))
    }};
    ($state:expr, $params:expr, async $tool:ty => $handler:expr) => {{
        let tool: $tool = parse_tool_args(&$params)?;
        let result = $handler($state, tool).await.map_err(tool_internal_error)?;
        Ok(CallToolResult::text_content(vec![
            serde_json::to_string_pretty(&result)
                .unwrap_or_else(|_| "{\"ok\":true}".to_string()).into(),
        ]))
    }};
}

// Usage becomes:
match params.name.as_str() {
    "search_code" => dispatch_tool!(state, params, async SearchCodeTool => |s, t| handle_search_code(&s.retriever, t)),
    "get_definition" => dispatch_tool!(state, params, async GetDefinitionTool => handle_get_definition),
    // ... one line per tool
}
```

**Pattern**: Template Method (via macro). **Risk**: Low. **Impact**: ~450 lines → ~60 lines.

### P3: Extract shared types to break storage↔indexer cycle

Move `TodoEntry`, `TodoKind`, `JSDocEntry` from `indexer/extract/symbol.rs` to a new `types` module (or into `storage/sqlite/schema.rs` where `SymbolRow` already lives):

```
// Before: storage/sqlite/mod.rs
pub use crate::indexer::extract::symbol::TodoEntry;  // cycle!

// After: shared types in storage/sqlite/schema.rs or a new types.rs
pub struct TodoEntry { ... }  // moved here
```

**Pattern**: Dependency inversion (types owned by the consumer). **Risk**: Low. **Impact**: Eliminates 1 cycle.

### P4: Move test-detection utilities to shared module

Move `is_test_file` and `is_test_symbol` from `retrieval/ranking/score.rs` to `path/` or a new `classify` utility module. Both the indexer (write path) and retrieval (read path) need these functions.

**Pattern**: Extract shared utility. **Risk**: Low — 2 functions. **Impact**: Eliminates indexer→retrieval cross-concern.

---

## Summary

This is a **well-engineered codebase** for its complexity. The retrieval pipeline decomposition, ML backend trait abstractions, and benchmark infrastructure are standout strengths. The main structural issues are concentrated in two files (`handlers/mod.rs` and `server/mod.rs` dispatch) and three dependency cycles that have clean, low-risk fixes.

The architecture follows a sensible layered approach with dependencies mostly flowing downward. The 19 modules map well to their responsibilities, with the exceptions noted above. The codebase is particularly strong in areas that have been iterated on extensively (retrieval ranking, embeddings, path handling) and weaker in areas that have grown organically (handlers, dispatch).

None of the identified issues are blocking — they are maintenance and developer-experience concerns that become more important as the codebase grows.

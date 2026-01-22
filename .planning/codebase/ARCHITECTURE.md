# Architecture

**Analysis Date:** 2026-01-22

## Pattern Overview

**Overall:** Multi-stage indexing and retrieval pipeline with hybrid search (keyword + semantic)

**Key Characteristics:**
- **Async-first**: Built on Tokio for concurrent file processing and network operations
- **Multi-backend storage**: SQLite (metadata), Tantivy (full-text search), LanceDB (vector embeddings)
- **Language-agnostic parsing**: Tree-Sitter based symbol extraction across 9 languages
- **Intent-driven ranking**: Query understanding influences result scoring and context assembly
- **Graph-based context**: Call hierarchies and dependency graphs augment search results

## Layers

**Indexing Pipeline (`src/indexer/`):**
- Purpose: Discover, parse, extract, and persist code structure and relationships
- Location: `src/indexer/`
- Contains: File scanning, Tree-Sitter parsing, symbol extraction, edge extraction, batch embeddings
- Depends on: Tree-Sitter, SQLite, Tantivy, LanceDB, embeddings backend
- Used by: Main entry point, watch mode loop

**Retrieval Engine (`src/retrieval/`):**
- Purpose: Search codebase and assemble contextual results for LLM consumption
- Location: `src/retrieval/`
- Contains: Query normalization, hybrid search, ranking, context assembly, graph expansion
- Depends on: SQLite, Tantivy, LanceDB, embeddings backend
- Used by: MCP tool handlers

**Storage Abstraction (`src/storage/`):**
- Purpose: Unified interface to multi-backend persistence
- Location: `src/storage/`
- Contains: SQLite (symbols, edges, metadata), Tantivy (BM25 index), LanceDB (vector DB)
- Depends on: rusqlite, tantivy, lancedb, arrow
- Used by: All indexing and retrieval code

**MCP Server (`src/server/`):**
- Purpose: Protocol handler for Model Context Protocol (MCP) requests
- Location: `src/server/`
- Contains: Tool listing, request dispatching, error mapping
- Depends on: rust-mcp-sdk
- Used by: Stdio transport, client connections

**Tool Handlers (`src/handlers/`):**
- Purpose: Implement MCP tool business logic
- Location: `src/handlers/`
- Contains: 11 tools for search, indexing, definitions, graphs, usage, stats
- Depends on: Indexer, Retriever, SQLite Store
- Used by: MCP server dispatcher

**Graph Engine (`src/graph/`):**
- Purpose: Build and traverse call hierarchies, type graphs, dependency graphs
- Location: `src/graph/`
- Contains: Depth-limited graph traversal, edge resolution tracking, evidence collection
- Depends on: SQLite Store
- Used by: get_call_hierarchy, explore_dependency_graph, get_type_graph tools

**Embeddings (`src/embeddings/`):**
- Purpose: Abstract embedding backend (real or mock)
- Location: `src/embeddings/`
- Contains: FastEmbed (BGE-base-en-v1.5 ONNX), Hash (deterministic for testing)
- Depends on: fastembed, or no external deps for hash
- Used by: Indexer and Retriever for vector operations

## Data Flow

**Indexing Flow:**

1. **File Scan** (`src/indexer/pipeline/scan.rs`)
   - Enumerate files matching `INDEX_PATTERNS` from `repo_roots`
   - Filter: ignore `node_modules`, hidden files, non-indexed extensions
   - Compare fingerprints (mtime_ns, size_bytes) against `file_fingerprints` table

2. **Parsing** (`src/indexer/pipeline/parsing.rs`)
   - Detect language by extension via `language_id_for_path()`
   - Create Tree-Sitter parser for language
   - Parse source code into CST

3. **Symbol Extraction** (`src/indexer/extract/{lang}.rs`)
   - Language-specific traversal of CST
   - Extract function, class, type, variable definitions
   - Capture: name, kind, file_path, start_line, end_line, is_exported
   - Return: `SymbolRow` structs for each symbol

4. **Edge Extraction** (`src/indexer/pipeline/edges.rs`)
   - For each symbol, scan its definition and context
   - Identify calls, references, type relationships
   - Emit `EdgeRow`: from_symbol_id → to_symbol_id, edge_type, at_file, at_line

5. **Embedding & Storage** (`src/indexer/pipeline/mod.rs`)
   - Batch extract symbol definitions from files
   - Call embedder with symbol name + definition text
   - Upsert to SQLite `symbols` table
   - Upsert to Tantivy full-text index
   - Upsert to LanceDB vector table
   - Update `file_fingerprints` to skip re-indexing

6. **Watch Loop** (optional)
   - If `WATCH_MODE=true`, spawn background task
   - Every `watch_debounce_ms` (default 100ms), re-index all files
   - Fingerprints prevent unnecessary re-parsing of unchanged files

**Retrieval Flow:**

1. **Query Normalization** (`src/retrieval/query.rs`)
   - Expand abbreviations (db→database, auth→authentication)
   - Apply stemming to multi-character words
   - Result: enriched query text

2. **Intent Detection** (`src/retrieval/query.rs`)
   - Pattern match: "test"/"spec" → Intent::Test
   - Pattern match: "schema"/"model"/"db" → Intent::Schema
   - Pattern match: "class"/"struct"/"type" → Intent::Definition
   - Pattern match: "who calls X" → Intent::Callers(X)

3. **Hybrid Search** (`src/retrieval/mod.rs`)
   - **Keyword path**: Tantivy BM25 search on normalized query (K results)
   - **Vector path**: Generate embedding for query, LanceDB similarity search (K results)
   - **Blend**: Combine using `hybrid_alpha` weight: `score = alpha * keyword + (1-alpha) * vector`

4. **Ranking** (`src/retrieval/ranking/score.rs`)
   - Apply structural adjustments:
     - Test file penalty (0.5x) unless Intent::Test
     - Index file penalty (index.ts deprioritized)
     - Directory semantics (src/ boosted, dist/ penalized)
     - Export status boost (`rank_exported_boost`)
   - Apply intent multipliers:
     - Definition: 1.5x boost
     - Schema: 50-75x boost (high signal)
     - Test: 2.0x boost
   - Popularity boost by incoming edge count
   - Final: `score = base_score * intent_mult * (1 + popularity_boost)`

5. **Diversification** (`src/retrieval/ranking/diversify.rs`)
   - Remove near-duplicates by similarity cluster
   - Ensure variety by symbol kind

6. **Context Assembly** (`src/retrieval/assembler/mod.rs`)
   - For each ranked hit, load full symbol definition from file
   - Expand with related symbols via graph traversal (50 candidates)
   - Format output:
     - Root symbols (search results): full source code (70% budget)
     - Extra symbols (from graph): simplified code (30% budget)
   - Cap total output at `max_context_bytes` (default 200KB)

## State Management

**Shared State** (`src/handlers/state.rs`):
```rust
pub struct AppState {
    pub config: Arc<Config>,
    pub indexer: IndexPipeline,
    pub retriever: Retriever,
}
```

- Passed to all MCP tool handlers
- Embedder locked behind `Arc<Mutex<Box<dyn Embedder>>>` (async safe)
- Storage instances created on-demand (SQLite opens connection pools, LanceDB/Tantivy are opened once)

**Caching** (`src/retrieval/cache.rs`):
- Retriever maintains LRU cache of search results
- Cache key includes config hash, max_context_bytes, ranking weights
- Avoids redundant re-ranking for identical queries

## Key Abstractions

**Embedder Trait** (`src/embeddings/mod.rs`):
- Purpose: Abstract embedding generation
- Implementations: `FastEmbedder` (real), `HashEmbedder` (deterministic)
- Pattern: Async trait, batch API, configurable dimensions

**SymbolRow** (`src/storage/sqlite/schema.rs`):
- Represents a code symbol (function, class, var, etc.)
- Fields: id, name, kind, file_path, start_line, end_line, language, exported, definition_text
- Uniqueness: `(file_path, start_line, end_line)` tuple identifies a symbol

**EdgeRow** (`src/storage/sqlite/schema.rs`):
- Represents a relationship: from_symbol → to_symbol
- Types: "call", "reference", "type_extends", "type_implements", "type_alias"
- Includes: evidence_count (how many sites call/reference), resolution (automatic or manual)

**VectorRecord** (`src/storage/vector.rs`):
- Arrow-compatible record for LanceDB
- Fields: id, name, definition_text, language, kind, embedding vector

## Entry Points

**Main Server** (`src/main.rs`):
- Location: `src/main.rs`
- Triggers: `cargo run` or `./target/release/code-intelligence-mcp-server`
- Responsibilities:
  1. Parse environment config (BASE_DIR, embeddings backend, ranking weights)
  2. Initialize storage (SQLite, Tantivy, LanceDB)
  3. Initialize embedder (FastEmbed or Hash)
  4. Create IndexPipeline and Retriever
  5. Optionally spawn watch loop
  6. Create MCP server with stdio transport
  7. Block on `server.start()`

**Watch Loop** (`src/indexer/pipeline/mod.rs:spawn_watch_loop`):
- Spawned as Tokio task if `watch_mode = true`
- Runs `index_all()` every `watch_debounce_ms`
- Non-blocking background task

**MCP Tool Dispatch** (`src/server/mod.rs:handle_call_tool_request`):
- Location: `src/server/mod.rs`
- Triggers: Client calls a tool
- Routes to handlers:
  - "search_code" → `handle_search_code()`
  - "refresh_index" → `handle_refresh_index()`
  - "get_definition" → `handle_get_definition()`
  - ... (9 more tools)

## Error Handling

**Strategy:** Rust `Result<T>` with `anyhow::Error` for internal code, MCP `CallToolError` for tool responses

**Patterns:**

- **Indexer errors**: File not found, parse failure, embedding generation failure → logged, indexing skips that file
- **Retrieval errors**: Query too long, embedding gen failure → propagated to tool handler
- **Storage errors**: SQLite connection issues → propagated with context
- **Tool handler errors**: Any `anyhow::Error` converted to `CallToolError` with message

**Recovery:**

- File fingerprints allow re-trying unchanged files
- Watch loop swallows errors and logs warnings (doesn't crash)
- Embedder backend switchable (FastEmbed → Hash fallback if model unavailable)

## Cross-Cutting Concerns

**Logging:**
- Framework: `tracing` with `tracing-subscriber`
- Configured via `RUST_LOG` env var
- Default: `info` level
- Output: stderr (for MCP compatibility with stdout)

**Validation:**
- Path canonicalization in `Config::from_env()`
- Query length trimmed, embedding batch size limits
- Symbol name required for tools, file paths normalized

**Authentication:**
- None (local-only MCP server)
- Environment variables for embeddings API keys/tokens (HuggingFace HF_TOKEN optional)

**Path Handling:**
- All paths canonicalized and stored as absolute
- Relative paths normalized to base_dir
- File paths stored relative to BASE_DIR for portability

---

*Architecture analysis: 2026-01-22*

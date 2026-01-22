# Technology Stack

**Analysis Date:** 2026-01-22

## Languages

**Primary:**
- Rust 2021 (Edition 2021) - Core server implementation, all indexing, retrieval, and storage logic

## Runtime

**Environment:**
- Linux, macOS, Windows (native Rust compilation)

**Architecture:**
- Async runtime: Tokio 1.x with full feature set
- Transport: stdio-based MCP (Model Context Protocol) via `rust-mcp-sdk`

## Frameworks

**Core Framework:**
- `rust-mcp-sdk` 0.8.1 - MCP server implementation with macros, server, and stdio transport features

**Protocol:**
- MCP (Model Context Protocol) v2025-11-25 - Server advertises tools to clients (OpenCode, Trae, Cursor)

**Optional Web UI:**
- `axum` 0.8 - Web framework (optional, feature-gated as `web-ui`)
- `tower-http` 0.6 - HTTP middleware including CORS support

## Key Dependencies

**Parsing & Symbol Extraction:**
- `tree-sitter` 0.25.0 - Generic parser engine
- `tree-sitter-rust` 0.23 - Rust grammar
- `tree-sitter-typescript` 0.23.2 - TypeScript/TSX grammar
- `tree-sitter-javascript` 0.25.0 - JavaScript grammar
- `tree-sitter-python` 0.25.0 - Python grammar
- `tree-sitter-go` 0.25.0 - Go grammar
- `tree-sitter-java` 0.23.5 - Java grammar
- `tree-sitter-c` 0.24.1 - C grammar
- `tree-sitter-cpp` 0.23.4 - C++ grammar

**Embeddings:**
- `fastembed` 4 - BGE-base-en-v1.5 embedding model (local, CPU/Metal GPU)
  - Default model: `BAAI/bge-base-en-v1.5`
  - Alternative models supported: `BAAI/bge-small-en-v1.5`, `sentence-transformers/all-MiniLM-L6-v2`, `jinaai/jina-embeddings-v2-base-en`

**Storage Layers:**
- `rusqlite` 0.33 - SQLite driver (bundled SQLite C library)
  - Stores: Symbols, file metadata, edges (call graph), index telemetry
- `tantivy` 0.25.0 - Full-text search engine (BM25)
  - Schema: Symbol name, file path, kind, exported status, n-gram tokenization
- `lancedb` 0.23.1 - Vector database for semantic search
  - Format: Arrow-based columnar storage
- `arrow-array` 56, `arrow-schema` 56 - Apache Arrow data structures for LanceDB integration

**Async & Concurrency:**
- `tokio` 1.x (full features) - Async runtime
- `async-trait` 0.1 - Async trait support
- `futures` 0.3 - Async utilities

**Data Serialization:**
- `serde` 1.0 - Serialization framework
- `serde_json` 1.0 - JSON serialization
- `url` 2.x - URL parsing and serialization

**Logging & Diagnostics:**
- `tracing` 0.1 - Structured logging
- `tracing-subscriber` 0.3 - Log filtering and output (env-filter enabled)

**Error Handling:**
- `anyhow` 1.0 - Flexible error handling

## Configuration

**Environment Variables:**

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `BASE_DIR` | Required | - | Repository root to index |
| `EMBEDDINGS_BACKEND` | String | `fastembed` | `fastembed` or `hash` (for testing) |
| `EMBEDDINGS_DEVICE` | String | `cpu` | `cpu` or `metal` (macOS GPU acceleration) |
| `EMBEDDINGS_MODEL_REPO` | String | `BAAI/bge-base-en-v1.5` | HuggingFace model identifier |
| `EMBEDDINGS_MODEL_DIR` | Path | `.cimcp/embeddings-cache` | Model cache directory |
| `EMBEDDINGS_AUTO_DOWNLOAD` | Boolean | `false` | Auto-download model on startup |
| `EMBEDDING_BATCH_SIZE` | Integer | `32` | Batch size for embedding generation |
| `HASH_EMBEDDING_DIM` | Integer | `64` | Dimension for hash-based embeddings |
| `WATCH_MODE` | Boolean | `true` | Enable file change monitoring |
| `WATCH_DEBOUNCE_MS` | Integer | `250` | Debounce interval for file changes |
| `INDEX_PATTERNS` | CSV | `**/*.ts,**/*.tsx,**/*.rs` | Glob patterns to index |
| `EXCLUDE_PATTERNS` | CSV | `**/node_modules/**,**/dist/**,**/build/**,**/.git/**,**/*.test.*` | Patterns to exclude |
| `DB_PATH` | Path | `.cimcp/code-intelligence.db` | SQLite database location |
| `VECTOR_DB_PATH` | Path | `.cimcp/vectors` | LanceDB vector store location |
| `TANTIVY_INDEX_PATH` | Path | `.cimcp/tantivy-index` | Tantivy index location |
| `VECTOR_SEARCH_LIMIT` | Integer | `20` | Max vector search results |
| `HYBRID_ALPHA` | Float (0-1) | `0.7` | Vector weight in hybrid search (keyword = 1-alpha) |
| `RANK_VECTOR_WEIGHT` | Float | `hybrid_alpha` | Vector scoring weight |
| `RANK_KEYWORD_WEIGHT` | Float | `1-hybrid_alpha` | Keyword scoring weight |
| `RANK_EXPORTED_BOOST` | Float | `0.1` | Boost for exported symbols |
| `RANK_INDEX_FILE_BOOST` | Float | `0.05` | Boost for index.ts-like files |
| `RANK_TEST_PENALTY` | Float | `0.1` | Penalty for test files (unless Intent::Test) |
| `RANK_POPULARITY_WEIGHT` | Float | `0.05` | Weight for graph popularity |
| `RANK_POPULARITY_CAP` | Integer | `50` | Cap on edge count for popularity |
| `MAX_CONTEXT_BYTES` | Integer | `200000` | Context window limit for results |
| `INDEX_NODE_MODULES` | Boolean | `false` | Include node_modules in indexing |
| `REPO_ROOTS` | CSV | `BASE_DIR` | Additional repository roots for multi-repo indexing |

**Build Configuration:**
- `Cargo.toml` - Feature flags: `web-ui` (enables Axum + Tower-HTTP)

## Platform Requirements

**Development:**
- Rust toolchain (stable)
- Protobuf compiler (for MCP SDK)
- macOS: Xcode (for Metal GPU acceleration)
- Linux: Standard build tools (gcc/clang)

**Runtime:**
- Memory: ~800MB-2GB depending on codebase size (embeddings model + indexes)
- Storage: Variable (SQLite + LanceDB + Tantivy indexes, typically 50-500MB for medium-large codebases)
- GPU (Optional): Metal on macOS, no CUDA/NVIDIA support currently

**Supported Platforms:**
- macOS (x86-64, ARM64 with Metal acceleration)
- Linux (x86-64)
- Windows (x86-64, MSVC toolchain)

## Data Location

All indexes are stored locally under `BASE_DIR`:
```
BASE_DIR/
├── .cimcp/
│   ├── code-intelligence.db       # SQLite metadata & edges
│   ├── vectors/                   # LanceDB vector store
│   ├── tantivy-index/             # Tantivy BM25 index
│   └── embeddings-cache/          # FastEmbed model cache
```

## Package Manager

**Rust Dependency Management:**
- Cargo with `Cargo.lock` for reproducible builds
- Dependencies published on crates.io
- Pre-built binaries distributed via npm: `@iceinvein/code-intelligence-mcp`

---

*Stack analysis: 2026-01-22*

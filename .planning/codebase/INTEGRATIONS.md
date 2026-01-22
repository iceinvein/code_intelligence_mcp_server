# External Integrations

**Analysis Date:** 2026-01-22

## APIs & External Services

**Model Context Protocol (MCP):**
- Service: MCP v2025-11-25 specification
  - SDK: `rust-mcp-sdk` 0.8.1
  - Transport: stdio-based bidirectional JSON-RPC
  - Clients: OpenCode, Trae, Cursor, other MCP-compatible tools
  - Auth: None (local process communication)
  - Capabilities: Tool listing, tool invocation, no resources/prompts

**Hugging Face Model Hub:**
- Service: Model repository for embeddings (only during initial setup)
  - Models:
    - `BAAI/bge-base-en-v1.5` (default, 300MB)
    - `BAAI/bge-small-en-v1.5`
    - `sentence-transformers/all-MiniLM-L6-v2`
    - `jinaai/jina-embeddings-v2-base-en`
  - Download: Automatic via `fastembed` crate on first run
  - Cache: Stored locally in `EMBEDDINGS_MODEL_DIR`
  - Auth: Optional `EMBEDDINGS_MODEL_HF_TOKEN` (not currently used by FastEmbed in the codebase)

## Data Storage

**Databases:**
- SQLite (Local)
  - Client: `rusqlite` 0.33 (bundled C library)
  - Location: `DB_PATH` (default `.cimcp/code-intelligence.db`)
  - Schema: symbols, files, edges, similarity_clusters, index_runs
  - Usage: Metadata storage, symbol definitions, call graphs, file tracking
  - Connection: Single instance, thread-safe via `rusqlite::Connection`

**Full-Text Search:**
- Tantivy (Local, in-process)
  - Version: 0.25.0
  - Location: `TANTIVY_INDEX_PATH` (default `.cimcp/tantivy-index`)
  - Index: BM25 ranking with custom n-gram tokenization
  - Fields: id, name, name_ngram, file_path, kind, exported, text, text_ngram
  - Schema version: "4" (stored in index metadata)
  - Writer: Mutex-protected `IndexWriter` for thread-safe concurrent updates

**Vector Database:**
- LanceDB (Local, Arrow-based columnar storage)
  - Version: 0.23.1
  - Location: `VECTOR_DB_PATH` (default `.cimcp/vectors`)
  - Connection: Async connector via `lancedb::connect()`
  - Table: `symbols` (vector embeddings + metadata)
  - Schema: id (string), vector (fixed-size list of f32), name, kind, file_path, exported, language, text
  - Vector Dimension: Model-dependent (BGE-Base: 768 dimensions)
  - Search: Top-K ANN queries via `ExecutableQuery`

**File Storage:**
- Local filesystem only
  - No cloud storage integration
  - File watching: Standard `fs::read_dir()` with manual polling (not `notify` crate)
  - Watch interval: `WATCH_DEBOUNCE_MS` (default 250ms)

**Caching:**
- In-memory embeddings: None (computed on-demand via FastEmbed)
- Model cache: Persistent disk cache in `EMBEDDINGS_MODEL_DIR`
- Results cache: None (retrieval is stateless)

## Authentication & Identity

**Auth Provider:**
- None required
- Local process communication only via stdin/stdout (MCP stdio transport)
- No user credentials, API keys, or authentication tokens in the server
- Optional: Hugging Face token (`EMBEDDINGS_MODEL_HF_TOKEN`) for private models (not implemented)

## Monitoring & Observability

**Error Tracking:**
- None (no external service)
- Errors logged via `tracing` to stderr

**Logs:**
- Structured logging via `tracing` 0.1
- Output: stderr
- Filter: `RUST_LOG` environment variable (defaults to "info")
- Topics: Config loading, indexing progress, search queries, errors

**Metrics:**
- Index run statistics stored in SQLite `index_runs` table
  - Fields: timestamp, duration_ms, files_indexed, symbols_indexed, errors
- No external metrics backend (Prometheus, DataDog, etc.)

## CI/CD & Deployment

**Hosting:**
- Local (embedded in client applications or standalone process)
- Docker: Not officially provided (would need Dockerfile + image build)
- Binaries: Pre-built via GitHub Actions (likely, check CI config)
- Distribution: npm package `@iceinvein/code-intelligence-mcp`

**CI Pipeline:**
- Platform: Not detected in codebase
- Tests: `cargo test` + integration tests in `tests/integration_index_search.rs`
- Coverage: Test workspace in `test_workspace/` for e2e validation
- Build script: `scripts/test_local.sh` for manual testing

## Environment Configuration

**Required env vars:**
- `BASE_DIR` - Repository root (no default, fails if missing)

**Optional env vars:**
- All others have sensible defaults (see STACK.md for full list)

**Secrets location:**
- No secrets stored in codebase
- Model tokens (if needed): Pass via `EMBEDDINGS_MODEL_HF_TOKEN` env var
- Database: SQLite file (local, no password protection)

## Webhooks & Callbacks

**Incoming:**
- None (purely tool-based RPC via MCP)

**Outgoing:**
- None (no external webhooks or callbacks)
- File change monitoring: Local polling only, no external event propagation

## Command-Line Interface

**Startup:**
- Binary: `code-intelligence-mcp-server` (Rust binary)
- Transport: Receives MCP requests on stdin, writes responses to stdout
- Start script: `scripts/start_mcp.sh` (sets up env vars, executes binary)

**Arguments:**
- `--help` - Print help text
- `--version` - Print version string
- No other CLI arguments (configuration via environment variables only)

## Model Integration

**Embedding Model:**
- FastEmbed BGE-Base-en-v1.5 (default, 768-dim vectors)
- Download: First run triggers download (~300MB) to `EMBEDDINGS_MODEL_DIR`
- Inference: Batched via `TextEmbedding::embed()` with configurable batch size
- Device: CPU or Metal GPU (macOS only)
- Fallback: Hash-based embeddings for testing (simple deterministic hashing, 64-dim by default)

## Format & Protocol Specifications

**MCP Protocol:**
- Version: 2025-11-25
- Transport: stdio (JSON-RPC 2.0)
- Server capabilities: Tools (list + invocation)
- No resources, prompts, or sampling support

**Index Serialization:**
- SQLite: Standard SQL tables
- Tantivy: Custom binary format (schema version 4)
- LanceDB: Apache Arrow format (IPC/column storage)
- JSON: serde_json for tool arguments/results

---

*Integration audit: 2026-01-22*

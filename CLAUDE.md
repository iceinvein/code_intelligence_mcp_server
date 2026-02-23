# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Code Intelligence MCP Server is a Rust-based local code indexing and semantic search engine that provides structure-aware code navigation for LLM agents. It implements the Model Context Protocol (MCP) and integrates with tools like OpenCode, Trae, and Cursor.

**Platform:** macOS only (Apple Silicon with Metal GPU acceleration).

**Core technologies:** Rust 2021, Tree-Sitter (parsing), SQLite (metadata), Tantivy (full-text search), LanceDB (vector embeddings), llama.cpp (Metal GPU inference for both embeddings and LLM descriptions).

## Glossary

Key acronyms and concepts used throughout the codebase:

| Term | Full Name | Description |
|------|-----------|-------------|
| **MCP** | Model Context Protocol | Open protocol for connecting LLM agents to external tools and data sources. This server implements MCP to expose code search/navigation tools. |
| **BM25** | Best Matching 25 | Probabilistic text retrieval algorithm used by Tantivy. Ranks documents by term frequency (TF) and inverse document frequency (IDF) — how often a term appears in a document vs. how rare it is across the corpus. |
| **IDF** | Inverse Document Frequency | A BM25 component measuring term rarity. High IDF = rare term = more discriminating. Low IDF = common term (e.g., "error") = less useful for ranking. IDF dilution is a recurring concern when adding synonyms or descriptions. |
| **RRF** | Reciprocal Rank Fusion | Technique for combining ranked lists from different search systems (BM25 keyword search + vector semantic search). Merges by reciprocal rank position rather than raw scores, making it robust across different scoring scales. |
| **GGUF** | GGML Unified Format | Binary format for quantized LLM model weights. Used by llama.cpp for both the embedding model (jina-code-0.5b) and the description LLM (Qwen2.5-Coder-1.5B). Q4_K_M quantization balances quality and speed. |
| **LLM** | Large Language Model | Used on-device (Qwen2.5-Coder-1.5B via llama.cpp) to generate natural-language descriptions for each indexed symbol, enriching BM25 search with human-readable terms. |

## Usage in Claude Code

### Embedded Mode (Default)

Add to `~/.claude.json` (or project-level `.mcp.json`):

```json
{
  "mcpServers": {
    "code-intelligence": {
      "command": "npx",
      "args": ["-y", "@iceinvein/code-intelligence-mcp"],
      "env": {}
    }
  }
}
```

Each Claude Code session spawns its own server process. The server auto-detects the working directory as `BASE_DIR` and begins indexing in the background. The embedding model (~531MB) and LLM (~1.1GB) are downloaded on first launch and cached in `~/.code-intelligence/models/`.

### Standalone Mode (Recommended for Multiple Sessions)

If you run multiple Claude Code sessions simultaneously, standalone mode avoids loading duplicate copies of the embedding model:

1. Start the standalone server (once):
   ```bash
   npx @iceinvein/code-intelligence-mcp-standalone
   ```

2. Configure Claude Code to connect (`~/.claude.json`):
   ```json
   {
     "mcpServers": {
       "code-intelligence": {
         "type": "streamable-http",
         "url": "http://localhost:3333/mcp"
       }
     }
   }
   ```

The standalone server auto-detects each session's workspace root via the MCP `roots` capability — no `BASE_DIR` needed.

### What Claude Code Gets

Once connected, Claude Code gains access to 23 MCP tools including:

- **`search_code`** — Primary semantic + keyword hybrid search (e.g., "how does auth work?" or "class User")
- **`get_definition`** / **`find_references`** — Jump to definitions and find all usages
- **`get_call_hierarchy`** / **`get_type_graph`** — Navigate call chains and type hierarchies
- **`explore_dependency_graph`** — Trace module-level imports/exports
- **`find_affected_code`** — Impact analysis before refactoring
- **`trace_data_flow`** — Follow variable reads/writes through the code
- **`get_index_stats`** / **`refresh_index`** — Monitor and trigger re-indexing

## Build & Run Commands

```bash
# Build
cargo build                    # Debug build
cargo build --release          # Release build

# Run tests
cargo test                                      # All tests
cargo test --test integration_index_search      # Integration tests only
./scripts/test_local.sh                         # End-to-end test with dummy workspace

# Run the server
./scripts/start_mcp.sh                          # Start MCP server (stdio transport)
BASE_DIR=/path/to/repo ./target/release/code-intelligence-mcp-server

# For faster testing (skip model download)
EMBEDDINGS_BACKEND=hash cargo test
```

## Standalone Server Mode

The server can run as a long-lived HTTP daemon serving multiple repos via Streamable HTTP transport. This is ideal when running multiple MCP clients (e.g. 5-6 Claude Code instances) — the embedding model (~531MB) and LLM (~1.1GB) are loaded once and shared. The LLM is automatically freed after descriptions are generated; the embedding model stays resident for queries. In stdio mode, leader election ensures only one instance per repo performs indexing/descriptions; followers never load the LLM.

```bash
# Start standalone server (default: localhost:3333)
./target/release/code-intelligence-mcp-server --standalone

# Custom port/host
./target/release/code-intelligence-mcp-server --standalone --port 4444 --host 0.0.0.0

# Via npx
npx @iceinvein/code-intelligence-mcp-standalone

# Via env var
CIMCP_MODE=standalone ./target/release/code-intelligence-mcp-server
```

Configure MCP clients to connect:
```json
{
  "mcpServers": {
    "code-intelligence": {
      "type": "streamable-http",
      "url": "http://localhost:3333/mcp"
    }
  }
}
```

Data stored in `~/.code-intelligence/` (repos, models, config).
Optional config: `~/.code-intelligence/server.toml`.

## Architecture

### Data Flow

1. **Indexing Pipeline** (`src/indexer/`): File scanning → Tree-Sitter parsing → Symbol extraction → Embedding generation → Multi-modal storage
2. **Retrieval Engine** (`src/retrieval/`): Query normalization → Hybrid search (Tantivy + LanceDB) → Intent detection → Signal-based ranking → Context assembly
3. **Graph Engine** (`src/graph/`): Call hierarchy, type graphs, and dependency graph traversal
4. **Chat Agent** (`src/chat/`): Web UI → Agent loop (up to 3 tool rounds) → Streaming LLM response via SSE

### Key Directories

- `src/indexer/extract/` - Language-specific symbol extractors (Rust, TypeScript, Python, Go, Java, C, C++)
- `src/storage/` - SQLite, Tantivy, and LanceDB storage layers
- `src/retrieval/ranking/` - Scoring signals and ranking logic
- `src/handlers/` - MCP tool implementations
- `src/server/` - MCP protocol handler routing
- `src/chat/` - Chat mode: agent loop, LLM backend, tool dispatch, web UI

### Storage Layers

- **SQLite** (`storage/sqlite/`): Symbols, edges, file metadata, index/search telemetry, LLM descriptions
- **Tantivy** (`storage/tantivy.rs`): Full-text search using BM25 ranking with n-gram tokenization. Indexes symbol names, code text (comments stripped), morphological variants, and LLM-generated descriptions.
- **LanceDB** (`storage/vector.rs`): Vector embeddings (896-dim, jina-code-0.5b via llama.cpp) for semantic similarity search. Combined with Tantivy results via RRF.

### Runtime Data Location

All data stored under `~/.code-intelligence/`:
- `repos/<hash>/code-intelligence.db` (SQLite, per-repo)
- `repos/<hash>/vectors/` (LanceDB, per-repo)
- `repos/<hash>/tantivy-index/` (per-repo)
- `repos/registry.json` (shared repo registry)
- `models/jina-code-embeddings-0.5b-gguf/` (shared embedding model, GGUF via llama.cpp)
- `models/qwen2.5-coder-1.5b-gguf/` (shared description LLM model, GGUF via llama.cpp)
- `models/qwen2.5-coder-14b-gguf/` (chat LLM model, only downloaded when `--chat` is used)
- `logs/` (shared log files)

The `<hash>` is the first 16 characters of `SHA256(BASE_DIR)`.

## Configuration

The server reads configuration from environment variables. Key ones:

| Variable | Default | Description |
|----------|---------|-------------|
| `BASE_DIR` | **required** | Repository root to index |
| `EMBEDDINGS_BACKEND` | `llamacpp` | `llamacpp` (default) or `hash` (fast testing) |
| `EMBEDDINGS_DEVICE` | `metal` | `metal` (Metal GPU) or `cpu` |
| `WATCH_MODE` | `true` | Auto-reindex on file changes |
| `INDEX_PATTERNS` | `**/*.ts,**/*.tsx,**/*.rs` | Glob patterns to index |
| `HYBRID_ALPHA` | `0.7` | Vector vs keyword weight (0-1) |
| `MAX_CONTEXT_BYTES` | `200000` | Context window size limit |
| `LEARNING_ENABLED` | `true` | Enable selection/affinity learning |
| `LEARNING_SELECTION_BOOST` | `0.1` | Max boost from user selection history |
| `LEARNING_FILE_AFFINITY_BOOST` | `0.05` | Max boost from file access frequency |

## Path Handling

**Standard:** Use camino for UTF-8 typed paths, centralized normalization.

This project uses `camino` for guaranteed UTF-8 paths at the type level. All file paths should use `Utf8PathBuf` (owned) or `&Utf8Path` (borrowed) instead of the standard library's `PathBuf` and `&Path`.

```rust
use crate::path::{PathNormalizer, Utf8Path, Utf8PathBuf, PathError};

// Create normalizer with base directory
let normalizer = PathNormalizer::new(base_dir);

// Normalize path for cross-platform comparison
let normalized = normalizer.normalize_for_compare(path)?;

// Convert to relative path within base
let relative = normalizer.relative_to_base(absolute_path)?;

// Security check against path escaping
normalizer.validate_within_base(user_input)?;
```

### Key Types

| Type | Use Case | Replaces |
|------|----------|----------|
| `Utf8PathBuf` | Owned UTF-8 path | `PathBuf` |
| `&Utf8Path` | Borrowed UTF-8 path | `&Path` |
| `PathNormalizer` | Centralized path operations | Manual path manipulation |
| `PathError` | Structured path errors | ad-hoc error handling |

### Migration Pattern

```rust
// OLD (don't use - scattered, error-prone):
let path = path.replace("\\", "/");
let relative = path.strip_prefix("/repo")?;

// NEW (use - centralized, tested):
let normalized = normalizer.normalize_for_compare(Utf8Path::new(path))?;
let relative = normalizer.relative_to_base(&normalized)?;
```

### Error Handling

```rust
use crate::path::PathError;

// PathError provides helpful error messages with context
match normalizer.relative_to_base(path) {
    Ok(rel) => /* use relative path */,
    Err(PathError::OutsideRepo { path, base }) => {
        anyhow::bail!("Path '{path}' is outside repository '{base}'")
    }
    Err(PathError::NonUtf8 { path }) => {
        anyhow::bail!("Path contains non-UTF-8 characters: {}", path.display())
    }
    Err(e) => return Err(e.into()),
}
```

### Platform

macOS-only (Apple Silicon). Paths are case-sensitive in code (APFS may be case-insensitive on disk).

The `src/path/mod.rs` module includes comprehensive parameterized tests using the `test-case` crate covering security validation, case sensitivity, and error message formatting.

## Adding a New Language

1. Add tree-sitter dependency to `Cargo.toml`
2. Create `src/indexer/extract/{lang}.rs` implementing symbol extraction
3. Register in `src/indexer/extract/mod.rs` dispatcher
4. Update `src/indexer/parser.rs` language detection

## Adding a New MCP Tool

1. Define tool with `#[macros::mcp_tool]` in `src/tools/mod.rs`
2. Implement handler in `src/handlers/mod.rs`
3. Add routing in `src/server/mod.rs`

## Adding a New Chat Tool

Chat tools are a curated subset of MCP tools exposed to the on-device LLM. To add an existing MCP tool to the chat agent:

1. Add a JSON tool definition to `tool_definitions()` in `src/chat/tools.rs`
2. Add a dispatch arm to `execute_tool()` in the same file
3. The handler function (from `src/handlers/mod.rs`) is called directly — no MCP routing needed

Keep tool descriptions concise — they are embedded in the LLM system prompt and consume context tokens. Tool results are truncated to 4,000 characters.

## Chat Architecture

The chat subsystem (`src/chat/`) provides a browser-based RAG chatbot using the same tool handlers as MCP:

- **`mod.rs`** — Axum HTTP server (routes: `GET /`, `POST /api/chat`, `GET /api/status`), SSE streaming, `ChatState`
- **`agent.rs`** — Multi-round agent loop (up to 3 tool rounds), Qwen2.5 Hermes-style prompt building, `<tool_call>` XML parsing
- **`llm.rs`** — `ChatLlm` struct wrapping Qwen2.5-Coder-14B (GGUF Q4_K_M, ~9GB), streaming + non-streaming generation via llama.cpp with Metal GPU
- **`tools.rs`** — 10 tool definitions as JSON (for LLM system prompt) + `execute_tool()` dispatch to `src/handlers/`
- **`ui.html`** — Single-file web UI (vanilla JS, marked.js, highlight.js, dark/light theme)

Activated by `--chat` flag in standalone mode. The 14B model loads in a background `tokio::spawn_blocking` task so the MCP server is not blocked. Chat HTTP server spawns on a separate port (default 3334).

## Ranking Signals

The retrieval pipeline uses a hybrid search approach: BM25 keyword search (via Tantivy) and vector semantic search (via LanceDB) are run in parallel, then merged using RRF (Reciprocal Rank Fusion) to produce a combined ranking. On top of this, the scoring system in `src/retrieval/ranking/score.rs` applies structural signals:

- Test file penalty (0.5x unless Intent::Test)
- Glue code filtering (index.ts barrel files deprioritized)
- Directory semantics (src/ boosted, dist/ penalized)
- Export status boost (exported/public symbols represent the primary API surface)
- Intent multipliers (Definition 1.5x, Schema 50-75x) — detected from query patterns
- Popularity boost by incoming edge count (PageRank-style graph signal)
- Score-gap detection — drops trailing results with a >2.5x score drop from the previous result
- Sub-query coverage — ensures multi-term queries have results matching each sub-query

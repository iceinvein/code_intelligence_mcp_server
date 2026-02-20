# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Code Intelligence MCP Server is a Rust-based local code indexing and semantic search engine that provides structure-aware code navigation for LLM agents. It implements the Model Context Protocol (MCP) and integrates with tools like OpenCode, Trae, and Cursor.

**Platform:** macOS only (Apple Silicon with Metal GPU acceleration).

**Core technologies:** Rust 2021, Tree-Sitter (parsing), SQLite (metadata), Tantivy (full-text search), LanceDB (vector embeddings), llama.cpp (Metal GPU inference for both embeddings and LLM descriptions).

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

The server can run as a long-lived HTTP daemon serving multiple repos via Streamable HTTP transport. This is ideal when running multiple MCP clients (e.g. 5-6 Claude Code instances) — the embedding model (~500MB) is loaded once and shared.

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

### Key Directories

- `src/indexer/extract/` - Language-specific symbol extractors (Rust, TypeScript, Python, Go, Java, C, C++)
- `src/storage/` - SQLite, Tantivy, and LanceDB storage layers
- `src/retrieval/ranking/` - Scoring signals and ranking logic
- `src/handlers/` - MCP tool implementations
- `src/server/` - MCP protocol handler routing

### Storage Layers

- **SQLite** (`storage/sqlite/`): Symbols, edges, file metadata, index/search telemetry
- **Tantivy** (`storage/tantivy.rs`): BM25 full-text search with n-gram tokenization
- **LanceDB** (`storage/vector.rs`): Vector embeddings for semantic similarity

### Runtime Data Location

All data stored under `~/.code-intelligence/`:
- `repos/<hash>/code-intelligence.db` (SQLite, per-repo)
- `repos/<hash>/vectors/` (LanceDB, per-repo)
- `repos/<hash>/tantivy-index/` (per-repo)
- `repos/registry.json` (shared repo registry)
- `models/jina-code-embeddings-0.5b-gguf/` (shared embedding model, GGUF via llama.cpp)
- `models/qwen2.5-coder-1.5b-gguf/` (shared LLM model, GGUF via llama.cpp)
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

## Ranking Signals

The scoring system in `src/retrieval/ranking/score.rs` applies:
- Test file penalty (0.5x unless Intent::Test)
- Glue code filtering (index.ts deprioritized)
- Directory semantics (src/ boosted, dist/ penalized)
- Export status boost
- Intent multipliers (Definition 1.5x, Schema 50-75x)
- Popularity boost by incoming edge count

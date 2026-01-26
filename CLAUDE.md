# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Code Intelligence MCP Server is a Rust-based local code indexing and semantic search engine that provides structure-aware code navigation for LLM agents. It implements the Model Context Protocol (MCP) and integrates with tools like OpenCode, Trae, and Cursor.

**Core technologies:** Rust 2021, Tree-Sitter (parsing), SQLite (metadata), Tantivy (full-text search), LanceDB (vector embeddings), FastEmbed (BGE-base-en-v1.5 model).

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

All indexes stored in `.cimcp/` under BASE_DIR:
- `code-intelligence.db` (SQLite)
- `vectors/` (LanceDB)
- `tantivy-index/`
- `embeddings-cache/` (model files)

## Configuration

The server reads configuration from environment variables. Key ones:

| Variable | Default | Description |
|----------|---------|-------------|
| `BASE_DIR` | **required** | Repository root to index |
| `EMBEDDINGS_BACKEND` | `fastembed` | `fastembed` (real) or `hash` (fast testing) |
| `EMBEDDINGS_DEVICE` | `cpu` | `cpu` or `metal` (macOS GPU) |
| `WATCH_MODE` | `true` | Auto-reindex on file changes |
| `INDEX_PATTERNS` | `**/*.ts,**/*.tsx,**/*.rs` | Glob patterns to index |
| `HYBRID_ALPHA` | `0.7` | Vector vs keyword weight (0-1) |
| `MAX_CONTEXT_BYTES` | `200000` | Context window size limit |

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

### Platform-Specific Behavior

- **Windows:** Backslashes converted to forward slashes, UNC paths normalized via `dunce`
- **Unix/Linux:** Paths used as-is, case-sensitive comparison
- **macOS:** Paths case-sensitive in code (HFS+/APFS may be case-insensitive on disk)

### Cross-Platform Testing

The `src/path/mod.rs` module includes comprehensive parameterized tests (58+ test cases) using the `test-case` crate. Tests cover:
- Windows backslash normalization
- UNC path handling (Windows-only)
- Security validation for path escape attempts
- Case sensitivity per-platform
- Helpful error message formatting

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

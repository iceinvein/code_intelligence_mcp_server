# Coding Conventions

**Analysis Date:** 2026-01-22

## Naming Patterns

**Files:**
- `snake_case` for all source files: `config.rs`, `indexer.rs`, `storage.rs`
- Module files use their directory name: `src/indexer/mod.rs`, `src/storage/sqlite/mod.rs`
- Tests co-located in same file with `#[cfg(test)]` modules

**Functions:**
- `snake_case` for all function names: `extract_rust_symbols()`, `parse_embeddings_backend()`, `from_env()`
- Async functions prefixed with async context where needed: `index_all()`, `search()`
- Private functions without public prefix; public functions use `pub`
- Handler functions named `handle_<action>`: `handle_refresh_index()`, `handle_search_code()`

**Variables:**
- `snake_case` for local variables and struct fields: `base_dir`, `embeddings_backend`, `query_vector`
- Single-letter variables used only in iterators and loops: `h`, `e`, `v`
- Descriptive names preferred: `embeddings_model_dir` not `emb_dir`

**Types:**
- `PascalCase` for struct/enum names: `Config`, `SearchResponse`, `EmbeddingsBackend`, `SymbolRow`
- `PascalCase` for enum variants: `Cpu`, `Metal`, `FastEmbed`, `Hash`
- Type aliases in `PascalCase`: `Result<T>` aliased as `anyhow::Result`
- Derive macros standardized: `#[derive(Debug, Clone, Serialize, Deserialize)]` is common pattern

## Code Style

**Formatting:**
- Edition: Rust 2021
- Automatic via `cargo fmt` (built-in rustfmt)
- Line length: No explicit limit enforced in codebase
- Indentation: 4 spaces (Rust standard)

**Linting:**
- `cargo clippy` default rules enforced (implied by commit history)
- No explicit `.clippy.toml` file in repo; uses Rust defaults

## Import Organization

**Order:**
1. External crates: `use anyhow::`, `use serde::`, `use tokio::`
2. SDK crates: `use rust_mcp_sdk::`
3. Internal crate modules: `use crate::{config::, retrieval::, storage::}`
4. Standard library: `use std::path::`, `use std::sync::`

**Path Aliases:**
- None detected. Full module paths used throughout
- Absolute imports preferred: `use crate::config::Config` not relative paths

**Pattern from `src/config.rs`:**
```rust
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    env,
    path::{Path, PathBuf},
};
```

**Pattern from `src/retrieval/mod.rs`:**
```rust
use crate::{
    config::Config,
    embeddings::Embedder,
    retrieval::assembler::{ContextAssembler, ContextItem},
    storage::{
        sqlite::{SqliteStore, SymbolRow},
        tantivy::TantivyIndex,
        vector::LanceVectorTable,
    },
};
```

## Error Handling

**Patterns:**
- Uses `anyhow::Result<T>` as standard return type for most functions
- `?` operator for propagation: `Config::from_env()?`
- `.context()` for adding context to errors: `.context("Failed to get current_dir")?`
- `.with_context()` with closure for dynamic context: `.with_context(|| format!("Invalid BASE_DIR: {base_dir_raw}"))?`
- `.map_err()` for converting error types when needed

**Conversion to SDK Types:**
- Main handler converts `anyhow::Error` to `McpSdkError::Internal` in `src/main.rs` lines 69-78:
```rust
Config::from_env().map_err(|err| McpSdkError::Internal {
    description: err.to_string(),
})?
```

- Tool handlers convert to `CallToolError`: `CallToolError::from_message(err.to_string())`
- `parse_tool_args()` in `src/handlers/mod.rs` handles serde errors: `.map_err(|err| CallToolError::invalid_arguments(&params.name, Some(err.to_string())))`

**Pattern for Fallback Values:**
- `.unwrap_or()` for defaults: `.unwrap_or(false)`, `.unwrap_or(32)`
- `.unwrap_or_else()` with closure: `env::var(key).ok().map(...).unwrap_or_else(|| "default".to_string())`
- `.ok()` with `.and_then()` for optional chaining: `path.canonicalize().ok().and_then(|d| i64::try_from(d.as_secs()).ok())`

**Lock Handling:**
- `.lock().unwrap_or_else(|e| e.into_inner())` pattern for Mutex to recover from poisoned locks:
```rust
let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
```

## Logging

**Framework:** `tracing` crate for structured logging

**Setup in `src/main.rs`:**
```rust
tracing_subscriber::fmt()
    .with_env_filter(
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    )
    .with_writer(std::io::stderr)
    .init();
```

**Patterns:**
- `info!()` for startup events: `info!(version = env!("CARGO_PKG_VERSION"), "Starting...")`
- `debug!()` for detailed trace information
- `error!()` for error events: `error!(error = %err, "Server exited with error")`
- Structured fields with `key = value` syntax

## Comments

**When to Comment:**
- Module-level comments with `//!` at top of files describing purpose
- Function comments for public APIs (limited in codebase)
- Inline comments for non-obvious logic (rare in this codebase)
- Example from `src/indexer/extract/rust.rs` line 53: `// TODO: extract enum variants fields?`

**Documentation:**
- Module comments describe file organization: `//! MCP tool definitions`
- No extensive rustdoc usage observed in codebase
- Focuses on code clarity over documentation

## Function Design

**Size:**
- Most functions 20-60 lines
- Larger functions (100+ lines) in core search/ranking logic: `Retriever::search()` in `src/retrieval/mod.rs` is 267 lines
- Language extractors split into helpers: `extract_symbols_with_parser()`, `symbol_from_node()`

**Parameters:**
- Self or &self for methods
- Owned types preferred for arguments: `&str` for strings, `&[T]` for slices
- Config passed as `Arc<Config>` for async contexts
- Database passed as `&SqliteStore` or opened fresh: `SqliteStore::open(&self.db_path)?`

**Return Values:**
- `Result<T>` for fallible operations
- Tuples for multiple returns: `(String, Vec<ContextItem>)` from context assembly
- Option<T> for nullable results: `Option<String>`, `Option<i64>`

## Module Design

**Exports:**
- `pub mod` for submodules in `mod.rs` files
- `pub use` for re-exporting key types: `pub use operations::SqliteStore;` in `src/storage/sqlite/mod.rs`
- Private modules for internal implementation details: `mod cache;`, `mod query;` in `src/retrieval/mod.rs`

**Barrel Files:**
- `mod.rs` files act as barrel exports: `src/storage/mod.rs` re-exports storage layers
- `src/handlers/mod.rs` exports handler functions and `AppState`
- Reduces import complexity at call sites

**Test Modules:**
- Inline `#[cfg(test)]` mod tests at end of file: `src/config.rs` lines 398-595
- Tests have access to private functions via same module
- Uses `static ENV_LOCK` for synchronization across tests that modify environment

## Struct and Enum Patterns

**Structs:**
- Data containers with derived traits: `#[derive(Debug, Clone, Serialize, Deserialize)]`
- Fields are public unless internal implementation detail
- Named after data they hold: `SymbolRow`, `EdgeRow`, `SearchResponse`

**Enums:**
- Variants for distinct states or options: `EmbeddingsBackend::FastEmbed | Hash`
- `#[serde(rename_all = "lowercase")]` for JSON compatibility: `src/config.rs` lines 9-20

**Configuration Pattern:**
- Single `Config` struct holding all settings: `src/config.rs` lines 22-55
- Static factory: `Config::from_env()` reads environment variables with fallback defaults
- Normalization methods: `normalize_path_to_base()`, `path_relative_to_base()`

---

*Convention analysis: 2026-01-22*

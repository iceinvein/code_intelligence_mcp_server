# Testing Patterns

**Analysis Date:** 2026-01-22

## Test Framework

**Runner:**
- `cargo test` with Tokio runtime support
- Async tests use `#[tokio::test]` macro

**Assertion Library:**
- Standard `assert!()` and `assert_eq!()` macros
- `assert!()` for boolean conditions
- `assert_eq!()` for equality checks

**Run Commands:**
```bash
cargo test                                    # Run all tests
cargo test --test integration_index_search    # Integration tests only
./scripts/test_local.sh                       # End-to-end test with dummy workspace
EMBEDDINGS_BACKEND=hash cargo test            # Skip model download (fast testing)
```

## Test File Organization

**Location:**
- Integration tests: `tests/integration_index_search.rs` (separate from src)
- Unit tests: Inline in same files with `#[cfg(test)]` modules at end
- Example: `src/config.rs` lines 398-595 contain test module

**Naming:**
- Test files in `tests/` directory: `integration_index_search.rs`
- Test functions use descriptive names: `indexes_and_searches_with_hash_embedder()`, `incremental_index_skips_unchanged_and_removes_deleted_files()`
- Pattern: `test_<function_or_behavior>()`

**Structure:**
```
tests/
├── integration_index_search.rs    # Full system tests
src/
├── config.rs                      # Includes #[cfg(test)] mod tests
├── ...
```

## Test Structure

**Suite Organization:**
From `src/config.rs` lines 398-595:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "code-intel-config-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn clear_env() {
        for k in [
            "BASE_DIR",
            "DB_PATH",
            // ... env vars to clear
        ] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn from_env_requires_base_dir() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let err = Config::from_env().unwrap_err().to_string();
        assert!(err.contains("BASE_DIR"));
    }
}
```

**Patterns:**
- Setup helper functions: `tmp_dir()`, `clear_env()`, `test_config()`
- Lock guards for environment isolation: `let _g = ENV_LOCK.lock()?;` prevents concurrent env tests
- Guard variable underscore prefix: `_g` indicates deliberately unused for lock semantics
- Cleanup implicit (temp dirs in `/tmp`, env removed after test)

## Async Testing

From `tests/integration_index_search.rs`:
```rust
#[tokio::test]
async fn indexes_and_searches_with_hash_embedder() {
    let dir = tmp_dir();

    // Setup
    std::fs::write(dir.join("a.ts"), r#"..."#).unwrap();

    let config = Arc::new(test_config(&dir));
    let sqlite = SqliteStore::open(&config.db_path).unwrap();
    let indexer = IndexPipeline::new(config.clone(), ...);

    // Execute
    let stats = indexer.index_all().await.unwrap();

    // Assert
    assert!(stats.files_indexed >= 2);
    assert!(stats.symbols_indexed >= 3);
}
```

**Async Patterns:**
- `#[tokio::test]` for async tests
- `.await` for futures
- `.unwrap()` acceptable in test context for setup failures
- Test isolation via temporary directories

## Mocking

**Not heavily used.** Instead uses:
- Real implementations with test backends: `HashEmbedder` instead of mock embedder
- Environment variable control: `EMBEDDINGS_BACKEND=hash cargo test` skips real model
- Temporary file systems: Each test creates own `/tmp` directory
- Real database (SQLite in-memory possible but not used)

**Approach:**
- Integration tests use real components in isolated temp environments
- No mocking framework (mockito, mocktopus) detected
- Hash embedder in `src/embeddings/hash.rs` serves as test backend

## Test Fixtures and Test Data

**Test Data:**
From `tests/integration_index_search.rs` lines 75-94:
```rust
std::fs::write(
    dir.join("a.ts"),
    r#"
export function alpha() { return beta() }
export function beta() { return 123 }
"#,
)
.unwrap();

std::fs::write(
    dir.join("lib.rs"),
    r#"
pub struct Foo {
  a: i32,
}

pub fn foo() -> Foo { Foo { a: 1 } }
"#,
)
.unwrap();
```

**Fixtures:**
- Inline string literals with `r#"..."#` for multi-line source
- Config fixture: `test_config()` builds Config with defaults for testing
- Directory fixture: `tmp_dir()` creates isolated temp environment

**Location:**
- Fixtures in test functions themselves
- No shared fixture files or factory modules
- Immutable test data

## Coverage

**Requirements:** Not enforced

**View Coverage:**
```bash
# No coverage tooling detected in Cargo.toml
# Can use tarpaulin if needed: cargo tarpaulin
```

## Test Types

**Unit Tests:**
- Scope: Single module functionality
- Approach: Inline tests in same file with subject
- Example: `src/config.rs` tests `Config::from_env()` behavior
- Files: `config.rs`, embedded in source modules

**Integration Tests:**
- Scope: Full pipeline from indexing through retrieval
- Approach: Create real components, test end-to-end
- Example: `tests/integration_index_search.rs` tests indexing, retrieval, and incremental updates
- Files: `tests/integration_index_search.rs`

**E2E Tests:**
- Script: `./scripts/test_local.sh` - End-to-end with actual repository
- Approach: Runs server against real test workspace
- Not detected as rust tests; bash script

## Common Test Patterns

**Setup with Arc/Mutex for Async:**
```rust
let config = Arc::new(test_config(&dir));
let tantivy = Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
let embedder = Arc::new(Mutex::new(
    Box::new(HashEmbedder::new(config.hash_embedding_dim)) as _,
));
let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
let vectors = Arc::new(
    lancedb
        .open_or_create_table("symbols", vector_dim)
        .await
        .unwrap(),
);
```

**Incremental Testing:**
From `tests/integration_index_search.rs` lines 155-219:
```rust
#[tokio::test]
async fn incremental_index_skips_unchanged_and_removes_deleted_files() {
    // Index once
    let stats1 = indexer.index_all().await.unwrap();
    assert_eq!(stats1.files_scanned, 2);
    assert_eq!(stats1.files_indexed, 2);

    // Index again - should skip
    let stats2 = indexer.index_all().await.unwrap();
    assert_eq!(stats2.files_unchanged, 2);
    assert_eq!(stats2.files_indexed, 0);

    // Delete file and re-index
    std::fs::remove_file(dir.join("a.ts")).unwrap();
    let stats3 = indexer.index_all().await.unwrap();
    assert_eq!(stats3.files_deleted, 1);
}
```

**Error Testing:**
From `src/config.rs` lines 456-461:
```rust
#[test]
fn from_env_requires_base_dir() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_env();
    let err = Config::from_env().unwrap_err().to_string();
    assert!(err.contains("BASE_DIR"));
}
```

**Assertion Validation:**
```rust
let resp = retriever.search("alpha", 1, true).await.unwrap();
assert!(resp.context.contains("export function alpha"));
assert!(resp.context.contains("export function beta"));

let resp2 = retriever.search("Foo", 3, false).await.unwrap();
assert!(resp2.context.contains("pub struct Foo"));
```

**Symbol Lookup Testing:**
```rust
let beta = sqlite
    .search_symbols_by_exact_name("beta", None, 10)
    .unwrap();
let beta = beta.first().unwrap();
let examples = sqlite.list_usage_examples_for_symbol(&beta.id, 20).unwrap();
assert!(examples.iter().any(|e| e.snippet.contains("beta")));
```

## Test Configuration

**Environment:**
- Tests use `EMBEDDINGS_BACKEND=hash` for fast model-free testing
- `test_config()` sets Hash embedder with 32-dim vectors for tests
- Watch mode disabled: `watch_mode: false`

**Isolation:**
- Each test creates unique temp directory: `code-intel-it-{nanos}-{c}`
- Environment variables locked with static `ENV_LOCK` mutex
- No shared state between tests

## Known Testing Gaps

**Areas without inline tests:**
- Storage implementations (SQLite, Tantivy, LanceDB) - tested via integration tests
- Ranking and scoring algorithms - tested via end-to-end search results
- Language-specific extractors (Rust, TypeScript, Python, etc.) - tested via integration tests

---

*Testing analysis: 2026-01-22*

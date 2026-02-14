# Logging Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add per-repo index logs, MCP access logs, and 7-day log retention to the code-intelligence MCP server.

**Architecture:** Three tracing layers (global exists, access log via target filter, per-repo via direct-write `RepoLogger`). Log cleanup runs at startup. See `docs/plans/2026-02-14-logging-design.md` for full design.

**Tech Stack:** Rust `tracing` + `tracing-subscriber` (with `env-filter` feature) + `tracing-appender` + `chrono`

---

### Task 1: Create `src/logging.rs` — `cleanup_old_logs` function

**Files:**
- Create: `src/logging.rs`
- Modify: `src/lib.rs:1-17` (add `pub mod logging;`)

**Step 1: Write the test for `cleanup_old_logs`**

Add to the bottom of `src/logging.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_cleanup_old_logs_removes_old_files() {
        let dir = TempDir::new().unwrap();
        let dir_path = dir.path();

        // Create a "fresh" log file (now)
        let fresh = dir_path.join("server.log.2026-02-14");
        fs::write(&fresh, "fresh").unwrap();

        // Create an "old" log file — backdate its mtime
        let old = dir_path.join("server.log.2026-01-01");
        fs::write(&old, "old").unwrap();
        // Set mtime to 30 days ago
        let thirty_days_ago = std::time::SystemTime::now()
            - std::time::Duration::from_secs(30 * 24 * 3600);
        filetime::FileTime::from_system_time(thirty_days_ago);
        filetime::set_file_mtime(
            &old,
            filetime::FileTime::from_system_time(thirty_days_ago),
        )
        .unwrap();

        // Create a non-log file (should NOT be deleted)
        let other = dir_path.join("config.toml");
        fs::write(&other, "config").unwrap();
        filetime::set_file_mtime(
            &other,
            filetime::FileTime::from_system_time(thirty_days_ago),
        )
        .unwrap();

        cleanup_old_logs(dir_path, 7);

        assert!(fresh.exists(), "Fresh log should remain");
        assert!(!old.exists(), "Old log should be deleted");
        assert!(other.exists(), "Non-log file should remain");
    }

    #[test]
    fn test_cleanup_old_logs_ignores_missing_dir() {
        // Should not panic on a non-existent directory
        cleanup_old_logs(std::path::Path::new("/nonexistent/path"), 7);
    }
}
```

**Step 2: Add `filetime` dev-dependency to Cargo.toml**

In `Cargo.toml` under `[dev-dependencies]`, add:

```toml
filetime = "0.2"
```

**Step 3: Write the implementation**

Create `src/logging.rs` with:

```rust
//! Logging utilities: log cleanup and per-repo logging.

use std::path::Path;

/// Delete log files older than `max_age_days` in `dir`.
///
/// Only targets files whose name contains ".log." to avoid deleting
/// non-log files. Silently skips if `dir` doesn't exist.
pub fn cleanup_old_logs(dir: &Path, max_age_days: u64) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return, // directory doesn't exist or can't be read
    };

    let max_age = std::time::Duration::from_secs(max_age_days * 24 * 3600);

    for entry in entries.flatten() {
        let path = entry.path();

        // Only target log files (name contains ".log.")
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.contains(".log.") => n.to_string(),
            _ => continue,
        };
        let _ = name; // used only for the guard above

        // Check file age via metadata modified time
        let metadata = match path.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let age = match std::time::SystemTime::now().duration_since(modified) {
            Ok(d) => d,
            Err(_) => continue,
        };

        if age > max_age {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "Failed to remove old log file"
                );
            } else {
                tracing::debug!(
                    path = %path.display(),
                    age_days = age.as_secs() / 86400,
                    "Removed old log file"
                );
            }
        }
    }
}
```

**Step 4: Register the module in `src/lib.rs`**

Add `pub mod logging;` to `src/lib.rs` (insert after `pub mod indexer;`, line 6):

```rust
pub mod logging;
```

**Step 5: Run test to verify it passes**

Run: `cargo test --lib logging::tests -- --test-threads=1`
Expected: 2 tests PASS

**Step 6: Commit**

```bash
git add src/logging.rs src/lib.rs Cargo.toml
git commit -m "feat(logging): add cleanup_old_logs with 7-day retention"
```

---

### Task 2: Create `RepoLogger` for per-repo index logs

**Files:**
- Modify: `src/logging.rs`

**Step 1: Write the test for `RepoLogger`**

Append to the `tests` module in `src/logging.rs`:

```rust
    #[test]
    fn test_repo_logger_creates_log_dir_and_writes() {
        let dir = TempDir::new().unwrap();
        let repo_data_dir = crate::path::Utf8PathBuf::from_path_buf(
            dir.path().to_path_buf()
        ).unwrap();

        let logger = RepoLogger::new(&repo_data_dir).expect("should create logger");

        logger.info("Index started");
        logger.info("Indexed 42 files");

        // Flush by dropping
        drop(logger);

        // Check that a log file was created in <repo_data_dir>/logs/
        let logs_dir = dir.path().join("logs");
        assert!(logs_dir.exists(), "logs/ dir should be created");
        let entries: Vec<_> = std::fs::read_dir(&logs_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!entries.is_empty(), "Should have at least one log file");

        // Read and verify contents
        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("Index started"), "Should contain first message");
        assert!(content.contains("Indexed 42 files"), "Should contain second message");
    }
```

**Step 2: Write the implementation**

Add to `src/logging.rs`, above the `#[cfg(test)]` block:

```rust
use crate::path::Utf8Path;

/// Per-repo logger that writes directly to a rolling log file.
///
/// Unlike the global tracing subscriber, this writes to repo-specific
/// log files under `<repo_data_dir>/logs/`. Uses `tracing_appender`
/// for daily rotation and non-blocking I/O.
pub struct RepoLogger {
    writer: tracing_appender::non_blocking::NonBlocking,
    // Guard must be held alive for the writer to flush on drop
    _guard: tracing_appender::non_blocking::WorkerGuard,
    repo_name: String,
}

impl RepoLogger {
    /// Create a new per-repo logger writing to `<repo_data_dir>/logs/index.log.YYYY-MM-DD`.
    ///
    /// Returns `None` if the logs directory can't be created.
    pub fn new(repo_data_dir: &Utf8Path) -> Option<Self> {
        let logs_dir = repo_data_dir.join("logs");
        std::fs::create_dir_all(logs_dir.as_std_path()).ok()?;

        let appender = tracing_appender::rolling::daily(logs_dir.as_std_path(), "index.log");
        let (writer, guard) = tracing_appender::non_blocking(appender);

        // Extract repo name from the parent path for log prefixing
        let repo_name = repo_data_dir
            .file_name()
            .unwrap_or("unknown")
            .to_string();

        Some(Self {
            writer,
            _guard: guard,
            repo_name,
        })
    }

    /// Write an INFO-level message to the per-repo log.
    pub fn info(&self, message: &str) {
        self.write("INFO", message);
    }

    /// Write a WARN-level message to the per-repo log.
    pub fn warn(&self, message: &str) {
        self.write("WARN", message);
    }

    /// Write an ERROR-level message to the per-repo log.
    pub fn error(&self, message: &str) {
        self.write("ERROR", message);
    }

    fn write(&self, level: &str, message: &str) {
        use std::io::Write;
        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let mut writer = self.writer.clone();
        let _ = writeln!(writer, "{} {} [{}] {}", timestamp, level, self.repo_name, message);
    }
}
```

**Step 3: Run tests**

Run: `cargo test --lib logging::tests -- --test-threads=1`
Expected: 3 tests PASS (2 from Task 1 + 1 new)

**Step 4: Commit**

```bash
git add src/logging.rs
git commit -m "feat(logging): add RepoLogger for per-repo index log files"
```

---

### Task 3: Add MCP access log layer to `main.rs`

**Files:**
- Modify: `src/main.rs:58-96` (logging setup block)

**Step 1: Add `tracing-subscriber` filter feature**

The access log layer needs `filter::Targets` from `tracing-subscriber`. Check if it's already available — the `env-filter` feature should include it. If not, we need to add `features = ["env-filter", "filter"]` in `Cargo.toml`.

Update `Cargo.toml` line 14:

```toml
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

No change needed — `filter::Targets` is in the base `tracing-subscriber` crate (not behind a feature gate).

**Step 2: Modify the subscriber setup in `main.rs`**

Replace lines 58-96 of `src/main.rs` (the logging setup block) with:

```rust
    // Set up file logging to global ~/.code-intelligence/logs directory
    let global_dir = code_intelligence_mcp_server::config::get_data_dir();
    let logs_dir = global_dir.join("logs");

    // Create logs directory if it doesn't exist
    std::fs::create_dir_all(&logs_dir).map_err(|err| McpSdkError::Internal {
        description: format!("Failed to create logs directory: {}", err),
    })?;

    // Clean up log files older than 7 days
    code_intelligence_mcp_server::logging::cleanup_old_logs(logs_dir.as_std_path(), 7);

    // Create a daily rotating file appender for global server log
    let file_appender = tracing_appender::rolling::daily(&logs_dir, "server.log");
    let (non_blocking_file, _guard) = tracing_appender::non_blocking(file_appender);

    // Create a daily rotating file appender for MCP access log
    let access_appender = tracing_appender::rolling::daily(&logs_dir, "access.log");
    let (non_blocking_access, _access_guard) = tracing_appender::non_blocking(access_appender);

    // Set up layered subscriber with stderr, file, and access log output
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(true)
        )
        .with(
            fmt::layer()
                .with_writer(non_blocking_file)
                .with_ansi(false)
        )
        .with(
            fmt::layer()
                .with_writer(non_blocking_access)
                .with_ansi(false)
                .with_filter(tracing_subscriber::filter::Targets::new()
                    .with_target("mcp_access", tracing::Level::INFO))
        )
        .init();

    // Keep guards alive for the duration of the program
    std::mem::forget(_guard);
    std::mem::forget(_access_guard);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        logs_dir = %logs_dir,
        "Starting code-intelligence-mcp-server"
    );
```

**Step 3: Add the `Layer` import**

Add to the imports at the top of `src/main.rs` (line 17):

```rust
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
```

(Add `Layer` to the existing import — it's needed for the `.with_filter()` method.)

**Step 4: Build to verify compilation**

Run: `cargo build 2>&1 | head -20`
Expected: Successful compilation

**Step 5: Commit**

```bash
git add src/main.rs Cargo.toml
git commit -m "feat(logging): add MCP access log layer and log cleanup on startup"
```

---

### Task 4: Add access log event to `dispatch_tool_call`

**Files:**
- Modify: `src/server/mod.rs:47-290` (`dispatch_tool_call` function)

**Step 1: Wrap the match with timing and emit access log event**

Replace the `dispatch_tool_call` function body in `src/server/mod.rs`. The key change is:
1. Clone `params.name` before the match
2. Record `Instant::now()` before
3. Emit `info!(target: "mcp_access", ...)` after

```rust
/// Shared tool dispatch — used by both embedded and standalone handlers
pub async fn dispatch_tool_call(
    state: &AppState,
    params: CallToolRequestParams,
) -> std::result::Result<CallToolResult, CallToolError> {
    let tool_name = params.name.clone();
    let start = std::time::Instant::now();

    let result = match params.name.as_str() {
        // ... all existing match arms stay EXACTLY the same ...
        _ => Err(CallToolError::unknown_tool(params.name)),
    };

    let duration_ms = start.elapsed().as_millis();
    let status = if result.is_ok() { "ok" } else { "error" };
    tracing::info!(
        target: "mcp_access",
        tool = %tool_name,
        duration_ms = duration_ms as u64,
        status = %status,
    );

    result
}
```

Concretely, this means:
1. Add `let tool_name = params.name.clone();` and `let start = std::time::Instant::now();` before the `match`
2. Wrap the match: `let result = match params.name.as_str() { ... };`
3. Add the timing + access log after the match
4. Change the last `_ =>` arm to use `params.name` (still valid since we only cloned, didn't move)
5. Return `result` at the end

**Step 2: Build to verify**

Run: `cargo build 2>&1 | head -20`
Expected: Successful compilation

**Step 3: Commit**

```bash
git add src/server/mod.rs
git commit -m "feat(logging): emit MCP access log event per tool call with timing"
```

---

### Task 5: Integrate `RepoLogger` into the indexing pipeline

**Files:**
- Modify: `src/indexer/pipeline/mod.rs` (IndexPipeline struct + key methods)

**Step 1: Add `RepoLogger` to `IndexPipeline`**

Add to the `IndexPipeline` struct (around line 60):

```rust
use crate::logging::RepoLogger;
```

Add field to the struct:

```rust
#[derive(Clone)]
pub struct IndexPipeline {
    config: Arc<Config>,
    db_path: Utf8PathBuf,
    tantivy: Arc<TantivyIndex>,
    vectors: Arc<LanceVectorTable>,
    embedder: Arc<Mutex<Box<dyn Embedder + Send>>>,
    cache: Arc<EmbeddingCache>,
    metrics: Arc<MetricsRegistry>,
    repo_logger: Option<Arc<RepoLogger>>,
}
```

Note: `RepoLogger` contains `NonBlocking` (which is `Clone`) and `WorkerGuard` (which is NOT `Clone`). Wrapping in `Arc` allows the `IndexPipeline` to remain `Clone`.

**Step 2: Initialize `RepoLogger` in `new()`**

In the `new()` method (around line 79), after computing `db_path`, create the logger:

```rust
    // Create per-repo logger
    let repo_data_dir = config.db_path.parent()
        .unwrap_or(&config.db_path);
    let repo_logger = RepoLogger::new(repo_data_dir).map(Arc::new);
```

And add `repo_logger` to the `Self { ... }` return struct.

**Step 3: Add repo logging calls to `index_all()`**

At the start of `index_all()` (line 115), log the run start:

```rust
    if let Some(ref logger) = self.repo_logger {
        logger.info(&format!("Index run started for {}", self.repo_name()));
    }
```

At the end of `index_all()`, before `Ok(stats)` (around line 648):

```rust
    if let Some(ref logger) = self.repo_logger {
        logger.info(&format!(
            "Index run completed: {} files scanned, {} indexed, {} unchanged, {} skipped, {} deleted, {} symbols",
            stats.files_scanned, stats.files_indexed, stats.files_unchanged,
            stats.files_skipped, stats.files_deleted, stats.symbols_indexed
        ));
    }
```

**Step 4: Add repo logging for errors in `index_files_sequential_internal()`**

In the sequential indexing loop (around line 674 for fingerprint errors, line 729 for read errors, line 751 for parse errors), add repo logger calls after each existing `tracing::warn!`:

```rust
    // After tracing::warn! for fingerprint failure:
    if let Some(ref logger) = self.repo_logger {
        logger.warn(&format!("Failed to fingerprint: {}", file.display()));
    }

    // After tracing::warn! for read failure:
    if let Some(ref logger) = self.repo_logger {
        logger.warn(&format!("Failed to read: {}", file.display()));
    }

    // After tracing::warn! for parse failure:
    if let Some(ref logger) = self.repo_logger {
        logger.warn(&format!("Failed to extract symbols: {}", file.display()));
    }
```

**Step 5: Build to verify**

Run: `cargo build 2>&1 | head -20`
Expected: Successful compilation

**Step 6: Commit**

```bash
git add src/indexer/pipeline/mod.rs
git commit -m "feat(logging): integrate RepoLogger into indexing pipeline"
```

---

### Task 6: Clean up per-repo log directories on startup

**Files:**
- Modify: `src/main.rs` (in `run_embedded()` and `run_standalone()`)

**Step 1: Add per-repo log cleanup in `run_embedded()`**

After the config is loaded and repo data dir exists (around line 202 in `run_embedded()`), add:

```rust
    // Clean up per-repo log files older than 7 days
    {
        let repo_logs_dir = config.db_path.parent()
            .unwrap_or(&config.db_path)
            .join("logs");
        code_intelligence_mcp_server::logging::cleanup_old_logs(
            repo_logs_dir.as_std_path(), 7
        );
    }
```

**Step 2: Build and run tests**

Run: `cargo build 2>&1 | head -20`
Run: `EMBEDDINGS_BACKEND=hash cargo test --lib logging 2>&1`
Expected: All tests pass

**Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat(logging): add per-repo log cleanup on embedded server startup"
```

---

### Task 7: End-to-end verification

**Step 1: Build release binary**

Run: `cargo build --release 2>&1 | tail -5`
Expected: Successful compilation

**Step 2: Run full test suite**

Run: `EMBEDDINGS_BACKEND=hash cargo test 2>&1 | tail -20`
Expected: All tests pass

**Step 3: Manual smoke test**

Start the server and verify log files appear:

```bash
# Start with a test repo
BASE_DIR=/tmp/test-repo ./target/release/code-intelligence-mcp-server &
sleep 2

# Check global logs
ls -la ~/.code-intelligence/logs/
# Expect: server.log.YYYY-MM-DD and access.log.YYYY-MM-DD

# Check per-repo logs
ls -la ~/.code-intelligence/repos/*/logs/
# Expect: index.log.YYYY-MM-DD (if indexing was triggered)

kill %1
```

**Step 4: Final commit (if any fixups)**

```bash
git add -A
git commit -m "fix(logging): address issues found during manual testing"
```

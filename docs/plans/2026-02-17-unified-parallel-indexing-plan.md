# Unified Parallel Indexing Pipeline — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the dual sequential/parallel indexing paths with a single unified pipeline that separates parsing from writing, adds connection pooling, batches all commits, and auto-tunes parallelism — targeting 3-5x speedup.

**Architecture:** Three-phase pipeline (Parse → Write → Embed) connected by a `ParsedFile` struct. Parse phase uses Rayon for parallel tree-sitter extraction with read-only pooled SQLite connections. Write phase batches SQLite transactions (chunks of 50 files) and commits Tantivy once. Embed phase is unchanged (existing `generate_embeddings_for_orphaned_symbols`).

**Tech Stack:** Rust 2021, Rayon (existing dep), rusqlite (existing dep), Tantivy (existing dep)

**Design doc:** `docs/plans/2026-02-17-unified-parallel-indexing-design.md`

---

## Task 1: SQLite Connection Pool

**Files:**
- Create: `src/storage/sqlite/pool.rs`
- Modify: `src/storage/sqlite/mod.rs` (add `pub mod pool;`)
- Test: inline `#[cfg(test)] mod tests` in pool.rs

### Step 1: Write the failing test

Add to `src/storage/sqlite/pool.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Utf8PathBuf;

    fn test_db_path() -> Utf8PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pool-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Utf8PathBuf::from_path_buf(dir.join("test.db")).unwrap()
    }

    #[test]
    fn pool_reuses_connections() {
        let db_path = test_db_path();
        let pool = SqlitePool::new(&db_path, 4).unwrap();

        // Get and return a connection
        {
            let conn = pool.get().unwrap();
            conn.execute("SELECT 1", []).unwrap();
        } // dropped, returned to pool

        // Pool should have 1 available connection
        assert_eq!(pool.available(), 1);

        // Get again — should reuse, not create new
        {
            let _conn = pool.get().unwrap();
            assert_eq!(pool.available(), 0);
        }
        assert_eq!(pool.available(), 1);
    }

    #[test]
    fn pool_respects_max_size() {
        let db_path = test_db_path();
        let pool = SqlitePool::new(&db_path, 2).unwrap();

        let _c1 = pool.get().unwrap();
        let _c2 = pool.get().unwrap();

        // Third connection should fail (pool exhausted)
        assert!(pool.try_get().is_none());
    }

    #[test]
    fn pool_connections_have_wal_and_fk() {
        let db_path = test_db_path();
        let pool = SqlitePool::new(&db_path, 2).unwrap();
        let conn = pool.get().unwrap();

        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal, "wal");

        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }
}
```

### Step 2: Run test to verify it fails

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib storage::sqlite::pool::tests -v`
Expected: FAIL — module doesn't exist yet

### Step 3: Write the implementation

Create `src/storage/sqlite/pool.rs`:

```rust
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::sync::Mutex;

use crate::path::{Utf8Path, Utf8PathBuf};

/// Simple SQLite connection pool.
///
/// Connections are initialized with WAL mode, foreign keys, and busy timeout.
/// On drop, `PooledConnection` returns the connection to the pool for reuse.
pub struct SqlitePool {
    db_path: Utf8PathBuf,
    pool: Mutex<Vec<Connection>>,
    max_size: usize,
    created: Mutex<usize>,
}

impl SqlitePool {
    pub fn new(db_path: &Utf8Path, max_size: usize) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create db parent dir: {parent}"))?;
        }
        Ok(Self {
            db_path: db_path.to_owned(),
            pool: Mutex::new(Vec::with_capacity(max_size)),
            max_size,
            created: Mutex::new(0),
        })
    }

    /// Get a connection from the pool (blocking).
    /// Reuses an existing connection or creates a new one (up to max_size).
    /// Returns error if pool is exhausted.
    pub fn get(&self) -> Result<PooledConnection<'_>> {
        // Try to reuse
        if let Some(conn) = self.pool.lock().unwrap().pop() {
            return Ok(PooledConnection {
                conn: Some(conn),
                pool: self,
            });
        }

        // Try to create
        let mut created = self.created.lock().unwrap();
        if *created < self.max_size {
            let conn = self.create_connection()?;
            *created += 1;
            return Ok(PooledConnection {
                conn: Some(conn),
                pool: self,
            });
        }

        anyhow::bail!(
            "SQLite pool exhausted: max_size={}, all connections checked out",
            self.max_size
        )
    }

    /// Non-blocking try_get — returns None if pool is exhausted.
    pub fn try_get(&self) -> Option<PooledConnection<'_>> {
        if let Some(conn) = self.pool.lock().unwrap().pop() {
            return Some(PooledConnection {
                conn: Some(conn),
                pool: self,
            });
        }

        let mut created = self.created.lock().unwrap();
        if *created < self.max_size {
            if let Ok(conn) = self.create_connection() {
                *created += 1;
                return Some(PooledConnection {
                    conn: Some(conn),
                    pool: self,
                });
            }
        }
        None
    }

    /// Number of connections currently available in the pool.
    pub fn available(&self) -> usize {
        self.pool.lock().unwrap().len()
    }

    fn create_connection(&self) -> Result<Connection> {
        let conn = Connection::open(self.db_path.as_str())
            .with_context(|| format!("Failed to open sqlite db: {}", self.db_path))?;
        let _ = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0)).ok();
        conn.execute("PRAGMA foreign_keys = ON", [])
            .context("Failed to enable foreign keys")?;
        conn.execute("PRAGMA synchronous = NORMAL", [])
            .context("Failed to set synchronous mode")?;
        conn.execute("PRAGMA busy_timeout = 5000", [])
            .context("Failed to set busy timeout")?;
        Ok(conn)
    }

    fn return_connection(&self, conn: Connection) {
        let mut pool = self.pool.lock().unwrap();
        if pool.len() < self.max_size {
            pool.push(conn);
        }
        // else: drop the connection (pool is full, shouldn't happen normally)
    }
}

/// RAII guard — returns connection to pool on drop.
/// Deref to `Connection` for direct use.
pub struct PooledConnection<'a> {
    conn: Option<Connection>,
    pool: &'a SqlitePool,
}

impl std::ops::Deref for PooledConnection<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("connection taken from PooledConnection")
    }
}

impl Drop for PooledConnection<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.return_connection(conn);
        }
    }
}
```

### Step 4: Register the module

In `src/storage/sqlite/mod.rs`, add after line 2 (`pub mod schema;`):
```rust
pub mod pool;
```

### Step 5: Run tests to verify they pass

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib storage::sqlite::pool::tests -- --test-threads=1`
Expected: 3 tests PASS

### Step 6: Commit

```bash
git add src/storage/sqlite/pool.rs src/storage/sqlite/mod.rs
git commit -m "feat(storage): add SQLite connection pool with RAII guard"
```

---

## Task 2: Batch SQLite Upsert Functions

**Files:**
- Modify: `src/storage/sqlite/queries/symbols.rs` (add `batch_upsert_symbols`)
- Modify: `src/storage/sqlite/queries/edges.rs` (add `batch_upsert_edges`)
- Modify: `src/storage/sqlite/queries/misc.rs` (add `batch_upsert_usage_examples`)
- Modify: `src/storage/sqlite/mod.rs` (add delegate methods)

These follow the exact pattern of the existing `batch_upsert_todos` in `src/storage/sqlite/queries/todos.rs:32`.

### Step 1: Write the failing test

Add to `src/storage/sqlite/queries/symbols.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::SqliteStore;

    #[test]
    fn batch_upsert_symbols_inserts_multiple() {
        let store = SqliteStore::open_in_memory().unwrap();
        store.init().unwrap();
        let conn = store.read().unwrap();

        let symbols = vec![
            SymbolRow {
                id: "s1".into(), file_path: "a.rs".into(), language: "rust".into(),
                kind: "function".into(), name: "foo".into(), exported: true,
                start_byte: 0, end_byte: 10, start_line: 1, end_line: 3,
                text: "fn foo() {}".into(),
            },
            SymbolRow {
                id: "s2".into(), file_path: "a.rs".into(), language: "rust".into(),
                kind: "function".into(), name: "bar".into(), exported: false,
                start_byte: 11, end_byte: 20, start_line: 4, end_line: 6,
                text: "fn bar() {}".into(),
            },
        ];

        batch_upsert_symbols(&conn, &symbols).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
```

### Step 2: Run test to verify it fails

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib storage::sqlite::queries::symbols::tests -v`
Expected: FAIL — `batch_upsert_symbols` not found

### Step 3: Implement batch functions

In `src/storage/sqlite/queries/symbols.rs`, add after `upsert_symbol`:

```rust
/// Batch upsert symbols within an existing transaction or connection.
/// Caller is responsible for transaction management.
pub fn batch_upsert_symbols(conn: &Connection, symbols: &[SymbolRow]) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        r#"
INSERT INTO symbols (
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, unixepoch())
ON CONFLICT(id) DO UPDATE SET
  file_path=excluded.file_path,
  language=excluded.language,
  kind=excluded.kind,
  name=excluded.name,
  exported=excluded.exported,
  start_byte=excluded.start_byte,
  end_byte=excluded.end_byte,
  start_line=excluded.start_line,
  end_line=excluded.end_line,
  text=excluded.text,
  updated_at=unixepoch()
"#,
    )?;
    for s in symbols {
        stmt.execute(params![
            s.id, s.file_path, s.language, s.kind, s.name,
            if s.exported { 1 } else { 0 },
            s.start_byte, s.end_byte, s.start_line, s.end_line, s.text
        ])
        .with_context(|| format!("Failed to batch upsert symbol: id={}", s.id))?;
    }
    Ok(())
}
```

In `src/storage/sqlite/queries/edges.rs`, add after `upsert_edge_evidence`:

```rust
/// Batch upsert edges and evidence within an existing transaction or connection.
pub fn batch_upsert_edges(
    conn: &Connection,
    edges: &[(EdgeRow, Vec<EdgeEvidenceRow>)],
) -> Result<()> {
    let mut edge_stmt = conn.prepare_cached(
        r#"
INSERT INTO edges(from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count, resolution, resolution_rank)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
ON CONFLICT(from_symbol_id, to_symbol_id, edge_type) DO UPDATE SET
  at_file=COALESCE(edges.at_file, excluded.at_file),
  at_line=COALESCE(edges.at_line, excluded.at_line),
  confidence=MAX(edges.confidence, excluded.confidence),
  evidence_count=MAX(edges.evidence_count, excluded.evidence_count),
  resolution_rank=MAX(edges.resolution_rank, excluded.resolution_rank),
  resolution=CASE
    WHEN excluded.resolution_rank > edges.resolution_rank THEN excluded.resolution
    ELSE edges.resolution
  END
"#,
    )?;
    let mut ev_stmt = conn.prepare_cached(
        r#"
INSERT INTO edge_evidence(from_symbol_id, to_symbol_id, edge_type, at_file, at_line, count)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(from_symbol_id, to_symbol_id, edge_type, at_file, at_line) DO UPDATE SET
  count=MAX(edge_evidence.count, excluded.count)
"#,
    )?;

    for (edge, evidence) in edges {
        let resolution_rank = edge_resolution_rank(edge.resolution.as_str());
        edge_stmt.execute(params![
            edge.from_symbol_id, edge.to_symbol_id, edge.edge_type,
            edge.at_file, edge.at_line.map(|v| v as i64),
            edge.confidence, edge.evidence_count as i64,
            edge.resolution, resolution_rank
        ])
        .with_context(|| format!("Failed to batch upsert edge: {} -> {}", edge.from_symbol_id, edge.to_symbol_id))?;

        for ev in evidence {
            ev_stmt.execute(params![
                ev.from_symbol_id, ev.to_symbol_id, ev.edge_type,
                ev.at_file, ev.at_line as i64, ev.count as i64
            ])?;
        }
    }
    Ok(())
}
```

In `src/storage/sqlite/queries/misc.rs`, add after `upsert_usage_example`:

```rust
/// Batch upsert usage examples within an existing transaction or connection.
pub fn batch_upsert_usage_examples(conn: &Connection, examples: &[UsageExampleRow]) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        r#"
INSERT INTO usage_examples(
  to_symbol_id, from_symbol_id, example_type, file_path, line, snippet
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(to_symbol_id, from_symbol_id, example_type, file_path) DO UPDATE SET
  line=excluded.line,
  snippet=excluded.snippet
"#,
    )?;
    for ex in examples {
        stmt.execute(params![
            ex.to_symbol_id, ex.from_symbol_id, ex.example_type,
            ex.file_path, ex.line.map(|v| v as i64), ex.snippet
        ])?;
    }
    Ok(())
}
```

### Step 4: Run tests to verify they pass

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib storage::sqlite::queries::symbols::tests -- --test-threads=1`
Expected: PASS

### Step 5: Commit

```bash
git add src/storage/sqlite/queries/symbols.rs src/storage/sqlite/queries/edges.rs src/storage/sqlite/queries/misc.rs
git commit -m "feat(storage): add batch upsert functions for symbols, edges, usage examples"
```

---

## Task 3: ParsedFile Struct + parse_single_file

**Files:**
- Create: `src/indexer/pipeline/parse.rs`
- Modify: `src/indexer/pipeline/mod.rs` (add `pub mod parse;`)

This is the core of the parse/write separation. `parse_single_file` extracts ALL data from one file and returns it as a `ParsedFile` — zero storage writes.

### Step 1: Write the failing test

Add to `src/indexer/pipeline/parse.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::pool::SqlitePool;
    use std::sync::Arc;

    fn test_config(base_dir: &std::path::Path) -> Arc<Config> {
        // Minimal config for testing — see tests/integration_index_search.rs for pattern
        Arc::new(Config {
            base_dir: Utf8PathBuf::from_path_buf(base_dir.to_path_buf()).unwrap(),
            ..Config::default_for_test(base_dir)
        })
    }

    #[test]
    fn parse_single_file_returns_symbols() {
        let dir = std::env::temp_dir().join(format!("parse-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.rs"), "pub fn hello() { }\nfn private_fn() { }").unwrap();

        let db_path = Utf8PathBuf::from_path_buf(dir.join("test.db")).unwrap();
        let pool = SqlitePool::new(&db_path, 2).unwrap();

        // Initialize schema via pool connection
        {
            let conn = pool.get().unwrap();
            crate::storage::sqlite::schema::init_schema(&conn).unwrap();
        }

        let config = test_config(&dir);
        let conn = pool.get().unwrap();
        let result = parse_single_file(&dir.join("lib.rs"), &config, &conn);

        match result {
            ParseResult::Parsed(parsed) => {
                // Should have file symbol + hello + private_fn
                assert!(parsed.symbol_rows.len() >= 2, "Expected >=2 symbols, got {}", parsed.symbol_rows.len());
                assert!(parsed.symbol_rows.iter().any(|s| s.name == "hello"));
            }
            other => panic!("Expected Parsed, got {:?}", other),
        }
    }

    #[test]
    fn parse_single_file_returns_unchanged_for_same_fingerprint() {
        let dir = std::env::temp_dir().join(format!("parse-unchanged-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.rs"), "pub fn hello() {}").unwrap();

        let db_path = Utf8PathBuf::from_path_buf(dir.join("test.db")).unwrap();
        let pool = SqlitePool::new(&db_path, 2).unwrap();
        {
            let conn = pool.get().unwrap();
            crate::storage::sqlite::schema::init_schema(&conn).unwrap();
        }

        let config = test_config(&dir);

        // First parse — should be Parsed
        let conn = pool.get().unwrap();
        let first = parse_single_file(&dir.join("lib.rs"), &config, &conn);
        assert!(matches!(first, ParseResult::Parsed(_)));

        // Simulate fingerprint being stored (the write phase would do this)
        if let ParseResult::Parsed(ref parsed) = first {
            let fp = &parsed.fingerprint;
            conn.execute(
                "INSERT INTO file_fingerprints(file_path, mtime_ns, size_bytes) VALUES (?1, ?2, ?3)",
                rusqlite::params![parsed.rel_path, fp.mtime_ns, fp.size_bytes],
            ).unwrap();
        }
        drop(conn);

        // Second parse — should be Unchanged
        let conn2 = pool.get().unwrap();
        let second = parse_single_file(&dir.join("lib.rs"), &config, &conn2);
        assert!(matches!(second, ParseResult::Unchanged));
    }
}
```

### Step 2: Run test to verify it fails

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib indexer::pipeline::parse::tests -v -- --test-threads=1`
Expected: FAIL — module doesn't exist

### Step 3: Implement ParsedFile and parse_single_file

Create `src/indexer/pipeline/parse.rs`. The implementation extracts the parsing logic from the existing `index_file_single()` in `parallel.rs:132-419` and `index_files_sequential_internal()` in `mod.rs:674-1072`, but returns data instead of writing.

Key references for the extraction:
- Symbol row construction: `parallel.rs:230-275`
- Import tag extraction: `parallel.rs:219-228`
- Edge extraction: `parallel.rs:309-341`
- Usage examples: `parallel.rs:343-352`
- Framework tags: `parallel.rs:282-291`
- Decorators: `parallel.rs:362-377`
- Framework patterns: `parallel.rs:379-409`
- Test file check: `parallel.rs:411-413`
- Fingerprint check: `parallel.rs:176-181`

The function signature:
```rust
pub fn parse_single_file(
    file: &Path,
    config: &Config,
    conn: &Connection,  // read-only, for fingerprint check + cross-file edge lookups
) -> ParseResult
```

It must NOT call any `upsert_*`, `delete_*`, `commit()`, or write operations.

### Step 4: Run tests

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib indexer::pipeline::parse::tests -v -- --test-threads=1`
Expected: PASS

### Step 5: Commit

```bash
git add src/indexer/pipeline/parse.rs src/indexer/pipeline/mod.rs
git commit -m "feat(indexer): add parse_single_file with ParsedFile struct (no writes)"
```

---

## Task 4: Write Batch Function

**Files:**
- Create: `src/indexer/pipeline/write.rs`
- Modify: `src/indexer/pipeline/mod.rs` (add `pub mod write;`)

### Step 1: Write the failing test

Add to `src/indexer/pipeline/write.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::pool::SqlitePool;
    use crate::storage::tantivy::TantivyIndex;

    #[test]
    fn write_batch_inserts_symbols_and_commits_tantivy() {
        let dir = std::env::temp_dir().join(format!("write-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let db_path = Utf8PathBuf::from_path_buf(dir.join("test.db")).unwrap();
        let pool = SqlitePool::new(&db_path, 2).unwrap();
        {
            let conn = pool.get().unwrap();
            crate::storage::sqlite::schema::init_schema(&conn).unwrap();
        }

        let tantivy = TantivyIndex::open_or_create(dir.join("tantivy")).unwrap();

        let parsed = vec![ParsedFile {
            rel_path: "lib.rs".into(),
            fingerprint: FileFingerprint { mtime_ns: 100, size_bytes: 50 },
            language: "rust".into(),
            symbol_rows: vec![SymbolRow {
                id: "s1".into(), file_path: "lib.rs".into(), language: "rust".into(),
                kind: "function".into(), name: "hello".into(), exported: true,
                start_byte: 0, end_byte: 20, start_line: 1, end_line: 2,
                text: "pub fn hello() {}".into(),
            }],
            edges: vec![],
            usage_examples: vec![],
            import_tags: vec![],
            framework_tags: vec![],
            todos: vec![],
            docstrings: vec![],
            decorators: vec![],
            framework_patterns: vec![],
            is_test_file: false,
        }];

        let conn = pool.get().unwrap();
        let stats = write_batch(&parsed, &conn, &tantivy).unwrap();

        assert_eq!(stats.symbols_written, 1);
        assert_eq!(stats.files_written, 1);

        // Verify SQLite
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Verify Tantivy searchable
        let hits = tantivy.search("hello", 5).unwrap();
        assert!(!hits.is_empty());
    }
}
```

### Step 2: Run test to verify it fails

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib indexer::pipeline::write::tests -v -- --test-threads=1`
Expected: FAIL — module doesn't exist

### Step 3: Implement write_batch

Create `src/indexer/pipeline/write.rs`.

The function processes `&[ParsedFile]` in chunks of 50:

```rust
pub struct WriteStats {
    pub files_written: usize,
    pub symbols_written: usize,
}

pub fn write_batch(
    parsed_files: &[ParsedFile],
    conn: &Connection,
    tantivy: &TantivyIndex,
) -> Result<WriteStats>
```

For each chunk of 50 files:
1. Begin `conn.unchecked_transaction()`
2. For each file in chunk:
   - Call `delete_symbols_by_file`, `delete_usage_examples_by_file`, `delete_todos_by_file`, `delete_docstrings_by_file`, `delete_decorators_by_file`, `delete_framework_patterns_by_file` (existing functions in `queries/`)
   - Call `batch_upsert_symbols` (new from Task 2)
   - Call `batch_upsert_edges` (new from Task 2)
   - Call `batch_upsert_usage_examples` (new from Task 2)
   - Call `batch_upsert_todos`, `batch_upsert_docstrings`, `batch_upsert_decorators`, `batch_upsert_framework_patterns` (existing)
   - Call `create_test_links_for_file` if `is_test_file`
   - Call `upsert_file_fingerprint`
3. `tx.commit()`
4. For each file in chunk: `tantivy.delete_symbols_by_file` + `tantivy.upsert_symbol` per symbol

After ALL chunks: single `tantivy.commit()`

Key: The delete functions in `queries/` take `&Connection` — a `Transaction` Derefs to `Connection`, so pass `&tx` directly.

### Step 4: Run tests

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib indexer::pipeline::write::tests -v -- --test-threads=1`
Expected: PASS

### Step 5: Commit

```bash
git add src/indexer/pipeline/write.rs src/indexer/pipeline/mod.rs
git commit -m "feat(indexer): add write_batch with chunked SQLite txns and single Tantivy commit"
```

---

## Task 5: Auto-tune Default Worker Count

**Files:**
- Modify: `src/config.rs` (change `parallel_workers` default)

### Step 1: Locate and modify the default

In `src/config.rs:324`, change:
```rust
// Before:
parallel_workers: 1,

// After:
parallel_workers: std::thread::available_parallelism()
    .map(|n| n.get().div_ceil(2))
    .unwrap_or(2)
    .max(2),
```

Also update the env var parsing at `src/config.rs:700-706` — the existing `PARALLEL_WORKERS` env var override already works, no change needed there.

### Step 2: Update the test assertion

In `src/config.rs`, find the test that asserts `parallel_workers >= 1` (line ~1376) and update:
```rust
// Before:
assert!(cfg.parallel_workers >= 1);

// After:
assert!(cfg.parallel_workers >= 2);
```

### Step 3: Run tests

Run: `EMBEDDINGS_BACKEND=hash cargo test --lib config -v`
Expected: PASS

### Step 4: Commit

```bash
git add src/config.rs
git commit -m "feat(config): auto-tune parallel_workers to half of available CPUs"
```

---

## Task 6: Wire the Unified Pipeline

**Files:**
- Modify: `src/indexer/pipeline/mod.rs` (replace branch + sequential path with unified pipeline)
- Modify: `src/indexer/pipeline/parallel.rs` (delete old functions, keep as thin module or merge)

This is the integration task. Replace the `if parallel_workers > 1 { ... } else { ... }` branch and the entire `index_files_sequential_internal` method with the unified three-phase pipeline.

### Step 1: Replace the branch in `index_files` method

In `src/indexer/pipeline/mod.rs`, replace lines 604-623 (the parallel/sequential branch) and delete `index_files_sequential_internal` (lines 674-1072) and `index_files_parallel_async` (lines 1074-1104).

The new code at the branch point (~line 604):

```rust
// Unified pipeline: parse → write → embed
let pool = SqlitePool::new(&self.db_path, self.config.parallel_workers + 2)?;
let config = self.config.clone();
let pool_ref = &pool;

// Phase 1: Parse (Rayon, parallel)
let parse_results = {
    let files_clone = uniq.clone();
    let config_clone = config.clone();
    tokio::task::spawn_blocking(move || {
        parse::parse_files(&files_clone, &config_clone, pool_ref)
    }).await??
};

// Tally stats from parse results
let mut parsed_files = Vec::new();
for result in parse_results {
    match result {
        ParseResult::Parsed(pf) => {
            stats.symbols_indexed += pf.symbol_rows.len();
            stats.files_indexed += 1;
            parsed_files.push(pf);
        }
        ParseResult::Unchanged => { stats.files_unchanged += 1; }
        ParseResult::Skipped { .. } => { stats.files_skipped += 1; }
    }
}

// Phase 2: Write (single thread, batched)
if !parsed_files.is_empty() {
    let conn = pool.get()?;
    write::write_batch(&parsed_files, &conn, &self.tantivy)?;
}

// Phase 3: Embed (async, unchanged)
self.generate_embeddings_for_orphaned_symbols().await?;
```

Note: `pool_ref` lifetime across `spawn_blocking` requires the pool to be `'static` or we need `Arc<SqlitePool>`. Use `Arc::new(pool)` and clone into the closure.

### Step 2: Delete old code

- Delete `index_files_sequential_internal` (~lines 674-1072 in mod.rs)
- Delete `index_files_parallel_async` (~lines 1074-1104 in mod.rs)
- Delete `index_file_single`, `index_file_with_retry`, `index_files_parallel` from parallel.rs (the entire file content except any re-exported types)
- Remove the `FileIndexResult`, `IndexFileResult` structs from parallel.rs (replaced by `ParseResult`)

### Step 3: Fix compilation

Run: `EMBEDDINGS_BACKEND=hash cargo check`

Fix any import issues. The main things to watch:
- `SqlitePool` needs to be imported in mod.rs
- `parse::ParseResult`, `parse::ParsedFile` need to be imported
- `write::write_batch` needs to be imported
- Remove unused imports from parallel.rs
- The `parse_files` function needs the `SqlitePool` to be `Send + Sync` (it is — `Mutex` makes it so)

### Step 4: Run existing integration tests

Run: `EMBEDDINGS_BACKEND=hash cargo test --test integration_index_search -- --test-threads=1`
Expected: PASS — existing behavior preserved

### Step 5: Run full test suite

Run: `EMBEDDINGS_BACKEND=hash cargo test -- --test-threads=1`
Expected: All existing tests PASS

### Step 6: Commit

```bash
git add src/indexer/pipeline/mod.rs src/indexer/pipeline/parallel.rs src/indexer/pipeline/parse.rs src/indexer/pipeline/write.rs
git commit -m "feat(indexer): unify sequential/parallel paths into single pipeline

Delete ~875 lines of duplicated indexing code. Replace with
three-phase pipeline: parse (Rayon) → write (batched) → embed.

- Single Tantivy commit (was 2*N per run)
- Chunked SQLite transactions (50 files each)
- SQLite connection pool (was 5*N opens per run)
- Auto-tuned parallel workers (half of CPU cores)"
```

---

## Task 7: Build + Smoke Test

**Files:** None — validation only

### Step 1: Release build

Run: `cargo build --release`
Expected: Clean build, no warnings

### Step 2: Run integration tests

Run: `EMBEDDINGS_BACKEND=hash cargo test -- --test-threads=1`
Expected: All tests pass

### Step 3: Manual smoke test with real repo

Run: `./scripts/test_local.sh`
Expected: Server starts, indexes dummy workspace, searches return results

### Step 4: Benchmark comparison (optional)

If the benchmark script is available:
Run: `python3 scripts/run_benchmark.py --live`
Expected: Results match or improve on R97 baseline (avg ~8.0)

### Step 5: Final commit (if any fixups needed)

Only if Steps 1-4 revealed issues that needed patching.

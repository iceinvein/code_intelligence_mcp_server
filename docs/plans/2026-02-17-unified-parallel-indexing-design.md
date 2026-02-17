# Unified Parallel Indexing Pipeline

**Date:** 2026-02-17
**Status:** Approved
**Goal:** 3-5x indexing speedup for large repos via parse/write separation, batched commits, connection pooling, and auto-tuned parallelism.

## Problem

The indexing pipeline has two code paths (sequential ~400 lines, parallel ~425 lines) that duplicate logic. The default sequential path has critical performance bottlenecks:

- **Tantivy commits per-file** (2x per file — lines 921 + 1068 in mod.rs). Each commit flushes segments to disk and reloads the reader. For 500 files = 1,000 commits.
- **No SQLite connection pooling.** `SqliteStore::open()` called ~5x per file in the sequential loop (lines 709, 723, 797, 924, 1061). Each call creates a new OS file handle, runs ~10 PRAGMAs, and executes full schema init. For 500 files = 2,500 connection cycles.
- **No SQLite write transactions.** Symbol and edge upserts are individual autocommit statements. For 8K symbols + ~20K edges = ~28K individual writes.
- **Single-threaded parsing.** Default `parallel_workers=1` wastes multi-core CPUs during the CPU-bound tree-sitter parsing phase.

## Architecture

Three-phase pipeline connected by a `ParsedFile` data struct:

```
Phase 1: PARSE (Rayon, N workers, read-only)
  files → par_iter → ParsedFile structs → Vec<ParseResult>

Phase 2: WRITE (single thread, batched transactions)
  Vec<ParsedFile> → chunked SQLite txns → batch Tantivy upserts → single commit

Phase 3: EMBED (async, batched — existing code, unchanged)
  orphaned symbols → batch embed (200 at a time) → LanceDB → similarity_clusters
```

### Parse/Write Separation

The core design principle: parsing produces data, writing consumes it. No storage writes during Phase 1.

```rust
/// Full output of parsing one file — everything needed to write.
struct ParsedFile {
    rel_path: String,
    fingerprint: FileFingerprint,
    language: String,
    symbol_rows: Vec<SymbolRow>,
    edges: Vec<(EdgeRow, Vec<EdgeEvidenceRow>)>,
    usage_examples: Vec<UsageExampleRow>,
    import_tags: Vec<String>,
    framework_tags: Vec<String>,
    todos: Vec<TodoRow>,
    docstrings: Vec<DocstringRow>,
    decorators: Vec<DecoratorRow>,
    framework_patterns: Vec<FrameworkPatternRow>,
    is_test_file: bool,
}

enum ParseResult {
    Unchanged,                    // fingerprint matched, skip
    Parsed(ParsedFile),          // full parse result
    Skipped { reason: String },  // unsupported language, read error, etc.
}
```

Edge extraction stays in Phase 1. Each Rayon worker gets a read-only pooled SQLite connection for cross-file lookups (package resolution, symbol-by-name). SQLite WAL supports unlimited concurrent readers.

This separation enables future evolution:
- A → B: Replace `collect()` with a channel for concurrent embedding
- B → C: Split the channel consumer into staged pipeline workers

### SQLite Connection Pool

Simple pool, no external crate (~40 lines):

```rust
pub struct SqlitePool {
    db_path: Utf8PathBuf,
    pool: Mutex<Vec<Connection>>,
    max_size: usize,
}

impl SqlitePool {
    pub fn new(db_path: &Utf8Path, max_size: usize) -> Result<Self>;
    pub fn get(&self) -> Result<PooledConnection<'_>>;
}

/// RAII guard — returns connection to pool on drop
pub struct PooledConnection<'a> {
    conn: Option<Connection>,
    pool: &'a SqlitePool,
}
```

Pool size: `parallel_workers + 2` (covers N parse workers + 1 write thread + 1 embed task). Connections are initialized once (PRAGMAs, schema) and reused. Eliminates ~2,500 connection open/close cycles for 500 files.

### Write Batching

Processes parsed files in chunks of 50:

```rust
for chunk in parsed_files.chunks(50) {
    // SQLite: one transaction per chunk
    let tx = conn.unchecked_transaction()?;
    for file in chunk {
        delete_old_data(&tx, &file.rel_path)?;
        batch_upsert_symbols(&tx, &file.symbol_rows)?;
        batch_upsert_edges(&tx, &file.edges)?;
        batch_upsert_usage_examples(&tx, &file.usage_examples)?;
        // ... todos, docstrings, decorators, framework_patterns, test_links, fingerprint
    }
    tx.commit()?;

    // Tantivy: delete + upsert for chunk, NO commit yet
    for file in chunk {
        tantivy.delete_symbols_by_file(&file.rel_path)?;
        for row in &file.symbol_rows {
            tantivy.upsert_symbol(row, &file.import_tags, &file.framework_tags, None)?;
        }
    }
}

// Single Tantivy commit after ALL chunks
tantivy.commit()?;
```

Chunk size 50 balances WAL growth, memory, and reader blocking. New `batch_upsert_symbols` and `batch_upsert_edges` functions follow the existing pattern used by `batch_upsert_todos`, `batch_upsert_decorators`, etc.

### Auto-tuned Worker Count

Replace hardcoded `parallel_workers: 1` default:

```rust
let default_workers = std::thread::available_parallelism()
    .map(|n| n.get().div_ceil(2))
    .unwrap_or(2)
    .max(2);
```

Typical results: M1 (8 cores) → 4, M2 Pro (12 cores) → 6, M3 Max (16 cores) → 8. Still overridable via `PARALLEL_WORKERS` env var.

## Code Changes

### Deleted (~875 lines)

| Function | File | Lines |
|----------|------|-------|
| `index_files_sequential_internal()` | mod.rs | ~400 |
| `index_file_single()` | parallel.rs | ~300 |
| `index_file_with_retry()` | parallel.rs | ~50 |
| `index_files_parallel()` | parallel.rs | ~75 |
| `index_files_parallel_async()` | mod.rs | ~30 |
| Sequential/parallel branch | mod.rs | ~20 |

### New (~350 lines)

| Component | File | Lines |
|-----------|------|-------|
| `SqlitePool` + `PooledConnection` | storage/sqlite/pool.rs | ~60 |
| `ParsedFile` + `ParseResult` structs | indexer/pipeline/parse.rs | ~40 |
| `parse_single_file()` | indexer/pipeline/parse.rs | ~120 |
| `parse_files()` (Rayon orchestrator) | indexer/pipeline/parse.rs | ~30 |
| `write_batch()` | indexer/pipeline/write.rs | ~80 |
| `batch_upsert_symbols()` | storage/sqlite/queries/symbols.rs | ~15 |
| `batch_upsert_edges()` | storage/sqlite/queries/edges.rs | ~20 |
| Unified `index_files()` in pipeline | indexer/pipeline/mod.rs | ~40 |

### Modified

| File | Change |
|------|--------|
| `config.rs` | Auto-tune default `parallel_workers` |
| `indexer/pipeline/mod.rs` | Replace branch with unified pipeline call |

### Unchanged

- `generate_embeddings_for_orphaned_symbols()` — already batched
- All tree-sitter extractors (`src/indexer/extract/`)
- Edge extraction logic (`edges.rs`) — called from new location
- All existing `batch_upsert_*` functions
- PageRank computation
- File watcher integration

## Expected Impact

| Metric | Before | After |
|--------|--------|-------|
| Tantivy commits per run | 2 * N_files | 1 |
| SQLite connections opened | ~5 * N_files | pool of ~6 |
| SQLite transactions | 0 (autocommit) | N_files / 50 |
| CPU utilization (parsing) | 1 core | N/2 cores |
| Code paths | 2 (sequential + parallel) | 1 unified |
| Lines of indexing code | ~1200 | ~350 |
| Expected speedup | baseline | 3-5x |

## Risks

- **Tantivy single commit:** If the process crashes mid-index, all Tantivy work is lost (vs per-file durability today). Acceptable because: (a) SQLite fingerprints track what was written, (b) re-index is idempotent, (c) the crash window is much shorter with faster indexing.
- **Chunk transaction failure:** If one file in a 50-file chunk has bad data, the whole chunk's transaction rolls back. Mitigation: parse phase validates data; write phase logs and skips individual file errors within the transaction.
- **Pool connection staleness:** Long-lived connections could theoretically hit issues. Mitigation: pool size is small, connections are short-lived (checked out per-file in parse, per-batch in write).

## Future Evolution (B → C path)

This design enables incremental evolution:
- **B (concurrent embedding):** Replace `collect()` after Rayon with a `crossbeam::channel`. A tokio task consumes `ParsedFile`s and runs embedding concurrently with parsing. Write phase waits for both to complete.
- **C (staged pipeline):** Split the channel consumer into separate write and embed stages. Each stage is a dedicated consumer with its own channel input.

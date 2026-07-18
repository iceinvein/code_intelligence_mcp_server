use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Mutex, MutexGuard};

use super::pool::{PooledConnection, SqlitePool};
use super::schema::SCHEMA_SQL;
use crate::path::Utf8Path;

const DEFAULT_READ_POOL_SIZE: usize = 8;

pub enum SqliteReadConnection<'a> {
    Pooled(PooledConnection<'a>),
    Single(MutexGuard<'a, Connection>),
}

impl Deref for SqliteReadConnection<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Pooled(conn) => conn,
            Self::Single(conn) => conn,
        }
    }
}

pub struct SqliteStore {
    // Mutations are serialized deliberately through one writer. File-backed
    // stores use independent WAL reader connections so concurrent requests do
    // not contend on this mutex for SELECTs. In-memory/test connections fall
    // back to the writer because independent `:memory:` connections do not
    // share a database.
    pub(crate) conn: Mutex<Connection>,
    read_pool: Option<SqlitePool>,
}

impl SqliteStore {
    /// Checkout a pooled read connection, or the single test connection for an
    /// in-memory store.
    pub fn read(&self) -> Result<SqliteReadConnection<'_>> {
        if let Some(pool) = &self.read_pool {
            return pool.get().map(SqliteReadConnection::Pooled);
        }
        self.conn
            .lock()
            .map(SqliteReadConnection::Single)
            .map_err(|e| {
                anyhow::anyhow!("Database connection lock is poisoned: {}", e).context(
                    "Connection lock poisoned - indicates a previous panic while holding it",
                )
            })
    }

    /// Acquire exclusive access to the repository's single writer connection.
    pub fn write(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|e| {
            anyhow::anyhow!("Database connection lock is poisoned: {}", e)
                .context("Connection lock poisoned - indicates a previous panic while holding it")
        })
    }
}

impl SqliteStore {
    pub fn open(db_path: &Utf8Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create db parent dir: {}", parent))?;
        }

        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open sqlite db: {}", db_path))?;

        // Enable WAL mode for better concurrent access (optional)
        // Use query_row for PRAGMA journal_mode as it returns a value
        let _ = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
            .ok(); // Silently ignore if WAL fails
                   // Enable foreign key constraints (required for ON DELETE CASCADE to work)
                   // This MUST be set on every connection as it's connection-specific, not database-wide
        match conn.execute("PRAGMA foreign_keys = ON", []) {
            Ok(_) => tracing::debug!("Foreign keys enabled on connection"),
            Err(e) => tracing::error!("Failed to enable foreign keys: {}", e),
        }

        // synchronous and busy_timeout don't return values, use execute
        let _ = conn.execute("PRAGMA synchronous=NORMAL", []).ok();
        let _ = conn.execute("PRAGMA busy_timeout=5000", []).ok();

        Ok(Self {
            conn: Mutex::new(conn),
            read_pool: Some(SqlitePool::new(db_path, DEFAULT_READ_POOL_SIZE)?),
        })
    }

    pub fn from_connection(conn: Connection) -> Self {
        Self {
            conn: Mutex::new(conn),
            read_pool: None,
        }
    }

    /// Create an in-memory SQLite database (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory SQLite")?;
        conn.execute("PRAGMA foreign_keys = ON", [])
            .context("Failed to enable foreign keys on in-memory connection")?;
        Ok(Self {
            conn: Mutex::new(conn),
            read_pool: None,
        })
    }

    pub fn init(&self) -> Result<()> {
        {
            // Exclusive connection access for the schema migrations below.
            let conn = self.write()?;
            conn.execute_batch(SCHEMA_SQL)
                .context("Failed to initialize sqlite schema: execute_batch SCHEMA_SQL")?;

            migrate_add_edges_location_columns(&conn)
                .with_context(|| "Failed to run migration: migrate_add_edges_location_columns")?;
            migrate_add_edges_confidence_column(&conn)
                .with_context(|| "Failed to run migration: migrate_add_edges_confidence_column")?;
            migrate_add_edges_evidence_count_column(&conn).with_context(|| {
                "Failed to run migration: migrate_add_edges_evidence_count_column"
            })?;
            migrate_add_edges_resolution_columns(&conn)
                .with_context(|| "Failed to run migration: migrate_add_edges_resolution_columns")?;
            migrate_add_search_runs_timing_columns(&conn).with_context(|| {
                "Failed to run migration: migrate_add_search_runs_timing_columns"
            })?;
            migrate_add_search_runs_stage_columns(&conn).with_context(|| {
                "Failed to run migration: migrate_add_search_runs_stage_columns"
            })?;
            migrate_add_index_runs_stage_columns(&conn)
                .with_context(|| "Failed to run migration: migrate_add_index_runs_stage_columns")?;
            migrate_external_reference_dedupe_columns(&conn).with_context(|| {
                "Failed to run migration: migrate_external_reference_dedupe_columns"
            })?;
        }
        Ok(())
    }

    pub fn clear_all(&self) -> Result<()> {
        self.write()?
            .execute_batch(
                r#"
DELETE FROM cross_repo_edges;
DELETE FROM external_indexes;
DELETE FROM edges;
DELETE FROM edge_evidence;
DELETE FROM module_bindings;
DELETE FROM index_metadata;
DELETE FROM symbols;
DELETE FROM file_fingerprints;
DELETE FROM usage_examples;
DELETE FROM index_runs;
DELETE FROM search_runs;
DELETE FROM similarity_clusters;
DELETE FROM symbol_metrics;
DELETE FROM query_selections;
DELETE FROM user_file_affinity;
DELETE FROM docstrings;
DELETE FROM packages;
DELETE FROM repositories;
"#,
            )
            .context("Failed to clear sqlite index: execute_batch DELETE FROM all tables")?;
        Ok(())
    }

    /// Batch query file affinity boost scores for multiple file paths
    ///
    /// Wrapper around queries::affinity::batch_get_affinity_boosts
    /// Returns HashMap mapping file_path to affinity_score (0.0-1.0)
    pub fn batch_get_affinity_boosts(&self, file_paths: &[&str]) -> Result<HashMap<String, f32>> {
        let conn = self.read()?;
        super::queries::affinity::batch_get_affinity_boosts(&conn, file_paths)
    }
}

fn migrate_add_edges_location_columns(conn: &Connection) -> Result<()> {
    let _ = conn.execute("ALTER TABLE edges ADD COLUMN at_file TEXT", []);
    let _ = conn.execute("ALTER TABLE edges ADD COLUMN at_line INTEGER", []);
    Ok(())
}

fn migrate_add_edges_confidence_column(conn: &Connection) -> Result<()> {
    let _ = conn.execute(
        "ALTER TABLE edges ADD COLUMN confidence REAL NOT NULL DEFAULT 1.0",
        [],
    );
    Ok(())
}

fn migrate_add_edges_evidence_count_column(conn: &Connection) -> Result<()> {
    let _ = conn.execute(
        "ALTER TABLE edges ADD COLUMN evidence_count INTEGER NOT NULL DEFAULT 1",
        [],
    );
    Ok(())
}

fn migrate_add_edges_resolution_columns(conn: &Connection) -> Result<()> {
    let _ = conn.execute(
        "ALTER TABLE edges ADD COLUMN resolution TEXT NOT NULL DEFAULT 'unknown'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE edges ADD COLUMN resolution_rank INTEGER NOT NULL DEFAULT 0",
        [],
    );
    Ok(())
}

fn migrate_add_search_runs_timing_columns(conn: &Connection) -> Result<()> {
    let _ = conn.execute(
        "ALTER TABLE search_runs ADD COLUMN embedding_ms INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE search_runs ADD COLUMN reranker_ms INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE search_runs ADD COLUMN scoring_ms INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE search_runs ADD COLUMN assembly_ms INTEGER NOT NULL DEFAULT 0",
        [],
    );
    Ok(())
}

fn migrate_add_search_runs_stage_columns(conn: &Connection) -> Result<()> {
    for sql in [
        "ALTER TABLE search_runs ADD COLUMN fusion_ms INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE search_runs ADD COLUMN search_path TEXT NOT NULL DEFAULT 'unknown'",
        "ALTER TABLE search_runs ADD COLUMN cache_status TEXT NOT NULL DEFAULT 'miss'",
        "ALTER TABLE search_runs ADD COLUMN subquery_count INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE search_runs ADD COLUMN keyword_candidates INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE search_runs ADD COLUMN vector_candidates INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE search_runs ADD COLUMN fused_candidates INTEGER NOT NULL DEFAULT 0",
    ] {
        let _ = conn.execute(sql, []);
    }
    Ok(())
}

fn migrate_add_index_runs_stage_columns(conn: &Connection) -> Result<()> {
    for column in [
        "scan_ms",
        "cleanup_ms",
        "parse_ms",
        "sqlite_write_ms",
        "tantivy_ms",
        "binding_ms",
        "edge_ms",
        "embedding_ms",
        "vector_write_ms",
        "pagerank_ms",
        "optimize_ms",
    ] {
        let sql = format!("ALTER TABLE index_runs ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0");
        let _ = conn.execute(&sql, []);
    }
    Ok(())
}

fn migrate_external_reference_dedupe_columns(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "external_references")? {
        return Ok(());
    }

    if !column_exists(conn, "external_references", "dedupe_key")? {
        conn.execute(
            "ALTER TABLE external_references ADD COLUMN dedupe_key TEXT",
            [],
        )
        .context("Failed to add external_references.dedupe_key")?;
    }
    if !column_exists(conn, "external_references", "updated_at")? {
        conn.execute(
            "ALTER TABLE external_references ADD COLUMN updated_at INTEGER",
            [],
        )
        .context("Failed to add external_references.updated_at")?;
    }

    conn.execute_batch(
        r#"
DROP INDEX IF EXISTS idx_external_references_index_dedupe;

UPDATE external_references
SET
  dedupe_key =
    CASE
      WHEN from_external_symbol_id IS NULL THEN 'n;'
      ELSE 's' || length(CAST(from_external_symbol_id AS BLOB)) || ':' || from_external_symbol_id || ';'
    END ||
    CASE
      WHEN to_external_symbol_id IS NULL THEN 'n;'
      ELSE 's' || length(CAST(to_external_symbol_id AS BLOB)) || ':' || to_external_symbol_id || ';'
    END ||
    's' || length(CAST(relationship AS BLOB)) || ':' || relationship || ';' ||
    's' || length(CAST(file_path AS BLOB)) || ':' || file_path || ';' ||
    'u' || line || ';' ||
    CASE
      WHEN column IS NULL THEN 'n;'
      ELSE 'u' || column || ';'
    END ||
    CASE
      WHEN end_line IS NULL THEN 'n;'
      ELSE 'u' || end_line || ';'
    END ||
    CASE
      WHEN end_column IS NULL THEN 'n;'
      ELSE 'u' || end_column || ';'
    END
;

UPDATE external_references
SET updated_at = COALESCE(updated_at, created_at, unixepoch())
WHERE updated_at IS NULL;

UPDATE external_references
SET
  confidence = (
    SELECT MAX(dupe.confidence)
    FROM external_references dupe
    WHERE dupe.external_index_id = external_references.external_index_id
      AND dupe.dedupe_key = external_references.dedupe_key
  ),
  provenance = (
    SELECT dupe.provenance
    FROM external_references dupe
    WHERE dupe.external_index_id = external_references.external_index_id
      AND dupe.dedupe_key = external_references.dedupe_key
    ORDER BY dupe.id DESC
    LIMIT 1
  ),
  metadata_json = (
    SELECT dupe.metadata_json
    FROM external_references dupe
    WHERE dupe.external_index_id = external_references.external_index_id
      AND dupe.dedupe_key = external_references.dedupe_key
    ORDER BY dupe.id DESC
    LIMIT 1
  ),
  updated_at = unixepoch()
WHERE id IN (
  SELECT MIN(id)
  FROM external_references
  GROUP BY external_index_id, dedupe_key
);

DELETE FROM external_references
WHERE id NOT IN (
  SELECT MIN(id)
  FROM external_references
  GROUP BY external_index_id, dedupe_key
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_external_references_index_dedupe
  ON external_references(external_index_id, dedupe_key);
"#,
    )
    .context("Failed to backfill external_references dedupe keys")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Utf8PathBuf;
    use crate::storage::sqlite::SymbolRow;
    use std::sync::{Arc, Barrier};

    fn symbol(id: usize) -> SymbolRow {
        SymbolRow {
            id: format!("symbol-{id}"),
            file_path: format!("src/file_{id}.rs"),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: format!("function_{id}"),
            exported: true,
            start_byte: 0,
            end_byte: 2,
            start_line: 1,
            end_line: 1,
            text: "fn f() {}".to_string(),
        }
    }

    #[test]
    fn pooled_reads_and_single_writer_are_safe_under_concurrency() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = Utf8PathBuf::from_path_buf(temp.path().join("concurrency.db")).unwrap();
        let store = Arc::new(SqliteStore::open(&db_path).unwrap());
        store.init().unwrap();
        store.upsert_symbol(&symbol(0)).unwrap();
        let barrier = Arc::new(Barrier::new(5));

        std::thread::scope(|scope| {
            let writer_store = store.clone();
            let writer_barrier = barrier.clone();
            let writer = scope.spawn(move || {
                writer_barrier.wait();
                for id in 1..100 {
                    writer_store.upsert_symbol(&symbol(id)).unwrap();
                }
            });

            let readers = (0..4)
                .map(|_| {
                    let reader_store = store.clone();
                    let reader_barrier = barrier.clone();
                    scope.spawn(move || {
                        reader_barrier.wait();
                        for _ in 0..100 {
                            assert!(reader_store.count_symbols().unwrap() >= 1);
                            assert!(reader_store.get_symbol_by_id("symbol-0").unwrap().is_some());
                        }
                    })
                })
                .collect::<Vec<_>>();

            writer.join().unwrap();
            for reader in readers {
                reader.join().unwrap();
            }
        });

        assert_eq!(store.count_symbols().unwrap(), 100);
    }
}

fn table_exists(conn: &Connection, table_name: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table_name],
            |row| row.get(0),
        )
        .with_context(|| format!("Failed to check sqlite table existence: table={table_name}"))?;
    Ok(count > 0)
}

fn column_exists(conn: &Connection, table_name: &str, column_name: &str) -> Result<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table_name})"))
        .with_context(|| format!("Failed to prepare PRAGMA table_info({table_name})"))?;
    let mut rows = stmt
        .query([])
        .with_context(|| format!("Failed to query PRAGMA table_info({table_name})"))?;

    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column_name {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod telemetry_migration_tests {
    use super::*;

    #[test]
    fn init_adds_stage_telemetry_to_existing_run_tables() {
        let conn = Connection::open_in_memory().expect("memory sqlite");
        conn.execute_batch(
            r#"
CREATE TABLE index_runs (
  id INTEGER PRIMARY KEY, started_at INTEGER NOT NULL, duration_ms INTEGER NOT NULL,
  files_scanned INTEGER NOT NULL, files_indexed INTEGER NOT NULL, files_skipped INTEGER NOT NULL,
  files_unchanged INTEGER NOT NULL, files_deleted INTEGER NOT NULL, symbols_indexed INTEGER NOT NULL
);
CREATE TABLE search_runs (
  id INTEGER PRIMARY KEY, started_at INTEGER NOT NULL, duration_ms INTEGER NOT NULL,
  keyword_ms INTEGER NOT NULL, vector_ms INTEGER NOT NULL, merge_ms INTEGER NOT NULL,
  query TEXT NOT NULL, query_limit INTEGER NOT NULL, exported_only INTEGER NOT NULL,
  result_count INTEGER NOT NULL
);
"#,
        )
        .expect("legacy run tables");
        let store = SqliteStore::from_connection(conn);
        store.init().expect("migrate schema");
        let conn = store.read().expect("read migrated schema");

        for column in ["scan_ms", "vector_write_ms", "optimize_ms"] {
            assert!(column_exists(&conn, "index_runs", column).unwrap());
        }
        for column in [
            "embedding_ms",
            "fusion_ms",
            "search_path",
            "cache_status",
            "fused_candidates",
        ] {
            assert!(column_exists(&conn, "search_runs", column).unwrap());
        }
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::super::queries::external::{
        ExternalIndexInsert, ExternalReferenceInsert, ExternalSymbolInsert, SymbolMappingInsert,
    };
    use super::super::SymbolRow;
    use super::SqliteStore;
    use rusqlite::Connection;
    use std::sync::Arc;
    use std::thread;

    fn sym(id: &str) -> SymbolRow {
        SymbolRow {
            id: id.into(),
            file_path: "a.rs".into(),
            language: "rust".into(),
            kind: "function".into(),
            name: "foo".into(),
            exported: true,
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            end_line: 3,
            text: "fn foo() {}".into(),
        }
    }

    // Concurrent read-path queries on one shared store must not panic. With a
    // RwLock<Connection>, multiple readers share one rusqlite Connection and
    // double-borrow its internal RefCell ("RefCell already mutably borrowed").
    #[test]
    fn concurrent_reads_do_not_panic() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        store.init().unwrap();
        store.upsert_symbol(&sym("s1")).unwrap();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let s = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                for _ in 0..2000 {
                    let _ = s.search_symbols_by_exact_name("foo", None, 5).unwrap();
                }
            }));
        }
        for h in handles {
            h.join()
                .expect("worker thread panicked (RefCell double-borrow under concurrent reads)");
        }
    }

    #[test]
    fn clear_all_removes_external_index_data() {
        let store = SqliteStore::open_in_memory().unwrap();
        store.init().unwrap();
        store.upsert_symbol(&sym("s1")).unwrap();
        store
            .upsert_external_index(&ExternalIndexInsert {
                id: "idx-rust-analyzer",
                source_kind: "lsp",
                producer: "rust-analyzer",
                language: "rust",
                root_path: "/repo",
                artifact_path: "/repo/.cache/ra.json",
                artifact_hash: "sha256:abc",
                status: "ready",
                diagnostics_json: "{}",
            })
            .unwrap();
        store
            .upsert_external_symbol(&ExternalSymbolInsert {
                id: "ext:target",
                external_index_id: "idx-rust-analyzer",
                external_symbol: "crate::target",
                display_name: "target",
                language: "rust",
                kind: "function",
                file_path: Some("src/lib.rs"),
                start_line: Some(1),
                end_line: Some(3),
                start_byte: Some(0),
                end_byte: Some(20),
                metadata_json: "{}",
            })
            .unwrap();
        store
            .upsert_symbol_mapping(&SymbolMappingInsert {
                external_symbol_id: "ext:target",
                internal_symbol_id: "s1",
                mapping_kind: "exact",
                confidence: 0.99,
            })
            .unwrap();
        store
            .upsert_external_reference(&ExternalReferenceInsert {
                external_index_id: "idx-rust-analyzer",
                from_external_symbol_id: None,
                to_external_symbol_id: Some("ext:target"),
                relationship: "reference",
                file_path: "src/main.rs",
                line: 42,
                column: Some(7),
                end_line: Some(42),
                end_column: Some(13),
                confidence: 0.9,
                provenance: "rust-analyzer",
                metadata_json: "{}",
            })
            .unwrap();

        store.clear_all().unwrap();

        let stats = store.external_index_stats("idx-rust-analyzer").unwrap();
        assert_eq!(stats.symbol_count, 0);
        assert_eq!(stats.reference_count, 0);
        assert_eq!(stats.mapping_count, 0);
    }

    #[test]
    fn init_migrates_old_external_references_table_for_dedupe_upserts() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
PRAGMA foreign_keys = ON;

CREATE TABLE external_indexes (
  id TEXT PRIMARY KEY NOT NULL,
  source_kind TEXT NOT NULL,
  producer TEXT NOT NULL,
  language TEXT NOT NULL,
  root_path TEXT NOT NULL,
  artifact_path TEXT NOT NULL,
  artifact_hash TEXT NOT NULL,
  status TEXT NOT NULL,
  diagnostics_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE external_symbols (
  id TEXT PRIMARY KEY NOT NULL,
  external_index_id TEXT NOT NULL,
  external_symbol TEXT NOT NULL,
  display_name TEXT NOT NULL,
  language TEXT NOT NULL,
  kind TEXT NOT NULL,
  file_path TEXT,
  start_line INTEGER,
  end_line INTEGER,
  start_byte INTEGER,
  end_byte INTEGER,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(external_index_id) REFERENCES external_indexes(id) ON DELETE CASCADE
);

CREATE TABLE external_references (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  external_index_id TEXT NOT NULL,
  from_external_symbol_id TEXT,
  to_external_symbol_id TEXT,
  relationship TEXT NOT NULL,
  file_path TEXT NOT NULL,
  line INTEGER NOT NULL,
  column INTEGER,
  end_line INTEGER,
  end_column INTEGER,
  confidence REAL NOT NULL DEFAULT 1.0,
  provenance TEXT NOT NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(external_index_id) REFERENCES external_indexes(id) ON DELETE CASCADE,
  FOREIGN KEY(from_external_symbol_id) REFERENCES external_symbols(id) ON DELETE SET NULL,
  FOREIGN KEY(to_external_symbol_id) REFERENCES external_symbols(id) ON DELETE SET NULL
);

INSERT INTO external_indexes (
  id, source_kind, producer, language, root_path, artifact_path, artifact_hash, status
)
VALUES (
  'idx-rust-analyzer', 'lsp', 'rust-analyzer', 'rust', '/repo',
  '/repo/.cache/ra.json', 'sha256:abc', 'ready'
);

INSERT INTO external_symbols (
  id, external_index_id, external_symbol, display_name, language, kind,
  file_path, start_line, end_line, start_byte, end_byte, metadata_json
)
VALUES (
  'ext:target', 'idx-rust-analyzer', 'crate::target', 'target', 'rust', 'function',
  'src/lib.rs', 1, 3, 0, 20, '{}'
);

INSERT INTO external_references (
  external_index_id, from_external_symbol_id, to_external_symbol_id, relationship,
  file_path, line, column, end_line, end_column, confidence, provenance, metadata_json
)
VALUES (
  'idx-rust-analyzer', NULL, 'ext:target', 'reference',
  'src/main.rs', 42, NULL, 42, 13, 0.5, 'old-import', '{"pass":1}'
);
"#,
        )
        .unwrap();

        let store = SqliteStore::from_connection(conn);
        store.init().unwrap();
        store.upsert_symbol(&sym("s1")).unwrap();
        store
            .upsert_external_index(&ExternalIndexInsert {
                id: "idx-rust-analyzer",
                source_kind: "lsp",
                producer: "rust-analyzer",
                language: "rust",
                root_path: "/repo",
                artifact_path: "/repo/.cache/ra.json",
                artifact_hash: "sha256:abc",
                status: "ready",
                diagnostics_json: "{}",
            })
            .unwrap();
        store
            .upsert_external_symbol(&ExternalSymbolInsert {
                id: "ext:target",
                external_index_id: "idx-rust-analyzer",
                external_symbol: "crate::target",
                display_name: "target",
                language: "rust",
                kind: "function",
                file_path: Some("src/lib.rs"),
                start_line: Some(1),
                end_line: Some(3),
                start_byte: Some(0),
                end_byte: Some(20),
                metadata_json: "{}",
            })
            .unwrap();
        store
            .upsert_symbol_mapping(&SymbolMappingInsert {
                external_symbol_id: "ext:target",
                internal_symbol_id: "s1",
                mapping_kind: "exact",
                confidence: 0.99,
            })
            .unwrap();

        store
            .upsert_external_reference(&ExternalReferenceInsert {
                external_index_id: "idx-rust-analyzer",
                from_external_symbol_id: None,
                to_external_symbol_id: Some("ext:target"),
                relationship: "reference",
                file_path: "src/main.rs",
                line: 42,
                column: None,
                end_line: Some(42),
                end_column: Some(13),
                confidence: 0.9,
                provenance: "rust-analyzer",
                metadata_json: "{}",
            })
            .unwrap();

        let stats = store.external_index_stats("idx-rust-analyzer").unwrap();
        assert_eq!(stats.reference_count, 1);

        let refs = store
            .list_external_references_to_internal_symbol("s1", Some("reference"), 10)
            .unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].confidence, 0.9);
        assert_eq!(refs[0].provenance, "rust-analyzer");
        assert_eq!(refs[0].metadata_json, "{}");
    }

    #[test]
    fn init_migrates_unicode_external_reference_dedupe_keys_as_utf8_bytes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
PRAGMA foreign_keys = ON;

CREATE TABLE external_indexes (
  id TEXT PRIMARY KEY NOT NULL,
  source_kind TEXT NOT NULL,
  producer TEXT NOT NULL,
  language TEXT NOT NULL,
  root_path TEXT NOT NULL,
  artifact_path TEXT NOT NULL,
  artifact_hash TEXT NOT NULL,
  status TEXT NOT NULL,
  diagnostics_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE external_symbols (
  id TEXT PRIMARY KEY NOT NULL,
  external_index_id TEXT NOT NULL,
  external_symbol TEXT NOT NULL,
  display_name TEXT NOT NULL,
  language TEXT NOT NULL,
  kind TEXT NOT NULL,
  file_path TEXT,
  start_line INTEGER,
  end_line INTEGER,
  start_byte INTEGER,
  end_byte INTEGER,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(external_index_id) REFERENCES external_indexes(id) ON DELETE CASCADE
);

CREATE TABLE external_references (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  external_index_id TEXT NOT NULL,
  from_external_symbol_id TEXT,
  to_external_symbol_id TEXT,
  relationship TEXT NOT NULL,
  file_path TEXT NOT NULL,
  line INTEGER NOT NULL,
  column INTEGER,
  end_line INTEGER,
  end_column INTEGER,
  confidence REAL NOT NULL DEFAULT 1.0,
  provenance TEXT NOT NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(external_index_id) REFERENCES external_indexes(id) ON DELETE CASCADE,
  FOREIGN KEY(from_external_symbol_id) REFERENCES external_symbols(id) ON DELETE SET NULL,
  FOREIGN KEY(to_external_symbol_id) REFERENCES external_symbols(id) ON DELETE SET NULL
);

INSERT INTO external_indexes (
  id, source_kind, producer, language, root_path, artifact_path, artifact_hash, status
)
VALUES (
  'idx-rust-analyzer', 'lsp', 'rust-analyzer', 'rust', '/repo',
  '/repo/.cache/ra.json', 'sha256:abc', 'ready'
);

INSERT INTO external_symbols (
  id, external_index_id, external_symbol, display_name, language, kind,
  file_path, start_line, end_line, start_byte, end_byte, metadata_json
)
VALUES (
  'ext:téarget', 'idx-rust-analyzer', 'crate::téarget', 'téarget', 'rust', 'function',
  'src/lib.rs', 1, 3, 0, 20, '{}'
);

INSERT INTO external_references (
  external_index_id, from_external_symbol_id, to_external_symbol_id, relationship,
  file_path, line, column, end_line, end_column, confidence, provenance, metadata_json
)
VALUES (
  'idx-rust-analyzer', NULL, 'ext:téarget', 'reference',
  'src/é_main.rs', 42, NULL, 42, 13, 0.5, 'old-import', '{"pass":1}'
);
"#,
        )
        .unwrap();

        let store = SqliteStore::from_connection(conn);
        store.init().unwrap();
        store.upsert_symbol(&sym("s1")).unwrap();
        store
            .upsert_symbol_mapping(&SymbolMappingInsert {
                external_symbol_id: "ext:téarget",
                internal_symbol_id: "s1",
                mapping_kind: "exact",
                confidence: 0.99,
            })
            .unwrap();

        store
            .upsert_external_reference(&ExternalReferenceInsert {
                external_index_id: "idx-rust-analyzer",
                from_external_symbol_id: None,
                to_external_symbol_id: Some("ext:téarget"),
                relationship: "reference",
                file_path: "src/é_main.rs",
                line: 42,
                column: None,
                end_line: Some(42),
                end_column: Some(13),
                confidence: 0.9,
                provenance: "rust-analyzer",
                metadata_json: "{}",
            })
            .unwrap();

        let stats = store.external_index_stats("idx-rust-analyzer").unwrap();
        assert_eq!(stats.reference_count, 1);
    }

    #[test]
    fn init_repairs_stale_unicode_external_reference_dedupe_keys() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
PRAGMA foreign_keys = ON;

CREATE TABLE external_indexes (
  id TEXT PRIMARY KEY NOT NULL,
  source_kind TEXT NOT NULL,
  producer TEXT NOT NULL,
  language TEXT NOT NULL,
  root_path TEXT NOT NULL,
  artifact_path TEXT NOT NULL,
  artifact_hash TEXT NOT NULL,
  status TEXT NOT NULL,
  diagnostics_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE external_symbols (
  id TEXT PRIMARY KEY NOT NULL,
  external_index_id TEXT NOT NULL,
  external_symbol TEXT NOT NULL,
  display_name TEXT NOT NULL,
  language TEXT NOT NULL,
  kind TEXT NOT NULL,
  file_path TEXT,
  start_line INTEGER,
  end_line INTEGER,
  start_byte INTEGER,
  end_byte INTEGER,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(external_index_id) REFERENCES external_indexes(id) ON DELETE CASCADE
);

CREATE TABLE external_references (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  external_index_id TEXT NOT NULL,
  from_external_symbol_id TEXT,
  to_external_symbol_id TEXT,
  relationship TEXT NOT NULL,
  file_path TEXT NOT NULL,
  line INTEGER NOT NULL,
  column INTEGER,
  end_line INTEGER,
  end_column INTEGER,
  confidence REAL NOT NULL DEFAULT 1.0,
  provenance TEXT NOT NULL,
  dedupe_key TEXT,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER,
  FOREIGN KEY(external_index_id) REFERENCES external_indexes(id) ON DELETE CASCADE,
  FOREIGN KEY(from_external_symbol_id) REFERENCES external_symbols(id) ON DELETE SET NULL,
  FOREIGN KEY(to_external_symbol_id) REFERENCES external_symbols(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX idx_external_references_index_dedupe
  ON external_references(external_index_id, dedupe_key);

INSERT INTO external_indexes (
  id, source_kind, producer, language, root_path, artifact_path, artifact_hash, status
)
VALUES (
  'idx-rust-analyzer', 'lsp', 'rust-analyzer', 'rust', '/repo',
  '/repo/.cache/ra.json', 'sha256:abc', 'ready'
);

INSERT INTO external_symbols (
  id, external_index_id, external_symbol, display_name, language, kind,
  file_path, start_line, end_line, start_byte, end_byte, metadata_json
)
VALUES (
  'ext:téarget', 'idx-rust-analyzer', 'crate::téarget', 'téarget', 'rust', 'function',
  'src/lib.rs', 1, 3, 0, 20, '{}'
);

INSERT INTO external_references (
  external_index_id, from_external_symbol_id, to_external_symbol_id, relationship,
  file_path, line, column, end_line, end_column, confidence, provenance, dedupe_key, metadata_json
)
VALUES (
  'idx-rust-analyzer', NULL, 'ext:téarget', 'reference',
  'src/é_main.rs', 42, NULL, 42, 13, 0.5, 'old-import',
  'n;s10:ext:téarget;s9:reference;s13:src/é_main.rs;u42;n;u42;u13;',
  '{"pass":1}'
);
"#,
        )
        .unwrap();

        let store = SqliteStore::from_connection(conn);
        store.init().unwrap();
        store.upsert_symbol(&sym("s1")).unwrap();
        store
            .upsert_symbol_mapping(&SymbolMappingInsert {
                external_symbol_id: "ext:téarget",
                internal_symbol_id: "s1",
                mapping_kind: "exact",
                confidence: 0.99,
            })
            .unwrap();

        store
            .upsert_external_reference(&ExternalReferenceInsert {
                external_index_id: "idx-rust-analyzer",
                from_external_symbol_id: None,
                to_external_symbol_id: Some("ext:téarget"),
                relationship: "reference",
                file_path: "src/é_main.rs",
                line: 42,
                column: None,
                end_line: Some(42),
                end_column: Some(13),
                confidence: 0.9,
                provenance: "rust-analyzer",
                metadata_json: "{}",
            })
            .unwrap();

        let stats = store.external_index_stats("idx-rust-analyzer").unwrap();
        assert_eq!(stats.reference_count, 1);
    }

    #[test]
    fn init_repairs_mixed_stale_and_correct_unicode_dedupe_keys_with_unique_index() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
PRAGMA foreign_keys = ON;

CREATE TABLE external_indexes (
  id TEXT PRIMARY KEY NOT NULL,
  source_kind TEXT NOT NULL,
  producer TEXT NOT NULL,
  language TEXT NOT NULL,
  root_path TEXT NOT NULL,
  artifact_path TEXT NOT NULL,
  artifact_hash TEXT NOT NULL,
  status TEXT NOT NULL,
  diagnostics_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE external_symbols (
  id TEXT PRIMARY KEY NOT NULL,
  external_index_id TEXT NOT NULL,
  external_symbol TEXT NOT NULL,
  display_name TEXT NOT NULL,
  language TEXT NOT NULL,
  kind TEXT NOT NULL,
  file_path TEXT,
  start_line INTEGER,
  end_line INTEGER,
  start_byte INTEGER,
  end_byte INTEGER,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(external_index_id) REFERENCES external_indexes(id) ON DELETE CASCADE
);

CREATE TABLE external_references (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  external_index_id TEXT NOT NULL,
  from_external_symbol_id TEXT,
  to_external_symbol_id TEXT,
  relationship TEXT NOT NULL,
  file_path TEXT NOT NULL,
  line INTEGER NOT NULL,
  column INTEGER,
  end_line INTEGER,
  end_column INTEGER,
  confidence REAL NOT NULL DEFAULT 1.0,
  provenance TEXT NOT NULL,
  dedupe_key TEXT,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER,
  FOREIGN KEY(external_index_id) REFERENCES external_indexes(id) ON DELETE CASCADE,
  FOREIGN KEY(from_external_symbol_id) REFERENCES external_symbols(id) ON DELETE SET NULL,
  FOREIGN KEY(to_external_symbol_id) REFERENCES external_symbols(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX idx_external_references_index_dedupe
  ON external_references(external_index_id, dedupe_key);

INSERT INTO external_indexes (
  id, source_kind, producer, language, root_path, artifact_path, artifact_hash, status
)
VALUES (
  'idx-rust-analyzer', 'lsp', 'rust-analyzer', 'rust', '/repo',
  '/repo/.cache/ra.json', 'sha256:abc', 'ready'
);

INSERT INTO external_symbols (
  id, external_index_id, external_symbol, display_name, language, kind,
  file_path, start_line, end_line, start_byte, end_byte, metadata_json
)
VALUES (
  'ext:téarget', 'idx-rust-analyzer', 'crate::téarget', 'téarget', 'rust', 'function',
  'src/lib.rs', 1, 3, 0, 20, '{}'
);

INSERT INTO external_references (
  external_index_id, from_external_symbol_id, to_external_symbol_id, relationship,
  file_path, line, column, end_line, end_column, confidence, provenance, dedupe_key, metadata_json
)
VALUES
(
  'idx-rust-analyzer', NULL, 'ext:téarget', 'reference',
  'src/é_main.rs', 42, NULL, 42, 13, 0.5, 'old-import',
  'n;s10:ext:téarget;s9:reference;s13:src/é_main.rs;u42;n;u42;u13;',
  '{"pass":1}'
),
(
  'idx-rust-analyzer', NULL, 'ext:téarget', 'reference',
  'src/é_main.rs', 42, NULL, 42, 13, 0.8, 'byte-import',
  'n;s11:ext:téarget;s9:reference;s14:src/é_main.rs;u42;n;u42;u13;',
  '{"pass":2}'
);
"#,
        )
        .unwrap();

        let store = SqliteStore::from_connection(conn);
        store.init().unwrap();
        store.upsert_symbol(&sym("s1")).unwrap();
        store
            .upsert_symbol_mapping(&SymbolMappingInsert {
                external_symbol_id: "ext:téarget",
                internal_symbol_id: "s1",
                mapping_kind: "exact",
                confidence: 0.99,
            })
            .unwrap();

        let stats = store.external_index_stats("idx-rust-analyzer").unwrap();
        assert_eq!(stats.reference_count, 1);

        store
            .upsert_external_reference(&ExternalReferenceInsert {
                external_index_id: "idx-rust-analyzer",
                from_external_symbol_id: None,
                to_external_symbol_id: Some("ext:téarget"),
                relationship: "reference",
                file_path: "src/é_main.rs",
                line: 42,
                column: None,
                end_line: Some(42),
                end_column: Some(13),
                confidence: 0.9,
                provenance: "rust-analyzer",
                metadata_json: "{}",
            })
            .unwrap();

        let stats = store.external_index_stats("idx-rust-analyzer").unwrap();
        assert_eq!(stats.reference_count, 1);
    }
}

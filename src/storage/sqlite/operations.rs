use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use super::schema::SCHEMA_SQL;
use crate::path::Utf8Path;

pub struct SqliteStore {
    // A single rusqlite Connection behind a Mutex. Every rusqlite operation --
    // even a SELECT -- mutably borrows the Connection's internal RefCell, so all
    // access must be serialized (Connection is Send but not Sync). A Mutex does
    // that; an RwLock would hand out concurrent read guards, letting two readers
    // double-borrow the RefCell and panic ("RefCell already borrowed"). Because
    // Connection is Send, Mutex<Connection> is Send + Sync, so no unsafe impls.
    pub(crate) conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Acquire exclusive access to the connection.
    ///
    /// Named `read` for historical call-site compatibility, but it takes the
    /// same exclusive lock as `write`: rusqlite needs exclusive access for every
    /// operation, including reads. Returns Result to surface a poisoned lock
    /// (a previous panic while holding it) instead of panicking again.
    pub fn read(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|e| {
            anyhow::anyhow!("Database connection lock is poisoned: {}", e)
                .context("Connection lock poisoned - indicates a previous panic while holding it")
        })
    }

    /// Acquire exclusive access to the connection (alias of `read`; see above).
    pub fn write(&self) -> Result<MutexGuard<'_, Connection>> {
        self.read()
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
        })
    }

    pub fn from_connection(conn: Connection) -> Self {
        Self {
            conn: Mutex::new(conn),
        }
    }

    /// Create an in-memory SQLite database (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory SQLite")?;
        conn.execute("PRAGMA foreign_keys = ON", [])
            .context("Failed to enable foreign keys on in-memory connection")?;
        Ok(Self {
            conn: Mutex::new(conn),
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
        }
        Ok(())
    }

    pub fn clear_all(&self) -> Result<()> {
        self.write()?
            .execute_batch(
                r#"
DELETE FROM cross_repo_edges;
DELETE FROM edges;
DELETE FROM edge_evidence;
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

#[cfg(test)]
mod concurrency_tests {
    use super::super::SymbolRow;
    use super::SqliteStore;
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
}

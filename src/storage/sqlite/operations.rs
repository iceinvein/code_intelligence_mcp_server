use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::schema::SCHEMA_SQL;

pub struct SqliteStore {
    pub(crate) conn: RwLock<Connection>,
}

// SAFETY: rusqlite::Connection is Send but not Sync due to internal RefCell.
// By wrapping it in RwLock, we provide synchronized access, making SqliteStore
// safe to share across threads (Send + Sync).
unsafe impl Send for SqliteStore {}
unsafe impl Sync for SqliteStore {}

impl SqliteStore {
    /// Get read access to the connection
    pub fn read(&self) -> RwLockReadGuard<Connection> {
        self.conn.read().unwrap()
    }

    /// Get write access to the connection
    pub fn write(&self) -> RwLockWriteGuard<Connection> {
        self.conn.write().unwrap()
    }
}

impl SqliteStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create db parent dir: {}", parent.display()))?;
        }

        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open sqlite db: {}", db_path.display()))?;

        // Enable WAL mode for better concurrent access
        // WAL allows multiple readers and one writer
        conn.execute("PRAGMA journal_mode=WAL", [])
            .context("Failed to enable WAL mode")?;
        conn.execute("PRAGMA synchronous=NORMAL", [])
            .context("Failed to set synchronous mode")?;
        conn.execute("PRAGMA busy_timeout=5000", []) // 5 second timeout
            .context("Failed to set busy timeout")?;

        Ok(Self { conn: RwLock::new(conn) })
    }

    pub fn from_connection(conn: Connection) -> Self {
        Self { conn: RwLock::new(conn) }
    }

    pub fn init(&self) -> Result<()> {
        {
            let conn = self.conn.write().unwrap();
            conn.execute_batch(SCHEMA_SQL)
                .context("Failed to initialize sqlite schema")?;

            migrate_add_edges_location_columns(&conn)?;
            migrate_add_edges_confidence_column(&conn)?;
            migrate_add_edges_evidence_count_column(&conn)?;
            migrate_add_edges_resolution_columns(&conn)?;
        }
        Ok(())
    }

    pub fn clear_all(&self) -> Result<()> {
        self.conn
            .write()
            .unwrap()
            .execute_batch(
                r#"
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
            .context("Failed to clear sqlite index")?;
        Ok(())
    }

    /// Batch query file affinity boost scores for multiple file paths
    ///
    /// Wrapper around queries::affinity::batch_get_affinity_boosts
    /// Returns HashMap mapping file_path to affinity_score (0.0-1.0)
    pub fn batch_get_affinity_boosts(
        &self,
        file_paths: &[&str],
    ) -> Result<HashMap<String, f32>> {
        super::queries::affinity::batch_get_affinity_boosts(&self.read(), file_paths)
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

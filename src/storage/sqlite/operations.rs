use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::path::Utf8Path;
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
    ///
    /// Returns Result to handle poisoned RwLock gracefully instead of panicking.
    /// A poisoned lock indicates a previous panic while holding the lock, which
    /// suggests corrupt state - this error surfaces that condition for handling.
    pub fn read(&self) -> Result<RwLockReadGuard<'_, Connection>> {
        self.conn.read().map_err(|e| {
            anyhow::anyhow!("RwLock read lock is poisoned: {}", e)
                .context("Database connection lock poisoned - indicates a previous panic while holding read lock")
        })
    }

    /// Get write access to the connection
    ///
    /// Returns Result to handle poisoned RwLock gracefully instead of panicking.
    /// A poisoned lock indicates a previous panic while holding the lock, which
    /// suggests corrupt state - this error surfaces that condition for handling.
    pub fn write(&self) -> Result<RwLockWriteGuard<'_, Connection>> {
        self.conn.write().map_err(|e| {
            anyhow::anyhow!("RwLock write lock is poisoned: {}", e)
                .context("Database connection lock poisoned - indicates a previous panic while holding write lock")
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
            conn: RwLock::new(conn),
        })
    }

    pub fn from_connection(conn: Connection) -> Self {
        Self {
            conn: RwLock::new(conn),
        }
    }

    pub fn init(&self) -> Result<()> {
        {
            // Write lock needed for migration functions that modify schema
            #[allow(clippy::readonly_write_lock)]
            let conn = self.write()?;
            conn.execute_batch(SCHEMA_SQL)
                .context("Failed to initialize sqlite schema: execute_batch SCHEMA_SQL")?;

            migrate_add_edges_location_columns(&conn)
                .with_context(|| "Failed to run migration: migrate_add_edges_location_columns")?;
            migrate_add_edges_confidence_column(&conn)
                .with_context(|| "Failed to run migration: migrate_add_edges_confidence_column")?;
            migrate_add_edges_evidence_count_column(&conn)
                .with_context(|| "Failed to run migration: migrate_add_edges_evidence_count_column")?;
            migrate_add_edges_resolution_columns(&conn)
                .with_context(|| "Failed to run migration: migrate_add_edges_resolution_columns")?;
        }
        Ok(())
    }

    pub fn clear_all(&self) -> Result<()> {
        self.write()?
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

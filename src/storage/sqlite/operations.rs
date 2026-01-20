use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use super::schema::SCHEMA_SQL;

pub struct SqliteStore {
    pub(crate) conn: Connection,
}

impl SqliteStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create db parent dir: {}", parent.display()))?;
        }

        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open sqlite db: {}", db_path.display()))?;
        Ok(Self { conn })
    }

    pub fn from_connection(conn: Connection) -> Self {
        Self { conn }
    }

    pub fn init(&self) -> Result<()> {
        self.conn
            .execute_batch(SCHEMA_SQL)
            .context("Failed to initialize sqlite schema")?;

        migrate_add_edges_location_columns(&self.conn)?;
        migrate_add_edges_confidence_column(&self.conn)?;
        migrate_add_edges_evidence_count_column(&self.conn)?;
        migrate_add_edges_resolution_columns(&self.conn)?;
        Ok(())
    }

    pub fn clear_all(&self) -> Result<()> {
        self.conn
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
"#,
            )
            .context("Failed to clear sqlite index")?;
        Ok(())
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

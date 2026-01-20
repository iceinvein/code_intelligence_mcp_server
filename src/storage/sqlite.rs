use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRow {
    pub id: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub name: String,
    pub exported: bool,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeRow {
    pub from_symbol_id: String,
    pub to_symbol_id: String,
    pub edge_type: String,
    pub at_file: Option<String>,
    pub at_line: Option<u32>,
    pub confidence: f32,
    pub evidence_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeEvidenceRow {
    pub from_symbol_id: String,
    pub to_symbol_id: String,
    pub edge_type: String,
    pub at_file: String,
    pub at_line: u32,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolHeaderRow {
    pub id: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub name: String,
    pub exported: bool,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFingerprintRow {
    pub file_path: String,
    pub mtime_ns: i64,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageExampleRow {
    pub to_symbol_id: String,
    pub from_symbol_id: Option<String>,
    pub example_type: String,
    pub file_path: String,
    pub line: Option<u32>,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexRunRow {
    pub started_at_unix_s: i64,
    pub duration_ms: u64,
    pub files_scanned: u64,
    pub files_indexed: u64,
    pub files_skipped: u64,
    pub files_unchanged: u64,
    pub files_deleted: u64,
    pub symbols_indexed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRunRow {
    pub started_at_unix_s: i64,
    pub duration_ms: u64,
    pub keyword_ms: u64,
    pub vector_ms: u64,
    pub merge_ms: u64,
    pub query: String,
    pub query_limit: u64,
    pub exported_only: bool,
    pub result_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimilarityClusterRow {
    pub symbol_id: String,
    pub cluster_key: String,
}

pub struct SqliteStore {
    conn: Connection,
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
            .execute_batch(
                r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS symbols (
  id TEXT PRIMARY KEY NOT NULL,
  file_path TEXT NOT NULL,
  language TEXT NOT NULL,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  exported INTEGER NOT NULL,
  start_byte INTEGER NOT NULL,
  end_byte INTEGER NOT NULL,
  start_line INTEGER NOT NULL,
  end_line INTEGER NOT NULL,
  text TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_symbols_file_path ON symbols(file_path);
CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
CREATE INDEX IF NOT EXISTS idx_symbols_exported ON symbols(exported);

CREATE TABLE IF NOT EXISTS edges (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  from_symbol_id TEXT NOT NULL,
  to_symbol_id TEXT NOT NULL,
  edge_type TEXT NOT NULL,
  at_file TEXT,
  at_line INTEGER,
  confidence REAL NOT NULL DEFAULT 1.0,
  evidence_count INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE(from_symbol_id, to_symbol_id, edge_type),
  FOREIGN KEY(from_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE,
  FOREIGN KEY(to_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_edges_from ON edges(from_symbol_id);
CREATE INDEX IF NOT EXISTS idx_edges_to ON edges(to_symbol_id);
CREATE INDEX IF NOT EXISTS idx_edges_type ON edges(edge_type);

CREATE TABLE IF NOT EXISTS edge_evidence (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  from_symbol_id TEXT NOT NULL,
  to_symbol_id TEXT NOT NULL,
  edge_type TEXT NOT NULL,
  at_file TEXT NOT NULL,
  at_line INTEGER NOT NULL,
  count INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE(from_symbol_id, to_symbol_id, edge_type, at_file, at_line),
  FOREIGN KEY(from_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE,
  FOREIGN KEY(to_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_edge_evidence_from ON edge_evidence(from_symbol_id);
CREATE INDEX IF NOT EXISTS idx_edge_evidence_to ON edge_evidence(to_symbol_id);
CREATE INDEX IF NOT EXISTS idx_edge_evidence_type ON edge_evidence(edge_type);
CREATE INDEX IF NOT EXISTS idx_edge_evidence_loc ON edge_evidence(at_file, at_line);

CREATE TABLE IF NOT EXISTS file_fingerprints (
  file_path TEXT PRIMARY KEY NOT NULL,
  mtime_ns INTEGER NOT NULL,
  size_bytes INTEGER NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_file_fingerprints_updated_at ON file_fingerprints(updated_at);

CREATE TABLE IF NOT EXISTS usage_examples (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  to_symbol_id TEXT NOT NULL,
  from_symbol_id TEXT,
  example_type TEXT NOT NULL,
  file_path TEXT NOT NULL,
  line INTEGER,
  snippet TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE(to_symbol_id, example_type, file_path, line, snippet),
  FOREIGN KEY(to_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE,
  FOREIGN KEY(from_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_usage_examples_to ON usage_examples(to_symbol_id);
CREATE INDEX IF NOT EXISTS idx_usage_examples_file ON usage_examples(file_path);

CREATE TABLE IF NOT EXISTS index_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at INTEGER NOT NULL,
  duration_ms INTEGER NOT NULL,
  files_scanned INTEGER NOT NULL,
  files_indexed INTEGER NOT NULL,
  files_skipped INTEGER NOT NULL,
  files_unchanged INTEGER NOT NULL,
  files_deleted INTEGER NOT NULL,
  symbols_indexed INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_index_runs_started_at ON index_runs(started_at);

CREATE TABLE IF NOT EXISTS search_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at INTEGER NOT NULL,
  duration_ms INTEGER NOT NULL,
  keyword_ms INTEGER NOT NULL,
  vector_ms INTEGER NOT NULL,
  merge_ms INTEGER NOT NULL,
  query TEXT NOT NULL,
  query_limit INTEGER NOT NULL,
  exported_only INTEGER NOT NULL,
  result_count INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_search_runs_started_at ON search_runs(started_at);

CREATE TABLE IF NOT EXISTS similarity_clusters (
  symbol_id TEXT PRIMARY KEY NOT NULL,
  cluster_key TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_similarity_clusters_key ON similarity_clusters(cluster_key);
"#,
            )
            .context("Failed to initialize sqlite schema")?;

        migrate_add_edges_location_columns(&self.conn)?;
        migrate_add_edges_confidence_column(&self.conn)?;
        migrate_add_edges_evidence_count_column(&self.conn)?;
        Ok(())
    }

    pub fn upsert_symbol(&self, symbol: &SymbolRow) -> Result<()> {
        self.conn
            .execute(
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
                params![
                    symbol.id,
                    symbol.file_path,
                    symbol.language,
                    symbol.kind,
                    symbol.name,
                    if symbol.exported { 1 } else { 0 },
                    symbol.start_byte,
                    symbol.end_byte,
                    symbol.start_line,
                    symbol.end_line,
                    symbol.text
                ],
            )
            .context("Failed to upsert symbol")?;
        Ok(())
    }

    pub fn upsert_edge(&self, edge: &EdgeRow) -> Result<()> {
        self.conn
            .execute(
                r#"
INSERT INTO edges(from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
ON CONFLICT(from_symbol_id, to_symbol_id, edge_type) DO UPDATE SET
  at_file=COALESCE(edges.at_file, excluded.at_file),
  at_line=COALESCE(edges.at_line, excluded.at_line),
  confidence=MAX(edges.confidence, excluded.confidence),
  evidence_count=MAX(edges.evidence_count, excluded.evidence_count)
"#,
                params![
                    edge.from_symbol_id,
                    edge.to_symbol_id,
                    edge.edge_type,
                    edge.at_file,
                    edge.at_line.map(|v| v as i64),
                    edge.confidence,
                    edge.evidence_count as i64
                ],
            )
            .context("Failed to upsert edge")?;
        Ok(())
    }

    pub fn upsert_edge_evidence(&self, evidence: &EdgeEvidenceRow) -> Result<()> {
        self.conn
            .execute(
                r#"
INSERT INTO edge_evidence(from_symbol_id, to_symbol_id, edge_type, at_file, at_line, count)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(from_symbol_id, to_symbol_id, edge_type, at_file, at_line) DO UPDATE SET
  count=MAX(edge_evidence.count, excluded.count)
"#,
                params![
                    evidence.from_symbol_id,
                    evidence.to_symbol_id,
                    evidence.edge_type,
                    evidence.at_file,
                    evidence.at_line as i64,
                    evidence.count as i64
                ],
            )
            .context("Failed to upsert edge evidence")?;
        Ok(())
    }

    pub fn list_edge_evidence(
        &self,
        from_symbol_id: &str,
        to_symbol_id: &str,
        edge_type: &str,
        limit: usize,
    ) -> Result<Vec<EdgeEvidenceRow>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT
  from_symbol_id, to_symbol_id, edge_type, at_file, at_line, count
FROM edge_evidence
WHERE from_symbol_id = ?1 AND to_symbol_id = ?2 AND edge_type = ?3
ORDER BY count DESC, at_file ASC, at_line ASC, id ASC
LIMIT ?4
"#,
            )
            .context("Failed to prepare list_edge_evidence")?;

        let mut rows = stmt.query(params![
            from_symbol_id,
            to_symbol_id,
            edge_type,
            limit as i64
        ])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(EdgeEvidenceRow {
                from_symbol_id: row.get(0)?,
                to_symbol_id: row.get(1)?,
                edge_type: row.get(2)?,
                at_file: row.get(3)?,
                at_line: u32::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                count: u32::try_from(row.get::<_, i64>(5)?).unwrap_or(1),
            });
        }
        Ok(out)
    }

    pub fn list_edges_from(&self, from_symbol_id: &str, limit: usize) -> Result<Vec<EdgeRow>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT
  from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count
FROM edges
WHERE from_symbol_id = ?1
ORDER BY edge_type ASC, to_symbol_id ASC
LIMIT ?2
"#,
            )
            .context("Failed to prepare list_edges_from")?;

        let mut rows = stmt.query(params![from_symbol_id, limit as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(EdgeRow {
                from_symbol_id: row.get(0)?,
                to_symbol_id: row.get(1)?,
                edge_type: row.get(2)?,
                at_file: row.get(3)?,
                at_line: row
                    .get::<_, Option<i64>>(4)?
                    .and_then(|v| u32::try_from(v).ok()),
                confidence: row.get::<_, f64>(5)? as f32,
                evidence_count: u32::try_from(row.get::<_, i64>(6)?).unwrap_or(1),
            });
        }
        Ok(out)
    }

    pub fn list_edges_to(&self, to_symbol_id: &str, limit: usize) -> Result<Vec<EdgeRow>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT
  from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count
FROM edges
WHERE to_symbol_id = ?1
ORDER BY edge_type ASC, from_symbol_id ASC
LIMIT ?2
"#,
            )
            .context("Failed to prepare list_edges_to")?;

        let mut rows = stmt.query(params![to_symbol_id, limit as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(EdgeRow {
                from_symbol_id: row.get(0)?,
                to_symbol_id: row.get(1)?,
                edge_type: row.get(2)?,
                at_file: row.get(3)?,
                at_line: row
                    .get::<_, Option<i64>>(4)?
                    .and_then(|v| u32::try_from(v).ok()),
                confidence: row.get::<_, f64>(5)? as f32,
                evidence_count: u32::try_from(row.get::<_, i64>(6)?).unwrap_or(1),
            });
        }
        Ok(out)
    }

    pub fn count_incoming_edges(&self, to_symbol_id: &str) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE to_symbol_id = ?1",
                params![to_symbol_id],
                |row| row.get(0),
            )
            .context("Failed to count incoming edges")?;
        Ok(count.max(0) as u64)
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

    pub fn upsert_similarity_cluster(&self, row: &SimilarityClusterRow) -> Result<()> {
        self.conn
            .execute(
                r#"
INSERT INTO similarity_clusters(symbol_id, cluster_key, updated_at)
VALUES (?1, ?2, unixepoch())
ON CONFLICT(symbol_id) DO UPDATE SET
  cluster_key=excluded.cluster_key,
  updated_at=unixepoch()
"#,
                params![row.symbol_id, row.cluster_key],
            )
            .context("Failed to upsert similarity cluster")?;
        Ok(())
    }

    pub fn get_similarity_cluster_key(&self, symbol_id: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT cluster_key FROM similarity_clusters WHERE symbol_id = ?1",
                params![symbol_id],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query similarity cluster key")
    }

    pub fn list_symbols_in_cluster(
        &self,
        cluster_key: &str,
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT s.id, s.name
FROM similarity_clusters c
JOIN symbols s ON s.id = c.symbol_id
WHERE c.cluster_key = ?1
ORDER BY s.name ASC, s.file_path ASC, s.kind ASC, s.id ASC
LIMIT ?2
"#,
            )
            .context("Failed to prepare list_symbols_in_cluster")?;
        let mut rows = stmt.query(params![cluster_key, limit as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?));
        }
        Ok(out)
    }

    pub fn delete_symbols_by_file(&self, file_path: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM symbols WHERE file_path = ?1",
                params![file_path],
            )
            .with_context(|| format!("Failed to delete symbols for file: {file_path}"))?;
        Ok(())
    }

    pub fn delete_usage_examples_by_file(&self, file_path: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM usage_examples WHERE file_path = ?1",
                params![file_path],
            )
            .with_context(|| format!("Failed to delete usage examples for file: {file_path}"))?;
        Ok(())
    }

    pub fn upsert_usage_example(&self, example: &UsageExampleRow) -> Result<()> {
        self.conn
            .execute(
                r#"
INSERT INTO usage_examples(
  to_symbol_id, from_symbol_id, example_type, file_path, line, snippet
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(to_symbol_id, example_type, file_path, line, snippet) DO NOTHING
"#,
                params![
                    example.to_symbol_id,
                    example.from_symbol_id,
                    example.example_type,
                    example.file_path,
                    example.line.map(|v| v as i64),
                    example.snippet
                ],
            )
            .context("Failed to upsert usage example")?;
        Ok(())
    }

    pub fn list_usage_examples_for_symbol(
        &self,
        to_symbol_id: &str,
        limit: usize,
    ) -> Result<Vec<UsageExampleRow>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT
  to_symbol_id, from_symbol_id, example_type, file_path, line, snippet
FROM usage_examples
WHERE to_symbol_id = ?1
ORDER BY example_type ASC, file_path ASC, line ASC
LIMIT ?2
"#,
            )
            .context("Failed to prepare list_usage_examples_for_symbol")?;

        let mut rows = stmt.query(params![to_symbol_id, limit as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(UsageExampleRow {
                to_symbol_id: row.get(0)?,
                from_symbol_id: row.get(1)?,
                example_type: row.get(2)?,
                file_path: row.get(3)?,
                line: row
                    .get::<_, Option<i64>>(4)?
                    .and_then(|v| u32::try_from(v).ok()),
                snippet: row.get(5)?,
            });
        }
        Ok(out)
    }

    pub fn insert_index_run(&self, run: &IndexRunRow) -> Result<()> {
        self.conn
            .execute(
                r#"
INSERT INTO index_runs(
  started_at, duration_ms, files_scanned, files_indexed, files_skipped, files_unchanged,
  files_deleted, symbols_indexed
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
"#,
                params![
                    run.started_at_unix_s,
                    run.duration_ms as i64,
                    run.files_scanned as i64,
                    run.files_indexed as i64,
                    run.files_skipped as i64,
                    run.files_unchanged as i64,
                    run.files_deleted as i64,
                    run.symbols_indexed as i64
                ],
            )
            .context("Failed to insert index run")?;
        Ok(())
    }

    pub fn insert_search_run(&self, run: &SearchRunRow) -> Result<()> {
        self.conn
            .execute(
                r#"
INSERT INTO search_runs(
  started_at, duration_ms, keyword_ms, vector_ms, merge_ms, query, query_limit, exported_only, result_count
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
"#,
                params![
                    run.started_at_unix_s,
                    run.duration_ms as i64,
                    run.keyword_ms as i64,
                    run.vector_ms as i64,
                    run.merge_ms as i64,
                    run.query,
                    run.query_limit as i64,
                    if run.exported_only { 1 } else { 0 },
                    run.result_count as i64
                ],
            )
            .context("Failed to insert search run")?;
        Ok(())
    }

    pub fn latest_index_run(&self) -> Result<Option<IndexRunRow>> {
        self.conn
            .query_row(
                r#"
SELECT
  started_at, duration_ms, files_scanned, files_indexed, files_skipped, files_unchanged,
  files_deleted, symbols_indexed
FROM index_runs
ORDER BY started_at DESC, id DESC
LIMIT 1
"#,
                [],
                |row| {
                    Ok(IndexRunRow {
                        started_at_unix_s: row.get(0)?,
                        duration_ms: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                        files_scanned: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                        files_indexed: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                        files_skipped: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                        files_unchanged: u64::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
                        files_deleted: u64::try_from(row.get::<_, i64>(6)?).unwrap_or(0),
                        symbols_indexed: u64::try_from(row.get::<_, i64>(7)?).unwrap_or(0),
                    })
                },
            )
            .optional()
            .context("Failed to query latest index run")
    }

    pub fn latest_search_run(&self) -> Result<Option<SearchRunRow>> {
        self.conn
            .query_row(
                r#"
SELECT
  started_at, duration_ms, keyword_ms, vector_ms, merge_ms, query, query_limit, exported_only, result_count
FROM search_runs
ORDER BY started_at DESC, id DESC
LIMIT 1
"#,
                [],
                |row| {
                    let exported_only: i64 = row.get(7)?;
                    Ok(SearchRunRow {
                        started_at_unix_s: row.get(0)?,
                        duration_ms: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                        keyword_ms: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                        vector_ms: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                        merge_ms: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                        query: row.get(5)?,
                        query_limit: u64::try_from(row.get::<_, i64>(6)?).unwrap_or(0),
                        exported_only: exported_only != 0,
                        result_count: u64::try_from(row.get::<_, i64>(8)?).unwrap_or(0),
                    })
                },
            )
            .optional()
            .context("Failed to query latest search run")
    }

    pub fn get_file_fingerprint(&self, file_path: &str) -> Result<Option<FileFingerprintRow>> {
        self.conn
            .query_row(
                r#"
SELECT file_path, mtime_ns, size_bytes
FROM file_fingerprints
WHERE file_path = ?1
"#,
                params![file_path],
                |row| {
                    Ok(FileFingerprintRow {
                        file_path: row.get(0)?,
                        mtime_ns: row.get(1)?,
                        size_bytes: row.get::<_, i64>(2)?.max(0) as u64,
                    })
                },
            )
            .optional()
            .context("Failed to query file fingerprint")
    }

    pub fn upsert_file_fingerprint(
        &self,
        file_path: &str,
        mtime_ns: i64,
        size_bytes: u64,
    ) -> Result<()> {
        self.conn
            .execute(
                r#"
INSERT INTO file_fingerprints(file_path, mtime_ns, size_bytes, updated_at)
VALUES (?1, ?2, ?3, unixepoch())
ON CONFLICT(file_path) DO UPDATE SET
  mtime_ns=excluded.mtime_ns,
  size_bytes=excluded.size_bytes,
  updated_at=unixepoch()
"#,
                params![file_path, mtime_ns, size_bytes as i64],
            )
            .context("Failed to upsert file fingerprint")?;
        Ok(())
    }

    pub fn delete_file_fingerprint(&self, file_path: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM file_fingerprints WHERE file_path = ?1",
                params![file_path],
            )
            .with_context(|| format!("Failed to delete file fingerprint for {file_path}"))?;
        Ok(())
    }

    pub fn list_all_file_fingerprints(&self, limit: usize) -> Result<Vec<FileFingerprintRow>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT file_path, mtime_ns, size_bytes
FROM file_fingerprints
ORDER BY file_path ASC
LIMIT ?1
"#,
            )
            .context("Failed to prepare list_all_file_fingerprints")?;

        let mut rows = stmt.query(params![limit as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(FileFingerprintRow {
                file_path: row.get(0)?,
                mtime_ns: row.get(1)?,
                size_bytes: row.get::<_, i64>(2)?.max(0) as u64,
            });
        }
        Ok(out)
    }

    pub fn count_symbols(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
            .context("Failed to count symbols")?;
        Ok(count.max(0) as u64)
    }

    pub fn count_edges(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))
            .context("Failed to count edges")?;
        Ok(count.max(0) as u64)
    }

    pub fn most_recent_symbol_update(&self) -> Result<Option<i64>> {
        let ts: Option<i64> = self
            .conn
            .query_row("SELECT MAX(updated_at) FROM symbols", [], |row| row.get(0))
            .optional()
            .context("Failed to query most recent symbol update")?
            .flatten();
        Ok(ts)
    }

    pub fn search_symbols_by_exact_name(
        &self,
        name: &str,
        file_path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let mut out = Vec::new();

        match file_path {
            Some(fp) => {
                let mut stmt = self
                    .conn
                    .prepare(
                        r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE name = ?1 AND file_path = ?2
ORDER BY exported DESC, start_byte ASC
LIMIT ?3
"#,
                    )
                    .context("Failed to prepare search_symbols_by_exact_name (file)")?;
                let mut rows = stmt.query(params![name, fp, limit as i64])?;
                while let Some(row) = rows.next()? {
                    out.push(SymbolRow {
                        id: row.get(0)?,
                        file_path: row.get(1)?,
                        language: row.get(2)?,
                        kind: row.get(3)?,
                        name: row.get(4)?,
                        exported: row.get::<_, i64>(5)? != 0,
                        start_byte: row.get::<_, i64>(6)? as u32,
                        end_byte: row.get::<_, i64>(7)? as u32,
                        start_line: row.get::<_, i64>(8)? as u32,
                        end_line: row.get::<_, i64>(9)? as u32,
                        text: row.get(10)?,
                    });
                }
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare(
                        r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE name = ?1
ORDER BY exported DESC, file_path ASC, start_byte ASC
LIMIT ?2
"#,
                    )
                    .context("Failed to prepare search_symbols_by_exact_name")?;
                let mut rows = stmt.query(params![name, limit as i64])?;
                while let Some(row) = rows.next()? {
                    out.push(SymbolRow {
                        id: row.get(0)?,
                        file_path: row.get(1)?,
                        language: row.get(2)?,
                        kind: row.get(3)?,
                        name: row.get(4)?,
                        exported: row.get::<_, i64>(5)? != 0,
                        start_byte: row.get::<_, i64>(6)? as u32,
                        end_byte: row.get::<_, i64>(7)? as u32,
                        start_line: row.get::<_, i64>(8)? as u32,
                        end_line: row.get::<_, i64>(9)? as u32,
                        text: row.get(10)?,
                    });
                }
            }
        }

        Ok(out)
    }

    pub fn search_symbols_by_text_substr(
        &self,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE instr(text, ?1) > 0
ORDER BY exported DESC, file_path ASC, start_byte ASC
LIMIT ?2
"#,
            )
            .context("Failed to prepare search_symbols_by_text_substr")?;

        let mut rows = stmt.query(params![needle, limit as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SymbolRow {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                name: row.get(4)?,
                exported: row.get::<_, i64>(5)? != 0,
                start_byte: row.get::<_, i64>(6)? as u32,
                end_byte: row.get::<_, i64>(7)? as u32,
                start_line: row.get::<_, i64>(8)? as u32,
                end_line: row.get::<_, i64>(9)? as u32,
                text: row.get(10)?,
            });
        }
        Ok(out)
    }

    pub fn get_symbol_by_id(&self, id: &str) -> Result<Option<SymbolRow>> {
        self.conn
            .query_row(
                r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE id = ?1
"#,
                params![id],
                |row| {
                    Ok(SymbolRow {
                        id: row.get(0)?,
                        file_path: row.get(1)?,
                        language: row.get(2)?,
                        kind: row.get(3)?,
                        name: row.get(4)?,
                        exported: row.get::<_, i64>(5)? != 0,
                        start_byte: row.get::<_, i64>(6)? as u32,
                        end_byte: row.get::<_, i64>(7)? as u32,
                        start_line: row.get::<_, i64>(8)? as u32,
                        end_line: row.get::<_, i64>(9)? as u32,
                        text: row.get(10)?,
                    })
                },
            )
            .optional()
            .context("Failed to query symbol by id")
    }

    pub fn list_symbol_headers_by_file(
        &self,
        file_path: &str,
        exported_only: bool,
    ) -> Result<Vec<SymbolHeaderRow>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line
FROM symbols
WHERE file_path = ?1 AND (?2 = 0 OR exported = 1)
ORDER BY start_byte ASC
"#,
            )
            .context("Failed to prepare list_symbol_headers_by_file")?;

        let mut rows = stmt.query(params![file_path, if exported_only { 1 } else { 0 }])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SymbolHeaderRow {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                name: row.get(4)?,
                exported: row.get::<_, i64>(5)? != 0,
                start_byte: row.get::<_, i64>(6)? as u32,
                end_byte: row.get::<_, i64>(7)? as u32,
                start_line: row.get::<_, i64>(8)? as u32,
                end_line: row.get::<_, i64>(9)? as u32,
            });
        }
        Ok(out)
    }

    pub fn list_symbol_id_name_pairs(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM symbols ORDER BY name ASC")
            .context("Failed to prepare list_symbol_id_name_pairs")?;

        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?));
        }
        Ok(out)
    }

    pub fn list_symbols_by_file(&self, file_path: &str) -> Result<Vec<SymbolRow>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE file_path = ?1
ORDER BY start_byte ASC
"#,
            )
            .context("Failed to prepare list_symbols_by_file")?;

        let mut rows = stmt.query(params![file_path])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SymbolRow {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                name: row.get(4)?,
                exported: row.get::<_, i64>(5)? != 0,
                start_byte: row.get::<_, i64>(6)? as u32,
                end_byte: row.get::<_, i64>(7)? as u32,
                start_line: row.get::<_, i64>(8)? as u32,
                end_line: row.get::<_, i64>(9)? as u32,
                text: row.get(10)?,
            });
        }
        Ok(out)
    }

    pub fn search_symbols_by_name_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE name LIKE (?1 || '%')
ORDER BY name ASC
LIMIT ?2
"#,
            )
            .context("Failed to prepare search_symbols_by_name_prefix")?;

        let mut rows = stmt.query(params![prefix, limit as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SymbolRow {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                name: row.get(4)?,
                exported: row.get::<_, i64>(5)? != 0,
                start_byte: row.get::<_, i64>(6)? as u32,
                end_byte: row.get::<_, i64>(7)? as u32,
                start_line: row.get::<_, i64>(8)? as u32,
                end_line: row.get::<_, i64>(9)? as u32,
                text: row.get(10)?,
            });
        }
        Ok(out)
    }

    pub fn search_symbols_by_name_substr(
        &self,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
SELECT
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
FROM symbols
WHERE instr(name, ?1) > 0
ORDER BY name ASC
LIMIT ?2
"#,
            )
            .context("Failed to prepare search_symbols_by_name_substr")?;

        let mut rows = stmt.query(params![needle, limit as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SymbolRow {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                name: row.get(4)?,
                exported: row.get::<_, i64>(5)? != 0,
                start_byte: row.get::<_, i64>(6)? as u32,
                end_byte: row.get::<_, i64>(7)? as u32,
                start_line: row.get::<_, i64>(8)? as u32,
                end_line: row.get::<_, i64>(9)? as u32,
                text: row.get(10)?,
            });
        }
        Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_symbol(id: &str, file_path: &str, name: &str) -> SymbolRow {
        SymbolRow {
            id: id.to_string(),
            file_path: file_path.to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: name.to_string(),
            exported: true,
            start_byte: 10,
            end_byte: 20,
            start_line: 2,
            end_line: 3,
            text: "export function foo() {}".to_string(),
        }
    }

    #[test]
    fn upsert_and_query_symbol() {
        let store = SqliteStore::from_connection(Connection::open_in_memory().unwrap());
        store.init().unwrap();

        let mut sym = sample_symbol("id1", "src/a.ts", "foo");
        store.upsert_symbol(&sym).unwrap();

        let fetched = store.get_symbol_by_id("id1").unwrap().unwrap();
        assert_eq!(fetched.name, "foo");
        assert_eq!(fetched.file_path, "src/a.ts");

        sym.name = "foo2".to_string();
        store.upsert_symbol(&sym).unwrap();

        let fetched2 = store.get_symbol_by_id("id1").unwrap().unwrap();
        assert_eq!(fetched2.name, "foo2");
    }

    #[test]
    fn queries_by_file_prefix_and_substr() {
        let store = SqliteStore::from_connection(Connection::open_in_memory().unwrap());
        store.init().unwrap();

        store
            .upsert_symbol(&sample_symbol("id1", "src/a.ts", "alpha"))
            .unwrap();
        store
            .upsert_symbol(&sample_symbol("id2", "src/a.ts", "alphabet"))
            .unwrap();
        store
            .upsert_symbol(&sample_symbol("id3", "src/b.ts", "beta"))
            .unwrap();

        let by_file = store.list_symbols_by_file("src/a.ts").unwrap();
        assert_eq!(by_file.len(), 2);

        let pref = store.search_symbols_by_name_prefix("alp", 10).unwrap();
        assert_eq!(pref.len(), 2);

        let sub = store.search_symbols_by_name_substr("pha", 10).unwrap();
        assert_eq!(sub.len(), 2);
    }

    #[test]
    fn upserts_edges_deduped() {
        let store = SqliteStore::from_connection(Connection::open_in_memory().unwrap());
        store.init().unwrap();

        store
            .upsert_symbol(&sample_symbol("id1", "src/a.ts", "alpha"))
            .unwrap();
        store
            .upsert_symbol(&sample_symbol("id2", "src/a.ts", "beta"))
            .unwrap();

        let edge = EdgeRow {
            from_symbol_id: "id1".to_string(),
            to_symbol_id: "id2".to_string(),
            edge_type: "reference".to_string(),
            at_file: None,
            at_line: None,
            confidence: 1.0,
            evidence_count: 1,
        };
        store.upsert_edge(&edge).unwrap();
        store.upsert_edge(&edge).unwrap();

        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn upsert_edge_keeps_max_confidence_and_evidence_count() {
        let store = SqliteStore::from_connection(Connection::open_in_memory().unwrap());
        store.init().unwrap();

        store
            .upsert_symbol(&sample_symbol("id1", "src/a.ts", "alpha"))
            .unwrap();
        store
            .upsert_symbol(&sample_symbol("id2", "src/a.ts", "beta"))
            .unwrap();

        store
            .upsert_edge(&EdgeRow {
                from_symbol_id: "id1".to_string(),
                to_symbol_id: "id2".to_string(),
                edge_type: "reference".to_string(),
                at_file: None,
                at_line: None,
                confidence: 0.5,
                evidence_count: 3,
            })
            .unwrap();
        store
            .upsert_edge(&EdgeRow {
                from_symbol_id: "id1".to_string(),
                to_symbol_id: "id2".to_string(),
                edge_type: "reference".to_string(),
                at_file: Some("src/a.ts".to_string()),
                at_line: Some(1),
                confidence: 0.9,
                evidence_count: 1,
            })
            .unwrap();

        let edges = store.list_edges_from("id1", 10).unwrap();
        assert_eq!(edges.len(), 1);
        assert!((edges[0].confidence - 0.9).abs() < 1e-6);
        assert_eq!(edges[0].evidence_count, 3);
        assert_eq!(edges[0].at_file.as_deref(), Some("src/a.ts"));
        assert_eq!(edges[0].at_line, Some(1));
    }

    #[test]
    fn upsert_and_list_edge_evidence_keeps_max_count() {
        let store = SqliteStore::from_connection(Connection::open_in_memory().unwrap());
        store.init().unwrap();

        store
            .upsert_symbol(&sample_symbol("id1", "src/a.ts", "alpha"))
            .unwrap();
        store
            .upsert_symbol(&sample_symbol("id2", "src/a.ts", "beta"))
            .unwrap();

        store
            .upsert_edge_evidence(&EdgeEvidenceRow {
                from_symbol_id: "id1".to_string(),
                to_symbol_id: "id2".to_string(),
                edge_type: "call".to_string(),
                at_file: "src/a.ts".to_string(),
                at_line: 10,
                count: 2,
            })
            .unwrap();
        store
            .upsert_edge_evidence(&EdgeEvidenceRow {
                from_symbol_id: "id1".to_string(),
                to_symbol_id: "id2".to_string(),
                edge_type: "call".to_string(),
                at_file: "src/a.ts".to_string(),
                at_line: 10,
                count: 1,
            })
            .unwrap();
        store
            .upsert_edge_evidence(&EdgeEvidenceRow {
                from_symbol_id: "id1".to_string(),
                to_symbol_id: "id2".to_string(),
                edge_type: "call".to_string(),
                at_file: "src/a.ts".to_string(),
                at_line: 12,
                count: 5,
            })
            .unwrap();

        let ev = store.list_edge_evidence("id1", "id2", "call", 10).unwrap();
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].at_line, 12);
        assert_eq!(ev[0].count, 5);
        assert_eq!(ev[1].at_line, 10);
        assert_eq!(ev[1].count, 2);
    }

    #[test]
    fn similarity_clusters_round_trip_and_list_members() {
        let store = SqliteStore::from_connection(Connection::open_in_memory().unwrap());
        store.init().unwrap();

        store
            .upsert_symbol(&sample_symbol("id1", "src/a.ts", "alpha"))
            .unwrap();
        store
            .upsert_symbol(&sample_symbol("id2", "src/b.ts", "beta"))
            .unwrap();
        store
            .upsert_symbol(&sample_symbol("id3", "src/c.ts", "gamma"))
            .unwrap();

        store
            .upsert_similarity_cluster(&SimilarityClusterRow {
                symbol_id: "id1".to_string(),
                cluster_key: "k1".to_string(),
            })
            .unwrap();
        store
            .upsert_similarity_cluster(&SimilarityClusterRow {
                symbol_id: "id2".to_string(),
                cluster_key: "k1".to_string(),
            })
            .unwrap();
        store
            .upsert_similarity_cluster(&SimilarityClusterRow {
                symbol_id: "id3".to_string(),
                cluster_key: "k2".to_string(),
            })
            .unwrap();

        let key1 = store.get_similarity_cluster_key("id1").unwrap();
        assert_eq!(key1.as_deref(), Some("k1"));

        let missing = store.get_similarity_cluster_key("does-not-exist").unwrap();
        assert!(missing.is_none());

        let members = store.list_symbols_in_cluster("k1", 10).unwrap();
        assert_eq!(
            members
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            vec!["id1", "id2"]
        );

        store
            .upsert_similarity_cluster(&SimilarityClusterRow {
                symbol_id: "id2".to_string(),
                cluster_key: "k2".to_string(),
            })
            .unwrap();
        let members_k1 = store.list_symbols_in_cluster("k1", 10).unwrap();
        assert_eq!(
            members_k1
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            vec!["id1"]
        );
        let members_k2 = store.list_symbols_in_cluster("k2", 10).unwrap();
        assert_eq!(
            members_k2
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            vec!["id2", "id3"]
        );
    }
}

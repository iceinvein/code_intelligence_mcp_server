use serde::{Deserialize, Serialize};

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
    pub resolution: String,
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

pub const SCHEMA_SQL: &str = r#"
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
  resolution TEXT NOT NULL DEFAULT 'unknown',
  resolution_rank INTEGER NOT NULL DEFAULT 0,
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
"#;

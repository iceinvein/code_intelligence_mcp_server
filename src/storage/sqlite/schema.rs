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

/// Identity metadata for one concrete declaration occurrence.
///
/// `symbol_id` addresses the source occurrence and remains the graph foreign
/// key. `logical_id` groups overloads/partial declarations under a stable,
/// location-independent identity. The canonical occurrence deliberately uses
/// the logical id as its symbol id for compatibility with existing import
/// targets; additional occurrences receive deterministic distinct ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolIdentityRow {
    pub symbol_id: String,
    pub logical_id: String,
    pub qualified_name: String,
    pub signature: String,
    pub occurrence_discriminator: String,
    pub is_canonical: bool,
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
pub struct ExternalIndexRow {
    pub id: String,
    pub source_kind: String,
    pub producer: String,
    pub language: String,
    pub root_path: String,
    pub artifact_path: String,
    pub artifact_hash: String,
    pub status: String,
    pub diagnostics_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalSymbolRow {
    pub id: String,
    pub external_index_id: String,
    pub external_symbol: String,
    pub display_name: String,
    pub language: String,
    pub kind: String,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub start_byte: Option<u32>,
    pub end_byte: Option<u32>,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalReferenceRow {
    pub id: i64,
    pub external_index_id: String,
    pub from_external_symbol_id: Option<String>,
    pub to_external_symbol_id: Option<String>,
    pub relationship: String,
    pub file_path: String,
    pub line: u32,
    pub column: Option<u32>,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
    pub confidence: f32,
    pub provenance: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolMappingRow {
    pub external_symbol_id: String,
    pub internal_symbol_id: String,
    pub mapping_kind: String,
    pub confidence: f32,
    pub created_at: i64,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleBindingRow {
    pub id: i64,
    pub file_path: String,
    pub binding_kind: String,
    pub source_module: String,
    pub source_file: Option<String>,
    pub imported_name: String,
    pub local_name: String,
    pub exported_name: String,
    pub target_symbol_id: Option<String>,
    pub at_line: u32,
    pub resolution: String,
    pub confidence: f32,
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
    pub scan_ms: u64,
    pub cleanup_ms: u64,
    pub parse_ms: u64,
    pub sqlite_write_ms: u64,
    pub tantivy_ms: u64,
    pub binding_ms: u64,
    pub edge_ms: u64,
    pub embedding_ms: u64,
    pub vector_write_ms: u64,
    pub pagerank_ms: u64,
    pub optimize_ms: u64,
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
    /// Time to generate query embedding(s) via llama.cpp (subset of vector_ms)
    pub embedding_ms: u64,
    /// Time for cross-encoder reranking (0 if disabled)
    pub reranker_ms: u64,
    /// Time for scoring adjustments + edge expansion + diversity
    pub scoring_ms: u64,
    /// Time for context assembly (text reads, token counting, formatting)
    pub assembly_ms: u64,
    pub fusion_ms: u64,
    pub search_path: String,
    pub cache_status: String,
    pub subquery_count: u64,
    pub keyword_candidates: u64,
    pub vector_candidates: u64,
    pub fused_candidates: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimilarityClusterRow {
    pub symbol_id: String,
    pub cluster_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolMetricsRow {
    pub symbol_id: String,
    pub pagerank: f64,
    pub in_degree: u32,
    pub out_degree: u32,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoMapSymbolRow {
    pub id: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub name: String,
    pub exported: bool,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
    pub pagerank: f64,
    pub in_degree: u32,
    pub out_degree: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuerySelectionRow {
    pub id: i64,
    pub query_text: String,
    pub query_normalized: String,
    pub selected_symbol_id: String,
    pub position: u32,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserFileAffinityRow {
    pub file_path: String,
    pub view_count: u32,
    pub edit_count: u32,
    pub last_accessed_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryRow {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub vcs_type: Option<String>,
    pub remote_url: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRow {
    pub id: String,
    pub repository_id: String,
    pub name: String,
    pub version: Option<String>,
    pub manifest_path: String,
    pub package_type: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocstringRow {
    pub symbol_id: String,
    pub raw_text: String,
    pub summary: Option<String>,
    pub params_json: Option<String>,
    pub returns_text: Option<String>,
    pub examples_json: Option<String>,
    pub updated_at: i64,
}

/// TODO/FIXME comment row for technical debt tracking (LANG-03)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoRow {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub file_path: String,
    pub line: u32,
    pub associated_symbol: Option<String>,
    pub created_at: i64,
}

/// Test-to-source file link for test coverage awareness (LANG-04)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestLinkRow {
    pub test_file_path: String,
    pub source_file_path: String,
    pub link_direction: String, // "bidirectional", "test_to_source", "source_to_test"
    pub created_at: i64,
}

/// Decorator row for framework metadata (LANG-02)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecoratorRow {
    pub symbol_id: String,
    pub name: String,
    pub arguments: Option<String>,
    pub target_line: u32,
    pub decorator_type: String,
    pub updated_at: i64,
}

/// Cross-repo edge row — tracks edges from symbols in this repo to symbols in other repos
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossRepoEdgeRow {
    pub id: i64,
    pub from_symbol_id: String,
    pub to_repo_hash: String,
    pub to_symbol_name: String,
    pub to_symbol_file: Option<String>,
    pub to_symbol_id: Option<String>,
    pub edge_type: String,
    pub at_file: Option<String>,
    pub at_line: Option<u32>,
    pub confidence: f32,
    pub resolution: String,
}

/// Co-change row for git co-change analysis (change impact prediction)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoChangeRow {
    pub file_a: String,
    pub file_b: String,
    pub co_change_count: u32,
    pub total_commits_a: u32,
    pub total_commits_b: u32,
    pub confidence: f32,
}

/// Framework pattern row for Elysia/Hono/Express route and plugin metadata
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameworkPatternRow {
    pub id: String,
    pub file_path: String,
    pub line: u32,
    pub framework: String,
    pub kind: String,
    pub http_method: Option<String>,
    pub path: Option<String>,
    pub name: Option<String>,
    pub handler: Option<String>,
    pub arguments: Option<String>,
    pub parent_chain: Option<String>,
    pub updated_at: i64,
}

// --- Shared extraction types (consumed by both indexer and storage) ---

/// TODO/FIXME comment kind for technical debt tracking (LANG-03)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoKind {
    Todo,
    Fixme,
}

/// TODO/FIXME comment entry extracted from source code (LANG-03)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoEntry {
    pub kind: TodoKind,
    pub text: String,
    pub file_path: String,
    pub line: u32,
    pub associated_symbol: Option<String>,
}

/// JSDoc comment entry for TypeScript/JavaScript documentation (LANG-01)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JSDocEntry {
    pub symbol_id: String,
    pub raw_text: String,
    pub summary: Option<String>,
    pub params: Vec<JSDocParam>,
    pub returns: Option<String>,
    pub examples: Vec<String>,
    pub deprecated: bool,
    pub throws: Vec<String>,
    pub see_also: Vec<String>,
    pub since: Option<String>,
}

/// JSDoc parameter documentation (LANG-01)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JSDocParam {
    pub name: String,
    pub type_annotation: Option<String>,
    pub description: Option<String>,
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

CREATE TABLE IF NOT EXISTS symbol_identities (
  symbol_id TEXT PRIMARY KEY NOT NULL,
  logical_id TEXT NOT NULL,
  qualified_name TEXT NOT NULL,
  signature TEXT NOT NULL DEFAULT '',
  occurrence_discriminator TEXT NOT NULL,
  is_canonical INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE(logical_id, occurrence_discriminator),
  FOREIGN KEY(symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_symbol_identities_logical
  ON symbol_identities(logical_id);
CREATE INDEX IF NOT EXISTS idx_symbol_identities_qualified
  ON symbol_identities(qualified_name);

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

-- Module-level imports and public API exposure paths. Unlike generic graph
-- edges, these rows retain both sides of aliases and may remain unresolved.
CREATE TABLE IF NOT EXISTS module_bindings (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  file_path TEXT NOT NULL,
  binding_kind TEXT NOT NULL,
  source_module TEXT NOT NULL DEFAULT '',
  source_file TEXT,
  imported_name TEXT NOT NULL DEFAULT '',
  local_name TEXT NOT NULL DEFAULT '',
  exported_name TEXT NOT NULL DEFAULT '',
  target_symbol_id TEXT,
  at_line INTEGER NOT NULL,
  resolution TEXT NOT NULL,
  confidence REAL NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE(
    file_path, binding_kind, source_module, imported_name,
    local_name, exported_name, at_line
  ),
  FOREIGN KEY(target_symbol_id) REFERENCES symbols(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_module_bindings_file
  ON module_bindings(file_path);
CREATE INDEX IF NOT EXISTS idx_module_bindings_source_file
  ON module_bindings(source_file);
CREATE INDEX IF NOT EXISTS idx_module_bindings_target
  ON module_bindings(target_symbol_id);
CREATE INDEX IF NOT EXISTS idx_module_bindings_public_name
  ON module_bindings(exported_name)
  WHERE binding_kind IN ('export', 're_export', 'export_all');

CREATE TABLE IF NOT EXISTS external_indexes (
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
CREATE INDEX IF NOT EXISTS idx_external_indexes_language_status
  ON external_indexes(language, status);

CREATE TABLE IF NOT EXISTS external_symbols (
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
CREATE INDEX IF NOT EXISTS idx_external_symbols_index_name
  ON external_symbols(external_index_id, display_name);
CREATE INDEX IF NOT EXISTS idx_external_symbols_index_file
  ON external_symbols(external_index_id, file_path);
CREATE INDEX IF NOT EXISTS idx_external_symbols_external_symbol
  ON external_symbols(external_symbol);

CREATE TABLE IF NOT EXISTS external_references (
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
  dedupe_key TEXT NOT NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE(external_index_id, dedupe_key),
  FOREIGN KEY(external_index_id) REFERENCES external_indexes(id) ON DELETE CASCADE,
  FOREIGN KEY(from_external_symbol_id) REFERENCES external_symbols(id) ON DELETE SET NULL,
  FOREIGN KEY(to_external_symbol_id) REFERENCES external_symbols(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_external_references_to
  ON external_references(to_external_symbol_id);
CREATE INDEX IF NOT EXISTS idx_external_references_from
  ON external_references(from_external_symbol_id);
CREATE INDEX IF NOT EXISTS idx_external_references_relationship
  ON external_references(relationship);
CREATE INDEX IF NOT EXISTS idx_external_references_file_line
  ON external_references(file_path, line);

CREATE TABLE IF NOT EXISTS symbol_mappings (
  external_symbol_id TEXT PRIMARY KEY NOT NULL,
  internal_symbol_id TEXT NOT NULL,
  mapping_kind TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 1.0,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(external_symbol_id) REFERENCES external_symbols(id) ON DELETE CASCADE,
  FOREIGN KEY(internal_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_symbol_mappings_internal
  ON symbol_mappings(internal_symbol_id);

CREATE TABLE IF NOT EXISTS index_metadata (
  key TEXT PRIMARY KEY NOT NULL,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

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
  symbols_indexed INTEGER NOT NULL,
  scan_ms INTEGER NOT NULL DEFAULT 0,
  cleanup_ms INTEGER NOT NULL DEFAULT 0,
  parse_ms INTEGER NOT NULL DEFAULT 0,
  sqlite_write_ms INTEGER NOT NULL DEFAULT 0,
  tantivy_ms INTEGER NOT NULL DEFAULT 0,
  binding_ms INTEGER NOT NULL DEFAULT 0,
  edge_ms INTEGER NOT NULL DEFAULT 0,
  embedding_ms INTEGER NOT NULL DEFAULT 0,
  vector_write_ms INTEGER NOT NULL DEFAULT 0,
  pagerank_ms INTEGER NOT NULL DEFAULT 0,
  optimize_ms INTEGER NOT NULL DEFAULT 0
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
  result_count INTEGER NOT NULL,
  embedding_ms INTEGER NOT NULL DEFAULT 0,
  reranker_ms INTEGER NOT NULL DEFAULT 0,
  scoring_ms INTEGER NOT NULL DEFAULT 0,
  assembly_ms INTEGER NOT NULL DEFAULT 0,
  fusion_ms INTEGER NOT NULL DEFAULT 0,
  search_path TEXT NOT NULL DEFAULT 'unknown',
  cache_status TEXT NOT NULL DEFAULT 'miss',
  subquery_count INTEGER NOT NULL DEFAULT 0,
  keyword_candidates INTEGER NOT NULL DEFAULT 0,
  vector_candidates INTEGER NOT NULL DEFAULT 0,
  fused_candidates INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_search_runs_started_at ON search_runs(started_at);

CREATE TABLE IF NOT EXISTS similarity_clusters (
  symbol_id TEXT PRIMARY KEY NOT NULL,
  cluster_key TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_similarity_clusters_key ON similarity_clusters(cluster_key);

-- Symbol metrics for PageRank and graph analysis (FNDN-08)
CREATE TABLE IF NOT EXISTS symbol_metrics (
  symbol_id TEXT PRIMARY KEY NOT NULL,
  pagerank REAL NOT NULL DEFAULT 0.0,
  in_degree INTEGER NOT NULL DEFAULT 0,
  out_degree INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_symbol_metrics_pagerank ON symbol_metrics(pagerank);

-- Query selections for learning from user choices (FNDN-09)
CREATE TABLE IF NOT EXISTS query_selections (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  query_text TEXT NOT NULL,
  query_normalized TEXT NOT NULL,
  selected_symbol_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(selected_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_query_selections_query ON query_selections(query_normalized);
CREATE INDEX IF NOT EXISTS idx_query_selections_symbol ON query_selections(selected_symbol_id);

-- User file affinity for personalization (FNDN-10)
CREATE TABLE IF NOT EXISTS user_file_affinity (
  file_path TEXT PRIMARY KEY NOT NULL,
  view_count INTEGER NOT NULL DEFAULT 0,
  edit_count INTEGER NOT NULL DEFAULT 0,
  last_accessed_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_user_file_affinity_accessed ON user_file_affinity(last_accessed_at);

-- Repositories for multi-repo support (FNDN-11)
CREATE TABLE IF NOT EXISTS repositories (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  root_path TEXT NOT NULL,
  vcs_type TEXT,
  remote_url TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Packages for package-aware scoring (FNDN-12)
CREATE TABLE IF NOT EXISTS packages (
  id TEXT PRIMARY KEY NOT NULL,
  repository_id TEXT NOT NULL,
  name TEXT NOT NULL,
  version TEXT,
  manifest_path TEXT NOT NULL,
  package_type TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(repository_id) REFERENCES repositories(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_packages_repo ON packages(repository_id);

-- Docstrings for documentation extraction (FNDN-13)
CREATE TABLE IF NOT EXISTS docstrings (
  symbol_id TEXT PRIMARY KEY NOT NULL,
  raw_text TEXT NOT NULL,
  summary TEXT,
  params_json TEXT,
  returns_text TEXT,
  examples_json TEXT,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY(symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);

-- TODO/FIXME comments for technical debt tracking (LANG-03)
CREATE TABLE IF NOT EXISTS todos (
  id TEXT PRIMARY KEY NOT NULL,
  kind TEXT NOT NULL,
  text TEXT NOT NULL,
  file_path TEXT NOT NULL,
  line INTEGER NOT NULL,
  associated_symbol TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_todos_file ON todos(file_path);
CREATE INDEX IF NOT EXISTS idx_todos_kind ON todos(kind);
CREATE INDEX IF NOT EXISTS idx_todos_symbol ON todos(associated_symbol);

-- Test-to-source file links for test coverage awareness (LANG-04)
CREATE TABLE IF NOT EXISTS test_links (
  test_file_path TEXT NOT NULL,
  source_file_path TEXT NOT NULL,
  link_direction TEXT NOT NULL DEFAULT 'bidirectional',
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  PRIMARY KEY (test_file_path, source_file_path)
);
CREATE INDEX IF NOT EXISTS idx_test_links_test ON test_links(test_file_path);
CREATE INDEX IF NOT EXISTS idx_test_links_source ON test_links(source_file_path);

-- Decorators for framework metadata (LANG-02)
CREATE TABLE IF NOT EXISTS decorators (
  symbol_id TEXT NOT NULL,
  name TEXT NOT NULL,
  arguments TEXT,
  target_line INTEGER NOT NULL,
  decorator_type TEXT NOT NULL DEFAULT 'unknown',
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  PRIMARY KEY (symbol_id, name),
  FOREIGN KEY(symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_decorators_name ON decorators(name);
CREATE INDEX IF NOT EXISTS idx_decorators_type ON decorators(decorator_type);

-- Embedding cache for avoiding recomputation (PERF-02)
CREATE TABLE IF NOT EXISTS embedding_cache (
    cache_key TEXT PRIMARY KEY NOT NULL,
    model_name TEXT NOT NULL,
    text_hash TEXT NOT NULL,
    embedding BLOB NOT NULL,
    vector_dim INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_accessed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    access_count INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_embedding_cache_model ON embedding_cache(model_name);
CREATE INDEX IF NOT EXISTS idx_embedding_cache_accessed ON embedding_cache(last_accessed_at);

-- Framework patterns for Elysia/Hono/Express route metadata
CREATE TABLE IF NOT EXISTS framework_patterns (
    id TEXT PRIMARY KEY NOT NULL,
    file_path TEXT NOT NULL,
    line INTEGER NOT NULL,
    framework TEXT NOT NULL,
    kind TEXT NOT NULL,
    http_method TEXT,
    path TEXT,
    name TEXT,
    handler TEXT,
    arguments TEXT,
    parent_chain TEXT,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_fp_file_path ON framework_patterns(file_path);
CREATE INDEX IF NOT EXISTS idx_fp_framework ON framework_patterns(framework);
CREATE INDEX IF NOT EXISTS idx_fp_kind ON framework_patterns(kind);
CREATE INDEX IF NOT EXISTS idx_fp_http_method ON framework_patterns(http_method);
CREATE INDEX IF NOT EXISTS idx_fp_path ON framework_patterns(path);

-- LLM-generated symbol descriptions cache
CREATE TABLE IF NOT EXISTS descriptions (
    symbol_id TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    description TEXT NOT NULL,
    generated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_descriptions_hash ON descriptions(content_hash);

-- Cross-repo edges: track references from symbols in this repo to symbols in other repos
CREATE TABLE IF NOT EXISTS cross_repo_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    from_symbol_id TEXT NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    to_repo_hash TEXT NOT NULL,
    to_symbol_name TEXT NOT NULL,
    to_symbol_file TEXT,
    to_symbol_id TEXT,
    edge_type TEXT NOT NULL,
    at_file TEXT,
    at_line INTEGER,
    confidence REAL NOT NULL DEFAULT 0.5,
    resolution TEXT NOT NULL DEFAULT 'cross_repo_unresolved',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(from_symbol_id, to_repo_hash, to_symbol_name, edge_type)
);
CREATE INDEX IF NOT EXISTS idx_cross_repo_edges_from ON cross_repo_edges(from_symbol_id);
CREATE INDEX IF NOT EXISTS idx_cross_repo_edges_to_repo ON cross_repo_edges(to_repo_hash);
CREATE INDEX IF NOT EXISTS idx_cross_repo_edges_resolution ON cross_repo_edges(resolution);

-- Co-change matrix for change impact prediction
CREATE TABLE IF NOT EXISTS co_changes (
  file_a TEXT NOT NULL,
  file_b TEXT NOT NULL,
  co_change_count INTEGER NOT NULL DEFAULT 1,
  total_commits_a INTEGER NOT NULL DEFAULT 0,
  total_commits_b INTEGER NOT NULL DEFAULT 0,
  confidence REAL NOT NULL DEFAULT 0.0,
  PRIMARY KEY (file_a, file_b)
);
CREATE INDEX IF NOT EXISTS idx_co_changes_file_a ON co_changes(file_a);
CREATE INDEX IF NOT EXISTS idx_co_changes_file_b ON co_changes(file_b);
CREATE INDEX IF NOT EXISTS idx_co_changes_confidence ON co_changes(confidence);

-- Cached aggregate stats for dashboard. Single row (id = 1) refreshed
-- after every index run so /api/repos/:id avoids COUNT(*) scans on
-- multi-million-row tables (symbols / edges).
CREATE TABLE IF NOT EXISTS repo_stats (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  symbols INTEGER NOT NULL,
  edges INTEGER NOT NULL,
  descriptions INTEGER NOT NULL,
  undescribed_symbols INTEGER NOT NULL,
  last_updated_unix_s INTEGER,
  computed_at_unix_s INTEGER NOT NULL
);
"#;

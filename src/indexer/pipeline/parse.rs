use crate::indexer::extract::symbol::{DataFlowEdge, Import, JSDocEntry, TodoEntry};
use crate::indexer::pipeline::utils::FileFingerprint;
use crate::storage::sqlite::schema::{
    DecoratorRow, EdgeEvidenceRow, EdgeRow, FrameworkPatternRow, UsageExampleRow,
};
use crate::storage::sqlite::SymbolRow;

use crate::{
    config::Config,
    indexer::{
        extract::csharp::extract_csharp_symbols,
        extract::go::extract_go_symbols,
        extract::java::extract_java_symbols,
        extract::javascript::extract_javascript_symbols,
        extract::kotlin::extract_kotlin_symbols,
        extract::python::extract_python_symbols,
        extract::ruby::extract_ruby_symbols,
        extract::rust::extract_rust_symbols,
        extract::swift::extract_swift_symbols,
        extract::typescript::extract_typescript_symbols_with_path,
        extract::{c::extract_c_symbols, cpp::extract_cpp_symbols},
        parser::{language_id_for_path, LanguageId},
        pipeline::{
            edges::{extract_edges_for_symbol, upsert_name_mapping, PackageLookupFn},
            parsing::symbol_kind_to_string,
            usage::extract_usage_examples_for_file,
            utils::{file_fingerprint, file_key_path, language_string, stable_symbol_id},
        },
    },
    storage::sqlite::{pool::SqlitePool, queries},
};
use anyhow::{Context, Result};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

/// Full output of parsing one file — everything needed to write.
///
/// `edges` is empty after parsing. Edge extraction is deferred to a separate
/// pipeline phase that runs after symbols are written, so cross-file edge
/// resolution (which queries SQLite for receiver-method targets) can see the
/// just-indexed symbols. The `imports`, `type_edges`, and `dataflow_edges`
/// fields carry the AST-derived inputs that the deferred edge extraction
/// needs, since the tree-sitter parse is discarded by then.
#[derive(Debug)]
pub struct ParsedFile {
    pub rel_path: String,
    pub fingerprint: FileFingerprint,
    pub language: String,
    pub symbol_rows: Vec<SymbolRow>,
    pub edges: Vec<(EdgeRow, Vec<EdgeEvidenceRow>)>,
    pub usage_examples: Vec<UsageExampleRow>,
    pub import_tags: String,
    pub framework_tags: String,
    pub todos: Vec<TodoEntry>,
    pub docstrings: Vec<JSDocEntry>,
    pub decorators: Vec<DecoratorRow>,
    pub framework_patterns: Vec<FrameworkPatternRow>,
    pub is_test_file: bool,
    pub imports: Vec<Import>,
    pub type_edges: Vec<(String, String)>,
    pub dataflow_edges: Vec<DataFlowEdge>,
}

/// Result of parsing a single file
#[derive(Debug)]
pub enum ParseResult {
    /// File unchanged (fingerprint matched), skip
    Unchanged,
    /// Fully parsed file with all extracted data
    Parsed(Box<ParsedFile>),
    /// File skipped (unsupported language, read error, etc.)
    Skipped { reason: String },
}

/// Parse a single file and return all extracted data.
/// Takes a read-only SQLite connection for fingerprint checks and cross-file lookups.
/// Does NOT write to any storage backend.
pub fn parse_single_file(file: &Path, config: &Config, conn: &Connection) -> ParseResult {
    // 1. Determine language from path
    let language_id = match language_id_for_path(file) {
        Some(id) => id,
        None => {
            return ParseResult::Skipped {
                reason: format!("Unsupported language for file: {}", file.display()),
            };
        }
    };

    // 2. Get file fingerprint
    let fp = match file_fingerprint(file) {
        Ok(fp) => fp,
        Err(e) => {
            return ParseResult::Skipped {
                reason: format!("Failed to fingerprint file: {}", e),
            };
        }
    };

    // 3. Check if unchanged by querying SQLite fingerprints table
    let rel = file_key_path(config, file);

    let is_unchanged = match conn
        .query_row(
            "SELECT mtime_ns, size_bytes FROM file_fingerprints WHERE file_path = ?1",
            [&rel],
            |row| {
                let mtime: i64 = row.get(0)?;
                let size: i64 = row.get(1)?;
                Ok((mtime, size as u64))
            },
        )
        .optional()
    {
        Ok(Some((mtime, size))) => mtime == fp.mtime_ns && size == fp.size_bytes,
        Ok(None) => false,
        Err(_) => false,
    };

    if is_unchanged {
        return ParseResult::Unchanged;
    }

    // 4. Read file
    let source = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            return ParseResult::Skipped {
                reason: format!("Failed to read file: {}", e),
            };
        }
    };

    // 5. Extract symbols via tree-sitter
    let extracted = match language_id {
        LanguageId::Typescript | LanguageId::Tsx => {
            extract_typescript_symbols_with_path(language_id, &source, &rel)
        }
        LanguageId::Rust => extract_rust_symbols(&source),
        LanguageId::Python => extract_python_symbols(&source),
        LanguageId::Go => extract_go_symbols(&source),
        LanguageId::C => extract_c_symbols(&source),
        LanguageId::Cpp => extract_cpp_symbols(&source),
        LanguageId::Java => extract_java_symbols(&source),
        LanguageId::Javascript => extract_javascript_symbols(&source),
        LanguageId::Ruby => extract_ruby_symbols(&source),
        LanguageId::Kotlin => extract_kotlin_symbols(&source),
        LanguageId::CSharp => extract_csharp_symbols(&source),
        LanguageId::Swift => extract_swift_symbols(&source),
    };

    let extracted = match extracted {
        Ok(syms) => syms,
        Err(e) => {
            return ParseResult::Skipped {
                reason: format!("Failed to extract symbols: {}", e),
            };
        }
    };

    // 6. Build SymbolRow vec (include file-level symbol)
    let mut symbol_rows = Vec::new();

    // Add file-level symbol
    let file_symbol_id = stable_symbol_id(&rel, "FILE_ROOT", 0);
    symbol_rows.push(SymbolRow {
        id: file_symbol_id,
        file_path: rel.clone(),
        language: language_string(language_id).to_string(),
        kind: "file".to_string(),
        name: rel.clone(),
        exported: false,
        start_byte: 0,
        end_byte: source.len() as u32,
        start_line: 1,
        end_line: source.lines().count() as u32,
        text: source.clone(),
    });

    for sym in extracted.symbols {
        let text = source
            .get(sym.bytes.start..sym.bytes.end)
            .unwrap_or("")
            .to_string();

        if text.trim().is_empty() {
            continue;
        }

        let start_byte_for_id = if sym.exported {
            0
        } else {
            sym.bytes.start as u32
        };
        let id = stable_symbol_id(&rel, &sym.name, start_byte_for_id);
        symbol_rows.push(SymbolRow {
            id,
            file_path: rel.clone(),
            language: language_string(language_id).to_string(),
            kind: symbol_kind_to_string(sym.kind),
            name: sym.name,
            exported: sym.exported,
            start_byte: sym.bytes.start as u32,
            end_byte: sym.bytes.end as u32,
            start_line: sym.lines.start,
            end_line: sym.lines.end,
            text,
        });
    }

    // 7. Extract import tags
    let import_tags = if language_id == LanguageId::Rust {
        crate::text::extract_rust_import_tags(&source)
    } else {
        let sources: Vec<String> = extracted
            .imports
            .iter()
            .map(|imp| imp.source.clone())
            .collect();
        crate::text::build_import_tags_from_sources(&sources)
    };

    // 8. Extract framework tags
    let framework_tags = crate::text::build_framework_vocab_tags(
        &extracted
            .framework_patterns
            .iter()
            .map(|p| (p.kind.to_string(), p.http_method.clone()))
            .collect::<Vec<_>>(),
    );

    // 9. Build name_to_id HashMap (used by usage-example extraction; edges
    //    are now extracted in a deferred phase after symbols are written, so
    //    we don't construct id_to_symbol here)
    let mut name_to_id: HashMap<String, String> = HashMap::new();
    for row in &symbol_rows {
        upsert_name_mapping(&mut name_to_id, row);
    }

    // 10. Edge extraction is deferred to `extract_edges_for_parsed_file`,
    //     which runs after the symbol-write phase. That order lets
    //     receiver-based cross-file resolution (which queries SQLite for
    //     class methods like `sessionManager.createSession`) see symbols
    //     that were just indexed in this run.
    let all_edges: Vec<(EdgeRow, Vec<EdgeEvidenceRow>)> = Vec::new();

    // 11. Extract usage examples
    let usage_examples = extract_usage_examples_for_file(
        &rel,
        &source,
        &name_to_id,
        &extracted.imports,
        &symbol_rows,
    );

    // 12. Check if test file (path-based classifier shared with retrieval)
    let is_test_file = crate::classify::is_test_file(&rel);

    // 13. Build decorator rows
    let decorators: Vec<DecoratorRow> = extracted
        .decorators
        .iter()
        .map(|d| DecoratorRow {
            symbol_id: d.symbol_id.clone(),
            name: d.name.clone(),
            arguments: d.arguments.clone(),
            target_line: d.target_line,
            decorator_type: serde_json::to_string(&d.decorator_type)
                .unwrap_or_else(|_| "unknown".to_string()),
            updated_at: 0,
        })
        .collect();

    // 14. Build framework pattern rows
    let framework_patterns: Vec<FrameworkPatternRow> = extracted
        .framework_patterns
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let id = format!("{}:{}:{}:{}", rel, p.line, p.column, i);
            FrameworkPatternRow {
                id,
                file_path: rel.clone(),
                line: p.line,
                framework: p.framework.clone(),
                kind: p.kind.to_string(),
                http_method: p.http_method.clone(),
                path: p.path.clone(),
                name: p.name.clone(),
                handler: p.handler.clone(),
                arguments: p.arguments.clone(),
                parent_chain: p.parent_chain.clone(),
                updated_at: 0,
            }
        })
        .collect();

    // 15. Return Parsed(ParsedFile { ... })
    ParseResult::Parsed(Box::new(ParsedFile {
        rel_path: rel,
        fingerprint: fp,
        language: language_string(language_id).to_string(),
        symbol_rows,
        edges: all_edges,
        usage_examples,
        import_tags,
        framework_tags,
        todos: extracted.todos,
        docstrings: extracted.jsdoc_entries,
        decorators,
        framework_patterns,
        is_test_file,
        imports: extracted.imports,
        type_edges: extracted.type_edges,
        dataflow_edges: extracted.dataflow_edges,
    }))
}

/// One file's edge bundle: each entry is an edge row plus its evidence rows.
pub type EdgeBundle = Vec<(EdgeRow, Vec<EdgeEvidenceRow>)>;

/// Extract edges for a single parsed file using the populated SQLite
/// connection. Runs in the indexing pipeline's deferred edge phase, after
/// symbols are written so cross-file lookups in
/// `resolve_method_on_receiver` and `resolve_imported_symbol_id_with_db`
/// can resolve targets that were indexed in this same run.
pub fn extract_edges_for_parsed_file(parsed: &ParsedFile, conn: &Connection) -> EdgeBundle {
    let mut name_to_id: HashMap<String, String> = HashMap::new();
    for row in &parsed.symbol_rows {
        upsert_name_mapping(&mut name_to_id, row);
    }
    let id_to_symbol: HashMap<String, &SymbolRow> = parsed
        .symbol_rows
        .iter()
        .map(|r| (r.id.clone(), r))
        .collect();

    let package_lookup_fn: PackageLookupFn<'_> = Box::new(|file_path: &str| -> Option<String> {
        queries::packages::get_package_for_file(conn, file_path)
            .ok()
            .flatten()
            .map(|pkg| pkg.id)
    });
    let package_lookup_ref: Option<&PackageLookupFn<'_>> = Some(&package_lookup_fn);

    let mut all_edges: Vec<(EdgeRow, Vec<EdgeEvidenceRow>)> = Vec::new();
    for row in &parsed.symbol_rows {
        let edges = extract_edges_for_symbol(
            row,
            &name_to_id,
            &id_to_symbol,
            &parsed.imports,
            &parsed.type_edges,
            &parsed.dataflow_edges,
            package_lookup_ref,
            Some(conn),
        );
        all_edges.extend(edges);
    }
    all_edges
}

/// Parse multiple files in parallel using Rayon.
///
/// Uses the provided connection pool to check file fingerprints and perform
/// cross-file lookups during edge extraction. Each worker thread gets its own
/// connection from the pool.
///
/// Returns a Vec of ParseResult (one per file). Caller is responsible for
/// tallying stats and writing to storage.
pub fn parse_files(
    files: &[PathBuf],
    config: &Config,
    pool: &SqlitePool,
) -> Result<Vec<ParseResult>> {
    // Create Rayon thread pool matching parallel_workers config
    let rayon_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.parallel_workers)
        .thread_name(|i| format!("parser-{}", i))
        .build()
        .context("Failed to build Rayon thread pool for parsing")?;

    // Parse files in parallel
    let results: Vec<ParseResult> = rayon_pool.install(|| {
        files
            .par_iter()
            .map(|file| {
                // Get connection from pool for this parse operation
                let conn = match pool.get() {
                    Ok(c) => c,
                    Err(e) => {
                        return ParseResult::Skipped {
                            reason: format!("Failed to get DB connection: {}", e),
                        };
                    }
                };

                parse_single_file(file, config, &conn)
            })
            .collect()
    });

    Ok(results)
}

/// Extract edges for a batch of already-parsed files in parallel, using a
/// connection from `pool` per worker. The connections see symbols that the
/// preceding write phase persisted, so `resolve_method_on_receiver` (and the
/// DB-aware fallbacks in `resolve_imported_symbol_id_with_db`) can resolve
/// cross-file class-method targets indexed in this run.
///
/// Returns one bundle per input file, aligned by index.
pub fn extract_edges_for_files(
    parsed_files: &[ParsedFile],
    config: &Config,
    pool: &SqlitePool,
) -> Result<Vec<EdgeBundle>> {
    let rayon_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.parallel_workers)
        .thread_name(|i| format!("edges-{}", i))
        .build()
        .context("Failed to build Rayon thread pool for edge extraction")?;

    let results: Vec<Vec<(EdgeRow, Vec<EdgeEvidenceRow>)>> = rayon_pool.install(|| {
        parsed_files
            .par_iter()
            .map(|pf| {
                let conn = match pool.get() {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(
                            file = %pf.rel_path,
                            error = %e,
                            "Failed to get DB connection for edge extraction"
                        );
                        return Vec::new();
                    }
                };
                extract_edges_for_parsed_file(pf, &conn)
            })
            .collect()
    });

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Utf8PathBuf;
    use std::time::SystemTime;

    fn temp_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid = std::process::id();
        std::env::temp_dir().join(format!("parse_test_{}_{}", pid, nanos))
    }

    #[test]
    fn test_parse_single_file_rust() {
        let tmp_dir = temp_dir();
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let base_dir = Utf8PathBuf::from_path_buf(tmp_dir.clone()).unwrap();
        let db_path = base_dir.join("test.db");

        // Create test database
        let conn = Connection::open(&db_path.as_str()).unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS file_fingerprints (
                file_path TEXT PRIMARY KEY,
                mtime_ns INTEGER,
                size_bytes INTEGER,
                updated_at INTEGER
            )",
            [],
        )
        .unwrap();

        conn.execute(
            "CREATE TABLE IF NOT EXISTS test_files (
                file_path TEXT PRIMARY KEY
            )",
            [],
        )
        .unwrap();

        // Create test file
        let test_file = tmp_dir.join("test.rs");
        std::fs::write(
            &test_file,
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();

        let config = Config {
            base_dir: base_dir.clone(),
            db_path: db_path.clone(),
            vector_db_path: base_dir.join("vectors"),
            tantivy_index_path: base_dir.join("tantivy"),
            embeddings_backend: crate::config::EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 8,
            vector_search_limit: 10,
            vector_guaranteed_results: 3,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 1.0,
            rank_index_file_boost: 0.0,
            rank_test_penalty: -5.0,
            rank_popularity_weight: 0.0,
            rank_popularity_cap: 0,
            index_patterns: vec![],
            exclude_patterns: vec![],
            watch_mode: false,
            watch_debounce_ms: 100,
            watch_min_index_interval_ms: 50,
            max_context_bytes: 10_000,
            index_node_modules: false,
            repo_roots: vec![base_dir.clone()],
            reranker_enabled: false,
            reranker_model_path: None,
            reranker_top_k: 20,
            reranker_cache_dir: None,
            learning_enabled: false,
            learning_selection_boost: 0.1,
            learning_file_affinity_boost: 0.05,
            max_context_tokens: 8192,
            token_encoding: "o200k_base".to_string(),
            parallel_workers: 4,
            embedding_cache_enabled: true,
            pagerank_damping: 0.85,
            pagerank_iterations: 20,
            synonym_expansion_enabled: true,
            acronym_expansion_enabled: true,
            rrf_enabled: true,
            rrf_k: 60.0,
            rrf_keyword_weight: 1.0,
            rrf_vector_weight: 1.0,
            rrf_graph_weight: 0.5,
            hyde_enabled: false,
            hyde_llm_backend: "openai".to_string(),
            hyde_api_key: None,
            hyde_max_tokens: 512,
            metrics_enabled: true,
            metrics_port: 9090,
            package_detection_enabled: true,
            llm_enabled: true,
            llm_device: crate::config::EmbeddingsDevice::Cpu,
            llm_model_dir: None,
            llm_max_tokens: 30,
            llm_batch_commit: 10,
            answer_llm_n_ctx: 16384,
            sampling_descriptions_enabled: true,
            leader_election_enabled: false,
            leader_heartbeat_interval_ms: 10_000,
            leader_ttl_seconds: 30,
            embedding_truncate_dim: None,
            embedding_dim_override: None,
        };

        // Parse file
        let result = parse_single_file(&test_file, &config, &conn);

        match result {
            ParseResult::Parsed(parsed) => {
                assert_eq!(parsed.language, "rust");
                assert!(parsed.symbol_rows.len() >= 2); // file + function
                assert_eq!(parsed.rel_path, "test.rs");

                // Check for file-level symbol
                let file_sym = parsed
                    .symbol_rows
                    .iter()
                    .find(|s| s.kind == "file")
                    .expect("Should have file-level symbol");
                assert_eq!(file_sym.name, "test.rs");

                // Check for function symbol
                let fn_sym = parsed
                    .symbol_rows
                    .iter()
                    .find(|s| s.name == "add")
                    .expect("Should have 'add' function");
                assert_eq!(fn_sym.kind, "function");
                assert!(fn_sym.exported);
            }
            ParseResult::Unchanged => panic!("File should not be unchanged on first parse"),
            ParseResult::Skipped { reason } => panic!("File should not be skipped: {}", reason),
        }

        // Cleanup
        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    #[test]
    fn test_parse_single_file_unchanged() {
        let tmp_dir = temp_dir();
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let base_dir = Utf8PathBuf::from_path_buf(tmp_dir.clone()).unwrap();
        let db_path = base_dir.join("test.db");

        // Create test database
        let conn = Connection::open(&db_path.as_str()).unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS file_fingerprints (
                file_path TEXT PRIMARY KEY,
                mtime_ns INTEGER,
                size_bytes INTEGER,
                updated_at INTEGER
            )",
            [],
        )
        .unwrap();

        // Create test file
        let test_file = tmp_dir.join("test.rs");
        std::fs::write(&test_file, "pub fn foo() {}").unwrap();

        let fp = file_fingerprint(&test_file).unwrap();

        // Insert fingerprint
        conn.execute(
            "INSERT INTO file_fingerprints (file_path, mtime_ns, size_bytes, updated_at) VALUES (?1, ?2, ?3, 0)",
            rusqlite::params!["test.rs", fp.mtime_ns, fp.size_bytes as i64],
        )
        .unwrap();

        let config = Config {
            base_dir: base_dir.clone(),
            db_path: db_path.clone(),
            vector_db_path: base_dir.join("vectors"),
            tantivy_index_path: base_dir.join("tantivy"),
            embeddings_backend: crate::config::EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 8,
            vector_search_limit: 10,
            vector_guaranteed_results: 3,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 1.0,
            rank_index_file_boost: 0.0,
            rank_test_penalty: -5.0,
            rank_popularity_weight: 0.0,
            rank_popularity_cap: 0,
            index_patterns: vec![],
            exclude_patterns: vec![],
            watch_mode: false,
            watch_debounce_ms: 100,
            watch_min_index_interval_ms: 50,
            max_context_bytes: 10_000,
            index_node_modules: false,
            repo_roots: vec![base_dir.clone()],
            reranker_enabled: false,
            reranker_model_path: None,
            reranker_top_k: 20,
            reranker_cache_dir: None,
            learning_enabled: false,
            learning_selection_boost: 0.1,
            learning_file_affinity_boost: 0.05,
            max_context_tokens: 8192,
            token_encoding: "o200k_base".to_string(),
            parallel_workers: 4,
            embedding_cache_enabled: true,
            pagerank_damping: 0.85,
            pagerank_iterations: 20,
            synonym_expansion_enabled: true,
            acronym_expansion_enabled: true,
            rrf_enabled: true,
            rrf_k: 60.0,
            rrf_keyword_weight: 1.0,
            rrf_vector_weight: 1.0,
            rrf_graph_weight: 0.5,
            hyde_enabled: false,
            hyde_llm_backend: "openai".to_string(),
            hyde_api_key: None,
            hyde_max_tokens: 512,
            metrics_enabled: true,
            metrics_port: 9090,
            package_detection_enabled: true,
            llm_enabled: true,
            llm_device: crate::config::EmbeddingsDevice::Cpu,
            llm_model_dir: None,
            llm_max_tokens: 30,
            llm_batch_commit: 10,
            answer_llm_n_ctx: 16384,
            sampling_descriptions_enabled: true,
            leader_election_enabled: false,
            leader_heartbeat_interval_ms: 10_000,
            leader_ttl_seconds: 30,
            embedding_truncate_dim: None,
            embedding_dim_override: None,
        };

        // Parse file (should be unchanged)
        let result = parse_single_file(&test_file, &config, &conn);

        match result {
            ParseResult::Unchanged => {
                // Success
            }
            ParseResult::Parsed(_) => panic!("File should be unchanged"),
            ParseResult::Skipped { reason } => panic!("File should not be skipped: {}", reason),
        }

        // Cleanup
        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    #[test]
    fn test_parse_single_file_unsupported_language() {
        let tmp_dir = temp_dir();
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let base_dir = Utf8PathBuf::from_path_buf(tmp_dir.clone()).unwrap();
        let db_path = base_dir.join("test.db");

        // Create test database
        let conn = Connection::open(&db_path.as_str()).unwrap();

        // Create test file with unsupported extension
        let test_file = tmp_dir.join("test.txt");
        std::fs::write(&test_file, "hello world").unwrap();

        let config = Config {
            base_dir: base_dir.clone(),
            db_path: db_path.clone(),
            vector_db_path: base_dir.join("vectors"),
            tantivy_index_path: base_dir.join("tantivy"),
            embeddings_backend: crate::config::EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 8,
            vector_search_limit: 10,
            vector_guaranteed_results: 3,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 1.0,
            rank_index_file_boost: 0.0,
            rank_test_penalty: -5.0,
            rank_popularity_weight: 0.0,
            rank_popularity_cap: 0,
            index_patterns: vec![],
            exclude_patterns: vec![],
            watch_mode: false,
            watch_debounce_ms: 100,
            watch_min_index_interval_ms: 50,
            max_context_bytes: 10_000,
            index_node_modules: false,
            repo_roots: vec![base_dir.clone()],
            reranker_enabled: false,
            reranker_model_path: None,
            reranker_top_k: 20,
            reranker_cache_dir: None,
            learning_enabled: false,
            learning_selection_boost: 0.1,
            learning_file_affinity_boost: 0.05,
            max_context_tokens: 8192,
            token_encoding: "o200k_base".to_string(),
            parallel_workers: 4,
            embedding_cache_enabled: true,
            pagerank_damping: 0.85,
            pagerank_iterations: 20,
            synonym_expansion_enabled: true,
            acronym_expansion_enabled: true,
            rrf_enabled: true,
            rrf_k: 60.0,
            rrf_keyword_weight: 1.0,
            rrf_vector_weight: 1.0,
            rrf_graph_weight: 0.5,
            hyde_enabled: false,
            hyde_llm_backend: "openai".to_string(),
            hyde_api_key: None,
            hyde_max_tokens: 512,
            metrics_enabled: true,
            metrics_port: 9090,
            package_detection_enabled: true,
            llm_enabled: true,
            llm_device: crate::config::EmbeddingsDevice::Cpu,
            llm_model_dir: None,
            llm_max_tokens: 30,
            llm_batch_commit: 10,
            answer_llm_n_ctx: 16384,
            sampling_descriptions_enabled: true,
            leader_election_enabled: false,
            leader_heartbeat_interval_ms: 10_000,
            leader_ttl_seconds: 30,
            embedding_truncate_dim: None,
            embedding_dim_override: None,
        };

        // Parse file (should be skipped)
        let result = parse_single_file(&test_file, &config, &conn);

        match result {
            ParseResult::Skipped { reason } => {
                assert!(reason.contains("Unsupported language"));
            }
            ParseResult::Unchanged => panic!("File should be skipped, not unchanged"),
            ParseResult::Parsed(_) => panic!("File should be skipped"),
        }

        // Cleanup
        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    /// Regression test for the q1 / pylon cross-file class-method edge loss
    /// observed at R028. When two TS files were indexed in the same fresh
    /// run, `pr-review-manager.ts -> sessionManager.createSession` had zero
    /// outgoing call edges because Phase 1 ran edge extraction against an
    /// empty SQLite (Phase 2 had not yet written symbols). The two-pass
    /// pipeline -- write symbols first, then extract edges -- restores the
    /// edge.
    #[test]
    fn cross_file_receiver_method_edge_resolves_after_two_pass_write() {
        use crate::storage::sqlite::queries;
        use crate::storage::sqlite::schema::SCHEMA_SQL;

        let tmp_dir = temp_dir();
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let base_dir = Utf8PathBuf::from_path_buf(tmp_dir.clone()).unwrap();
        let db_path = base_dir.join("test.db");

        // Two TS files: session-manager.ts (definer) and pr-review-manager.ts
        // (caller). Mirrors the pylon shape the bench exercises.
        let target_file = tmp_dir.join("session-manager.ts");
        std::fs::write(
            &target_file,
            "class SessionManager {\n  createSession(cwd: string): string {\n    return 'sid'\n  }\n}\nexport const sessionManager = new SessionManager()\n",
        )
        .unwrap();

        let caller_file = tmp_dir.join("pr-review-manager.ts");
        std::fs::write(
            &caller_file,
            "import { sessionManager } from './session-manager'\nexport function runRevalidationSession(): string {\n  return sessionManager.createSession('/cwd')\n}\n",
        )
        .unwrap();

        // Real on-disk DB so parse_single_file's fingerprint check and the
        // edge-phase cross-file lookups share state.
        let conn = Connection::open(db_path.as_str()).unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();

        let config = test_config(base_dir.clone(), db_path.clone());

        // Phase 1 surrogate: parse each file with the (empty) DB.
        let target_parsed = match parse_single_file(&target_file, &config, &conn) {
            ParseResult::Parsed(pf) => *pf,
            other => panic!("target parse failed: {:?}", other),
        };
        let caller_parsed = match parse_single_file(&caller_file, &config, &conn) {
            ParseResult::Parsed(pf) => *pf,
            other => panic!("caller parse failed: {:?}", other),
        };

        // ParsedFile.edges should be empty after parse — edge extraction is
        // deferred to a phase that runs after symbols are written.
        assert!(
            target_parsed.edges.is_empty(),
            "edge extraction must be deferred out of parse_single_file"
        );
        assert!(
            caller_parsed.edges.is_empty(),
            "edge extraction must be deferred out of parse_single_file"
        );

        // Phase 2 surrogate: write symbols only. (We bypass Tantivy here;
        // we're testing SQLite-backed cross-file edge resolution.)
        queries::symbols::batch_upsert_symbols(&conn, &target_parsed.symbol_rows).unwrap();
        queries::symbols::batch_upsert_symbols(&conn, &caller_parsed.symbol_rows).unwrap();

        // Phase 2.5: extract edges with the populated DB visible.
        let edges = extract_edges_for_parsed_file(&caller_parsed, &conn);

        // The target method's stable id under the indexer's scheme.
        let target_method_id = target_parsed
            .symbol_rows
            .iter()
            .find(|r| r.name == "createSession")
            .expect("createSession symbol must be indexed in session-manager.ts")
            .id
            .clone();

        let call_edge = edges
            .iter()
            .find(|(e, _)| e.edge_type == "call" && e.to_symbol_id == target_method_id);
        assert!(
            call_edge.is_some(),
            "expected a call edge from runRevalidationSession to createSession in session-manager.ts; got: {:?}",
            edges
                .iter()
                .map(|(e, _)| (e.edge_type.clone(), e.to_symbol_id.clone()))
                .collect::<Vec<_>>()
        );

        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    fn test_config(base_dir: Utf8PathBuf, db_path: Utf8PathBuf) -> Config {
        Config {
            base_dir: base_dir.clone(),
            db_path,
            vector_db_path: base_dir.join("vectors"),
            tantivy_index_path: base_dir.join("tantivy"),
            embeddings_backend: crate::config::EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 8,
            vector_search_limit: 10,
            vector_guaranteed_results: 3,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 1.0,
            rank_index_file_boost: 0.0,
            rank_test_penalty: -5.0,
            rank_popularity_weight: 0.0,
            rank_popularity_cap: 0,
            index_patterns: vec![],
            exclude_patterns: vec![],
            watch_mode: false,
            watch_debounce_ms: 100,
            watch_min_index_interval_ms: 50,
            max_context_bytes: 10_000,
            index_node_modules: false,
            repo_roots: vec![base_dir.clone()],
            reranker_enabled: false,
            reranker_model_path: None,
            reranker_top_k: 20,
            reranker_cache_dir: None,
            learning_enabled: false,
            learning_selection_boost: 0.1,
            learning_file_affinity_boost: 0.05,
            max_context_tokens: 8192,
            token_encoding: "o200k_base".to_string(),
            parallel_workers: 2,
            embedding_cache_enabled: true,
            pagerank_damping: 0.85,
            pagerank_iterations: 20,
            synonym_expansion_enabled: true,
            acronym_expansion_enabled: true,
            rrf_enabled: true,
            rrf_k: 60.0,
            rrf_keyword_weight: 1.0,
            rrf_vector_weight: 1.0,
            rrf_graph_weight: 0.5,
            hyde_enabled: false,
            hyde_llm_backend: "openai".to_string(),
            hyde_api_key: None,
            hyde_max_tokens: 512,
            metrics_enabled: false,
            metrics_port: 9090,
            package_detection_enabled: false,
            llm_enabled: false,
            llm_device: crate::config::EmbeddingsDevice::Cpu,
            llm_model_dir: None,
            llm_max_tokens: 30,
            llm_batch_commit: 10,
            answer_llm_n_ctx: 16384,
            sampling_descriptions_enabled: false,
            leader_election_enabled: false,
            leader_heartbeat_interval_ms: 10_000,
            leader_ttl_seconds: 30,
            embedding_truncate_dim: None,
            embedding_dim_override: None,
        }
    }
}

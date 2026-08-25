use crate::indexer::extract::symbol::{
    DataFlowEdge, ExtractedInheritanceRelation, Import, JSDocEntry, ModuleBinding,
    ModuleBindingKind, TodoEntry,
};
use crate::indexer::pipeline::utils::FileFingerprint;
use crate::storage::sqlite::schema::{
    DecoratorRow, DocMetaRow, EdgeEvidenceRow, EdgeRow, FrameworkPatternRow, ModuleBindingRow,
    UsageExampleRow,
};
use crate::storage::sqlite::{SymbolIdentityRow, SymbolRow};

use crate::{
    config::Config,
    indexer::{
        extract::csharp::extract_csharp_symbols,
        extract::go::extract_go_symbols,
        extract::java::extract_java_symbols,
        extract::javascript::extract_javascript_symbols,
        extract::kotlin::extract_kotlin_symbols,
        extract::markdown::extract_markdown_symbols,
        extract::python::extract_python_symbols,
        extract::ruby::extract_ruby_symbols,
        extract::rust::extract_rust_symbols,
        extract::swift::extract_swift_symbols,
        extract::typescript::extract_typescript_symbols_with_path,
        extract::{c::extract_c_symbols, cpp::extract_cpp_symbols},
        parser::{language_id_for_path, LanguageId},
        pipeline::{
            doc_links,
            edges::{extract_edges_for_symbol, upsert_name_mapping, PackageLookupFn},
            identity::build_symbol_occurrences,
            usage::extract_usage_examples_for_file,
            utils::{
                content_hash_hex, file_fingerprint, file_key_path, language_string,
                stable_symbol_id,
            },
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

pub const MAX_SOURCE_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Full output of parsing one file — everything needed to write.
///
/// `edges` is empty after parsing. Edge extraction is deferred to a separate
/// pipeline phase that runs after symbols are written, so cross-file edge
/// resolution (which queries SQLite for receiver-method targets) can see the
/// just-indexed symbols. The `imports`, `type_edges`, and `dataflow_edges`
/// fields carry the AST-derived inputs that the deferred edge extraction
/// needs, since the tree-sitter parse is discarded by then.
#[derive(Debug, Clone)]
pub struct ParsedFile {
    pub rel_path: String,
    pub fingerprint: FileFingerprint,
    pub content_hash: String,
    pub language: String,
    pub symbol_rows: Vec<SymbolRow>,
    pub symbol_identities: Vec<SymbolIdentityRow>,
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
    pub module_bindings: Vec<ModuleBinding>,
    pub type_edges: Vec<(String, String)>,
    pub inheritance_relations: Vec<ExtractedInheritanceRelation>,
    pub dataflow_edges: Vec<DataFlowEdge>,
    /// Documentation metadata (front-matter + path classification) for
    /// markdown files; `None` for code files.
    pub doc_meta: Option<DocMetaRow>,
}

/// Result of parsing a single file
#[derive(Debug)]
pub enum ParseResult {
    /// File unchanged (mtime and size matched), skip
    Unchanged,
    /// File content is unchanged but its mtime or size moved, so the stored
    /// fingerprint needs restamping. No reparse, no re-embed.
    Restamped {
        file_path: String,
        fingerprint: FileFingerprint,
        content_hash: String,
    },
    /// Fully parsed file with all extracted data
    Parsed(Box<ParsedFile>),
    /// File skipped (unsupported language, read error, etc.)
    Skipped { reason: String, file_path: String },
}

/// Parse a single file and return all extracted data.
/// Takes a read-only SQLite connection for fingerprint checks and cross-file lookups.
/// Does NOT write to any storage backend.
pub fn parse_single_file(file: &Path, config: &Config, conn: &Connection) -> ParseResult {
    let rel = file_key_path(config, file);

    // 1. Determine language from path
    let language_id = match language_id_for_path(file) {
        Some(id) => id,
        None => {
            return ParseResult::Skipped {
                reason: format!("Unsupported language for file: {}", file.display()),
                file_path: rel,
            };
        }
    };

    // 2. Get file fingerprint
    let fp = match file_fingerprint(file) {
        Ok(fp) => fp,
        Err(e) => {
            return ParseResult::Skipped {
                reason: format!("Failed to fingerprint file: {}", e),
                file_path: rel,
            };
        }
    };

    // 3. Load the stored fingerprint once. mtime plus size is the cheap check
    //    and keeps the steady-state watcher path free of file reads.
    let stored = conn
        .query_row(
            "SELECT mtime_ns, size_bytes, content_hash FROM file_fingerprints WHERE file_path = ?1",
            [&rel],
            |row| {
                let mtime: i64 = row.get(0)?;
                let size: i64 = row.get(1)?;
                let hash: Option<String> = row.get(2)?;
                Ok((mtime, size as u64, hash))
            },
        )
        .optional();
    let stored = match stored {
        Ok(stored) => stored,
        // A real SQL error reads the same as "no stored row" from here on, which
        // means a full reparse of the whole tree. Say so, or a database reached
        // before `init()` added `content_hash` looks like a mysteriously cold
        // index rather than a schema problem.
        Err(error) => {
            tracing::debug!(
                file = %rel,
                %error,
                "could not read the stored fingerprint, treating the file as new"
            );
            None
        }
    };

    if let Some((mtime, size, _)) = &stored {
        if *mtime == fp.mtime_ns && *size == fp.size_bytes {
            return ParseResult::Unchanged;
        }
    }

    if fp.size_bytes > MAX_SOURCE_FILE_BYTES {
        return ParseResult::Skipped {
            reason: format!(
                "File too large to index: {} bytes exceeds {} bytes",
                fp.size_bytes, MAX_SOURCE_FILE_BYTES
            ),
            file_path: rel,
        };
    }

    // 4. Read the file once. The bytes serve both the content hash and the
    //    tree-sitter parse below.
    let source = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            return ParseResult::Skipped {
                reason: format!("Failed to read file: {}", e),
                file_path: rel,
            };
        }
    };

    let content_hash = content_hash_hex(source.as_bytes());

    // 4b. Second chance: git rewrites mtimes wholesale on checkout, rebase, and
    //     worktree creation, so a stat mismatch does not imply a content change.
    if let Some((_, _, Some(stored_hash))) = &stored {
        if stored_hash == &content_hash {
            return ParseResult::Restamped {
                file_path: rel,
                fingerprint: fp,
                content_hash,
            };
        }
    }

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
        LanguageId::Markdown => extract_markdown_symbols(&source),
    };

    let mut extracted = match extracted {
        Ok(syms) => syms,
        Err(e) => {
            return ParseResult::Skipped {
                reason: format!("Failed to extract symbols: {}", e),
                file_path: rel,
            };
        }
    };

    // 6. Build source-addressable occurrences and logical identities (include
    // one file-level symbol). The canonical occurrence keeps the legacy
    // location-independent id; overloads/partials receive distinct ids.
    let language = language_string(language_id).to_string();
    // Some extractors emit TODO/FIXME rows without a file path (e.g. the Rust
    // extractor passes ""); stamp the parse-time relative path here so todo
    // consumers (issue tracking, debt views) always have provenance.
    for todo in &mut extracted.todos {
        if todo.file_path.is_empty() {
            todo.file_path = rel.clone();
        }
    }
    let (mut symbol_rows, mut symbol_identities) =
        match build_symbol_occurrences(&rel, &language, &source, &extracted.symbols) {
            Ok(rows) => rows,
            Err(error) => {
                return ParseResult::Skipped {
                    reason: format!("Failed to allocate symbol identities: {error}"),
                    file_path: rel,
                };
            }
        };

    // Add file-level symbol
    let file_symbol_id = stable_symbol_id(&rel, "FILE_ROOT", 0);
    symbol_rows.insert(
        0,
        SymbolRow {
            id: file_symbol_id.clone(),
            file_path: rel.clone(),
            language: language.clone(),
            kind: "file".to_string(),
            name: rel.clone(),
            exported: false,
            start_byte: 0,
            end_byte: source.len() as u32,
            start_line: 1,
            end_line: source.lines().count() as u32,
            text: source.clone(),
        },
    );
    symbol_identities.insert(
        0,
        SymbolIdentityRow {
            symbol_id: file_symbol_id.clone(),
            logical_id: file_symbol_id,
            qualified_name: rel.clone(),
            signature: "file".to_string(),
            occurrence_discriminator: "file_root".to_string(),
            is_canonical: true,
        },
    );

    // Preserve module-level public names independently from symbol flags. A
    // direct export is a binding from the file's API surface to its concrete
    // symbol. Python package initializers expose public from-imports as
    // re-exports, matching how packages such as django.urls define their API.
    let mut module_bindings = extracted.module_bindings;
    if !matches!(
        language_id,
        LanguageId::Typescript | LanguageId::Tsx | LanguageId::Javascript | LanguageId::Python
    ) {
        for import in &extracted.imports {
            let (imported_name, local_name) = match language_id {
                LanguageId::Rust | LanguageId::Java | LanguageId::Kotlin | LanguageId::CSharp => {
                    let imported_name = if matches!(
                        language_id,
                        LanguageId::Java | LanguageId::Kotlin | LanguageId::CSharp
                    ) {
                        import
                            .source
                            .split('.')
                            .next_back()
                            .unwrap_or(&import.name)
                            .to_string()
                    } else {
                        import.name.clone()
                    };
                    (
                        imported_name,
                        import.alias.clone().unwrap_or_else(|| import.name.clone()),
                    )
                }
                LanguageId::Go
                | LanguageId::Swift
                | LanguageId::C
                | LanguageId::Cpp
                | LanguageId::Ruby => (
                    "*".to_string(),
                    import.alias.clone().unwrap_or_else(|| import.name.clone()),
                ),
                _ => unreachable!("explicit binding extractor handles this language"),
            };
            let binding = ModuleBinding {
                kind: ModuleBindingKind::Import,
                source: import.source.clone(),
                imported_name,
                local_name,
                exported_name: String::new(),
                at_line: import.at_line,
            };
            let duplicate = module_bindings.iter().any(|existing| {
                existing.kind == binding.kind
                    && existing.source == binding.source
                    && existing.imported_name == binding.imported_name
                    && existing.local_name == binding.local_name
                    && existing.at_line == binding.at_line
            });
            if !duplicate {
                module_bindings.push(binding);
            }
        }
    }
    if language_id == LanguageId::Python && (rel == "__init__.py" || rel.ends_with("/__init__.py"))
    {
        for binding in &mut module_bindings {
            if binding.kind == ModuleBindingKind::Import && !binding.local_name.starts_with('_') {
                binding.kind = if binding.imported_name == "*" {
                    ModuleBindingKind::ExportAll
                } else {
                    ModuleBindingKind::ReExport
                };
                binding.exported_name = if binding.imported_name == "*" {
                    "*".to_string()
                } else {
                    binding.local_name.clone()
                };
            }
        }
    }

    for row in symbol_rows.iter().filter(|row| {
        row.kind != "file"
            && row.exported
            && (language_id != LanguageId::Python || !row.name.contains('.'))
    }) {
        let already_explicit = module_bindings.iter().any(|binding| {
            binding.kind == ModuleBindingKind::Export && binding.local_name == row.name
        });
        if !already_explicit {
            module_bindings.push(ModuleBinding {
                kind: ModuleBindingKind::Export,
                source: String::new(),
                imported_name: String::new(),
                local_name: row.name.clone(),
                exported_name: row.name.clone(),
                at_line: row.start_line,
            });
        }
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
    let doc_meta = build_doc_meta(&rel, &source);
    ParseResult::Parsed(Box::new(ParsedFile {
        rel_path: rel,
        fingerprint: fp,
        content_hash,
        language: language_string(language_id).to_string(),
        symbol_rows,
        symbol_identities,
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
        module_bindings,
        type_edges: extracted.type_edges,
        inheritance_relations: extracted.inheritance_relations,
        dataflow_edges: extracted.dataflow_edges,
        doc_meta,
    }))
}

/// Build documentation metadata for a markdown file: path classification
/// refined by YAML front-matter. Returns `None` for non-markdown files.
fn build_doc_meta(rel_path: &str, source: &str) -> Option<DocMetaRow> {
    use crate::indexer::extract::markdown::{classify_doc_path, parse_front_matter};
    if !rel_path.to_lowercase().ends_with(".md") {
        return None;
    }
    let fm = parse_front_matter(source).unwrap_or_default();
    Some(DocMetaRow {
        file_path: rel_path.to_string(),
        doc_type: classify_doc_path(rel_path).as_str().to_string(),
        status: fm.status,
        date: fm.date,
        number: fm.number,
        labels: fm.labels,
    })
}

/// One file's edge bundle: each entry is an edge row plus its evidence rows.
pub type EdgeBundle = Vec<(EdgeRow, Vec<EdgeEvidenceRow>)>;

pub struct BindingFileBundle {
    pub binding_edges: EdgeBundle,
    pub module_bindings: Vec<ModuleBindingRow>,
}

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
        // Document sections contribute `documents` cross-link edges instead of
        // call/type/dataflow edges (docs-indexing design, Phase 3): backtick
        // refs resolve to code symbols, TODO/issue numbers produce `tracks`
        // edges. Prose never resolves as spurious calls.
        if row.kind == "document" {
            let issue_number = parsed.doc_meta.as_ref().and_then(|m| m.number);
            match doc_links::extract_doc_link_edges(row, issue_number, conn) {
                Ok((doc_edges, _stale)) => all_edges.extend(doc_edges),
                Err(error) => tracing::warn!(
                    file = %row.file_path,
                    section = %row.name,
                    %error,
                    "Failed to extract document cross-links"
                ),
            }
            continue;
        }
        let edges = extract_edges_for_symbol(
            row,
            &name_to_id,
            &id_to_symbol,
            &parsed.imports,
            &parsed.type_edges,
            &parsed.inheritance_relations,
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
                            file_path: file_key_path(config, file),
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

/// Resolve module bindings after every symbol in the batch is visible.
///
/// The returned rows are persisted before ordinary edge extraction. That
/// makes public-name indirection (`default`, renamed exports, chained barrels)
/// available to the DB-backed call/reference resolver in the next phase.
pub fn resolve_bindings_for_files(
    parsed_files: &[ParsedFile],
    config: &Config,
    pool: &SqlitePool,
) -> Result<Vec<BindingFileBundle>> {
    let rayon_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.parallel_workers)
        .thread_name(|i| format!("bindings-{}", i))
        .build()
        .context("Failed to build Rayon thread pool for binding resolution")?;
    let catalog = super::bindings::BindingCatalog::from_parsed_files(parsed_files);

    rayon_pool.install(|| {
        parsed_files
            .par_iter()
            .map(|parsed| {
                let conn = pool.get().with_context(|| {
                    format!(
                        "Failed to get DB connection for binding resolution: {}",
                        parsed.rel_path
                    )
                })?;
                let (module_bindings, binding_edges) =
                    super::bindings::resolve_for_file(parsed, &conn, &catalog)?;
                Ok(BindingFileBundle {
                    binding_edges,
                    module_bindings,
                })
            })
            .collect::<Result<Vec<_>>>()
    })
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

    /// The full Config literal previously duplicated across three tests.
    /// Only `repo_roots` (via `root`) varies between call sites.
    fn test_config(root: &crate::path::Utf8Path) -> Config {
        Config {
            base_dir: root.to_path_buf(),
            db_path: root.join("test.db"),
            vector_db_path: root.join("vectors"),
            tantivy_index_path: root.join("tantivy"),
            embeddings_backend: crate::config::EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 8,
            vector_search_limit: 10,
            vector_guaranteed_results: 3,
            docs_max_hits: 4,
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
            repo_roots: vec![root.to_path_buf()],
            reranker_enabled: false,
            descriptions_enabled: false,
            store_query_text: false,
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
            external_index_auto: false,
            external_index_producer: None,
            external_index_on_refresh: "disabled".to_string(),
            external_index_min_interval_ms: 60_000,
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
        }
    }

    /// Return the `TempDir` so the caller keeps it alive; dropping it here
    /// would delete the directory out from under the test.
    fn test_setup() -> (Config, Connection, Utf8PathBuf, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::storage::sqlite::schema::SCHEMA_SQL)
            .unwrap();
        let config = test_config(root.as_path());
        (config, conn, root, dir)
    }

    #[test]
    fn test_parse_single_file_rust() {
        let tmp_dir = temp_dir();
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let base_dir = Utf8PathBuf::from_path_buf(tmp_dir.clone()).unwrap();
        let db_path = base_dir.join("test.db");

        // Create test database
        let conn = Connection::open(db_path.as_str()).unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS file_fingerprints (
                file_path TEXT PRIMARY KEY,
                mtime_ns INTEGER,
                size_bytes INTEGER,
                content_hash TEXT,
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
            "use crate::math::Number;\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();

        let config = test_config(base_dir.as_path());

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
                assert!(parsed.module_bindings.iter().any(|binding| {
                    binding.kind == ModuleBindingKind::Import
                        && binding.source == "crate::math::Number"
                        && binding.imported_name == "Number"
                        && binding.local_name == "Number"
                        && binding.at_line == 1
                }));
            }
            ParseResult::Unchanged => panic!("File should not be unchanged on first parse"),
            ParseResult::Restamped { .. } => {
                panic!("File should not be restamped on first parse")
            }
            ParseResult::Skipped { reason, .. } => {
                panic!("File should not be skipped: {}", reason)
            }
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
        let conn = Connection::open(db_path.as_str()).unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS file_fingerprints (
                file_path TEXT PRIMARY KEY,
                mtime_ns INTEGER,
                size_bytes INTEGER,
                content_hash TEXT,
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

        let config = test_config(base_dir.as_path());

        // Parse file (should be unchanged)
        let result = parse_single_file(&test_file, &config, &conn);

        match result {
            ParseResult::Unchanged => {
                // Success
            }
            ParseResult::Restamped { .. } => panic!("File should be unchanged, not restamped"),
            ParseResult::Parsed(_) => panic!("File should be unchanged"),
            ParseResult::Skipped { reason, .. } => {
                panic!("File should not be skipped: {}", reason)
            }
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
        let conn = Connection::open(db_path.as_str()).unwrap();

        // Create test file with unsupported extension
        let test_file = tmp_dir.join("test.txt");
        std::fs::write(&test_file, "hello world").unwrap();

        let config = test_config(base_dir.as_path());

        // Parse file (should be skipped)
        let result = parse_single_file(&test_file, &config, &conn);

        match result {
            ParseResult::Skipped { reason, .. } => {
                assert!(reason.contains("Unsupported language"));
            }
            ParseResult::Unchanged => panic!("File should be skipped, not unchanged"),
            ParseResult::Restamped { .. } => panic!("File should be skipped, not restamped"),
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

        let config = two_pass_test_config(base_dir.clone(), db_path.clone());

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

    /// Config for the two-pass cross-file edge test, which needs a real
    /// on-disk DB file (distinct from the in-memory `test_config` above)
    /// shared between `parse_single_file` and the deferred edge phase.
    fn two_pass_test_config(base_dir: Utf8PathBuf, db_path: Utf8PathBuf) -> Config {
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
            docs_max_hits: 4,
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
            descriptions_enabled: false,
            store_query_text: false,
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
            external_index_auto: false,
            external_index_producer: None,
            external_index_on_refresh: "disabled".to_string(),
            external_index_min_interval_ms: 60_000,
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

    #[test]
    fn touched_file_with_identical_content_is_restamped_not_reparsed() {
        // Arrange: index a file once so a fingerprint row exists.
        let (config, conn, dir, _tmp) = test_setup();
        let file = dir.join("lib.rs");
        std::fs::write(&file, "pub fn probe() -> usize { 1 }\n").unwrap();

        let first = parse_single_file(file.as_std_path(), &config, &conn);
        let parsed = match first {
            ParseResult::Parsed(p) => p,
            other => panic!("first parse must produce Parsed, got {other:?}"),
        };
        crate::storage::sqlite::queries::files::upsert_file_fingerprint(
            &conn,
            &parsed.rel_path,
            parsed.fingerprint.mtime_ns,
            parsed.fingerprint.size_bytes,
            Some(&parsed.content_hash),
        )
        .unwrap();

        // Act: rewrite the same bytes so mtime moves but content does not.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&file, "pub fn probe() -> usize { 1 }\n").unwrap();
        let second = parse_single_file(file.as_std_path(), &config, &conn);

        // Assert: recognised as unchanged, and the new stat is reported for restamping.
        match second {
            ParseResult::Restamped {
                file_path,
                fingerprint,
                content_hash,
            } => {
                assert_eq!(file_path, parsed.rel_path);
                assert_eq!(content_hash, parsed.content_hash);
                assert_ne!(
                    fingerprint.mtime_ns, parsed.fingerprint.mtime_ns,
                    "restamp must carry the new mtime, not the stored one"
                );
            }
            other => panic!("expected Restamped, got {other:?}"),
        }
    }

    #[test]
    fn unchanged_stat_still_short_circuits_without_hashing() {
        let (config, conn, dir, _tmp) = test_setup();
        let file = dir.join("lib.rs");
        std::fs::write(&file, "pub fn probe() -> usize { 1 }\n").unwrap();

        let parsed = match parse_single_file(file.as_std_path(), &config, &conn) {
            ParseResult::Parsed(p) => p,
            other => panic!("expected Parsed, got {other:?}"),
        };
        crate::storage::sqlite::queries::files::upsert_file_fingerprint(
            &conn,
            &parsed.rel_path,
            parsed.fingerprint.mtime_ns,
            parsed.fingerprint.size_bytes,
            Some(&parsed.content_hash),
        )
        .unwrap();

        // Nothing touched the file, so the mtime and size path wins.
        assert!(matches!(
            parse_single_file(file.as_std_path(), &config, &conn),
            ParseResult::Unchanged
        ));
    }

    #[test]
    fn legacy_null_hash_falls_back_to_reparse() {
        let (config, conn, dir, _tmp) = test_setup();
        let file = dir.join("lib.rs");
        std::fs::write(&file, "pub fn probe() -> usize { 1 }\n").unwrap();

        let parsed = match parse_single_file(file.as_std_path(), &config, &conn) {
            ParseResult::Parsed(p) => p,
            other => panic!("expected Parsed, got {other:?}"),
        };
        // Simulate a pre-migration row: stat differs, hash is NULL.
        crate::storage::sqlite::queries::files::upsert_file_fingerprint(
            &conn,
            &parsed.rel_path,
            parsed.fingerprint.mtime_ns - 1,
            parsed.fingerprint.size_bytes,
            None,
        )
        .unwrap();

        assert!(matches!(
            parse_single_file(file.as_std_path(), &config, &conn),
            ParseResult::Parsed(_)
        ));
    }

    #[test]
    fn changed_content_still_reparses() {
        let (config, conn, dir, _tmp) = test_setup();
        let file = dir.join("lib.rs");
        std::fs::write(&file, "pub fn probe() -> usize { 1 }\n").unwrap();

        let parsed = match parse_single_file(file.as_std_path(), &config, &conn) {
            ParseResult::Parsed(p) => p,
            other => panic!("expected Parsed, got {other:?}"),
        };
        crate::storage::sqlite::queries::files::upsert_file_fingerprint(
            &conn,
            &parsed.rel_path,
            parsed.fingerprint.mtime_ns,
            parsed.fingerprint.size_bytes,
            Some(&parsed.content_hash),
        )
        .unwrap();

        std::fs::write(&file, "pub fn probe() -> usize { 2 }\n").unwrap();
        assert!(matches!(
            parse_single_file(file.as_std_path(), &config, &conn),
            ParseResult::Parsed(_)
        ));
    }
}

use crate::{
    config::Config,
    indexer::{
        extract::go::extract_go_symbols,
        extract::java::extract_java_symbols,
        extract::javascript::extract_javascript_symbols,
        extract::python::extract_python_symbols,
        extract::rust::extract_rust_symbols,
        extract::typescript::extract_typescript_symbols_with_path,
        extract::{c::extract_c_symbols, cpp::extract_cpp_symbols},
        parser::{language_id_for_path, LanguageId},
        pipeline::{
            edges::{extract_edges_for_symbol, upsert_name_mapping},
            parsing::symbol_kind_to_string,
            stats::IndexRunStats,
            usage::extract_usage_examples_for_file,
            utils::{file_fingerprint, file_key, language_string, stable_symbol_id},
        },
    },
    storage::{
        sqlite::{schema::DecoratorRow, SqliteStore, SymbolRow},
        tantivy::TantivyIndex,
        vector::LanceVectorTable,
    },
};
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

/// Result of indexing a single file
#[derive(Debug)]
pub struct FileIndexResult {
    pub file_path: PathBuf,
    pub success: bool,
    pub symbols_count: usize,
    pub error: Option<String>,
}

/// Index a single file with retry logic
///
/// This function handles indexing a single file with the following steps:
/// 1. Check if file is unchanged (skip if so)
/// 2. Extract symbols using tree-sitter
/// 3. Delete old data from SQLite/Tantivy/LanceDB
/// 4. Generate embeddings
/// 5. Insert new data into all storage backends
///
/// Retries up to 2 times on failure before giving up.
fn index_file_with_retry(
    file: &Path,
    config: &Config,
    db_path: &Path,
    tantivy: &TantivyIndex,
    vectors: &LanceVectorTable,
    max_retries: usize,
) -> FileIndexResult {
    let mut attempt = 0;
    let mut last_error = None;

    while attempt <= max_retries {
        match index_file_single(file, config, db_path, tantivy, vectors) {
            Ok(count) => {
                return FileIndexResult {
                    file_path: file.to_path_buf(),
                    success: true,
                    symbols_count: count,
                    error: None,
                };
            }
            Err(err) => {
                last_error = Some(err.to_string());
                attempt += 1;
                if attempt <= max_retries {
                    tracing::warn!(
                        file = %file.display(),
                        attempt = attempt,
                        error = %err,
                        "Indexing failed, retrying..."
                    );
                    // Small delay before retry
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }

    FileIndexResult {
        file_path: file.to_path_buf(),
        success: false,
        symbols_count: 0,
        error: last_error,
    }
}

/// Index a single file (no retry) - parses and stores symbol data
///
/// This is the core indexing logic that:
/// - Reads file content
/// - Extracts symbols via tree-sitter
/// - Updates SQLite, Tantivy indexes
/// - Does NOT update vectors (done in a separate batch step)
fn index_file_single(
    file: &Path,
    config: &Config,
    db_path: &Path,
    tantivy: &TantivyIndex,
    _vectors: &LanceVectorTable,
) -> Result<usize> {
    let rel = file_key(config, file);

    let language_id = language_id_for_path(file)
        .ok_or_else(|| anyhow::anyhow!("Unsupported language for file: {}", file.display()))?;

    let fp = file_fingerprint(file)?;

    // Per-thread SQLite connection
    let sqlite = SqliteStore::open(db_path)?;
    sqlite.init()?;

    // Log package membership for this file
    match sqlite.get_package_for_file(&rel) {
        Ok(Some(pkg)) => {
            tracing::debug!(
                file = %rel,
                package_id = %pkg.id,
                package_name = %pkg.name,
                "Indexing file with package"
            );
        }
        Ok(None) => {
            tracing::trace!(
                file = %rel,
                "No package found for file during indexing"
            );
        }
        Err(err) => {
            tracing::warn!(
                file = %rel,
                error = %err,
                "Failed to look up package for file"
            );
        }
    }

    // Check if unchanged
    let is_unchanged = sqlite.get_file_fingerprint(&rel)?.is_some_and(|existing| {
        existing.mtime_ns == fp.mtime_ns && existing.size_bytes == fp.size_bytes
    });

    if is_unchanged {
        return Ok(0); // File unchanged, skip
    }

    let source = fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

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
    }
    .with_context(|| format!("Failed to extract symbols from: {}", file.display()))?;

    // Delete old data
    tantivy.delete_symbols_by_file(&rel)?;
    // Note: We skip vector deletion here to avoid async in sync context
    // Vectors will be updated in batch later

    // Delete symbols first - test_links have ON DELETE CASCADE, so they auto-delete
    sqlite.delete_symbols_by_file(&rel)?;
    sqlite.delete_usage_examples_by_file(&rel)?;
    sqlite.delete_todos_by_file(&rel)?;
    sqlite.delete_docstrings_by_file(&rel)?;
    sqlite.delete_decorators_by_file(&rel)?;
    // Note: test_links auto-delete via ON DELETE CASCADE when symbols are deleted

    let mut name_to_id: HashMap<String, String> = HashMap::new();
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

    if symbol_rows.is_empty() {
        sqlite.upsert_file_fingerprint(&rel, fp.mtime_ns, fp.size_bytes)?;
        return Ok(0);
    }

    // Update Tantivy
    for row in &symbol_rows {
        tantivy.upsert_symbol(row)?;
        upsert_name_mapping(&mut name_to_id, row);
    }
    tantivy.commit()?;

    // Build id_to_symbol HashMap for edge extraction
    let id_to_symbol: HashMap<String, &SymbolRow> =
        symbol_rows.iter().map(|r| (r.id.clone(), r)).collect();

    // Update SQLite
    for row in &symbol_rows {
        sqlite.upsert_symbol(row)?;
    }

    for row in &symbol_rows {
        // Create package lookup function for cross-package edge resolution
        let db_path_for_lookup = db_path.to_path_buf();
        let package_lookup_fn: super::edges::PackageLookupFn =
            Box::new(move |file_path: &str| -> Option<String> {
                if let Ok(sqlite) = SqliteStore::open(&db_path_for_lookup) {
                    if let Ok(Some(pkg)) = sqlite.get_package_for_file(file_path) {
                        return Some(pkg.id);
                    }
                }
                None
            });

        // Use a reference to the package lookup function
        let package_lookup_ref: Option<&super::edges::PackageLookupFn> = Some(&package_lookup_fn);

        let edges = extract_edges_for_symbol(
            row,
            &name_to_id,
            &id_to_symbol,
            &extracted.imports,
            &extracted.type_edges,
            &extracted.dataflow_edges,
            package_lookup_ref,
        );
        for (edge, evidence) in edges {
            let _ = sqlite.upsert_edge(&edge);
            for ev in evidence {
                let _ = sqlite.upsert_edge_evidence(&ev);
            }
        }
    }

    let examples = extract_usage_examples_for_file(
        &rel,
        &source,
        &name_to_id,
        &extracted.imports,
        &symbol_rows,
    );
    for ex in examples {
        let _ = sqlite.upsert_usage_example(&ex);
    }

    if !extracted.todos.is_empty() {
        let _ = sqlite.batch_upsert_todos(&extracted.todos);
    }

    if !extracted.jsdoc_entries.is_empty() {
        let _ = sqlite.batch_upsert_docstrings(&extracted.jsdoc_entries);
    }

    if !extracted.decorators.is_empty() {
        let decorator_rows: Vec<DecoratorRow> = extracted
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
        let _ = sqlite.batch_upsert_decorators(&decorator_rows);
    }

    if sqlite.is_test_file(&rel) {
        let _ = sqlite.create_test_links_for_file(&rel);
    }

    sqlite.upsert_file_fingerprint(&rel, fp.mtime_ns, fp.size_bytes)?;

    Ok(symbol_rows.len())
}

/// Index multiple files in parallel using Rayon
///
/// This function creates a thread pool and processes files in parallel.
/// Each thread gets its own SQLite connection to avoid lock contention.
/// Tantivy is shared via Arc (thread-safe).
/// LanceDB operations are skipped in parallel mode (handled separately).
///
/// Progress is logged every 100 files.
///
/// Note: Embeddings and vector updates are NOT performed in this function.
/// They must be handled in a separate sequential pass or batch operation.
pub fn index_files_parallel(
    config: Arc<Config>,
    db_path: PathBuf,
    tantivy: Arc<TantivyIndex>,
    vectors: Arc<LanceVectorTable>,
    files: Vec<PathBuf>,
) -> Result<IndexRunStats> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.parallel_workers)
        .thread_name(|i| format!("indexer-{}", i))
        .build()
        .context("Failed to build Rayon thread pool")?;

    let stats = Arc::new(Mutex::new(IndexRunStats {
        files_scanned: files.len(),
        ..Default::default()
    }));
    let processed = Arc::new(AtomicUsize::new(0));

    // Clone Arcs for the thread pool
    let tantivy_clone = tantivy.clone();
    let _vectors_clone = vectors.clone(); // Kept for API compatibility

    pool.install(|| {
        files.par_iter().for_each(|file| {
            let result = index_file_with_retry(
                file,
                &config,
                &db_path,
                &tantivy_clone,
                &_vectors_clone,
                2, // max 2 retries
            );

            let mut stats_guard = stats.lock().unwrap();
            if result.success {
                stats_guard.files_indexed += 1;
                stats_guard.symbols_indexed += result.symbols_count;
            } else {
                stats_guard.files_skipped += 1;
                if let Some(error) = result.error {
                    tracing::warn!(
                        file = %file.display(),
                        error = %error,
                        "Failed to index file after retries"
                    );
                }
            }
            drop(stats_guard);

            let count = processed.fetch_add(1, Ordering::Relaxed);
            if count.is_multiple_of(100) {
                tracing::info!("Progress: {}/{} files", count + 1, files.len());
            }
        });
    });

    let final_stats = stats.lock().unwrap().clone();
    tracing::info!(
        "Parallel indexing complete: {} files indexed, {} skipped",
        final_stats.files_indexed,
        final_stats.files_skipped
    );

    Ok(final_stats)
}

pub mod edges;
pub mod parallel;
pub mod parsing;
pub mod scan;
pub mod stats;
pub mod usage;
pub mod utils;

use crate::indexer::package;

use crate::{
    config::Config,
    embeddings::Embedder,
    graph::pagerank,
    indexer::{
        extract::c::extract_c_symbols,
        extract::cpp::extract_cpp_symbols,
        extract::go::extract_go_symbols,
        extract::java::extract_java_symbols,
        extract::javascript::extract_javascript_symbols,
        extract::python::extract_python_symbols,
        extract::rust::extract_rust_symbols,
        extract::typescript::extract_typescript_symbols_with_path,
        parser::{language_id_for_path, LanguageId},
    },
    metrics::MetricsRegistry,
    storage::{
        cache::EmbeddingCache,
        sqlite::{SimilarityClusterRow, SqliteStore, SymbolRow},
        tantivy::TantivyIndex,
        vector::{LanceVectorTable, VectorRecord},
    },
};
use anyhow::{Context, Result};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio::time::sleep;

use self::edges::{extract_edges_for_symbol, upsert_name_mapping};
use self::parallel::index_files_parallel;
use self::parsing::symbol_kind_to_string;
use self::scan::{scan_files, should_index_file};
use self::stats::IndexRunStats;
use self::usage::extract_usage_examples_for_file;
use self::utils::{
    cluster_key_from_vector, file_fingerprint, file_key, language_string, stable_symbol_id,
    unix_now_s,
};

#[derive(Clone)]
pub struct IndexPipeline {
    config: Arc<Config>,
    db_path: PathBuf,
    tantivy: Arc<TantivyIndex>,
    vectors: Arc<LanceVectorTable>,
    embedder: Arc<Mutex<Box<dyn Embedder + Send>>>,
    cache: Arc<EmbeddingCache>,
    metrics: Arc<MetricsRegistry>,
}

impl IndexPipeline {
    pub fn new(
        config: Arc<Config>,
        tantivy: Arc<TantivyIndex>,
        vectors: Arc<LanceVectorTable>,
        embedder: Arc<Mutex<Box<dyn Embedder + Send>>>,
        metrics: Arc<MetricsRegistry>,
    ) -> Self {
        let db_path = config.db_path.clone();

        // Initialize cache
        let sqlite = SqliteStore::open(&db_path).expect("Failed to open SQLite database");
        let model_name = match config.embeddings_backend {
            crate::config::EmbeddingsBackend::JinaCode => "jinaai/jina-embeddings-v2-base-code",
            crate::config::EmbeddingsBackend::FastEmbed => {
                config.embeddings_model_repo.as_deref().unwrap_or("unknown")
            }
            crate::config::EmbeddingsBackend::Hash => "hash",
        };
        let cache = Arc::new(EmbeddingCache::new(
            Arc::new(sqlite),
            model_name,
            config.embedding_cache_enabled,
            1024 * 1024 * 1024, // 1GB max
        ));

        Self {
            config,
            db_path,
            tantivy,
            vectors,
            embedder,
            cache,
            metrics,
        }
    }

    pub async fn index_all(&self) -> Result<IndexRunStats> {
        let _timer = self.metrics.index_duration.start_timer();

        let started_at = Instant::now();
        let started_at_unix_s = unix_now_s();

        // Discover and store packages if enabled
        if self.config.package_detection_enabled {
            if let Err(e) = self.index_packages_and_repositories() {
                tracing::warn!(
                    error = %e,
                    "Package detection failed, continuing with indexing"
                );
            }
        }

        let mut files = Vec::new();
        for root in &self.config.repo_roots {
            files.extend(scan_files(&self.config, root)?);
        }
        let stats = self.index_files(files, true).await?;

        // Record Prometheus metrics
        self.metrics.index_files_total.inc_by(stats.files_indexed as f64);
        self.metrics.index_symbols_total.inc_by(stats.symbols_indexed as f64);
        self.metrics.index_files_skipped.inc_by(stats.files_skipped as f64);
        self.metrics.index_files_unchanged.inc_by(stats.files_unchanged as f64);

        // Cache metrics
        let cache_stats = self.cache.stats();
        self.metrics.index_cache_hits.inc_by(cache_stats.hits as f64);
        self.metrics.index_cache_misses.inc_by(cache_stats.misses as f64);

        self.persist_index_run_metrics(started_at_unix_s, started_at.elapsed(), &stats)?;

        // Update resource gauges
        self.update_resource_gauges()?;

        // Note: timer observes duration when dropped
        Ok(stats)
    }

    fn update_resource_gauges(&self) -> Result<()> {
        let sqlite = SqliteStore::open(&self.db_path)?;
        let symbol_count = sqlite.count_symbols()?;

        self.metrics.symbol_count.set(symbol_count as f64);

        // Get index sizes
        let tantivy_size = Self::dir_size(&self.config.tantivy_index_path)?;
        let db_size = std::fs::metadata(&self.db_path)?.len() as u64;

        self.metrics.index_size_bytes.set((tantivy_size + db_size) as f64);

        Ok(())
    }

    fn dir_size(path: &PathBuf) -> Result<u64> {
        Ok(std::fs::read_dir(path)?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum())
    }

    /// Discover packages and repositories and store them in SQLite.
    ///
    /// This function:
    /// 1. Discovers all package manifests in the workspace
    /// 2. Detects git repositories
    /// 3. Stores repositories and packages in the database
    fn index_packages_and_repositories(&self) -> Result<()> {
        let sqlite = SqliteStore::open(&self.db_path)?;
        sqlite.init()?;

        // Discover packages from all repo roots
        let mut packages = package::discover_packages(&self.config, &self.config.repo_roots)?;

        if packages.is_empty() {
            tracing::debug!("No packages discovered in workspace");
            return Ok(());
        }

        // Detect repositories and assign repository_id to packages
        let repositories = package::detect_repositories(&mut packages)?;

        // Get current timestamp for created_at
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Upsert all repositories
        for repo in repositories {
            let repo_row = crate::storage::sqlite::schema::RepositoryRow {
                id: repo.id,
                name: repo.name,
                root_path: repo.root_path,
                vcs_type: Some(repo.vcs_type.to_string()),
                remote_url: repo.remote_url,
                created_at,
            };
            sqlite.upsert_repository(&repo_row)?;
        }

        // Upsert all packages
        for pkg in packages {
            let pkg_row = crate::storage::sqlite::schema::PackageRow {
                id: pkg.id,
                repository_id: pkg.repository_id.unwrap_or_default(),
                name: pkg.name.unwrap_or_else(|| {
                    // Fallback name: use directory name
                    PathBuf::from(&pkg.root_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string()
                }),
                version: pkg.version,
                manifest_path: pkg.manifest_path,
                package_type: pkg.package_type.to_string(),
                created_at,
            };
            sqlite.upsert_package(&pkg_row)?;
        }

        // Log summary
        let repo_count = sqlite.list_all_repositories()?.len();
        let pkg_count = sqlite.list_all_packages()?.len();

        tracing::info!(
            repositories = repo_count,
            packages = pkg_count,
            "Discovered packages and repositories"
        );

        Ok(())
    }

    pub async fn index_paths(&self, paths: &[PathBuf]) -> Result<IndexRunStats> {
        let started_at = Instant::now();
        let started_at_unix_s = unix_now_s();
        let mut files = Vec::new();
        for p in paths {
            if p.is_dir() {
                files.extend(scan_files(&self.config, p)?);
            } else if p.is_file() && should_index_file(&self.config, p) {
                files.push(p.to_path_buf());
            }
        }
        let stats = self.index_files(files, false).await?;
        self.persist_index_run_metrics(started_at_unix_s, started_at.elapsed(), &stats)?;
        Ok(stats)
    }

    pub fn spawn_watch_loop(&self) -> tokio::task::JoinHandle<()> {
        let pipeline = self.clone();
        tokio::spawn(async move {
            let interval_ms = pipeline.config.watch_debounce_ms.max(50);
            let mut consecutive_failures = 0;
            let max_backoff_ms = 5000; // Max 5 seconds backoff

            loop {
                sleep(Duration::from_millis(interval_ms)).await;
                if let Err(err) = pipeline.index_all().await {
                    consecutive_failures += 1;
                    let backoff_ms = (interval_ms * (1 << consecutive_failures.min(8))).min(max_backoff_ms);
                    tracing::warn!(
                        error = %err,
                        consecutive_failures = consecutive_failures,
                        backoff_ms = backoff_ms,
                        "Watch index run failed, backing off"
                    );
                    sleep(Duration::from_millis(backoff_ms)).await;
                } else {
                    consecutive_failures = 0; // Reset on success
                }
            }
        })
    }

    fn persist_index_run_metrics(
        &self,
        started_at_unix_s: i64,
        elapsed: Duration,
        stats: &IndexRunStats,
    ) -> Result<()> {
        let sqlite = SqliteStore::open(&self.db_path)?;
        sqlite.init()?;
        let run = crate::storage::sqlite::IndexRunRow {
            started_at_unix_s,
            duration_ms: elapsed.as_millis().min(u64::MAX as u128) as u64,
            files_scanned: stats.files_scanned as u64,
            files_indexed: stats.files_indexed as u64,
            files_skipped: stats.files_skipped as u64,
            files_unchanged: stats.files_unchanged as u64,
            files_deleted: stats.files_deleted as u64,
            symbols_indexed: stats.symbols_indexed as u64,
        };
        let _ = sqlite.insert_index_run(&run);
        Ok(())
    }

    async fn index_files(
        &self,
        files: Vec<PathBuf>,
        cleanup_deleted: bool,
    ) -> Result<IndexRunStats> {
        let mut seen = HashSet::new();
        let mut uniq = Vec::new();
        for p in files {
            let abs = p.canonicalize().unwrap_or(p);
            if seen.insert(abs.clone()) {
                uniq.push(abs);
            }
        }

        let mut stats = IndexRunStats {
            files_scanned: uniq.len(),
            ..Default::default()
        };

        // Cleanup deleted files first
        if cleanup_deleted {
            let mut scanned_rel: HashSet<String> = HashSet::new();
            for file in &uniq {
                scanned_rel.insert(file_key(&self.config, file));
            }

            let existing = {
                let sqlite = SqliteStore::open(&self.db_path)?;
                sqlite.init()?;
                sqlite.list_all_file_fingerprints(1_000_000)?
            };

            let to_delete = existing
                .into_iter()
                .filter(|fp| !scanned_rel.contains(&fp.file_path))
                .map(|fp| fp.file_path)
                .collect::<Vec<_>>();

            let mut any = false;
            for file_path in to_delete {
                {
                    let sqlite = SqliteStore::open(&self.db_path)?;
                    sqlite.init()?;

                    // Delete symbols first - test_links have ON DELETE CASCADE, so they auto-delete
                    sqlite.delete_symbols_by_file(&file_path)?;
                    sqlite.delete_usage_examples_by_file(&file_path)?;
                    sqlite.delete_todos_by_file(&file_path)?;
                    sqlite.delete_docstrings_by_file(&file_path)?;
                    sqlite.delete_decorators_by_file(&file_path)?;
                    sqlite.delete_file_fingerprint(&file_path)?;
                }

                self.tantivy.delete_symbols_by_file(&file_path)?;
                self.vectors.delete_records_by_file_path(&file_path).await?;

                stats.files_deleted += 1;
                any = true;
            }

            if any {
                self.tantivy.commit()?;
            }
        }

        // Choose parallel or sequential indexing based on config
        let indexing_stats = if self.config.parallel_workers > 1 {
            // Parallel path (no embeddings/vectors in parallel mode)
            tracing::info!(
                "Using parallel indexing with {} workers",
                self.config.parallel_workers
            );
            self.index_files_parallel_async(uniq.clone()).await?
        } else {
            // Sequential path (includes embeddings/vectors)
            tracing::info!("Using sequential indexing");
            // For now, keep the original logic inline
            // TODO: Refactor into index_files_sequential helper
            self.index_files_sequential_internal(&uniq, &mut stats).await?
        };

        stats.files_indexed = indexing_stats.files_indexed;
        stats.files_skipped = indexing_stats.files_skipped;
        stats.files_unchanged = indexing_stats.files_unchanged;
        stats.symbols_indexed = indexing_stats.symbols_indexed;

        // Compute PageRank scores after all indexing is complete
        // Only run if the graph structure changed (files indexed or deleted)
        if stats.files_indexed > 0 || stats.files_deleted > 0 {
            let sqlite = SqliteStore::open(&self.db_path)?;
            sqlite.init()?;
            pagerank::compute_and_store_pagerank(&sqlite, &self.config)
                .context("Failed to compute PageRank scores")?;
        } else {
            tracing::debug!("Skipping PageRank computation (no files indexed or deleted)");
        }

        // Log cache statistics
        let cache_stats = self.cache.stats();
        tracing::info!(
            hits = cache_stats.hits,
            misses = cache_stats.misses,
            hit_rate = %format!("{:.1}%", cache_stats.hit_rate * 100.0),
            "Embedding cache statistics"
        );

        tracing::debug!(?stats, "Index run completed");
        Ok(stats)
    }

    /// Internal sequential indexing implementation (original logic)
    async fn index_files_sequential_internal(
        &self,
        uniq: &[PathBuf],
        stats: &mut IndexRunStats,
    ) -> Result<IndexRunStats> {

        let mut name_to_id: HashMap<String, String> = HashMap::new();

        for file in uniq {
            let rel = file_key(&self.config, &file);

            let language_id = match language_id_for_path(&file) {
                Some(id) => id,
                None => {
                    stats.files_skipped += 1;
                    continue;
                }
            };

            let fp = match file_fingerprint(&file) {
                Ok(fp) => fp,
                Err(err) => {
                    tracing::warn!(
                        file = %file.display(),
                        error = %err,
                        "Failed to fingerprint file"
                    );
                    stats.files_skipped += 1;
                    continue;
                }
            };

            let is_unchanged = {
                let sqlite = SqliteStore::open(&self.db_path)?;
                sqlite.init()?;
                sqlite.get_file_fingerprint(&rel)?.is_some_and(|existing| {
                    existing.mtime_ns == fp.mtime_ns && existing.size_bytes == fp.size_bytes
                })
            };

            if is_unchanged {
                stats.files_unchanged += 1;
                continue;
            }

            // Log package membership for this file
            {
                let sqlite = SqliteStore::open(&self.db_path)?;
                sqlite.init()?;
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
            }

            let source = match fs::read_to_string(&file) {
                Ok(s) => s,
                Err(err) => {
                    tracing::warn!(file = %file.display(), error = %err, "Failed to read file");
                    stats.files_skipped += 1;
                    continue;
                }
            };

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
            };

            let extracted = match extracted {
                Ok(syms) => syms,
                Err(err) => {
                    tracing::warn!(
                        file = %file.display(),
                        error = %err,
                        "Failed to extract symbols"
                    );
                    stats.files_skipped += 1;
                    continue;
                }
            };
            self.tantivy.delete_symbols_by_file(&rel)?;
            self.vectors
                .delete_records_by_file_path(&rel)
                .await
                .with_context(|| format!("Failed to delete old vectors for {rel}"))?;

            {
                let sqlite = SqliteStore::open(&self.db_path)?;
                sqlite.init()?;

                // Delete symbols first - test_links have ON DELETE CASCADE, so they auto-delete
                if let Err(err) = sqlite.delete_symbols_by_file(&rel) {
                    tracing::error!(
                        file = %rel,
                        error = %err,
                        error_chain = %err.chain().map(|e| e.to_string()).collect::<Vec<_>>().join(" -> "),
                        "Failed to delete old symbols (full error chain)"
                    );
                    return Err(err).with_context(|| format!("Failed to delete old symbols for {rel}"));
                }
                sqlite
                    .delete_usage_examples_by_file(&rel)
                    .with_context(|| format!("Failed to delete old usage examples for {rel}"))?;
                sqlite
                    .delete_todos_by_file(&rel)
                    .with_context(|| format!("Failed to delete old todos for {rel}"))?;
                sqlite
                    .delete_docstrings_by_file(&rel)
                    .with_context(|| format!("Failed to delete old docstrings for {rel}"))?;
                sqlite
                    .delete_decorators_by_file(&rel)
                    .with_context(|| format!("Failed to delete old decorators for {rel}"))?;
                // Note: test_links auto-delete via ON DELETE CASCADE when symbols are deleted
            }

            let mut symbol_rows = Vec::new();

            // 1. Add File-Level Symbol (Document Indexing)
            // We index the file itself as a symbol to allow retrieval of the "whole file" concept.
            let file_symbol_id = stable_symbol_id(&rel, "FILE_ROOT", 0);
            symbol_rows.push(SymbolRow {
                id: file_symbol_id,
                file_path: rel.clone(),
                language: language_string(language_id).to_string(),
                kind: "file".to_string(),
                name: rel.clone(), // Name is the relative path
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

            if !symbol_rows.is_empty() {
                let vectors = self
                    .embed_and_build_vector_records(&symbol_rows)
                    .await
                    .with_context(|| format!("Failed to embed symbols for {rel}"))?;

                for row in &symbol_rows {
                    self.tantivy.upsert_symbol(row)?;
                    upsert_name_mapping(&mut name_to_id, row);
                }

                // Build id_to_symbol HashMap for edge extraction
                let id_to_symbol: HashMap<String, &SymbolRow> = symbol_rows
                    .iter()
                    .map(|r| (r.id.clone(), r))
                    .collect();

                // Commit Tantivy changes immediately to ensure they are persisted
                // even if vector indexing panics (which has been observed with lance).
                self.tantivy.commit()?;

                {
                    let sqlite = SqliteStore::open(&self.db_path)?;
                    sqlite.init()?;
                    for row in &symbol_rows {
                        sqlite.upsert_symbol(row)?;
                    }
                    for row in &symbol_rows {
                        let edges = extract_edges_for_symbol(
                            row,
                            &name_to_id,
                            &id_to_symbol,
                            &extracted.imports,
                            &extracted.type_edges,
                            &extracted.dataflow_edges,
                            None,
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

                    for rec in &vectors {
                        let _ = sqlite.upsert_similarity_cluster(&SimilarityClusterRow {
                            symbol_id: rec.id.clone(),
                            cluster_key: cluster_key_from_vector(&rec.vector),
                        });
                    }

                    // Store TODO entries extracted from this file
                    if !extracted.todos.is_empty() {
                        let _ = sqlite.batch_upsert_todos(&extracted.todos);
                    }

                    // Store JSDoc entries extracted from this file
                    if !extracted.jsdoc_entries.is_empty() {
                        let _ = sqlite.batch_upsert_docstrings(&extracted.jsdoc_entries);
                    }

                    // Store decorator entries extracted from this file
                    if !extracted.decorators.is_empty() {
                        use crate::storage::sqlite::schema::DecoratorRow;
                        let decorator_rows: Vec<DecoratorRow> = extracted
                            .decorators
                            .iter()
                            .map(|d| DecoratorRow {
                                symbol_id: d.symbol_id.clone(),
                                name: d.name.clone(),
                                arguments: d.arguments.clone(),
                                target_line: d.target_line,
                                decorator_type: serde_json::to_string(&d.decorator_type).unwrap_or_else(|_| "unknown".to_string()),
                                updated_at: 0,
                            })
                            .collect();
                        let _ = sqlite.batch_upsert_decorators(&decorator_rows);
                    }

                    // Create test links if this is a test file
                    if sqlite.is_test_file(&rel) {
                        let _ = sqlite.create_test_links_for_file(&rel);
                    }

                    sqlite.upsert_file_fingerprint(&rel, fp.mtime_ns, fp.size_bytes)?;
                }

                // Add vectors last, as this step is prone to panics in some environments.
                // We wrap it in a result check just in case, though panics escape this.
                if let Err(e) = self.vectors.add_records(&vectors).await {
                    tracing::error!("Failed to add vector records for {}: {}", rel, e);
                }
            } else {
                let sqlite = SqliteStore::open(&self.db_path)?;
                sqlite.init()?;
                sqlite.upsert_file_fingerprint(&rel, fp.mtime_ns, fp.size_bytes)?;
            }

            stats.symbols_indexed += symbol_rows.len();
            stats.files_indexed += 1;
            self.tantivy.commit()?;
        }

        Ok(stats.clone())
    }

    /// Async wrapper for parallel indexing
    ///
    /// Calls the synchronous rayon-based parallel indexing in a blocking task
    /// to avoid blocking the tokio runtime.
    async fn index_files_parallel_async(&self, files: Vec<PathBuf>) -> Result<IndexRunStats> {
        let config = self.config.clone();
        let db_path = self.db_path.clone();
        let tantivy = self.tantivy.clone();
        let vectors = self.vectors.clone();

        // Run parallel indexing in blocking task
        tokio::task::spawn_blocking(move || {
            index_files_parallel(config, db_path, tantivy, vectors, files)
        })
        .await
        .context("Join error in parallel indexing")?
    }

    /// Index files sequentially (original logic with embeddings)
    ///
    /// This is the original indexing logic that includes:
    /// - File cleanup
    /// - Symbol extraction with embeddings
    /// - Vector storage
    /// Note: Currently using index_files_sequential_internal instead
    #[allow(dead_code)]
    async fn index_files_sequential(&self, _files: Vec<PathBuf>) -> Result<IndexRunStats> {
        // Placeholder - logic moved to index_files_sequential_internal
        let stats = IndexRunStats::default();
        Ok(stats)
    }

    async fn embed_and_build_vector_records(
        &self,
        rows: &[SymbolRow],
    ) -> Result<Vec<VectorRecord>> {
        let mut vectors = Vec::with_capacity(rows.len());
        let mut uncached_texts = Vec::new();
        let mut uncached_indices = Vec::new();

        // Check cache for each text
        for (i, row) in rows.iter().enumerate() {
            if let Some(cached) = self.cache.get(&row.text) {
                vectors.push((i, cached));
            } else {
                uncached_texts.push(row.text.clone());
                uncached_indices.push(i);
            }
        }

        // Embed uncached texts in batch
        let new_embeddings = if !uncached_texts.is_empty() {
            let mut embedder = self.embedder.lock().await;
            embedder.embed(&uncached_texts)?
        } else {
            Vec::new()
        };

        // Store new embeddings in cache
        for (text, embedding) in uncached_texts.iter().zip(&new_embeddings) {
            let _ = self.cache.put(text, embedding);
        }

        // Merge cached and new embeddings
        let mut result = vec![Vec::new(); rows.len()];
        for (i, vec) in vectors {
            result[i] = vec;
        }
        for (i, emb) in uncached_indices.iter().zip(new_embeddings) {
            result[*i] = emb;
        }

        // Build VectorRecords
        let mut out = Vec::with_capacity(rows.len());
        for (row, vector) in rows.iter().zip(result) {
            out.push(VectorRecord {
                id: row.id.clone(),
                vector,
                name: row.name.clone(),
                kind: row.kind.clone(),
                file_path: row.file_path.clone(),
                exported: row.exported,
                language: row.language.clone(),
                text: row.text.clone(),
            });
        }

        Ok(out)
    }
}

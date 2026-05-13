pub mod describe;
pub mod edges;
pub mod parallel;
pub mod parse;
pub mod parsing;
pub mod scan;
pub mod stats;
pub mod usage;
pub mod utils;
pub mod watch;
pub mod write;

use crate::indexer::package;

use crate::{
    config::Config,
    embeddings::Embedder,
    graph::pagerank,
    logging::RepoLogger,
    metrics::MetricsRegistry,
    path::Utf8PathBuf,
    storage::{
        cache::EmbeddingCache,
        sqlite::{SimilarityClusterRow, SqliteStore, SymbolRow},
        tantivy::TantivyIndex,
        vector::{LanceVectorTable, VectorRecord},
    },
};
use anyhow::{Context, Result};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio::time::sleep;

use self::scan::{scan_files, should_index_file};
use self::stats::IndexRunStats;
use self::utils::{cluster_key_from_vector, file_fingerprint, file_key_path, unix_now_s};

/// Determine whether a symbol warrants embedding and LLM description generation.
///
/// Skips symbols that almost never appear in search results:
/// - File-kind symbols (their text is the entire file; BM25 handles them)
/// - Unexported symbols with fewer than 3 lines (tiny private helpers)
/// - Test symbols detected by name or file path heuristics
pub(crate) fn should_generate_embedding(
    kind: &str,
    name: &str,
    file_path: &str,
    exported: bool,
    start_line: u32,
    end_line: u32,
) -> bool {
    if kind == "file" {
        return false;
    }
    let line_count = end_line.saturating_sub(start_line);
    if !exported && line_count < 3 {
        return false;
    }
    if crate::classify::is_test_file(file_path) {
        return false;
    }
    if crate::classify::is_test_symbol(name) {
        return false;
    }
    true
}

#[derive(Clone)]
pub struct IndexPipeline {
    config: Arc<Config>,
    db_path: Utf8PathBuf,
    tantivy: Arc<TantivyIndex>,
    vectors: Arc<LanceVectorTable>,
    embedder: Arc<Mutex<Box<dyn Embedder + Send>>>,
    cache: Arc<EmbeddingCache>,
    metrics: Arc<MetricsRegistry>,
    repo_logger: Option<Arc<RepoLogger>>,
    /// Guard to prevent concurrent `generate_embeddings_for_orphaned_symbols` runs
    /// (background startup recovery vs post-indexing can race, duplicating LanceDB records).
    embedding_generation_lock: Arc<tokio::sync::Mutex<()>>,
}

impl IndexPipeline {
    /// Get repository name for logging purposes
    fn repo_name(&self) -> &str {
        self.config.base_dir.file_name().unwrap_or("unknown")
    }

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
            crate::config::EmbeddingsBackend::LlamaCpp => "jinaai/jina-code-embeddings-1.5b",
            crate::config::EmbeddingsBackend::Hash => "hash",
        };
        let cache = Arc::new(EmbeddingCache::new(
            Arc::new(sqlite),
            model_name,
            config.embedding_cache_enabled,
            1024 * 1024 * 1024, // 1GB max
        ));

        // Create per-repo logger
        let repo_data_dir = config.db_path.parent().unwrap_or(&config.db_path);
        let repo_logger = RepoLogger::new(repo_data_dir).map(Arc::new);

        Self {
            config,
            db_path,
            tantivy,
            vectors,
            embedder,
            cache,
            metrics,
            repo_logger,
            embedding_generation_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub async fn index_all(&self) -> Result<IndexRunStats> {
        let _timer = self.metrics.index_duration.start_timer();

        let started_at = Instant::now();
        let started_at_unix_s = unix_now_s();

        if let Some(ref logger) = self.repo_logger {
            logger.info(&format!("Index run started for {}", self.repo_name()));
        }

        // Discover and store packages if enabled
        if self.config.package_detection_enabled {
            if let Err(e) = self.index_packages_and_repositories() {
                tracing::warn!(
                    repo = %self.repo_name(),
                    error = %e,
                    "Package detection failed, continuing with indexing"
                );
            }
        }

        let mut files = Vec::new();
        for root in &self.config.repo_roots {
            files.extend(scan_files(&self.config, root.as_std_path())?);
        }
        let stats = self.index_files(files, true).await?;

        // Record Prometheus metrics
        self.metrics
            .index_files_total
            .inc_by(stats.files_indexed as f64);
        self.metrics
            .index_symbols_total
            .inc_by(stats.symbols_indexed as f64);
        self.metrics
            .index_files_skipped
            .inc_by(stats.files_skipped as f64);
        self.metrics
            .index_files_unchanged
            .inc_by(stats.files_unchanged as f64);

        // Cache metrics
        let cache_stats = self.cache.stats();
        self.metrics
            .index_cache_hits
            .inc_by(cache_stats.hits as f64);
        self.metrics
            .index_cache_misses
            .inc_by(cache_stats.misses as f64);

        self.persist_index_run_metrics(started_at_unix_s, started_at.elapsed(), &stats)?;

        // Update resource gauges
        self.update_resource_gauges()?;

        // Compact LanceDB fragments and prune old versions after index runs
        // that modified data. This prevents unbounded growth of the vectors directory.
        if stats.files_indexed > 0 || stats.files_deleted > 0 {
            if let Err(e) = self.vectors.optimize().await {
                tracing::warn!(
                    repo = %self.repo_name(),
                    error = %e,
                    "LanceDB optimization failed (non-fatal)"
                );
            }
        }

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

        self.metrics
            .index_size_bytes
            .set((tantivy_size + db_size) as f64);

        Ok(())
    }

    fn dir_size(path: &Utf8PathBuf) -> Result<u64> {
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
            // Convert absolute manifest_path to relative for consistency with symbol file_paths
            let manifest_path = if let Ok(rel) =
                PathBuf::from(&pkg.manifest_path).strip_prefix(&self.config.base_dir)
            {
                rel.to_string_lossy().to_string()
            } else {
                pkg.manifest_path.clone()
            };

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
                manifest_path,
                package_type: pkg.package_type.to_string(),
                created_at,
            };
            sqlite.upsert_package(&pkg_row)?;
        }

        // Log summary
        let repo_count = sqlite.list_all_repositories()?.len();
        let pkg_count = sqlite.list_all_packages()?.len();

        tracing::info!(
            repo = %self.repo_name(),
            repositories = repo_count,
            packages = pkg_count,
            "Discovered packages and repositories"
        );

        Ok(())
    }

    pub async fn index_paths(&self, paths: &[Utf8PathBuf]) -> Result<IndexRunStats> {
        let started_at = Instant::now();
        let started_at_unix_s = unix_now_s();
        let mut files = Vec::new();
        for p in paths {
            let std_path = p.as_std_path();
            if std_path.is_dir() {
                files.extend(scan_files(&self.config, std_path)?);
            } else if std_path.is_file() && should_index_file(&self.config, std_path) {
                files.push(std_path.to_path_buf());
            }
        }
        let stats = self.index_files(files, false).await?;
        self.persist_index_run_metrics(started_at_unix_s, started_at.elapsed(), &stats)?;
        Ok(stats)
    }

    /// Check if any files in the workspace have changed since last indexing
    ///
    /// Returns Ok(true) if changes are detected, Ok(false) if no changes,
    /// or Err() if checking fails.
    #[allow(dead_code)]
    fn check_for_changes(&self) -> Result<bool> {
        let sqlite = SqliteStore::open(&self.db_path)?;
        sqlite.init()?;

        // Scan all files in the workspace
        let mut files = Vec::new();
        for root in &self.config.repo_roots {
            files.extend(scan_files(&self.config, root.as_std_path())?);
        }

        // Check if any files have changed by comparing fingerprints
        for file in &files {
            let rel = file_key_path(&self.config, file);
            let fp = file_fingerprint(file)?;

            // Check if file is already indexed and unchanged
            if let Ok(Some(existing)) = sqlite.get_file_fingerprint(&rel) {
                // File exists in index - check if it changed
                if existing.mtime_ns != fp.mtime_ns || existing.size_bytes != fp.size_bytes {
                    // File changed - need to re-index
                    return Ok(true);
                }
            } else {
                // File not in index yet - need to index
                return Ok(true);
            }
        }

        // Check for deleted files
        let scanned_rel: HashSet<String> = files
            .iter()
            .map(|f| file_key_path(&self.config, f))
            .collect();

        let existing = sqlite.list_all_file_fingerprints(1_000_000)?;
        for fp in existing {
            if !scanned_rel.contains(&fp.file_path) {
                // File was deleted - need to re-index
                return Ok(true);
            }
        }

        // No changes detected
        Ok(false)
    }

    pub fn spawn_watch_loop(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let pipeline = self.clone();
        tokio::spawn(async move {
            let debounce_ms = pipeline.config.watch_debounce_ms.max(50);
            let min_index_interval = pipeline.config.watch_min_index_interval_ms;
            let mut consecutive_failures: u32 = 0;
            let max_backoff_ms: u64 = 5000;
            let mut last_index_time: Option<Instant> = None;

            let repo_name = pipeline
                .config
                .base_dir
                .file_name()
                .unwrap_or("unknown")
                .to_string();

            // Create tokio channel for the notify → async bridge
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();

            // Create the OS-native file watcher.  Must stay alive for the
            // duration of this task — dropping it stops OS event delivery.
            let _watcher = match watch::create_watcher(&pipeline.config, tx) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!(
                        repo = %repo_name,
                        error = %e,
                        "Failed to create file watcher, falling back to no watch"
                    );
                    return;
                }
            };

            tracing::info!(
                repo = %repo_name,
                debounce_ms = debounce_ms,
                "File watcher active (OS-native)"
            );

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!(repo = %repo_name, "File watcher cancelled");
                        break;
                    }
                    Some(_) = rx.recv() => {
                        // Drain any queued signals
                        while rx.try_recv().is_ok() {}

                        // Debounce: wait, then drain again to coalesce bursts
                        sleep(Duration::from_millis(debounce_ms)).await;
                        while rx.try_recv().is_ok() {}

                        // Rate limiting
                        if let Some(last_time) = last_index_time {
                            let elapsed = last_time.elapsed().as_millis() as u64;
                            if elapsed < min_index_interval {
                                tracing::debug!(
                                    repo = %repo_name,
                                    elapsed_ms = elapsed,
                                    min_interval_ms = min_index_interval,
                                    "Rate limiting: skipping index, too soon since last run"
                                );
                                continue;
                            }
                        }

                        tracing::info!(
                            repo = %repo_name,
                            "File change detected, starting index run"
                        );

                        match pipeline.index_all().await {
                            Ok(stats) => {
                                last_index_time = Some(Instant::now());
                                consecutive_failures = 0;
                                tracing::info!(
                                    repo = %repo_name,
                                    files_indexed = stats.files_indexed,
                                    symbols_indexed = stats.symbols_indexed,
                                    "Watch index run completed"
                                );
                            }
                            Err(err) => {
                                consecutive_failures += 1;
                                let backoff_ms = (debounce_ms
                                    * (1u64 << consecutive_failures.min(8)))
                                .min(max_backoff_ms);
                                tracing::warn!(
                                    repo = %repo_name,
                                    error = %err,
                                    consecutive_failures = consecutive_failures,
                                    backoff_ms = backoff_ms,
                                    "Watch index run failed, backing off"
                                );
                                sleep(Duration::from_millis(backoff_ms)).await;
                            }
                        }
                    }
                }
            }
        })
    }

    /// Spawn the background description worker.
    ///
    /// Generates LLM descriptions for all undescribed symbols, updates Tantivy
    /// index with descriptions appended to the text field.
    pub fn spawn_description_worker(
        &self,
        llm: std::sync::Arc<dyn crate::llm::LlmGenerator>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let db = std::sync::Arc::new(
            SqliteStore::open(&self.db_path).expect("Failed to open SQLite for description worker"),
        );
        let tantivy = self.tantivy.clone();
        let max_tokens = self.config.llm_max_tokens;
        let batch_size = self.config.llm_batch_commit;

        tokio::spawn(async move {
            if let Err(e) = describe::run_description_worker(
                db, tantivy, llm, max_tokens, batch_size, cancel, false,
            )
            .await
            {
                tracing::error!("Description worker failed: {}", e);
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

        // Open a single SQLite handle for the prep work below. Both branches
        // here used to open (and drop) a fresh connection per call/iteration,
        // which under WAL mode opens three FDs each (.db, -wal, -shm) and
        // could pile up alongside the parallel parse pool.
        let needs_setup_sqlite = self.tantivy.was_recreated() || cleanup_deleted;
        let setup_sqlite = if needs_setup_sqlite {
            let s = SqliteStore::open(&self.db_path)?;
            s.init()?;
            Some(s)
        } else {
            None
        };

        // If Tantivy was recreated (schema version mismatch), clear file fingerprints
        // so every file is treated as "changed" and gets re-indexed into the new index.
        if self.tantivy.was_recreated() {
            let sqlite = setup_sqlite
                .as_ref()
                .expect("setup_sqlite is Some when was_recreated is true");
            let cleared = sqlite.clear_all_file_fingerprints()?;
            tracing::warn!(
                repo = %self.repo_name(),
                cleared_fingerprints = cleared,
                "Tantivy index was recreated — cleared file fingerprints to force full re-index"
            );
        }

        // Cleanup deleted files first
        if cleanup_deleted {
            let sqlite = setup_sqlite
                .as_ref()
                .expect("setup_sqlite is Some when cleanup_deleted is true");

            let mut scanned_rel: HashSet<String> = HashSet::new();
            for file in &uniq {
                scanned_rel.insert(file_key_path(&self.config, file));
            }

            let existing = sqlite.list_all_file_fingerprints(1_000_000)?;

            let to_delete = existing
                .into_iter()
                .filter(|fp| !scanned_rel.contains(&fp.file_path))
                .map(|fp| fp.file_path)
                .collect::<Vec<_>>();

            let mut any = false;
            for file_path in to_delete {
                // Reuse the single SQLite connection across all deletions
                // instead of opening one per file (each open = 3 WAL FDs).
                sqlite.delete_symbols_by_file(&file_path)?;
                sqlite.delete_usage_examples_by_file(&file_path)?;
                sqlite.delete_todos_by_file(&file_path)?;
                sqlite.delete_docstrings_by_file(&file_path)?;
                sqlite.delete_decorators_by_file(&file_path)?;
                sqlite.delete_file_fingerprint(&file_path)?;

                self.tantivy.delete_symbols_by_file(&file_path)?;
                self.vectors.delete_records_by_file_path(&file_path).await?;

                stats.files_deleted += 1;
                any = true;
            }

            if any {
                self.tantivy.commit()?;
            }
        }
        drop(setup_sqlite);

        // Unified pipeline: parse → write → embed
        use crate::storage::sqlite::pool::SqlitePool;

        let pool = std::sync::Arc::new(SqlitePool::new(
            &self.db_path,
            self.config.parallel_workers + 2,
        )?);

        // Phase 1: Parse (Rayon, parallel)
        let parse_results = {
            let files_clone = uniq.clone();
            let config_clone = self.config.clone();
            let pool_clone = pool.clone();
            tokio::task::spawn_blocking(move || {
                parse::parse_files(&files_clone, &config_clone, &pool_clone)
            })
            .await
            .context("Join error in parse phase")??
        };

        // Tally stats from parse results
        let mut parsed_files = Vec::new();
        for result in parse_results {
            match result {
                parse::ParseResult::Parsed(pf) => {
                    stats.symbols_indexed += pf.symbol_rows.len();
                    stats.files_indexed += 1;
                    parsed_files.push(*pf);
                }
                parse::ParseResult::Unchanged => {
                    stats.files_unchanged += 1;
                }
                parse::ParseResult::Skipped { reason } => {
                    tracing::debug!(reason = %reason, "File skipped during parse");
                    stats.files_skipped += 1;
                }
            }
        }

        // Phase 2: Write (single thread, batched)
        if !parsed_files.is_empty() {
            let conn = pool.get()?;
            write::write_batch(&parsed_files, &conn, &self.tantivy)?;
            drop(conn);
        }

        // Phase 3: Embed + PageRank (concurrent)
        // Embedding reads symbols from SQLite → writes to LanceDB.
        // PageRank reads edges from SQLite → writes pagerank scores back to SQLite.
        // No conflicts — run them concurrently.
        let embed_fut = self.generate_embeddings_for_orphaned_symbols();

        let pagerank_fut = {
            let need_pagerank = stats.files_indexed > 0 || stats.files_deleted > 0;
            let db_path = self.db_path.clone();
            let config = self.config.clone();
            let fi = stats.files_indexed;
            let fd = stats.files_deleted;
            async move {
                if !need_pagerank {
                    tracing::debug!("Skipping PageRank computation (no files indexed or deleted)");
                    return Ok::<(), anyhow::Error>(());
                }
                tokio::task::spawn_blocking(move || {
                    let sqlite = SqliteStore::open(&db_path)?;
                    sqlite.init()?;
                    pagerank::compute_and_store_pagerank(&sqlite, &config).with_context(|| {
                        format!(
                            "Failed to compute PageRank scores: files_indexed={}, files_deleted={}",
                            fi, fd
                        )
                    })
                })
                .await
                .context("Join error in PageRank computation")?
            }
        };

        let (embed_result, pagerank_result) = tokio::join!(embed_fut, pagerank_fut);
        if let Err(e) = embed_result {
            tracing::warn!(
                "Embedding generation failed (model may still be loading): {e}. \
                 Vectors will be generated once the model is ready."
            );
        }
        pagerank_result?;

        // Log cache statistics
        let cache_stats = self.cache.stats();
        tracing::info!(
            repo = %self.repo_name(),
            hits = cache_stats.hits,
            misses = cache_stats.misses,
            hit_rate = %format!("{:.1}%", cache_stats.hit_rate * 100.0),
            "Embedding cache statistics"
        );

        tracing::debug!(
            repo = %self.repo_name(),
            ?stats,
            "Index run completed"
        );

        if let Some(ref logger) = self.repo_logger {
            logger.info(&format!(
                "Index run completed: {} files scanned, {} indexed, {} unchanged, {} skipped, {} deleted, {} symbols",
                stats.files_scanned, stats.files_indexed, stats.files_unchanged,
                stats.files_skipped, stats.files_deleted, stats.symbols_indexed
            ));
        }

        Ok(stats)
    }

    /// Generate embeddings and similarity clusters for symbols that don't have them yet.
    ///
    /// This is called after parallel indexing to populate:
    /// - LanceDB vectors
    /// - similarity_clusters table
    ///
    /// Also used at startup for recovery when LanceDB data is missing
    /// but symbols exist in SQLite.
    ///
    /// Processes symbols in batches of 200 to bound peak memory usage.
    /// Trivial symbols (file-kind, tiny private helpers, test symbols) are
    /// skipped with a placeholder cluster entry so they are not re-fetched.
    ///
    /// I/O is pipelined: while embedding batch N, batch N-1's vectors are
    /// written to LanceDB in a background task, overlapping compute and I/O.
    pub async fn generate_embeddings_for_orphaned_symbols(&self) -> Result<()> {
        // Acquire lock to prevent concurrent runs (background startup recovery
        // vs post-indexing can race, duplicating LanceDB records).
        let _guard = self.embedding_generation_lock.lock().await;

        use crate::storage::sqlite::schema::SymbolRow;

        const BATCH_SIZE: usize = 200;
        let mut total_embedded: usize = 0;
        let mut total_skipped: usize = 0;
        let mut pending_write: Option<tokio::task::JoinHandle<Result<()>>> = None;

        loop {
            let sqlite = SqliteStore::open(&self.db_path)?;
            sqlite.init()?;

            let symbols_need_embeddings =
                sqlite.list_symbols_without_similarity_clusters(BATCH_SIZE)?;

            if symbols_need_embeddings.is_empty() {
                break;
            }

            let batch_len = symbols_need_embeddings.len();

            if total_embedded == 0 && total_skipped == 0 {
                tracing::info!(
                    repo = %self.repo_name(),
                    first_batch = batch_len,
                    "Generating embeddings for symbols after parallel indexing"
                );
            }

            // Partition into symbols to embed vs skip
            let mut to_embed: Vec<SymbolRow> = Vec::new();
            let mut to_skip_ids: Vec<String> = Vec::new();

            for sym in symbols_need_embeddings {
                if should_generate_embedding(
                    &sym.kind,
                    &sym.name,
                    &sym.file_path,
                    sym.exported,
                    sym.start_line,
                    sym.end_line,
                ) {
                    to_embed.push(SymbolRow {
                        id: sym.id,
                        file_path: sym.file_path,
                        language: sym.language,
                        kind: sym.kind,
                        name: sym.name,
                        exported: sym.exported,
                        start_byte: sym.start_byte,
                        end_byte: sym.end_byte,
                        start_line: sym.start_line,
                        end_line: sym.end_line,
                        text: sym.text,
                    });
                } else {
                    to_skip_ids.push(sym.id.clone());
                }
            }

            // Write placeholder similarity_clusters for skipped symbols
            // so they don't get re-fetched in the next batch.
            if !to_skip_ids.is_empty() {
                total_skipped += to_skip_ids.len();
                for id in &to_skip_ids {
                    let _ = sqlite.upsert_similarity_cluster(&SimilarityClusterRow {
                        symbol_id: id.clone(),
                        cluster_key: "__skipped__".to_string(),
                    });
                }
            }

            // Wait for the previous background write to finish before starting
            // a new one — limits concurrency to one in-flight write at a time.
            if let Some(handle) = pending_write.take() {
                handle
                    .await
                    .context("Background write task panicked")?
                    .context(
                        "Failed to add vector records for parallel indexing (pipelined write)",
                    )?;
            }

            if to_embed.is_empty() {
                // All symbols in this batch were skipped. If the batch was
                // full, there may be more symbols to process.
                if batch_len < BATCH_SIZE {
                    break;
                }
                continue;
            }

            // Generate embeddings for this batch
            let vectors = self
                .embed_and_build_vector_records(&to_embed)
                .await
                .with_context(|| {
                    format!(
                        "Failed to embed symbols batch: batch_size={}, total_so_far={}",
                        to_embed.len(),
                        total_embedded
                    )
                })?;

            total_embedded += vectors.len();

            // Spawn the LanceDB write + cluster upserts in a background task
            // so that the next batch's embedding runs concurrently with I/O.
            let vectors_to_write = vectors;
            let vectors_arc = Arc::clone(&self.vectors);
            let db_path = self.db_path.clone();
            pending_write = Some(tokio::spawn(async move {
                vectors_arc.add_records(&vectors_to_write).await?;
                let write_sqlite = SqliteStore::open(&db_path)?;
                write_sqlite.init()?;
                for rec in &vectors_to_write {
                    let _ = write_sqlite.upsert_similarity_cluster(&SimilarityClusterRow {
                        symbol_id: rec.id.clone(),
                        cluster_key: cluster_key_from_vector(&rec.vector),
                    });
                }
                Ok(())
            }));

            // If we got fewer than BATCH_SIZE, there are no more symbols left
            if batch_len < BATCH_SIZE {
                break;
            }
        }

        // Flush the final pending write
        if let Some(handle) = pending_write.take() {
            handle
                .await
                .context("Background write task panicked")?
                .context("Failed to add vector records for parallel indexing (final flush)")?;
        }

        if total_embedded > 0 || total_skipped > 0 {
            tracing::info!(
                repo = %self.repo_name(),
                embedded = total_embedded,
                skipped = total_skipped,
                "Generated embeddings and similarity clusters after parallel indexing"
            );
        }

        Ok(())
    }

    /// Build enriched text for embedding that includes semantic context.
    async fn embed_and_build_vector_records(
        &self,
        rows: &[SymbolRow],
    ) -> Result<Vec<VectorRecord>> {
        let mut vectors = Vec::with_capacity(rows.len());
        let mut uncached_texts: Vec<String> = Vec::new();
        let mut uncached_indices = Vec::new();

        // Preprocess text for embedding: semantic header + comment-stripped body.
        // Cache keys use preprocessed text (old raw-text entries become automatic misses).
        let embedding_texts: Vec<String> = rows
            .iter()
            .map(|row| {
                crate::text::prepare_embedding_text(&row.name, &row.kind, &row.file_path, &row.text)
            })
            .collect();

        // Check cache for each preprocessed text
        for (i, emb_text) in embedding_texts.iter().enumerate() {
            if let Some(cached) = self.cache.get(emb_text) {
                vectors.push((i, cached));
            } else {
                uncached_texts.push(emb_text.clone());
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

        // Store new embeddings in cache (keyed on preprocessed text)
        for (emb_text, embedding) in uncached_texts.iter().zip(&new_embeddings) {
            let _ = self.cache.put(emb_text, embedding);
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
            });
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::should_generate_embedding;

    #[test]
    fn skips_file_kind() {
        assert!(!should_generate_embedding(
            "file",
            "mod.rs",
            "src/mod.rs",
            true,
            0,
            100
        ));
    }

    #[test]
    fn skips_tiny_private_helper() {
        // 2 lines, not exported
        assert!(!should_generate_embedding(
            "function",
            "helper",
            "src/lib.rs",
            false,
            10,
            12
        ));
    }

    #[test]
    fn keeps_tiny_exported() {
        // 2 lines but exported — should be kept
        assert!(should_generate_embedding(
            "function",
            "get_name",
            "src/lib.rs",
            true,
            10,
            12
        ));
    }

    #[test]
    fn skips_test_file() {
        assert!(!should_generate_embedding(
            "function",
            "run",
            "src/lib.test.ts",
            true,
            0,
            50
        ));
    }

    #[test]
    fn skips_test_symbol() {
        assert!(!should_generate_embedding(
            "function",
            "test_something",
            "src/lib.rs",
            true,
            0,
            50
        ));
    }

    #[test]
    fn keeps_normal_exported_function() {
        assert!(should_generate_embedding(
            "function",
            "parse_config",
            "src/config.rs",
            true,
            0,
            30
        ));
    }
}

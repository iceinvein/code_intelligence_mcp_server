//! Session management for standalone mode: maps repo paths to per-repo AppState instances.

mod initial_index;
mod worktree;

pub use initial_index::RepoAccess;
pub use worktree::{
    data_dir_has_index_artifacts, resolve_base_repo, seed_index_from_base, SeedPlan,
};

use crate::{
    config::StandaloneConfig,
    embeddings::SharedEmbedder,
    handlers::AppState,
    indexer::pipeline::IndexPipeline,
    metrics::MetricsRegistry,
    path::Utf8PathBuf,
    registry::{RepoEntry, RepoRegistry},
    reranker::Reranker,
    retrieval::Retriever,
    server::jobs::{self, JobRegistry},
    storage::{sqlite::SqliteStore, tantivy::TantivyIndex, vector::LanceDbStore},
};
use anyhow::{Context, Result};
use dashmap::DashMap;
use serde::Serialize;
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Decide whether to spawn the index-time LLM description worker.
///
/// Descriptions are off by default (`descriptions_enabled` false): the backfill
/// is a multi-hour index-time cost with no proven retrieval benefit (R005/R006).
/// They run only when explicitly enabled, an LLM backend is available, and the
/// bench ablation knob is not set.
fn should_spawn_description_worker(
    descriptions_enabled: bool,
    llm_enabled: bool,
    bench_disabled: bool,
) -> bool {
    descriptions_enabled && llm_enabled && !bench_disabled
}

/// One repo awaiting an indexing decision. Held in memory only: pending consent
/// is transient, agent-driven state, so it is intentionally not persisted (that
/// would write a registry entry for every temp/worktree dir the gate exists to
/// skip). Cleared on restart; the gate re-records it when an agent retries.
#[derive(Debug, Clone, Serialize)]
pub struct PendingConsent {
    pub repo_path: String,
    pub repo_id: String,
    pub detected: String,
    pub recommendation: String,
    pub detail: Option<String>,
    pub first_seen_unix_s: i64,
    pub last_seen_unix_s: i64,
    pub occurrences: u32,
}

fn now_unix_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub struct SessionManager {
    pub standalone_config: Arc<StandaloneConfig>,
    pub registry: Arc<RepoRegistry>,
    embedder: Arc<SharedEmbedder>,
    /// Cross-encoder reranker shared across all repos (loads the ~600MB model
    /// once). `None` when `reranker_enabled` is false. Each repo's `Retriever`
    /// receives a clone of this handle.
    reranker: Option<Arc<dyn Reranker>>,
    /// Keyed by canonical repo path string.
    repos: DashMap<String, Arc<RepoRuntime>>,
    /// Per-key init locks to prevent TOCTOU races when two sessions init the same repo
    init_locks: DashMap<String, Arc<Mutex<()>>>,
    /// Separate locks for first-index readiness checks and job registration.
    initial_index_locks: DashMap<String, Arc<Mutex<()>>>,
    /// Tracks the last time each repo was accessed, for TTL-based eviction
    last_accessed: DashMap<String, Instant>,
    /// Repositories awaiting a user decision about their first full index.
    pending_consent: DashMap<String, PendingConsent>,
    metrics: Arc<MetricsRegistry>,
    /// Shared handle for all background indexing jobs.
    job_registry: JobRegistry,
}

struct RepoRuntime {
    state: Arc<AppState>,
    watch_cancel: CancellationToken,
    watcher_started: AtomicBool,
}

impl RepoRuntime {
    fn ensure_watcher_started(&self) -> bool {
        use std::sync::atomic::Ordering;

        if !self.state.config.watch_mode {
            return false;
        }
        if self.watcher_started.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.state
            .indexer
            .spawn_watch_loop(self.watch_cancel.clone());
        true
    }
}

impl SessionManager {
    pub async fn new(
        standalone_config: StandaloneConfig,
        registry: RepoRegistry,
        embedder: Arc<SharedEmbedder>,
        job_registry: Option<JobRegistry>,
        reranker: Option<Arc<dyn Reranker>>,
    ) -> Result<Self> {
        let metrics = Arc::new(MetricsRegistry::new().context("Failed to create MetricsRegistry")?);
        let job_registry = job_registry.unwrap_or_else(jobs::new_job_registry);

        Ok(Self {
            standalone_config: Arc::new(standalone_config),
            registry: Arc::new(registry),
            embedder,
            reranker,
            repos: DashMap::new(),
            init_locks: DashMap::new(),
            initial_index_locks: DashMap::new(),
            last_accessed: DashMap::new(),
            pending_consent: DashMap::new(),
            metrics,
            job_registry,
        })
    }

    pub fn job_registry(&self) -> JobRegistry {
        self.job_registry.clone()
    }

    #[cfg(test)]
    pub(crate) fn loaded_repo_count(&self) -> usize {
        self.repos.len()
    }

    pub async fn get_or_create_repo(&self, repo_path: &Utf8PathBuf) -> Result<Arc<AppState>> {
        Ok(self.get_or_create_runtime(repo_path).await?.state.clone())
    }

    async fn get_or_create_runtime(&self, repo_path: &Utf8PathBuf) -> Result<Arc<RepoRuntime>> {
        let repo_path = crate::path::canonicalize_existing_dir(repo_path)
            .context("Failed to canonicalize repository path")?;
        let canonical = repo_path.as_str().to_string();

        // Fast path: check if already exists (no lock needed)
        if let Some(entry) = self.repos.get(&canonical) {
            let _ = self.registry.touch(&canonical);
            self.last_accessed.insert(canonical, Instant::now());
            return Ok(entry.value().clone());
        }

        // Slow path: acquire per-key init lock to prevent TOCTOU race
        // (two sessions binding to the same repo simultaneously)
        let lock = self
            .init_locks
            .entry(canonical.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-check after acquiring lock (another task may have initialized it)
        if let Some(entry) = self.repos.get(&canonical) {
            let _ = self.registry.touch(&canonical);
            self.last_accessed.insert(canonical, Instant::now());
            return Ok(entry.value().clone());
        }

        let repo_entry = self
            .registry
            .register(&canonical)
            .context("Failed to register repository")?;

        let (state, watch_cancel) = self
            .init_repo_state(repo_path.clone(), &repo_entry)
            .await
            .context("Failed to initialize repository state")?;

        let runtime = Arc::new(RepoRuntime {
            state: Arc::new(state),
            watch_cancel,
            watcher_started: AtomicBool::new(false),
        });
        self.repos.insert(canonical.clone(), runtime.clone());
        self.last_accessed.insert(canonical, Instant::now());

        Ok(runtime)
    }

    /// Record (or refresh) a repo awaiting a consent decision. Keyed by repo id
    /// so repeated tool calls on the same repo bump `occurrences`/`last_seen`
    /// instead of duplicating.
    pub fn record_pending(&self, repo_path: &crate::path::Utf8Path) {
        let repo_id = RepoRegistry::path_hash(repo_path.as_str());
        let now = now_unix_s();
        self.pending_consent
            .entry(repo_id.clone())
            .and_modify(|p| {
                p.last_seen_unix_s = now;
                p.occurrences = p.occurrences.saturating_add(1);
            })
            .or_insert_with(|| {
                let class = crate::server::project_check::classify_repo(repo_path);
                PendingConsent {
                    repo_path: repo_path.as_str().to_string(),
                    repo_id,
                    detected: class.kind().to_string(),
                    recommendation: class.recommendation(),
                    detail: class.detail(),
                    first_seen_unix_s: now,
                    last_seen_unix_s: now,
                    occurrences: 1,
                }
            });
    }

    /// Snapshot of pending repos, oldest first.
    pub fn list_pending(&self) -> Vec<PendingConsent> {
        let mut v: Vec<PendingConsent> = self
            .pending_consent
            .iter()
            .map(|e| e.value().clone())
            .collect();
        v.sort_by_key(|p| p.first_seen_unix_s);
        v
    }

    /// Whether a repo id is currently pending.
    pub fn is_pending(&self, repo_id: &str) -> bool {
        self.pending_consent.contains_key(repo_id)
    }

    /// Drop a pending entry once the user has resolved it.
    pub fn clear_pending(&self, repo_id: &str) {
        self.pending_consent.remove(repo_id);
    }

    /// Evict repos that have not been accessed within `warm_ttl_seconds`.
    ///
    /// A TTL of `0` is treated as "never evict" (infinite lifetime).
    pub async fn evict_idle_repos(&self) {
        // Opportunistically drop decline records for ephemeral repos (worktrees,
        // temp copies) whose paths have since been deleted.
        if let Err(e) = self.registry.prune_declined_missing() {
            tracing::debug!(error = %e, "prune_declined_missing failed");
        }

        let ttl_secs = self.standalone_config.warm_ttl_seconds;
        if ttl_secs == 0 {
            return;
        }
        let ttl = Duration::from_secs(ttl_secs);

        // Collect keys to evict without holding DashMap shard locks during async work.
        let to_evict: Vec<String> = self
            .last_accessed
            .iter()
            .filter(|entry| {
                if entry.value().elapsed() <= ttl {
                    return false;
                }

                let repo_id = RepoRegistry::path_hash(entry.key());
                jobs::most_recent_running_for_repo(&self.job_registry, &repo_id).is_none()
            })
            .map(|entry| entry.key().clone())
            .collect();

        for key in to_evict {
            // Cancel the watcher before dropping the AppState.
            if let Some((_, runtime)) = self.repos.remove(&key) {
                runtime.watch_cancel.cancel();
            }
            let repo_id = RepoRegistry::path_hash(&key);
            crate::server::jobs::mark_running_watch_jobs_for_repo_failed(
                &self.job_registry,
                &repo_id,
                "repo evicted while watch job was running".to_string(),
            );
            self.last_accessed.remove(&key);
            self.init_locks.remove(&key);

            tracing::info!(
                repo = %key,
                ttl_secs,
                "Evicted idle repo from session cache"
            );
        }
    }

    /// Spawn a background task that calls [`evict_idle_repos`] every 60 seconds.
    ///
    /// The loop runs for the lifetime of the process — no cancellation is provided
    /// because the process owns the SessionManager and both die together.
    pub fn spawn_eviction_loop(self: &Arc<Self>) {
        let session = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                session.evict_idle_repos().await;
            }
        });
    }

    async fn init_repo_state(
        &self,
        repo_path: Utf8PathBuf,
        entry: &RepoEntry,
    ) -> Result<(AppState, CancellationToken)> {
        // Build per-repo config
        let config = self
            .standalone_config
            .repo_config(repo_path, &entry.data_dir);
        let config_arc = Arc::new(config);

        // Create storage directories
        std::fs::create_dir_all(&entry.data_dir).context("Failed to create repo data directory")?;
        if let Some(db_parent) = config_arc.db_path.parent() {
            std::fs::create_dir_all(db_parent).context("Failed to create db parent directory")?;
        }
        std::fs::create_dir_all(&config_arc.vector_db_path)
            .context("Failed to create vector db directory")?;
        std::fs::create_dir_all(&config_arc.tantivy_index_path)
            .context("Failed to create tantivy index directory")?;

        // Open SQLite and initialize schema
        let sqlite =
            SqliteStore::open(&config_arc.db_path).context("Failed to open SQLite store")?;
        sqlite
            .init()
            .context("Failed to initialize SQLite schema")?;
        let sqlite_arc = Arc::new(sqlite);

        // Open Tantivy index
        let tantivy = TantivyIndex::open_or_create(&config_arc.tantivy_index_path)
            .context("Failed to open Tantivy index")?;
        let tantivy_arc = Arc::new(tantivy);

        // Get embedder dimension. `SharedEmbedder::dim()` is cached at
        // construction so this requires no lock and no async work.
        let embedding_dim = self.embedder.dim();

        // Connect LanceDB, migrate if embedding dimension changed, then open table
        let lancedb = LanceDbStore::connect(&config_arc.vector_db_path)
            .await
            .context("Failed to connect to LanceDB")?;
        let _migrated = lancedb
            .migrate_vector_table("symbols", embedding_dim)
            .await
            .context("Failed to migrate vector table")?;
        let vectors = lancedb
            .open_or_create_table("symbols", embedding_dim)
            .await
            .context("Failed to open or create LanceDB table")?;
        let vectors_arc = Arc::new(vectors);

        // Create IndexPipeline
        let indexer = IndexPipeline::new_with_jobs(
            config_arc.clone(),
            sqlite_arc.clone(),
            tantivy_arc.clone(),
            vectors_arc.clone(),
            self.embedder.clone(),
            self.metrics.clone(),
            Some(self.job_registry.clone()),
        );

        // Create Retriever. The reranker (if enabled) is shared across all
        // repos; hyde is still unwired.
        let retriever = Retriever::new(
            config_arc.clone(),
            sqlite_arc.clone(),
            tantivy_arc,
            vectors_arc,
            self.embedder.clone(),
            self.reranker.clone(),
            None, // hyde_generator
            self.metrics.clone(),
        );

        let state = AppState {
            config: config_arc,
            indexer,
            retriever,
            sqlite: sqlite_arc,
            mcp_runtime: std::sync::Arc::new(once_cell::sync::OnceCell::new()),
            answer_generator: std::sync::Arc::new(once_cell::sync::OnceCell::new()),
            ask_code_cache: std::sync::Arc::new(Default::default()),
        };

        // The first-index coordinator starts the watcher only after a
        // successful native full index.
        let watch_cancel = CancellationToken::new();

        // Spawn the LLM description worker (gated by BENCH_DISABLE_DESCRIPTIONS env).
        // The worker pulls undescribed symbols from SQLite, runs Qwen to generate
        // descriptions, and re-upserts symbols into Tantivy with the description
        // field populated. Pre-v4 stdio mode wired this in run_embedded(); the v4
        // refactor (4736a0d) dropped the call site. Re-adding here so the daemon
        // exercises descriptions by default; bench arms set the env to ablate.
        let descriptions_disabled =
            std::env::var("BENCH_DISABLE_DESCRIPTIONS").as_deref() == Ok("1");
        if should_spawn_description_worker(
            state.config.descriptions_enabled,
            state.config.llm_enabled,
            descriptions_disabled,
        ) {
            let llm_config = state.config.clone();
            let llm_indexer = state.indexer.clone();
            let desc_cancel = watch_cancel.clone();
            tokio::spawn(async move {
                let generator = match tokio::task::spawn_blocking(move || {
                    crate::llm::create_llm_generator(&llm_config)
                })
                .await
                {
                    Ok(Ok(Some(llm))) => llm,
                    Ok(Ok(None)) => {
                        tracing::debug!(
                            "LLM descriptions unavailable, skipping description worker"
                        );
                        return;
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("Failed to create LLM generator: {}", e);
                        return;
                    }
                    Err(e) => {
                        tracing::warn!("LLM generator task panicked: {}", e);
                        return;
                    }
                };
                let _desc_handle = llm_indexer.spawn_description_worker(generator, desc_cancel);
                tracing::info!("LLM description worker spawned");
            });
        } else {
            tracing::info!(
                descriptions_enabled = state.config.descriptions_enabled,
                bench_disabled = descriptions_disabled,
                "LLM description worker not spawned (descriptions off by default; \
                 set DESCRIPTIONS_ENABLED=1 to generate them)"
            );
        }

        Ok((state, watch_cancel))
    }

    /// Delete a registered repository: cancel its watcher, drop the in-memory
    /// AppState, unregister from `registry.json`, and remove the on-disk
    /// data directory (`~/.code-intelligence/repos/<hash>/`).
    ///
    /// Returns the removed registry entry on success. Returns `Ok(None)` if
    /// the hash is unknown.
    ///
    /// The operation is best-effort: storage handles are released before
    /// the data directory is removed, but a failure to delete the directory
    /// (e.g. permission error) does NOT roll back the registry change.
    /// The caller sees the error and can retry the directory removal
    /// manually; the registry entry stays gone so the daemon does not
    /// attempt to reopen a half-deleted repo.
    pub async fn delete_repo_by_hash(&self, hash: &str) -> Result<Option<RepoEntry>> {
        let entry = match self.registry.get_by_hash(hash)? {
            Some(e) => e,
            None => return Ok(None),
        };

        let canonical = entry.path.clone();

        // 1. Cancel watcher and drop in-memory AppState (releases SQLite,
        //    Tantivy, LanceDB handles before we rmtree the dir).
        if let Some((_, runtime)) = self.repos.remove(&canonical) {
            runtime.watch_cancel.cancel();
        }
        crate::server::jobs::mark_running_watch_jobs_for_repo_failed(
            &self.job_registry,
            hash,
            "repo deleted while watch job was running".to_string(),
        );
        self.last_accessed.remove(&canonical);
        self.init_locks.remove(&canonical);
        self.initial_index_locks.remove(&canonical);

        // 2. Remove on-disk data directory. Missing dirs are treated as
        //    success; permission errors propagate and leave the registry entry
        //    intact so the user can retry from the API/UI.
        let data_dir = entry.data_dir.as_std_path();
        match std::fs::remove_dir_all(data_dir) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("Failed to remove repo data directory: {}", entry.data_dir)
                });
            }
        }

        // 3. Remove from registry.json after the destructive filesystem step
        //    succeeds. This avoids orphaning a registered repo when deletion
        //    fails.
        let removed = self
            .registry
            .remove_by_hash(hash)
            .context("Failed to remove repo from registry")?;

        tracing::info!(
            repo = %canonical,
            hash = %hash,
            data_dir = %entry.data_dir,
            "Deleted repo index"
        );

        Ok(removed)
    }

    /// Resolve a cross-repo symbol synchronously by looking up the target repo's AppState.
    ///
    /// This is a blocking helper used by the `CrossRepoResolver` trait implementation.
    /// It accesses the DashMap directly (no async init), so the target repo must already
    /// be initialized. Returns None if the repo is not currently loaded.
    fn resolve_symbol_in_loaded_repo(
        &self,
        to_repo_hash: &str,
        to_symbol_name: &str,
        to_symbol_file: Option<&str>,
    ) -> anyhow::Result<
        Option<(
            std::sync::Arc<crate::storage::sqlite::SqliteStore>,
            crate::storage::sqlite::SymbolRow,
        )>,
    > {
        // Look up the repo entry by hash to get its canonical path
        let entry = self.registry.get_by_hash(to_repo_hash)?;
        let entry = match entry {
            Some(e) => e,
            None => return Ok(None),
        };

        // Check if the repo is currently loaded
        let state = match self.repos.get(&entry.path) {
            Some(runtime) => runtime.state.clone(),
            None => return Ok(None),
        };

        // Search for the symbol by name, optionally scoped to file
        let symbols =
            state
                .sqlite
                .search_symbols_by_exact_name(to_symbol_name, to_symbol_file, 1)?;

        match symbols.into_iter().next() {
            Some(sym) => Ok(Some((state.sqlite.clone(), sym))),
            None => Ok(None),
        }
    }

    #[cfg(test)]
    pub async fn new_for_test(data_dir: Utf8PathBuf) -> Self {
        use crate::config::EmbeddingsBackend;
        use crate::embeddings::hash::HashEmbedder;

        let standalone_config = StandaloneConfig {
            embeddings_backend: EmbeddingsBackend::Hash,
            hash_embedding_dim: 64,
            ..StandaloneConfig::default()
        };

        let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));

        let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));

        Self::new(standalone_config, registry, embedder, None, None)
            .await
            .expect("Failed to create test SessionManager")
    }

    /// Build a test SessionManager with a custom TTL.
    #[cfg(test)]
    pub async fn new_for_test_with_ttl(data_dir: Utf8PathBuf, warm_ttl_seconds: u64) -> Self {
        use crate::config::EmbeddingsBackend;
        use crate::embeddings::hash::HashEmbedder;

        let standalone_config = StandaloneConfig {
            embeddings_backend: EmbeddingsBackend::Hash,
            hash_embedding_dim: 64,
            warm_ttl_seconds,
            ..StandaloneConfig::default()
        };

        let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));

        let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));

        Self::new(standalone_config, registry, embedder, None, None)
            .await
            .expect("Failed to create test SessionManager")
    }
}

impl crate::graph::CrossRepoResolver for SessionManager {
    fn resolve_cross_repo_symbol(
        &self,
        to_repo_hash: &str,
        to_symbol_name: &str,
        to_symbol_file: Option<&str>,
    ) -> anyhow::Result<
        Option<(
            std::sync::Arc<crate::storage::sqlite::SqliteStore>,
            crate::storage::sqlite::SymbolRow,
        )>,
    > {
        self.resolve_symbol_in_loaded_repo(to_repo_hash, to_symbol_name, to_symbol_file)
    }

    fn list_cross_repo_edges_from(
        &self,
        sqlite: &crate::storage::sqlite::SqliteStore,
        from_symbol_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<crate::storage::sqlite::CrossRepoEdgeRow>> {
        sqlite.list_cross_repo_edges_from(from_symbol_id, limit)
    }

    fn repo_name_for_hash(&self, repo_hash: &str) -> anyhow::Result<Option<String>> {
        let entry = self.registry.get_by_hash(repo_hash)?;
        Ok(entry.map(|e| e.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── helpers ────────────────────────────────────────────────────────────────

    /// Create a temporary directory that persists until the returned `TempDir` is dropped.
    fn temp_data_dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        (dir, path)
    }

    fn temp_repo_dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        (dir, path)
    }

    fn canonical_key(path: &Utf8PathBuf) -> String {
        crate::path::canonicalize_existing_dir(path)
            .unwrap()
            .to_string()
    }

    async fn wait_for_terminal_job(
        registry: &crate::server::jobs::JobRegistry,
        job_id: &str,
    ) -> crate::server::jobs::Job {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let job = registry.get(job_id).unwrap().clone();
                if job.status != crate::server::jobs::JobStatus::Running {
                    break job;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("initial index job timed out")
    }

    #[tokio::test]
    async fn unindexed_repo_requires_approval_even_when_explicitly_selected() {
        let (_data, data_dir) = temp_data_dir();
        let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
        let (_repo, repo_path) = temp_repo_dir();

        let access = manager.resolve_repo(repo_path.as_path()).await.unwrap();
        assert!(matches!(access, RepoAccess::NeedsApproval));
        assert_eq!(manager.loaded_repo_count(), 0);
    }

    #[tokio::test]
    async fn approval_starts_real_initial_index_without_file_event() {
        let (_data, data_dir) = temp_data_dir();
        let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
        let (_repo, repo_path) = temp_repo_dir();
        std::fs::write(
            repo_path.join("lib.rs"),
            "pub fn indexed_after_approval() -> usize { 1 }\n",
        )
        .unwrap();

        let access = manager
            .approve_and_start_initial_index(repo_path.as_path())
            .await
            .unwrap();
        let job_id = match access {
            RepoAccess::Indexing { job, started: true } => job.id,
            _ => panic!("approval must start the initial job"),
        };
        let finished = wait_for_terminal_job(&manager.job_registry, &job_id).await;
        assert_eq!(finished.status, crate::server::jobs::JobStatus::Succeeded);

        let ready = manager.resolve_repo(repo_path.as_path()).await.unwrap();
        let state = match ready {
            RepoAccess::Ready(state) => state,
            _ => panic!("successful initial index must unlock the repo"),
        };
        assert!(state.sqlite.count_symbols().unwrap() > 0);
    }

    #[tokio::test]
    async fn concurrent_approvals_share_one_initial_job() {
        let (_data, data_dir) = temp_data_dir();
        let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
        let (_repo, repo_path) = temp_repo_dir();

        let (left, right) = tokio::join!(
            manager.approve_and_start_initial_index(repo_path.as_path()),
            manager.approve_and_start_initial_index(repo_path.as_path())
        );
        let ids = [left.unwrap(), right.unwrap()]
            .into_iter()
            .map(|access| match access {
                RepoAccess::Indexing { job, .. } => job.id,
                RepoAccess::Ready(_) => String::from("ready"),
                _ => panic!("approval must not request consent again"),
            })
            .collect::<Vec<_>>();
        assert!(ids[0] == ids[1] || ids.iter().any(|id| id == "ready"));
    }

    #[tokio::test]
    async fn legacy_successful_index_run_is_backfilled_as_ready() {
        let (_data, data_dir) = temp_data_dir();
        let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
        let (_repo, repo_path) = temp_repo_dir();
        std::fs::write(repo_path.join("lib.rs"), "pub fn legacy_probe() {}\n").unwrap();
        let canonical = crate::path::canonicalize_existing_dir(&repo_path).unwrap();

        manager.registry.register(canonical.as_str()).unwrap();
        let runtime = manager.get_or_create_runtime(&canonical).await.unwrap();
        runtime.state.indexer.index_all().await.unwrap();

        let access = manager.resolve_repo(canonical.as_path()).await.unwrap();
        assert!(matches!(access, RepoAccess::Ready(_)));
        let entry = manager.registry.get(canonical.as_str()).unwrap().unwrap();
        assert!(entry.initial_index_completed_at.is_some());
    }

    #[tokio::test]
    async fn empty_repo_becomes_ready_after_successful_full_scan() {
        let (_data, data_dir) = temp_data_dir();
        let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
        let (_repo, repo_path) = temp_repo_dir();

        let started = manager
            .approve_and_start_initial_index(repo_path.as_path())
            .await
            .unwrap();
        let job_id = match started {
            RepoAccess::Indexing { job, .. } => job.id,
            _ => panic!("empty repository must still start a full scan"),
        };
        let finished = wait_for_terminal_job(&manager.job_registry, &job_id).await;
        assert_eq!(finished.status, crate::server::jobs::JobStatus::Succeeded);
        assert!(matches!(
            manager.resolve_repo(repo_path.as_path()).await.unwrap(),
            RepoAccess::Ready(_)
        ));
    }

    #[tokio::test]
    async fn persisted_approval_restarts_without_another_prompt() {
        let (_data, data_dir) = temp_data_dir();
        let (_repo, repo_path) = temp_repo_dir();
        let canonical = crate::path::canonicalize_existing_dir(&repo_path).unwrap();
        let first = SessionManager::new_for_test(data_dir.clone()).await;
        first
            .registry
            .approve_initial_index(canonical.as_str())
            .unwrap();
        drop(first);

        let restarted = Arc::new(SessionManager::new_for_test(data_dir).await);
        let access = restarted.resolve_repo(canonical.as_path()).await.unwrap();
        assert!(matches!(access, RepoAccess::Indexing { started: true, .. }));
    }

    #[tokio::test]
    async fn failed_initial_job_retries_without_another_prompt() {
        let (_data, data_dir) = temp_data_dir();
        let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
        let (_repo, repo_path) = temp_repo_dir();
        let canonical = crate::path::canonicalize_existing_dir(&repo_path).unwrap();
        let repo_id = RepoRegistry::path_hash(canonical.as_str());
        manager
            .registry
            .approve_initial_index(canonical.as_str())
            .unwrap();
        crate::server::jobs::register_running(
            &manager.job_registry,
            "failed-initial".to_string(),
            crate::server::jobs::JobKind::InitialBind,
            repo_id,
            canonical.to_string(),
        );
        crate::server::jobs::mark_failed(
            &manager.job_registry,
            "failed-initial",
            "intentional failure".to_string(),
        );

        let access = manager.resolve_repo(canonical.as_path()).await.unwrap();
        match access {
            RepoAccess::Indexing { job, started: true } => {
                assert_ne!(job.id, "failed-initial");
            }
            _ => panic!("persisted approval must start a replacement job"),
        }
    }

    #[tokio::test]
    async fn decline_never_initializes_repository_runtime() {
        let (_data, data_dir) = temp_data_dir();
        let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
        let (_repo, repo_path) = temp_repo_dir();
        let canonical = crate::path::canonicalize_existing_dir(&repo_path).unwrap();

        manager.decline_initial_index(canonical.as_path()).unwrap();
        assert!(matches!(
            manager.resolve_repo(canonical.as_path()).await.unwrap(),
            RepoAccess::Declined
        ));
        assert_eq!(manager.loaded_repo_count(), 0);
    }

    #[tokio::test]
    async fn watcher_starts_only_when_persisted_index_is_ready() {
        use std::sync::atomic::Ordering;

        let (_data, data_dir) = temp_data_dir();
        let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
        let (_repo, repo_path) = temp_repo_dir();
        let canonical = crate::path::canonicalize_existing_dir(&repo_path).unwrap();
        manager.registry.register(canonical.as_str()).unwrap();
        let runtime = manager.get_or_create_runtime(&canonical).await.unwrap();
        assert!(!runtime.watcher_started.load(Ordering::Acquire));

        runtime.state.indexer.index_all().await.unwrap();
        assert!(matches!(
            manager.resolve_repo(canonical.as_path()).await.unwrap(),
            RepoAccess::Ready(_)
        ));
        assert!(runtime.watcher_started.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn disabled_consent_gate_auto_authorizes_but_still_starts_index() {
        use crate::config::EmbeddingsBackend;
        use crate::embeddings::hash::HashEmbedder;

        let (_data, data_dir) = temp_data_dir();
        let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));
        let config = StandaloneConfig {
            data_dir: data_dir.clone(),
            embeddings_backend: EmbeddingsBackend::Hash,
            hash_embedding_dim: 64,
            index_consent_required: false,
            ..StandaloneConfig::default()
        };
        let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));
        let manager = Arc::new(
            SessionManager::new(config, registry, embedder, None, None)
                .await
                .unwrap(),
        );
        let (_repo, repo_path) = temp_repo_dir();

        assert!(matches!(
            manager.resolve_repo(repo_path.as_path()).await.unwrap(),
            RepoAccess::Indexing { started: true, .. }
        ));
    }

    // ─── existing tests ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_or_create_returns_same_state_for_same_repo() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;

        let (_repo, repo_path) = temp_repo_dir();

        let state1 = manager.get_or_create_repo(&repo_path).await.unwrap();
        let state2 = manager.get_or_create_repo(&repo_path).await.unwrap();

        // Same Arc — not recreated
        assert!(Arc::ptr_eq(&state1.config, &state2.config));
    }

    // ─── last_accessed tracking ──────────────────────────────────────────────────

    #[tokio::test]
    async fn last_accessed_populated_after_get_or_create() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (_repo, repo_path) = temp_repo_dir();

        assert!(
            manager.last_accessed.is_empty(),
            "no entries before first access"
        );

        manager.get_or_create_repo(&repo_path).await.unwrap();

        let key = canonical_key(&repo_path);
        assert!(
            manager.last_accessed.contains_key(&key),
            "last_accessed should have an entry after get_or_create"
        );
    }

    #[tokio::test]
    async fn last_accessed_updated_on_fast_path() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (_repo, repo_path) = temp_repo_dir();

        // First call — slow path (initialisation)
        manager.get_or_create_repo(&repo_path).await.unwrap();

        let key = canonical_key(&repo_path);
        let first_ts = *manager.last_accessed.get(&key).unwrap();

        // Sleep briefly so the two Instants differ
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Second call — fast path (repo already in map)
        manager.get_or_create_repo(&repo_path).await.unwrap();

        let second_ts = *manager.last_accessed.get(&key).unwrap();
        assert!(
            second_ts >= first_ts,
            "last_accessed should be refreshed on fast-path hit"
        );
    }

    #[tokio::test]
    async fn record_pending_tracks_and_dedupes_by_repo_id() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir =
            crate::path::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("utf8 path");
        let sm = SessionManager::new_for_test(data_dir).await;

        // A plain absolute path (not under TMPDIR, not a worktree) classifies as standard.
        let p = crate::path::Utf8Path::new("/Users/dev/proj");
        sm.record_pending(p);
        sm.record_pending(p);

        let pending = sm.list_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].repo_path, "/Users/dev/proj");
        assert_eq!(pending[0].detected, "standard");
        assert_eq!(pending[0].occurrences, 2);
        assert!(sm.is_pending(&pending[0].repo_id));

        sm.clear_pending(&pending[0].repo_id);
        assert_eq!(sm.list_pending().len(), 0);
        assert!(!sm.is_pending(&pending[0].repo_id));
    }

    #[tokio::test]
    async fn list_pending_is_sorted_by_first_seen() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir =
            crate::path::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("utf8 path");
        let sm = SessionManager::new_for_test(data_dir).await;
        sm.record_pending(crate::path::Utf8Path::new("/Users/dev/a"));
        sm.record_pending(crate::path::Utf8Path::new("/Users/dev/b"));
        let pending = sm.list_pending();
        assert_eq!(pending.len(), 2);
        assert!(pending[0].first_seen_unix_s <= pending[1].first_seen_unix_s);
    }

    // ─── eviction ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn evict_idle_repos_removes_expired_entries() {
        let (_data, data_dir) = temp_data_dir();
        // TTL of 1 second
        let manager = SessionManager::new_for_test_with_ttl(data_dir, 1).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);

        // Overwrite last_accessed with a timestamp far in the past
        manager
            .last_accessed
            .insert(key.clone(), Instant::now() - Duration::from_secs(10));

        manager.evict_idle_repos().await;

        assert!(
            !manager.repos.contains_key(&key),
            "expired repo should be evicted from repos"
        );
        assert!(
            !manager.last_accessed.contains_key(&key),
            "expired repo should be evicted from last_accessed"
        );
        assert!(
            !manager.init_locks.contains_key(&key),
            "expired repo should be evicted from init_locks"
        );
    }

    #[tokio::test]
    async fn evict_idle_repos_preserves_recently_accessed() {
        let (_data, data_dir) = temp_data_dir();
        // TTL of 300 seconds (default)
        let manager = SessionManager::new_for_test_with_ttl(data_dir, 300).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);

        // last_accessed is "just now" — should NOT be evicted
        manager.evict_idle_repos().await;

        assert!(
            manager.repos.contains_key(&key),
            "recently accessed repo must NOT be evicted"
        );
        assert!(
            manager.last_accessed.contains_key(&key),
            "last_accessed entry must survive for recent repos"
        );
    }

    #[tokio::test]
    async fn evict_idle_repos_preserves_repo_with_running_initial_index() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test_with_ttl(data_dir, 1).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);
        let repo_id = RepoRegistry::path_hash(&key);
        let job = crate::server::jobs::register_running(
            &manager.job_registry,
            "long-first-index".to_string(),
            crate::server::jobs::JobKind::InitialBind,
            repo_id,
            key.clone(),
        );
        manager
            .last_accessed
            .insert(key.clone(), Instant::now() - Duration::from_secs(10));

        manager.evict_idle_repos().await;

        assert!(
            manager.repos.contains_key(&key),
            "a repo with an active first index must stay loaded"
        );
        assert_eq!(
            manager.job_registry.get(&job.id).unwrap().status,
            crate::server::jobs::JobStatus::Running,
            "cache eviction must not fail an active first-index job"
        );
    }

    #[tokio::test]
    async fn delete_repo_by_hash_removes_state_registry_and_dir() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);
        let hash = crate::registry::RepoRegistry::path_hash(&key);

        // Capture the data dir BEFORE delete so we can check it was rmtree'd
        let entry = manager.registry.get_by_hash(&hash).unwrap().unwrap();
        let data_dir = entry.data_dir.clone();
        assert!(
            data_dir.as_std_path().exists(),
            "data dir should exist after init"
        );

        let removed = manager.delete_repo_by_hash(&hash).await.unwrap();
        assert!(removed.is_some(), "delete should return the removed entry");

        // In-memory state cleared
        assert!(!manager.repos.contains_key(&key));
        assert!(!manager.last_accessed.contains_key(&key));
        assert!(!manager.init_locks.contains_key(&key));

        // Registry no longer knows the repo
        assert!(manager.registry.get_by_hash(&hash).unwrap().is_none());

        // On-disk data directory is gone
        assert!(
            !data_dir.as_std_path().exists(),
            "data dir should be removed: {}",
            data_dir
        );
    }

    #[tokio::test]
    async fn delete_repo_by_hash_unknown_returns_none() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;

        let removed = manager
            .delete_repo_by_hash("0000000000000000")
            .await
            .unwrap();
        assert!(removed.is_none());
    }

    #[tokio::test]
    async fn delete_repo_by_hash_keeps_registry_when_data_dir_delete_fails() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (_repo, repo_path) = temp_repo_dir();

        let entry = manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
        std::fs::remove_dir_all(entry.data_dir.as_std_path()).unwrap();
        std::fs::write(entry.data_dir.as_std_path(), b"not a directory").unwrap();

        let err = manager.delete_repo_by_hash(&hash).await.unwrap_err();

        assert!(
            err.to_string()
                .contains("Failed to remove repo data directory"),
            "unexpected delete error: {err}"
        );
        assert!(
            manager.registry.get_by_hash(&hash).unwrap().is_some(),
            "registry entry must remain so a failed data-dir deletion can be retried"
        );
    }

    #[tokio::test]
    async fn delete_repo_by_hash_works_when_not_loaded() {
        // Repo is registered but NOT currently in the session cache
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir.clone()).await;
        let (_repo, repo_path) = temp_repo_dir();

        // Register directly via registry — never call get_or_create_repo
        let entry = manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());

        assert!(entry.data_dir.as_std_path().exists());

        let removed = manager.delete_repo_by_hash(&hash).await.unwrap();
        assert!(removed.is_some());
        assert!(!entry.data_dir.as_std_path().exists());
        assert!(manager.registry.get_by_hash(&hash).unwrap().is_none());
    }

    #[tokio::test]
    async fn description_worker_skipped_when_bench_disable_descriptions_set() {
        // This test confirms init_repo_state does not panic when the env is set
        // and the worker-spawn branch is the disabled path. It's a weak test by
        // design - tokio::spawn is fire-and-forget. The strong assertion that
        // descriptions are not written lives in storage::tantivy tests.
        static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _guard = ENV_LOCK.lock().await;

        std::env::set_var("BENCH_DISABLE_DESCRIPTIONS", "1");

        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir.clone()).await;
        let repo_path = Utf8PathBuf::from(data_dir.as_str()).join("fake-repo");
        std::fs::create_dir_all(&repo_path).unwrap();
        std::fs::write(
            repo_path.join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.0.0\"\n",
        )
        .unwrap();

        let _state = manager.get_or_create_repo(&repo_path).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        std::env::remove_var("BENCH_DISABLE_DESCRIPTIONS");
    }

    #[test]
    fn description_worker_gate() {
        // Off by default: descriptions_enabled=false means no worker, even with
        // the LLM available.
        assert!(!should_spawn_description_worker(false, true, false));
        // Opted in, LLM available, not bench-disabled → spawn.
        assert!(should_spawn_description_worker(true, true, false));
        // Bench ablation overrides the opt-in.
        assert!(!should_spawn_description_worker(true, true, true));
        // No LLM backend → nothing to spawn.
        assert!(!should_spawn_description_worker(true, false, false));
    }

    #[tokio::test]
    async fn evict_idle_repos_prunes_declined_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let manager = SessionManager::new_for_test(data_dir).await;

        // A declined repo whose path does not exist.
        manager
            .registry
            .set_consent("/no/such/declined", crate::registry::IndexConsent::Declined)
            .unwrap();

        manager.evict_idle_repos().await;

        assert_eq!(
            manager
                .registry
                .consent_status("/no/such/declined")
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn evict_idle_repos_zero_ttl_never_evicts() {
        let (_data, data_dir) = temp_data_dir();
        // TTL of 0 means infinite — never evict
        let manager = SessionManager::new_for_test_with_ttl(data_dir, 0).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);

        // Backdate to look very old
        manager
            .last_accessed
            .insert(key.clone(), Instant::now() - Duration::from_secs(86400));

        manager.evict_idle_repos().await;

        assert!(
            manager.repos.contains_key(&key),
            "TTL=0 must never evict any repo"
        );
    }
}

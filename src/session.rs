//! Session management for standalone mode — maps repo paths to per-repo AppState instances

use crate::{
    config::StandaloneConfig,
    embeddings::Embedder,
    handlers::AppState,
    indexer::pipeline::IndexPipeline,
    metrics::MetricsRegistry,
    path::Utf8PathBuf,
    registry::{RepoEntry, RepoRegistry},
    retrieval::Retriever,
    storage::{sqlite::SqliteStore, tantivy::TantivyIndex, vector::LanceDbStore},
};
use anyhow::{Context, Result};
use dashmap::DashMap;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct SessionManager {
    pub standalone_config: Arc<StandaloneConfig>,
    pub registry: Arc<RepoRegistry>,
    embedder: Arc<Mutex<Box<dyn Embedder + Send>>>,
    /// Keyed by canonical repo path string. Value is (AppState, watcher cancel token).
    repos: DashMap<String, (Arc<AppState>, CancellationToken)>,
    /// Per-key init locks to prevent TOCTOU races when two sessions init the same repo
    init_locks: DashMap<String, Arc<Mutex<()>>>,
    /// Tracks the last time each repo was accessed, for TTL-based eviction
    last_accessed: DashMap<String, Instant>,
    metrics: Arc<MetricsRegistry>,
}

impl SessionManager {
    pub async fn new(
        standalone_config: StandaloneConfig,
        registry: RepoRegistry,
        embedder: Box<dyn Embedder + Send>,
    ) -> Result<Self> {
        let metrics = Arc::new(MetricsRegistry::new().context("Failed to create MetricsRegistry")?);

        Ok(Self {
            standalone_config: Arc::new(standalone_config),
            registry: Arc::new(registry),
            embedder: Arc::new(Mutex::new(embedder)),
            repos: DashMap::new(),
            init_locks: DashMap::new(),
            last_accessed: DashMap::new(),
            metrics,
        })
    }

    pub async fn get_or_create_repo(&self, repo_path: &Utf8PathBuf) -> Result<Arc<AppState>> {
        let canonical = repo_path.as_str().to_string();

        // Fast path: check if already exists (no lock needed)
        if let Some(entry) = self.repos.get(&canonical) {
            let _ = self.registry.touch(&canonical);
            self.last_accessed.insert(canonical, Instant::now());
            return Ok(entry.0.clone());
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
            return Ok(entry.0.clone());
        }

        let repo_entry = self
            .registry
            .register(&canonical)
            .context("Failed to register repository")?;

        let (state, watch_cancel) = self
            .init_repo_state(repo_path.clone(), &repo_entry)
            .await
            .context("Failed to initialize repository state")?;

        let state_arc = Arc::new(state);
        self.repos
            .insert(canonical.clone(), (state_arc.clone(), watch_cancel));
        self.last_accessed.insert(canonical, Instant::now());

        Ok(state_arc)
    }

    /// Evict repos that have not been accessed within `warm_ttl_seconds`.
    ///
    /// A TTL of `0` is treated as "never evict" (infinite lifetime).
    pub async fn evict_idle_repos(&self) {
        let ttl_secs = self.standalone_config.warm_ttl_seconds;
        if ttl_secs == 0 {
            return;
        }
        let ttl = Duration::from_secs(ttl_secs);

        // Collect keys to evict without holding DashMap shard locks during async work.
        let to_evict: Vec<String> = self
            .last_accessed
            .iter()
            .filter(|entry| entry.value().elapsed() > ttl)
            .map(|entry| entry.key().clone())
            .collect();

        for key in to_evict {
            // Cancel the watcher before dropping the AppState.
            if let Some((_, (_, cancel))) = self.repos.remove(&key) {
                cancel.cancel();
            }
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

        // Get embedder dimension
        let embedding_dim = {
            let embedder = self.embedder.lock().await;
            embedder.dim()
        };

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
        let indexer = IndexPipeline::new(
            config_arc.clone(),
            tantivy_arc.clone(),
            vectors_arc.clone(),
            self.embedder.clone(),
            self.metrics.clone(),
        );

        // Create Retriever (no reranker, no hyde for now)
        let retriever = Retriever::new(
            config_arc.clone(),
            tantivy_arc,
            vectors_arc,
            self.embedder.clone(),
            None, // reranker
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

        // Start file watcher for auto-reindexing.
        // Store the cancel token so we can stop the watcher on eviction.
        let watch_cancel = CancellationToken::new();
        if state.config.watch_mode {
            state.indexer.spawn_watch_loop(watch_cancel.clone());
        }

        Ok((state, watch_cancel))
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
            Some(s) => s.0.clone(),
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

        let embedder = Box::new(HashEmbedder::new(64));

        Self::new(standalone_config, registry, embedder)
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

        let embedder = Box::new(HashEmbedder::new(64));

        Self::new(standalone_config, registry, embedder)
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

        let key = repo_path.as_str().to_string();
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

        let key = repo_path.as_str().to_string();
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

    // ─── eviction ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn evict_idle_repos_removes_expired_entries() {
        let (_data, data_dir) = temp_data_dir();
        // TTL of 1 second
        let manager = SessionManager::new_for_test_with_ttl(data_dir, 1).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = repo_path.as_str().to_string();

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
        let key = repo_path.as_str().to_string();

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
    async fn evict_idle_repos_zero_ttl_never_evicts() {
        let (_data, data_dir) = temp_data_dir();
        // TTL of 0 means infinite — never evict
        let manager = SessionManager::new_for_test_with_ttl(data_dir, 0).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = repo_path.as_str().to_string();

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

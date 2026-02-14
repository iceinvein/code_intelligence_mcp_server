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
    storage::{
        sqlite::SqliteStore,
        tantivy::TantivyIndex,
        vector::LanceDbStore,
    },
};
use anyhow::{Context, Result};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct SessionManager {
    pub standalone_config: Arc<StandaloneConfig>,
    pub registry: Arc<RepoRegistry>,
    embedder: Arc<Mutex<Box<dyn Embedder + Send>>>,
    repos: DashMap<String, Arc<AppState>>,  // keyed by canonical repo path string
    /// Per-key init locks to prevent TOCTOU races when two sessions init the same repo
    init_locks: DashMap<String, Arc<Mutex<()>>>,
    metrics: Arc<MetricsRegistry>,
}

impl SessionManager {
    pub async fn new(
        standalone_config: StandaloneConfig,
        registry: RepoRegistry,
        embedder: Box<dyn Embedder + Send>,
    ) -> Result<Self> {
        let metrics = Arc::new(MetricsRegistry::new()
            .context("Failed to create MetricsRegistry")?);

        Ok(Self {
            standalone_config: Arc::new(standalone_config),
            registry: Arc::new(registry),
            embedder: Arc::new(Mutex::new(embedder)),
            repos: DashMap::new(),
            init_locks: DashMap::new(),
            metrics,
        })
    }

    pub async fn get_or_create_repo(&self, repo_path: &Utf8PathBuf) -> Result<Arc<AppState>> {
        let canonical = repo_path.as_str().to_string();

        // Fast path: check if already exists (no lock needed)
        if let Some(state) = self.repos.get(&canonical) {
            let _ = self.registry.touch(&canonical);
            return Ok(state.clone());
        }

        // Slow path: acquire per-key init lock to prevent TOCTOU race
        // (two sessions binding to the same repo simultaneously)
        let lock = self.init_locks
            .entry(canonical.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-check after acquiring lock (another task may have initialized it)
        if let Some(state) = self.repos.get(&canonical) {
            let _ = self.registry.touch(&canonical);
            return Ok(state.clone());
        }

        let entry = self.registry.register(&canonical)
            .context("Failed to register repository")?;

        let state = self.init_repo_state(repo_path.clone(), &entry).await
            .context("Failed to initialize repository state")?;

        let state_arc = Arc::new(state);
        self.repos.insert(canonical, state_arc.clone());

        Ok(state_arc)
    }

    async fn init_repo_state(&self, repo_path: Utf8PathBuf, entry: &RepoEntry) -> Result<AppState> {
        // Build per-repo config
        let config = self.standalone_config.repo_config(repo_path, &entry.data_dir);
        let config_arc = Arc::new(config);

        // Create storage directories
        std::fs::create_dir_all(&entry.data_dir)
            .context("Failed to create repo data directory")?;
        if let Some(db_parent) = config_arc.db_path.parent() {
            std::fs::create_dir_all(db_parent)
                .context("Failed to create db parent directory")?;
        }
        std::fs::create_dir_all(&config_arc.vector_db_path)
            .context("Failed to create vector db directory")?;
        std::fs::create_dir_all(&config_arc.tantivy_index_path)
            .context("Failed to create tantivy index directory")?;

        // Open SQLite and initialize schema
        let sqlite = SqliteStore::open(&config_arc.db_path)
            .context("Failed to open SQLite store")?;
        sqlite.init()
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
        let lancedb = LanceDbStore::connect(&config_arc.vector_db_path).await
            .context("Failed to connect to LanceDB")?;
        let _migrated = lancedb.migrate_vector_table("symbols", embedding_dim).await
            .context("Failed to migrate vector table")?;
        let vectors = lancedb.open_or_create_table("symbols", embedding_dim).await
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
            is_leader: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            role_rx: tokio::sync::watch::channel(crate::leader::Role::Leader).1,
        };

        // Start file watcher for auto-reindexing
        if state.config.watch_mode {
            let watch_cancel = tokio_util::sync::CancellationToken::new();
            state.indexer.spawn_watch_loop(watch_cancel);
        }

        Ok(state)
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

        let registry = RepoRegistry::new(
            data_dir.join("registry.json"),
            data_dir.join("repos"),
        );

        let embedder = Box::new(HashEmbedder::new(64));

        Self::new(standalone_config, registry, embedder)
            .await
            .expect("Failed to create test SessionManager")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_or_create_returns_same_state_for_same_repo() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let manager = SessionManager::new_for_test(data_dir).await;

        // Create a temp repo dir
        let repo_dir = tempfile::tempdir().unwrap();
        let repo_path = Utf8PathBuf::from_path_buf(repo_dir.path().to_path_buf()).unwrap();

        let state1 = manager.get_or_create_repo(&repo_path).await.unwrap();
        let state2 = manager.get_or_create_repo(&repo_path).await.unwrap();

        // Same Arc — not recreated
        assert!(Arc::ptr_eq(&state1.config, &state2.config));
    }
}

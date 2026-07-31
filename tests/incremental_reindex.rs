use code_intelligence_mcp_server::config::{EmbeddingsBackend, StandaloneConfig};
use code_intelligence_mcp_server::embeddings::hash::HashEmbedder;
use code_intelligence_mcp_server::embeddings::SharedEmbedder;
use code_intelligence_mcp_server::path::Utf8PathBuf;
use code_intelligence_mcp_server::registry::RepoRegistry;
use code_intelligence_mcp_server::server::jobs;
use code_intelligence_mcp_server::session::SessionManager;
use std::sync::Arc;

#[tokio::test]
async fn touching_files_without_editing_them_reindexes_nothing() {
    let data_temp = tempfile::tempdir().unwrap();
    let repo_temp = tempfile::tempdir().unwrap();
    let data_dir = Utf8PathBuf::from_path_buf(data_temp.path().to_path_buf()).unwrap();
    let repo = Utf8PathBuf::from_path_buf(repo_temp.path().to_path_buf()).unwrap();
    std::fs::write(
        repo.join("lib.rs").as_std_path(),
        "pub fn probe() -> usize { 1 }\n",
    )
    .unwrap();

    let config = StandaloneConfig {
        data_dir: data_dir.clone(),
        embeddings_backend: EmbeddingsBackend::Hash,
        hash_embedding_dim: 64,
        ..StandaloneConfig::default()
    };
    let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));
    let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));
    let manager = Arc::new(
        SessionManager::new(
            config,
            registry,
            embedder,
            Some(jobs::new_job_registry()),
            None,
        )
        .await
        .unwrap(),
    );

    let canonical = code_intelligence_mcp_server::path::canonicalize_existing_dir(&repo).unwrap();
    manager.registry.register(canonical.as_str()).unwrap();
    let state = manager.get_or_create_repo(&canonical).await.unwrap();

    let first = state.indexer.index_all().await.unwrap();
    assert_eq!(first.files_indexed, 1);

    // Rewrite identical bytes, the way `git checkout` does.
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(
        repo.join("lib.rs").as_std_path(),
        "pub fn probe() -> usize { 1 }\n",
    )
    .unwrap();

    let second = state.indexer.index_all().await.unwrap();
    assert_eq!(
        second.files_indexed, 0,
        "identical content must not reparse"
    );
    assert_eq!(second.files_unchanged, 1);
    assert_eq!(second.embeddings_generated, 0);

    // The restamp persisted, so a third pass takes the no-read fast path.
    let third = state.indexer.index_all().await.unwrap();
    assert_eq!(third.files_indexed, 0);
    assert_eq!(third.files_unchanged, 1);
}

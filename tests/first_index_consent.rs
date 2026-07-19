use code_intelligence_mcp_server::config::{EmbeddingsBackend, StandaloneConfig};
use code_intelligence_mcp_server::embeddings::hash::HashEmbedder;
use code_intelligence_mcp_server::embeddings::SharedEmbedder;
use code_intelligence_mcp_server::path::Utf8PathBuf;
use code_intelligence_mcp_server::registry::RepoRegistry;
use code_intelligence_mcp_server::retrieval::ContextMode;
use code_intelligence_mcp_server::server::jobs;
use code_intelligence_mcp_server::session::{RepoAccess, SessionManager};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn first_index_requires_consent_then_becomes_searchable_without_file_event() {
    let data_temp = tempfile::tempdir().unwrap();
    let repo_temp = tempfile::tempdir().unwrap();
    let data_dir = Utf8PathBuf::from_path_buf(data_temp.path().to_path_buf()).unwrap();
    let repo = Utf8PathBuf::from_path_buf(repo_temp.path().to_path_buf()).unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/lib.rs"),
        "pub fn consent_index_probe() -> bool { true }\n",
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

    let before = manager.resolve_repo(repo.as_path()).await.unwrap();
    assert!(matches!(before, RepoAccess::NeedsApproval));

    let started = manager
        .approve_and_start_initial_index(repo.as_path())
        .await
        .unwrap();
    assert!(matches!(started, RepoAccess::Indexing { .. }));

    let state = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match manager.resolve_repo(repo.as_path()).await.unwrap() {
                RepoAccess::Ready(state) => break state,
                RepoAccess::Indexing { .. } => tokio::task::yield_now().await,
                RepoAccess::NeedsApproval => panic!("approval was not persisted"),
                RepoAccess::Declined => panic!("repository was unexpectedly declined"),
            }
        }
    })
    .await
    .expect("first index timed out");

    let results = state
        .retriever
        .search("consent_index_probe", 10, false, ContextMode::None)
        .await
        .unwrap();
    assert!(results
        .response
        .hits
        .iter()
        .any(|hit| hit.name == "consent_index_probe"));
}

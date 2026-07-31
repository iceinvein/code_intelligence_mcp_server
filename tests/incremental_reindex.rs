use code_intelligence_mcp_server::config::{EmbeddingsBackend, StandaloneConfig};
use code_intelligence_mcp_server::embeddings::hash::HashEmbedder;
use code_intelligence_mcp_server::embeddings::SharedEmbedder;
use code_intelligence_mcp_server::indexer::pipeline::utils::file_fingerprint;
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

    let fingerprint_after_first = state
        .sqlite
        .get_file_fingerprint("lib.rs")
        .unwrap()
        .expect("first pass must persist a fingerprint row");

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

    // Counts alone don't distinguish `Unchanged` from `Restamped` (both count
    // as files_unchanged), so assert directly against the persisted row: the
    // stored stat must have moved (proving pass 2 actually took the
    // second-chance path rather than vacuously passing because the
    // filesystem's mtime granularity happened not to move), it must match
    // the current on-disk stat (proving the restamp wrote the *new* stat,
    // not a stale one), and the hash must remain non-NULL.
    let on_disk_after_rewrite = file_fingerprint(repo.join("lib.rs").as_std_path()).unwrap();
    let fingerprint_after_second = state
        .sqlite
        .get_file_fingerprint("lib.rs")
        .unwrap()
        .expect("restamp must keep a fingerprint row for lib.rs");
    assert_ne!(
        fingerprint_after_second.mtime_ns, fingerprint_after_first.mtime_ns,
        "stored mtime must move after the rewrite, or pass 2 never exercised the second-chance path"
    );
    assert_eq!(
        fingerprint_after_second.mtime_ns, on_disk_after_rewrite.mtime_ns,
        "restamp must persist the post-rewrite mtime, not the stale pass-1 value"
    );
    assert!(
        fingerprint_after_second.content_hash.is_some(),
        "restamp must not null out the content hash"
    );

    // The restamp persisted, so a third pass takes the no-read fast path.
    let third = state.indexer.index_all().await.unwrap();
    assert_eq!(third.files_indexed, 0);
    assert_eq!(third.files_unchanged, 1);

    let fingerprint_after_third = state
        .sqlite
        .get_file_fingerprint("lib.rs")
        .unwrap()
        .expect("fingerprint row must still exist after pass 3");
    assert_eq!(
        fingerprint_after_third.mtime_ns, fingerprint_after_second.mtime_ns,
        "pass 3 must not touch the fingerprint again: nothing changed on disk"
    );
}

/// A pass that replaces a file's symbols must not leave their index rows behind.
///
/// `write_batch` disables foreign keys while it swaps a file's rows, so
/// `ON DELETE CASCADE` does not fire; the rows an earlier pass's PageRank and
/// clustering wrote for a renamed symbol would survive as orphans and
/// `validate_graph_integrity` would fail the whole run.
#[tokio::test]
async fn renaming_a_symbol_does_not_orphan_its_index_rows() {
    let data_temp = tempfile::tempdir().unwrap();
    let repo_temp = tempfile::tempdir().unwrap();
    let data_dir = Utf8PathBuf::from_path_buf(data_temp.path().to_path_buf()).unwrap();
    let repo = Utf8PathBuf::from_path_buf(repo_temp.path().to_path_buf()).unwrap();
    std::fs::write(
        repo.join("alpha.rs").as_std_path(),
        "pub fn alpha_probe() -> usize { alpha_helper() }\npub fn alpha_helper() -> usize { 1 }\n",
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

    // Rename both symbols so every id from pass 1 disappears.
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(
        repo.join("alpha.rs").as_std_path(),
        "pub fn renamed_probe() -> usize { renamed_helper() }\npub fn renamed_helper() -> usize { 1 }\n",
    )
    .unwrap();

    let second = state
        .indexer
        .index_all()
        .await
        .expect("replacing a file's symbols must not fail graph validation");
    assert_eq!(second.files_indexed, 1);

    let db = data_dir
        .join("repos")
        .join(RepoRegistry::path_hash(canonical.as_str()))
        .join("code-intelligence.db");
    let conn = rusqlite::Connection::open(db.as_std_path()).unwrap();
    for table in ["symbol_metrics", "similarity_clusters"] {
        let orphans: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM {table} t \
                     LEFT JOIN symbols s ON s.id = t.symbol_id WHERE s.id IS NULL"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "no {table} row may outlive its symbol");
    }
}

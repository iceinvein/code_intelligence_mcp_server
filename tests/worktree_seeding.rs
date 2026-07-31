use code_intelligence_mcp_server::config::{EmbeddingsBackend, StandaloneConfig};
use code_intelligence_mcp_server::embeddings::hash::HashEmbedder;
use code_intelligence_mcp_server::embeddings::SharedEmbedder;
use code_intelligence_mcp_server::path::{Utf8Path, Utf8PathBuf};
use code_intelligence_mcp_server::registry::RepoRegistry;
use code_intelligence_mcp_server::server::jobs;
use code_intelligence_mcp_server::session::{RepoAccess, SessionManager};
use std::sync::Arc;
use std::time::Duration;

fn utf8(p: &std::path::Path) -> Utf8PathBuf {
    Utf8PathBuf::from_path_buf(p.to_path_buf()).unwrap()
}

async fn manager_for(data_dir: &Utf8Path) -> Arc<SessionManager> {
    let config = StandaloneConfig {
        data_dir: data_dir.to_path_buf(),
        embeddings_backend: EmbeddingsBackend::Hash,
        hash_embedding_dim: 64,
        ..StandaloneConfig::default()
    };
    let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));
    let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));
    Arc::new(
        SessionManager::new(
            config,
            registry,
            embedder,
            Some(jobs::new_job_registry()),
            None,
        )
        .await
        .unwrap(),
    )
}

/// Init a git repo with two committed Rust files.
fn init_base_repo(root: &Utf8Path) -> git2::Repository {
    let repo = git2::Repository::init(root.as_std_path()).unwrap();
    std::fs::write(
        root.join("alpha.rs").as_std_path(),
        "pub fn alpha_probe() -> usize { 1 }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("beta.rs").as_std_path(),
        "pub fn beta_probe() -> usize { 2 }\n",
    )
    .unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("alpha.rs")).unwrap();
    index.add_path(std::path::Path::new("beta.rs")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    drop(index);
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("Test", "test@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    // `tree` borrows `repo`, so it has to go before `repo` is moved out.
    drop(tree);
    repo
}

async fn index_to_ready(manager: &Arc<SessionManager>, repo: &Utf8Path) {
    manager.approve_and_start_initial_index(repo).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match manager.resolve_repo(repo).await.unwrap() {
                RepoAccess::Ready(_) => break,
                RepoAccess::Indexing { .. } => tokio::task::yield_now().await,
                _ => panic!("unexpected repo access state while indexing"),
            }
        }
    })
    .await
    .expect("initial index timed out");
}

/// Wait for a repo to finish its (seeded) first pass, returning its state.
async fn wait_for_ready(
    manager: &Arc<SessionManager>,
    repo: &Utf8Path,
) -> Arc<code_intelligence_mcp_server::handlers::AppState> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match manager.resolve_repo(repo).await.unwrap() {
                RepoAccess::Ready(state) => break state,
                RepoAccess::Indexing { .. } => tokio::task::yield_now().await,
                _ => panic!("unexpected repo access state"),
            }
        }
    })
    .await
    .expect("seeded index timed out")
}

#[tokio::test]
async fn worktree_of_indexed_repo_seeds_without_consent_and_indexes_nothing() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let base = utf8(work_temp.path()).join("base");
    std::fs::create_dir_all(base.as_std_path()).unwrap();

    let manager = manager_for(&data_dir).await;
    let repo = init_base_repo(&base);
    index_to_ready(&manager, base.as_path()).await;

    let base_symbols = match manager.resolve_repo(base.as_path()).await.unwrap() {
        RepoAccess::Ready(state) => state.sqlite.count_symbols().unwrap(),
        _ => panic!("base must be ready"),
    };
    assert!(base_symbols > 0);

    // Create the worktree.
    let wt = utf8(work_temp.path()).join("feature");
    // Default options create a branch named after the worktree, which is what
    // `git worktree add <path>` does. Passing None avoids borrowing a temporary
    // WorktreeAddOptions, which does not outlive the call.
    repo.worktree("feature", wt.as_std_path(), None).unwrap();

    // Binding it must NOT ask for consent; it seeds and starts indexing.
    let access = manager.resolve_repo(wt.as_path()).await.unwrap();
    assert!(
        matches!(access, RepoAccess::Indexing { .. }),
        "a seedable worktree must skip the consent prompt"
    );

    let state = wait_for_ready(&manager, wt.as_path()).await;

    // The seeded index carries the base's symbols.
    assert_eq!(state.sqlite.count_symbols().unwrap(), base_symbols);

    // And the seeded pass parsed nothing, because no content differs.
    let run = state.sqlite.latest_index_run().unwrap().unwrap();
    assert_eq!(run.files_indexed, 0, "clean worktree must reparse nothing");
    assert_eq!(run.files_unchanged, 2);

    // The registry records the provenance the prune sweep needs.
    let entry = manager
        .registry
        .get(
            std::fs::canonicalize(wt.as_std_path())
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap()
        .unwrap();
    assert!(entry.seeded_from.is_some());
}

/// Two sessions binding the same fresh worktree must end up sharing one seeded
/// index. The second one waits on the base's init lock, so by the time it runs
/// the first has already seeded; it must recognise that instead of treating its
/// own refusal to re-seed as a failure and deleting the index in use.
#[tokio::test]
async fn two_sessions_binding_the_same_worktree_share_one_seed() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let base = utf8(work_temp.path()).join("base");
    std::fs::create_dir_all(base.as_std_path()).unwrap();

    let manager = manager_for(&data_dir).await;
    let repo = init_base_repo(&base);
    index_to_ready(&manager, base.as_path()).await;

    let wt = utf8(work_temp.path()).join("feature");
    repo.worktree("feature", wt.as_std_path(), None).unwrap();

    let (left, right) = tokio::join!(
        manager.resolve_repo(wt.as_path()),
        manager.resolve_repo(wt.as_path())
    );
    for access in [left.unwrap(), right.unwrap()] {
        assert!(
            matches!(access, RepoAccess::Indexing { .. } | RepoAccess::Ready(_)),
            "neither concurrent bind may fall back to the consent prompt"
        );
    }

    let state = wait_for_ready(&manager, wt.as_path()).await;
    assert!(state.sqlite.count_symbols().unwrap() > 0);
    let run = state.sqlite.latest_index_run().unwrap().unwrap();
    assert_eq!(run.files_indexed, 0, "the seed must not be built twice");

    let wt_key = std::fs::canonicalize(wt.as_std_path())
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let entry = manager
        .registry
        .get(&wt_key)
        .unwrap()
        .expect("the surviving bind must keep its registry entry");
    assert!(entry.seeded_from.is_some());
    assert!(
        entry
            .data_dir
            .join("code-intelligence.db")
            .as_std_path()
            .is_file(),
        "the seeded index must survive the second bind"
    );
}

#[tokio::test]
async fn worktree_with_one_changed_file_indexes_only_that_file() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let base = utf8(work_temp.path()).join("base");
    std::fs::create_dir_all(base.as_std_path()).unwrap();

    let manager = manager_for(&data_dir).await;
    let repo = init_base_repo(&base);
    index_to_ready(&manager, base.as_path()).await;

    let wt = utf8(work_temp.path()).join("feature");
    repo.worktree("feature", wt.as_std_path(), None).unwrap();
    // Diverge one file before the first bind.
    std::fs::write(
        wt.join("beta.rs").as_std_path(),
        "pub fn beta_probe_renamed() -> usize { 99 }\n",
    )
    .unwrap();

    manager.resolve_repo(wt.as_path()).await.unwrap();
    let state = wait_for_ready(&manager, wt.as_path()).await;

    let run = state.sqlite.latest_index_run().unwrap().unwrap();
    assert_eq!(run.files_indexed, 1, "only the diverged file may reparse");
    assert_eq!(run.files_unchanged, 1);

    // The new symbol is searchable and the old one is gone.
    assert!(!state
        .sqlite
        .search_symbols_by_exact_name("beta_probe_renamed", None, 5)
        .unwrap()
        .is_empty());
    assert!(state
        .sqlite
        .search_symbols_by_exact_name("beta_probe", None, 5)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn worktree_of_unindexed_repo_still_asks_for_consent() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let base = utf8(work_temp.path()).join("base");
    std::fs::create_dir_all(base.as_std_path()).unwrap();

    let manager = manager_for(&data_dir).await;
    let repo = init_base_repo(&base);
    // Note: the base is NEVER indexed.

    let wt = utf8(work_temp.path()).join("feature");
    repo.worktree("feature", wt.as_std_path(), None).unwrap();

    assert!(
        matches!(
            manager.resolve_repo(wt.as_path()).await.unwrap(),
            RepoAccess::NeedsApproval
        ),
        "with no indexed base there is nothing to seed from"
    );
    // No half-built data dir was left behind.
    let wt_canonical = std::fs::canonicalize(wt.as_std_path()).unwrap();
    let id = RepoRegistry::path_hash(wt_canonical.to_str().unwrap());
    let data = data_dir.join("repos").join(&id);
    assert!(
        !data.join("code-intelligence.db").as_std_path().exists(),
        "a failed or skipped seed must leave no database"
    );
}

#[tokio::test]
async fn sparse_worktree_prunes_index_rows_for_absent_files() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let base = utf8(work_temp.path()).join("base");
    std::fs::create_dir_all(base.as_std_path()).unwrap();

    let manager = manager_for(&data_dir).await;
    let repo = init_base_repo(&base);
    index_to_ready(&manager, base.as_path()).await;

    let wt = utf8(work_temp.path()).join("feature");
    repo.worktree("feature", wt.as_std_path(), None).unwrap();
    // Stand in for a sparse checkout: one indexed file is simply not present.
    std::fs::remove_file(wt.join("beta.rs").as_std_path()).unwrap();

    manager.resolve_repo(wt.as_path()).await.unwrap();
    let state = wait_for_ready(&manager, wt.as_path()).await;

    let run = state.sqlite.latest_index_run().unwrap().unwrap();
    assert_eq!(
        run.files_deleted, 1,
        "the absent file's rows must be pruned"
    );
    assert!(state
        .sqlite
        .search_symbols_by_exact_name("beta_probe", None, 5)
        .unwrap()
        .is_empty());
    // The file that IS present survived.
    assert!(!state
        .sqlite
        .search_symbols_by_exact_name("alpha_probe", None, 5)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn stale_graph_format_base_falls_back_to_consent() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let base = utf8(work_temp.path()).join("base");
    std::fs::create_dir_all(base.as_std_path()).unwrap();

    let manager = manager_for(&data_dir).await;
    let repo = init_base_repo(&base);
    index_to_ready(&manager, base.as_path()).await;

    // Rewrite the base's graph format version so it no longer matches.
    let base_canonical = std::fs::canonicalize(base.as_std_path()).unwrap();
    let base_entry = manager
        .registry
        .get(base_canonical.to_str().unwrap())
        .unwrap()
        .unwrap();
    {
        let conn = rusqlite::Connection::open(
            base_entry
                .data_dir
                .join("code-intelligence.db")
                .as_std_path(),
        )
        .unwrap();
        conn.execute(
            "UPDATE index_metadata SET value = 'ancient' WHERE key = 'graph_index_version'",
            [],
        )
        .unwrap();
    }

    let wt = utf8(work_temp.path()).join("feature");
    repo.worktree("feature", wt.as_std_path(), None).unwrap();

    assert!(
        matches!(
            manager.resolve_repo(wt.as_path()).await.unwrap(),
            RepoAccess::NeedsApproval
        ),
        "a base on an older graph format is not worth seeding from"
    );

    // Nothing was left half-built.
    let wt_canonical = std::fs::canonicalize(wt.as_std_path()).unwrap();
    let wt_data = data_dir
        .join("repos")
        .join(RepoRegistry::path_hash(wt_canonical.to_str().unwrap()));
    assert!(!wt_data.join("code-intelligence.db").as_std_path().exists());
}

/// A seed that fails partway (here: a base whose Tantivy index will not open,
/// standing in for a clone torn by a concurrent watcher pass) must leave nothing
/// behind. A surviving data dir would make `data_dir_has_index_artifacts` true
/// and block every later seed attempt for this worktree.
#[tokio::test]
async fn failed_seed_leaves_no_partial_data_dir() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let base = utf8(work_temp.path()).join("base");
    std::fs::create_dir_all(base.as_std_path()).unwrap();

    let manager = manager_for(&data_dir).await;
    let repo = init_base_repo(&base);
    index_to_ready(&manager, base.as_path()).await;

    let base_canonical = std::fs::canonicalize(base.as_std_path()).unwrap();
    let base_entry = manager
        .registry
        .get(base_canonical.to_str().unwrap())
        .unwrap()
        .unwrap();
    std::fs::write(
        base_entry
            .data_dir
            .join("tantivy-index/meta.json")
            .as_std_path(),
        b"{ this is not valid tantivy metadata",
    )
    .unwrap();

    let wt = utf8(work_temp.path()).join("feature");
    repo.worktree("feature", wt.as_std_path(), None).unwrap();

    assert!(
        matches!(
            manager.resolve_repo(wt.as_path()).await.unwrap(),
            RepoAccess::NeedsApproval
        ),
        "a seed that fails must fall back to the consent path"
    );

    let wt_canonical = std::fs::canonicalize(wt.as_std_path()).unwrap();
    let wt_key = wt_canonical.to_str().unwrap().to_string();
    let wt_data = data_dir
        .join("repos")
        .join(RepoRegistry::path_hash(&wt_key));
    assert!(
        !wt_data.as_std_path().exists(),
        "a failed seed must remove the data dir it created: {wt_data}"
    );
    assert!(
        manager.registry.get(&wt_key).unwrap().is_none(),
        "a failed seed must drop the registry entry it created"
    );

    // The fallback is a real full index, not a crippled one: approving the
    // worktree parses both files from scratch and they become searchable.
    index_to_ready(&manager, wt.as_path()).await;
    let state = wait_for_ready(&manager, wt.as_path()).await;
    let run = state.sqlite.latest_index_run().unwrap().unwrap();
    assert_eq!(
        run.files_indexed, 2,
        "the fallback must index every file itself"
    );
    assert!(!state
        .sqlite
        .search_symbols_by_exact_name("alpha_probe", None, 5)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn deleting_a_seeded_worktree_prunes_its_index() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let base = utf8(work_temp.path()).join("base");
    std::fs::create_dir_all(base.as_std_path()).unwrap();

    let manager = manager_for(&data_dir).await;
    let repo = init_base_repo(&base);
    index_to_ready(&manager, base.as_path()).await;

    let wt = utf8(work_temp.path()).join("feature");
    repo.worktree("feature", wt.as_std_path(), None).unwrap();
    manager.resolve_repo(wt.as_path()).await.unwrap();
    let _ready = wait_for_ready(&manager, wt.as_path()).await;

    let wt_canonical = std::fs::canonicalize(wt.as_std_path()).unwrap();
    let wt_key = wt_canonical.to_str().unwrap().to_string();
    let wt_data = data_dir
        .join("repos")
        .join(RepoRegistry::path_hash(&wt_key));
    assert!(wt_data.as_std_path().exists());

    // Simulate `git worktree remove`.
    std::fs::remove_dir_all(wt.as_std_path()).unwrap();
    manager.evict_idle_repos().await;

    assert!(manager.registry.get(&wt_key).unwrap().is_none());
    assert!(
        !wt_data.as_std_path().exists(),
        "the seeded data dir must be removed with its registry entry"
    );
    // The base is untouched.
    assert!(manager
        .registry
        .get(
            std::fs::canonicalize(base.as_std_path())
                .unwrap()
                .to_str()
                .unwrap()
        )
        .unwrap()
        .is_some());
}

/// A repo a user registered by hand is never auto-pruned, however stale its
/// path: only entries the daemon seeded carry `seeded_from`.
#[tokio::test]
async fn hand_registered_missing_repo_survives_the_prune_sweep() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let manager = manager_for(&data_dir).await;

    let vanished = utf8(work_temp.path()).join("hand-registered");
    std::fs::create_dir_all(vanished.as_std_path()).unwrap();
    let entry = manager.registry.register(vanished.as_str()).unwrap();
    std::fs::remove_dir_all(vanished.as_std_path()).unwrap();

    manager.evict_idle_repos().await;

    assert!(
        manager.registry.get(vanished.as_str()).unwrap().is_some(),
        "an entry with no seed provenance must never be pruned"
    );
    assert!(entry.data_dir.as_std_path().exists());
}

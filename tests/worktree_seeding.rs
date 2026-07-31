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
    manager_with_ttl(data_dir, StandaloneConfig::default().warm_ttl_seconds).await
}

async fn manager_with_ttl(data_dir: &Utf8Path, warm_ttl_seconds: u64) -> Arc<SessionManager> {
    let config = StandaloneConfig {
        data_dir: data_dir.to_path_buf(),
        embeddings_backend: EmbeddingsBackend::Hash,
        hash_embedding_dim: 64,
        warm_ttl_seconds,
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

    // One sweep only arms the deletion, so a checkout that is briefly
    // unreachable survives.
    manager.evict_idle_repos().await;
    assert!(
        manager.registry.get(&wt_key).unwrap().is_some(),
        "a single absent sweep must not delete an index"
    );
    assert!(wt_data.as_std_path().exists());

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

    for _ in 0..3 {
        manager.evict_idle_repos().await;
    }

    assert!(
        manager.registry.get(vanished.as_str()).unwrap().is_some(),
        "an entry with no seed provenance must never be pruned"
    );
    assert!(entry.data_dir.as_std_path().exists());
}

/// Register a repo and stamp it as seeded, without paying for a real seed: the
/// prune sweep only reads `seeded_from`, the path, and the running jobs.
fn register_seeded(manager: &Arc<SessionManager>, path: &Utf8Path) -> Utf8PathBuf {
    std::fs::create_dir_all(path.as_std_path()).unwrap();
    manager.registry.register(path.as_str()).unwrap();
    manager
        .registry
        .mark_seeded_from(path.as_str(), "basehash00000000")
        .unwrap()
        .data_dir
}

/// A job running against a seeded index holds live SQLite and Tantivy handles,
/// and would rebuild a partial data dir right after the removal. That leftover
/// would then block this worktree from ever being seeded again.
#[tokio::test]
async fn a_running_job_blocks_the_prune_sweep() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let manager = manager_for(&data_dir).await;

    let seeded = utf8(work_temp.path()).join("feature");
    let seeded_data = register_seeded(&manager, seeded.as_path());
    std::fs::remove_dir_all(seeded.as_std_path()).unwrap();

    let job_registry = manager.job_registry();
    jobs::register_running(
        &job_registry,
        "delta-pass".to_string(),
        jobs::JobKind::InitialBind,
        RepoRegistry::path_hash(seeded.as_str()),
        seeded.as_str().to_string(),
    );

    for _ in 0..3 {
        manager.evict_idle_repos().await;
    }
    assert!(
        manager.registry.get(seeded.as_str()).unwrap().is_some(),
        "an index with a running job must not be deleted, however absent its checkout"
    );
    assert!(seeded_data.as_std_path().exists());

    // Once the job is done the sweep may collect it, two sightings as usual.
    jobs::mark_succeeded(&job_registry, "delta-pass", serde_json::Value::Null);
    manager.evict_idle_repos().await;
    assert!(manager.registry.get(seeded.as_str()).unwrap().is_some());
    manager.evict_idle_repos().await;

    assert!(manager.registry.get(seeded.as_str()).unwrap().is_none());
    assert!(!seeded_data.as_std_path().exists());
}

/// A checkout that comes back disarms the pending deletion, so a volume that
/// unmounts and remounts repeatedly is never collected.
#[tokio::test]
async fn a_reappearing_checkout_clears_the_pending_prune() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let manager = manager_for(&data_dir).await;

    let seeded = utf8(work_temp.path()).join("feature");
    let seeded_data = register_seeded(&manager, seeded.as_path());

    // Absent once: armed.
    std::fs::remove_dir_all(seeded.as_std_path()).unwrap();
    manager.evict_idle_repos().await;
    assert!(manager.registry.get(seeded.as_str()).unwrap().is_some());

    // Back again: disarmed.
    std::fs::create_dir_all(seeded.as_std_path()).unwrap();
    manager.evict_idle_repos().await;
    assert!(manager.registry.get(seeded.as_str()).unwrap().is_some());

    // Absent again: this is a first sighting once more, so it must survive.
    std::fs::remove_dir_all(seeded.as_std_path()).unwrap();
    manager.evict_idle_repos().await;
    assert!(
        manager.registry.get(seeded.as_str()).unwrap().is_some(),
        "a checkout that came back must reset the count of absent sweeps"
    );
    assert!(seeded_data.as_std_path().exists());

    manager.evict_idle_repos().await;
    assert!(manager.registry.get(seeded.as_str()).unwrap().is_none());
    assert!(!seeded_data.as_std_path().exists());
}

/// `warm_ttl_seconds = 0` means never evict, and a deployment that never evicts
/// a warm repo must never delete one of its indexes either.
#[tokio::test]
async fn zero_ttl_never_prunes_a_seeded_index() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let manager = manager_with_ttl(&data_dir, 0).await;

    let seeded = utf8(work_temp.path()).join("feature");
    let seeded_data = register_seeded(&manager, seeded.as_path());
    std::fs::remove_dir_all(seeded.as_std_path()).unwrap();

    for _ in 0..3 {
        manager.evict_idle_repos().await;
    }

    assert!(
        manager.registry.get(seeded.as_str()).unwrap().is_some(),
        "a zero TTL must disable the prune sweep entirely"
    );
    assert!(seeded_data.as_std_path().exists());
}

/// Seeding an entry the user registered by hand must not enroll it for pruning:
/// `seeded_from` is what makes an index one the daemon may delete on its own.
#[tokio::test]
async fn seeding_a_hand_registered_worktree_leaves_it_out_of_the_prune_sweep() {
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
    let wt_key = std::fs::canonicalize(wt.as_std_path())
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Stand in for a repo added through the JSON API and never approved.
    manager.registry.register(&wt_key).unwrap();

    // It still seeds: this is the second hook point, on an unapproved entry.
    assert!(
        matches!(
            manager.resolve_repo(wt.as_path()).await.unwrap(),
            RepoAccess::Indexing { .. }
        ),
        "a registered but unapproved worktree must still be seedable"
    );
    let state = wait_for_ready(&manager, wt.as_path()).await;
    assert_eq!(
        state
            .sqlite
            .latest_index_run()
            .unwrap()
            .unwrap()
            .files_indexed,
        0,
        "the seed must still have spared the first pass its work"
    );

    let entry = manager.registry.get(&wt_key).unwrap().unwrap();
    assert!(
        entry.seeded_from.is_none(),
        "an entry the seed did not create must not be stamped as seeded"
    );

    std::fs::remove_dir_all(wt.as_std_path()).unwrap();
    for _ in 0..3 {
        manager.evict_idle_repos().await;
    }
    assert!(
        manager.registry.get(&wt_key).unwrap().is_some(),
        "the user's entry must survive the sweep"
    );
    assert!(entry.data_dir.as_std_path().exists());
}

/// A cloned store that will not open must not fail the bind either.
///
/// `validate_seeded_index` deliberately does not open the cloned `vectors/`
/// (LanceDB's open is async and that validation is synchronous), and the
/// clone-window guard's generation tuple does not cover the base's vector stage.
/// So a Lance dataset copied mid-write first surfaces where `init_repo_state`
/// opens it, by which point the seed has registered the entry, stamped its
/// provenance and approved it. Propagating that error would strand the worktree
/// for good: `index_runs` was cleared, so `has_persisted_index_run` stays false
/// while `initial_index_approved_at` is set, and every later bind would walk back
/// into the same failing open with nothing left to re-run it.
#[tokio::test]
async fn a_seeded_index_that_will_not_open_falls_back_instead_of_failing_the_bind() {
    let data_temp = tempfile::tempdir().unwrap();
    let work_temp = tempfile::tempdir().unwrap();
    let data_dir = utf8(data_temp.path());
    let base = utf8(work_temp.path()).join("base");
    std::fs::create_dir_all(base.as_std_path()).unwrap();

    let manager = manager_for(&data_dir).await;
    let repo = init_base_repo(&base);
    index_to_ready(&manager, base.as_path()).await;

    // Corrupt the base's Lance manifests, standing in for a dataset cloned
    // mid-write. The seed itself still succeeds: it validates SQLite and Tantivy,
    // and only checks that `vectors/` came across at all.
    let base_canonical = std::fs::canonicalize(base.as_std_path()).unwrap();
    let base_entry = manager
        .registry
        .get(base_canonical.to_str().unwrap())
        .unwrap()
        .unwrap();
    let versions = base_entry.data_dir.join("vectors/symbols.lance/_versions");
    assert!(versions.as_std_path().is_dir());
    std::fs::remove_dir_all(versions.as_std_path()).unwrap();

    let wt = utf8(work_temp.path()).join("feature");
    repo.worktree("feature", wt.as_std_path(), None).unwrap();

    assert!(
        matches!(
            manager
                .resolve_repo(wt.as_path())
                .await
                .expect("a seed whose stores will not open must not fail the bind"),
            RepoAccess::NeedsApproval
        ),
        "the bind must fall through to the consent path"
    );

    let wt_key = std::fs::canonicalize(wt.as_std_path())
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let wt_data = data_dir
        .join("repos")
        .join(RepoRegistry::path_hash(&wt_key));
    assert!(
        !wt_data.as_std_path().exists(),
        "the unusable seed must be removed: {wt_data}"
    );
    assert!(
        manager.registry.get(&wt_key).unwrap().is_none(),
        "the unusable seed must not leave an approved registry entry behind"
    );

    // And the clean slate is real: approving the worktree indexes it from
    // scratch, which is the recovery the propagated error used to make impossible.
    index_to_ready(&manager, wt.as_path()).await;
    let state = wait_for_ready(&manager, wt.as_path()).await;
    assert_eq!(
        state
            .sqlite
            .latest_index_run()
            .unwrap()
            .unwrap()
            .files_indexed,
        2,
        "the fallback must index every file itself"
    );
}

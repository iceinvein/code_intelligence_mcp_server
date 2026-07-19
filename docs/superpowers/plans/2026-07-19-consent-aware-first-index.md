# Consent-Aware First Index Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Require one explicit user approval before a repository's first full index, start that index immediately as a background job, and keep later watcher and manual reindexes automatic.

**Architecture:** Persist first-index approval and completion timestamps in `RepoEntry`, then centralize lifecycle resolution in a repository-scoped coordinator owned by `SessionManager`. MCP and HTTP entry points consume the coordinator's typed result, while model-facing payloads explain when to ask the user, when indexing is running, and when normal tool dispatch is safe.

**Tech Stack:** Rust 2021, Tokio, DashMap, SQLite, Tantivy, LanceDB, rust-mcp-sdk, Axum, Serde, React/TypeScript, Bun.

## Global Constraints

- Use `Utf8PathBuf` and `&Utf8Path` for repository paths. Do not introduce `PathBuf` into public repository lifecycle interfaces.
- Default behavior with `INDEX_CONSENT_REQUIRED=true` must ask once before the first full index for explicit and implicit bindings.
- Preserve `INDEX_CONSENT_REQUIRED=false` as the CI and benchmark opt-out. It must still start a real first-index job immediately.
- Treat a persisted successful index run as authoritative legacy completion, including for empty repositories.
- Optional external producer failure remains non-fatal after native indexing succeeds.
- Do not start a watcher until the first native full index completes successfully.
- Do not ask again after approval, including after an indexing failure or daemon restart.
- Do not add AI attribution to commits, pull requests, documentation, or source comments.
- Do not use em dashes in prose, documentation, comments, commit messages, or UI copy.
- Follow test-driven development for every behavior change.

---

## File Structure

- Modify `src/registry.rs`: persist authorization and completion timestamps and expose atomic lifecycle mutations.
- Modify `src/session.rs`: store repository runtimes with idempotent watcher startup and expose the lifecycle coordinator module.
- Create `src/session/initial_index.rs`: derive readiness, serialize startup, launch the first-index job, and return typed access outcomes.
- Modify `src/server/jobs.rs`: locate running jobs by repository and kind.
- Modify `src/indexer/pipeline/mod.rs`: add the `InitialBind` external-index trigger and ensure watcher jobs are always `WatchReindex`.
- Modify `src/server/consent.rs`: build model-facing consent and progress payloads.
- Modify `src/server/standalone.rs`: route every MCP binding through the coordinator and remove explicit-binding bypasses.
- Modify `src/server/mod.rs`: add the chat-permission rule to server instructions.
- Modify `src/server/api/consent.rs`: use the shared coordinator for dashboard approval and decline.
- Modify `src/server/api/repos.rs`: register pending repositories and prevent manual reindex from bypassing first-index consent.
- Modify `src/server/api/query.rs`: prevent dashboard queries from opening partial or unapproved repository state.
- Modify `src/tools/mod.rs`: clarify `approve_indexing` user-confirmation requirements.
- Modify `ui/src/api/consent.ts`: include first-index job metadata in consent responses.
- Modify `README.md`, `npm/README.md`, `AGENTS.md`, and `CLAUDE.md`: document the corrected first-index lifecycle.

---

### Task 1: Persist first-index authorization and completion

**Files:**
- Modify: `src/registry.rs:77-257`
- Test: `src/registry.rs:325-570`
- Modify: `src/server/api/consent.rs:159-207`
- Modify: `src/server/api/repos.rs:409-490`

**Interfaces:**
- Produces: `RepoEntry.initial_index_approved_at: Option<String>`.
- Produces: `RepoEntry.initial_index_completed_at: Option<String>`.
- Produces: `RepoRegistry::approve_initial_index(&self, repo_path: &str) -> Result<RepoEntry>`.
- Produces: `RepoRegistry::mark_initial_index_completed(&self, repo_path: &str) -> Result<RepoEntry>`.
- Preserves: `RepoRegistry::register` creates storage identity but does not authorize indexing.

- [ ] **Step 1: Write failing registry lifecycle tests**

Add these focused tests to `src/registry.rs`:

```rust
#[test]
fn register_does_not_authorize_or_complete_first_index() {
    let dir = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    let reg = RepoRegistry::new(root.join("registry.json"), root.join("repos"));

    let entry = reg.register("/Users/dev/project").unwrap();

    assert!(entry.initial_index_approved_at.is_none());
    assert!(entry.initial_index_completed_at.is_none());
}

#[test]
fn approval_and_completion_survive_registry_reload() {
    let dir = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    let registry_path = root.join("registry.json");
    let repos_dir = root.join("repos");
    let reg = RepoRegistry::new(registry_path.clone(), repos_dir.clone());

    let approved = reg.approve_initial_index("/Users/dev/project").unwrap();
    assert!(approved.initial_index_approved_at.is_some());
    assert!(approved.initial_index_completed_at.is_none());

    reg.mark_initial_index_completed("/Users/dev/project")
        .unwrap();
    let reloaded = RepoRegistry::new(registry_path, repos_dir)
        .get("/Users/dev/project")
        .unwrap()
        .unwrap();
    assert!(reloaded.initial_index_approved_at.is_some());
    assert!(reloaded.initial_index_completed_at.is_some());
}

#[test]
fn legacy_entry_defaults_first_index_timestamps_to_none() {
    let json = r#"{
      "path":"/repo",
      "name":"repo",
      "data_dir":"/data/repo",
      "created_at":"2026-01-01T00:00:00Z",
      "last_accessed":"2026-01-01T00:00:00Z",
      "consent":"approved"
    }"#;
    let entry: RepoEntry = serde_json::from_str(json).unwrap();
    assert!(entry.initial_index_approved_at.is_none());
    assert!(entry.initial_index_completed_at.is_none());
}
```

- [ ] **Step 2: Run the new tests and verify RED**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features registry::tests::register_does_not_authorize_or_complete_first_index -- --nocapture
```

Expected: compilation fails because the timestamp fields and lifecycle methods do not exist.

- [ ] **Step 3: Add lifecycle fields and atomic registry mutations**

Extend `RepoEntry` with Serde-defaulted fields:

```rust
#[serde(default)]
pub initial_index_approved_at: Option<String>,
#[serde(default)]
pub initial_index_completed_at: Option<String>,
```

Every `RepoEntry` constructor must initialize both fields to `None`. Change `register` so re-registering an entry only updates `last_accessed`; it must not change `consent` or either lifecycle timestamp.

Add these methods using the existing load, mutate, and atomic save pattern:

```rust
pub fn approve_initial_index(&self, repo_path: &str) -> Result<RepoEntry> {
    let _ = self.register(repo_path)?;
    let hash = Self::path_hash(repo_path);
    let mut registry = self.load()?;
    let entry = registry
        .repos
        .get_mut(&hash)
        .with_context(|| format!("Repository disappeared during approval: {repo_path}"))?;
    entry.consent = IndexConsent::Approved;
    if entry.initial_index_approved_at.is_none() {
        entry.initial_index_approved_at = Some(chrono::Utc::now().to_rfc3339());
    }
    let approved = entry.clone();
    self.save(&registry)?;
    Ok(approved)
}

pub fn mark_initial_index_completed(&self, repo_path: &str) -> Result<RepoEntry> {
    let hash = Self::path_hash(repo_path);
    let mut registry = self.load()?;
    let entry = registry
        .repos
        .get_mut(&hash)
        .with_context(|| format!("Cannot complete unregistered repository: {repo_path}"))?;
    entry.initial_index_completed_at = Some(chrono::Utc::now().to_rfc3339());
    let completed = entry.clone();
    self.save(&registry)?;
    Ok(completed)
}
```

When `set_consent(..., Declined)` updates an incomplete entry, clear `initial_index_approved_at`. Do not clear a completed timestamp.

- [ ] **Step 4: Update all explicit `RepoEntry` test fixtures**

Add these fields to the literals in `src/server/api/consent.rs` and `src/server/api/repos.rs`:

```rust
initial_index_approved_at: None,
initial_index_completed_at: None,
```

Replace `register_flips_declined_entry_back_to_approved` with:

```rust
#[test]
fn registration_preserves_decline_until_explicit_approval() {
    let dir = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
    let reg = RepoRegistry::new(root.join("registry.json"), root.join("repos"));
    reg.set_consent("/Users/dev/project", IndexConsent::Declined)
        .unwrap();

    let registered = reg.register("/Users/dev/project").unwrap();
    assert_eq!(registered.consent, IndexConsent::Declined);
    assert!(registered.initial_index_approved_at.is_none());

    let approved = reg.approve_initial_index("/Users/dev/project").unwrap();
    assert_eq!(approved.consent, IndexConsent::Approved);
    assert!(approved.initial_index_approved_at.is_some());
}
```

- [ ] **Step 5: Run registry and affected API unit tests and verify GREEN**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features registry::tests -- --nocapture
EMBEDDINGS_BACKEND=hash cargo test --no-default-features server::api::consent::tests -- --nocapture
EMBEDDINGS_BACKEND=hash cargo test --no-default-features server::api::repos::tests -- --nocapture
```

Expected: all selected tests pass.

- [ ] **Step 6: Commit Task 1**

```bash
git add src/registry.rs src/server/api/consent.rs src/server/api/repos.rs
git commit -m "feat: persist first index lifecycle"
```

---

### Task 2: Add job, external-index, and watcher lifecycle primitives

**Files:**
- Modify: `src/server/jobs.rs:190-196`
- Test: `src/server/jobs.rs:591-650`
- Modify: `src/indexer/pipeline/mod.rs:66-76,275-285,666-806,1497-1680`

**Interfaces:**
- Produces: `jobs::most_recent_running_for_repo_kind(registry, repo_id, kind) -> Option<Job>`.
- Produces: `ExternalIndexTrigger::InitialBind`.
- Guarantees: `InitialBind` follows explicit external-index policy.
- Guarantees: watcher-originated runs are always labeled `WatchReindex`.

- [ ] **Step 1: Write failing job-kind and trigger-policy tests**

Add to `src/server/jobs.rs`:

```rust
#[test]
fn running_job_lookup_filters_by_kind() {
    let reg = new_job_registry();
    register_running(
        &reg,
        "manual".to_string(),
        JobKind::ManualReindex,
        "repo".to_string(),
        "/repo".to_string(),
    );
    register_running(
        &reg,
        "initial".to_string(),
        JobKind::InitialBind,
        "repo".to_string(),
        "/repo".to_string(),
    );

    let found = most_recent_running_for_repo_kind(&reg, "repo", JobKind::InitialBind)
        .unwrap();
    assert_eq!(found.id, "initial");
}
```

Extend the external-index policy tests in `src/indexer/pipeline/mod.rs` so `InitialBind` is false for disabled policy and true for both `explicit` and `watch` policies.

Add this assertion to `external_index_refresh_policy_is_opt_in`:

```rust
assert!(!IndexPipeline::should_run_external_index(
    &cfg,
    ExternalIndexTrigger::InitialBind,
));
```

Add this assertion to `external_index_watch_policy_runs_for_manual_and_watch_refresh`:

```rust
assert!(IndexPipeline::should_run_external_index(
    &cfg,
    ExternalIndexTrigger::InitialBind,
));
```

Add this explicit-policy test:

```rust
#[test]
fn external_index_explicit_policy_runs_for_initial_bind() {
    let standalone = StandaloneConfig {
        external_index_auto: true,
        external_index_on_refresh: "explicit".to_string(),
        ..StandaloneConfig::default()
    };
    let config = standalone.repo_config(
        Utf8PathBuf::from("/tmp/repo"),
        &Utf8PathBuf::from("/tmp/data"),
    );

    assert!(IndexPipeline::should_run_external_index(
        &config,
        ExternalIndexTrigger::InitialBind,
    ));
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features running_job_lookup_filters_by_kind -- --nocapture
EMBEDDINGS_BACKEND=hash cargo test --no-default-features external_index_explicit_policy_runs_for_initial_bind -- --nocapture
```

Expected: compilation fails because the helper and trigger variant do not exist.

- [ ] **Step 3: Implement the typed running-job lookup**

Add to `src/server/jobs.rs`:

```rust
pub fn most_recent_running_for_repo_kind(
    registry: &JobRegistry,
    repo_id: &str,
    kind: JobKind,
) -> Option<Job> {
    registry
        .iter()
        .filter(|entry| {
            let job = entry.value();
            job.repo_id == repo_id && job.kind == kind && job.status == JobStatus::Running
        })
        .map(|entry| entry.value().clone())
        .max_by_key(|job| job.started_at_unix_s)
}
```

- [ ] **Step 4: Add `InitialBind` to external-index policy**

Change the trigger enum and policy to:

```rust
pub enum ExternalIndexTrigger {
    InitialBind,
    ManualRefresh,
    Watch,
}

match config.external_index_on_refresh.as_str() {
    "explicit" => matches!(
        trigger,
        ExternalIndexTrigger::InitialBind | ExternalIndexTrigger::ManualRefresh
    ),
    "watch" => true,
    _ => false,
}
```

Keep cooldown behavior restricted to `Watch`.

- [ ] **Step 5: Make watcher jobs unambiguously incremental**

Remove `first_run` from `spawn_watch_loop`. Every filesystem-triggered run must register:

```rust
let job_id = pipeline.register_watch_job(
    crate::server::jobs::JobKind::WatchReindex,
    &repo_path,
    coalesced.saturating_sub(1),
);
```

The new coordinator owns the only `InitialBind` job.

- [ ] **Step 6: Run focused tests and verify GREEN**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features server::jobs::tests -- --nocapture
EMBEDDINGS_BACKEND=hash cargo test --no-default-features indexer::pipeline::tests::external_index -- --nocapture
```

Expected: all selected tests pass.

- [ ] **Step 7: Commit Task 2**

```bash
git add src/server/jobs.rs src/indexer/pipeline/mod.rs
git commit -m "feat: add initial index job primitives"
```

---

### Task 3: Build the first-index coordinator and defer watcher startup

**Files:**
- Create: `src/session/initial_index.rs`
- Modify: `src/session.rs:1-410`
- Test: `src/session.rs:600-970`

**Interfaces:**
- Produces: `RepoAccess::Ready(Arc<AppState>)`.
- Produces: `RepoAccess::NeedsApproval`.
- Produces: `RepoAccess::Indexing { job: Job, started: bool }`.
- Produces: `RepoAccess::Declined`.
- Produces: `SessionManager::resolve_repo(self: &Arc<Self>, repo_path: &Utf8Path) -> Result<RepoAccess>`.
- Produces: `SessionManager::approve_and_start_initial_index(self: &Arc<Self>, repo_path: &Utf8Path) -> Result<RepoAccess>`.
- Produces: `SessionManager::decline_initial_index(&self, repo_path: &Utf8Path) -> Result<()>`.
- Produces: `SessionManager::job_registry(&self) -> JobRegistry` for HTTP state sharing and job polling.
- Produces for unit tests: `SessionManager::loaded_repo_count(&self) -> usize` behind `#[cfg(test)]`.

- [ ] **Step 1: Write failing coordinator tests**

Add this bounded job helper to the `src/session.rs` test module:

```rust
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
```

Add these behaviors to `src/session.rs`:

```rust
#[tokio::test]
async fn unindexed_repo_requires_approval_even_when_explicitly_selected() {
    let (_data, data_dir) = temp_data_dir();
    let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
    let (_repo, repo_path) = temp_repo_dir();

    let access = manager.resolve_repo(repo_path.as_path()).await.unwrap();
    assert!(matches!(access, RepoAccess::NeedsApproval));
    assert!(!manager.repos.contains_key(&canonical_key(&repo_path)));
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
```

Add the legacy, empty-repository, restart, decline, and watcher tests:

```rust
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
    assert!(matches!(
        access,
        RepoAccess::Indexing { started: true, .. }
    ));
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
    assert!(!manager.repos.contains_key(canonical.as_str()));
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
```

- [ ] **Step 2: Run one coordinator test and verify RED**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features session::tests::unindexed_repo_requires_approval_even_when_explicitly_selected -- --nocapture
```

Expected: compilation fails because `RepoAccess` and `resolve_repo` do not exist.

- [ ] **Step 3: Replace tuple cache entries with an idempotent runtime wrapper**

Add this private runtime to `src/session.rs`:

```rust
struct RepoRuntime {
    state: Arc<AppState>,
    watch_cancel: CancellationToken,
    watcher_started: std::sync::atomic::AtomicBool,
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
```

Change the cache to `DashMap<String, Arc<RepoRuntime>>`. Refactor initialization into `get_or_create_runtime`, which opens stores but does not start the watcher. Update eviction and deletion to cancel `runtime.watch_cancel`.

Add a second repository-keyed lock map to `SessionManager`:

```rust
initial_index_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
```

Keep this separate from the existing `init_locks`. `init_locks` protects runtime construction, while `initial_index_locks` protects first-index readiness checks and job registration. Reusing the same lock for both paths would deadlock when the coordinator calls `get_or_create_runtime`. Initialize the new map in the constructor and remove its entry when a repository is deleted.

Make `SessionManager.job_registry` a non-optional `JobRegistry`, initialized with `jobs::new_job_registry()` when the constructor receives `None`. Pass `Some(self.job_registry.clone())` into `IndexPipeline::new_with_jobs`.

Expose a clone of the shared handle:

```rust
pub fn job_registry(&self) -> JobRegistry {
    self.job_registry.clone()
}

#[cfg(test)]
pub(crate) fn loaded_repo_count(&self) -> usize {
    self.repos.len()
}
```

- [ ] **Step 4: Implement typed lifecycle derivation**

Create `src/session/initial_index.rs`, export it from `src/session.rs`, and define:

```rust
pub enum RepoAccess {
    Ready(Arc<AppState>),
    NeedsApproval,
    Indexing { job: crate::server::jobs::Job, started: bool },
    Declined,
}
```

Implement `has_persisted_index_run` by checking whether `entry.data_dir.join("code-intelligence.db")` exists before opening it. If it exists, initialize the schema and use `SqliteStore::latest_index_run()?.is_some()`.

`resolve_repo` must apply this order:

```rust
if entry.consent == IndexConsent::Declined {
    return Ok(RepoAccess::Declined);
}
if entry.initial_index_completed_at.is_some() || self.has_persisted_index_run(&entry)? {
    self.registry.mark_initial_index_completed(canonical.as_str())?;
    let runtime = self.get_or_create_runtime(&canonical).await?;
    runtime.ensure_watcher_started();
    return Ok(RepoAccess::Ready(runtime.state.clone()));
}
if entry.initial_index_approved_at.is_none() {
    self.record_pending(canonical.as_path());
    return Ok(RepoAccess::NeedsApproval);
}
self.start_or_get_initial_index(&canonical).await
```

For an unregistered path, record it as pending and return `NeedsApproval`. If `index_consent_required` is false, call `approve_initial_index` and continue to `start_or_get_initial_index` instead.

- [ ] **Step 5: Implement deduplicated background startup**

Use `initial_index_locks` to serialize the final readiness and running-job checks. Look up only `JobKind::InitialBind` with `most_recent_running_for_repo_kind`. While holding this coordinator lock, re-read the registry and persisted-run state, return ready if another caller completed the index, or return the existing running job if another caller registered one. Only then call `get_or_create_runtime`, register the new job, and spawn it. The runtime call uses `init_locks`, so the two lock domains remain non-reentrant.

Register a unique job ID using the repository hash and UNIX nanoseconds, create the runtime without a watcher, and spawn the index:

```rust
let outcome = runtime
    .state
    .indexer
    .index_all_with_external_index(ExternalIndexTrigger::InitialBind)
    .await;

match outcome {
    Ok(outcome) => {
        if let Err(error) = registry.mark_initial_index_completed(repo_path.as_str()) {
            jobs::mark_failed(&jobs, &job_id, error.to_string());
            return;
        }
        runtime.ensure_watcher_started();
        jobs::mark_succeeded(
            &jobs,
            &job_id,
            serde_json::json!({
                "stats": outcome.stats,
                "external_index": outcome.external_index,
            }),
        );
    }
    Err(error) => jobs::mark_failed(&jobs, &job_id, error.to_string()),
}
```

Add the same watchdog pattern used by manual reindex so panic or cancellation cannot leave the job running forever.

- [ ] **Step 6: Implement approval, decline, retry, and legacy backfill**

`approve_and_start_initial_index` canonicalizes the path, persists `approve_initial_index`, clears pending consent, and delegates to `start_or_get_initial_index`.

`decline_initial_index` canonicalizes the path, writes `IndexConsent::Declined`, clears pending consent, and never creates runtime state.

If approval exists but the last `InitialBind` job failed, `resolve_repo` starts one replacement job. If a successful index run exists but the completion timestamp is absent, it backfills the timestamp and returns ready.

- [ ] **Step 7: Run coordinator tests and verify GREEN**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features session::tests -- --nocapture
```

Expected: all session tests pass, including real hash-backed indexing without a watcher event.

- [ ] **Step 8: Commit Task 3**

```bash
git add src/session.rs src/session/initial_index.rs
git commit -m "feat: coordinate consent-aware first indexing"
```

---

### Task 4: Gate MCP tools and teach the model the chat workflow

**Files:**
- Modify: `src/server/consent.rs:1-80`
- Modify: `src/server/mod.rs:28-59`
- Modify: `src/server/standalone.rs:118-801`
- Modify: `src/tools/mod.rs:190-220`
- Test: `src/server/consent.rs:48-90`
- Test: `src/server/standalone.rs:800-1125`

**Interfaces:**
- Consumes: `RepoAccess` from Task 3.
- Produces: `indexing_payload(job: &Job, started: bool) -> Value`.
- Produces: model instructions that require an explicit chat confirmation.
- Guarantees: `?repo=`, roots, `bind_workspace`, and fallback all use the same lifecycle gate.

- [ ] **Step 1: Write failing model-contract tests**

Update `src/server/consent.rs` tests:

```rust
#[test]
fn consent_payload_requires_chat_confirmation_before_approval() {
    let value = consent_required_payload("/Users/me/project", "deadbeefdeadbeef");
    let action = value["action"].as_str().unwrap();
    assert!(action.contains("Tell the user in chat"));
    assert!(action.contains("wait for explicit approval"));
    assert!(action.contains("approve_indexing"));
    assert!(value["message"].as_str().unwrap().contains("first full index"));
}

#[test]
fn indexing_payload_distinguishes_started_from_in_progress() {
    let job = crate::server::jobs::Job {
        id: "initial-repo-1".to_string(),
        kind: crate::server::jobs::JobKind::InitialBind,
        repo_id: "repo".to_string(),
        repo_path: "/repo".to_string(),
        status: crate::server::jobs::JobStatus::Running,
        started_at_unix_s: 1,
        finished_at_unix_s: None,
        duration_ms: None,
        stats: None,
        error: None,
        coalesced_count: 0,
    };
    assert_eq!(indexing_payload(&job, true)["status"], "indexing_started");
    assert_eq!(
        indexing_payload(&job, false)["status"],
        "indexing_in_progress"
    );
}
```

Add a `server_instructions` assertion that the instructions contain both `consent_required` and `wait for explicit user approval`.

```rust
#[test]
fn server_instructions_require_explicit_chat_approval() {
    let instructions = server_instructions();
    assert!(instructions.contains("consent_required"));
    assert!(instructions.contains("wait for explicit user approval"));
    assert!(instructions.contains("approve_indexing"));
}
```

- [ ] **Step 2: Run payload tests and verify RED**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features server::consent::tests -- --nocapture
```

Expected: the existing payload text assertion fails and `indexing_payload` is undefined.

- [ ] **Step 3: Implement lifecycle payloads and instructions**

Use this exact permission language in `consent_required_payload`:

```text
Tell the user in chat that this repository needs its first full index and that indexing uses local compute, memory, and disk. Ask for permission and wait for explicit approval. Only then call approve_indexing with decision "approve". If the user declines, call it with decision "decline".
```

Add `indexing_payload` that returns `status`, `repo`, `repo_id`, `job_id`, `message`, and an action telling the model to inform the user and retry later. Add the same permission rule to `server_instructions` and the `ApproveIndexingTool` documentation.

- [ ] **Step 4: Remove binding-specific consent bypass and prewarming**

Delete `GateDecision`, `consent_decision`, and `may_auto_index`. Remove background `get_or_create_repo` prewarming from `try_url_query_binding`, roots binding, and `handle_bind_workspace`.

Replace `Resolved::Consent` with a general blocked payload variant:

```rust
pub(crate) enum Resolved {
    Ready(Arc<AppState>),
    Blocked(serde_json::Value),
}
```

After resolving the bound path, call `session_manager.resolve_repo(repo_path.as_path())` and map:

```rust
match access {
    RepoAccess::Ready(state) => Resolved::Ready(state),
    RepoAccess::NeedsApproval => Resolved::Blocked(consent_required_payload(path, repo_id)),
    RepoAccess::Declined => Resolved::Blocked(declined_payload(path, repo_id)),
    RepoAccess::Indexing { job, started } => {
        Resolved::Blocked(indexing_payload(&job, started))
    }
}
```

`bind_workspace` must record the binding and return this lifecycle payload immediately. A ready repository still returns `{ "ok": true, ... }`.

Extract the session-independent body as:

```rust
async fn bind_workspace_path(
    &self,
    session_id: &SessionId,
    repo_path: Utf8PathBuf,
) -> Result<serde_json::Value, CallToolError>
```

`handle_bind_workspace` retains runtime and path validation, then calls this helper.

- [ ] **Step 5: Route `approve_indexing` through the coordinator**

For `approve`, call `Arc<SessionManager>::approve_and_start_initial_index` and map `RepoAccess` to ready or indexing JSON. For `decline`, call `decline_initial_index`. Preserve absolute-path and directory validation in `handle_approve_indexing`.

Task-augmented tool calls must continue returning a `CallToolError` containing the structured blocked payload because their SDK return type cannot carry a normal tool result.

- [ ] **Step 6: Update standalone tests**

Delete the old explicit-bypass decision matrix and add:

First extract the body of the existing `test_handler` helper into `test_handler_in(data_dir: Utf8PathBuf)`. Keep `test_handler` for tests that do not touch persistent state. Tests that start an index must create a `TempDir`, pass its UTF-8 path to `test_handler_in`, and retain the `TempDir` until the job has been observed. This prevents the registry directory from disappearing before the background task writes to it.

```rust
#[tokio::test]
async fn url_binding_records_path_without_initializing_repo() {
    let handler = test_handler();
    let session_id = "explicit-url".to_string();
    let repo = Utf8PathBuf::from("/Users/dev/url-project");
    handler.upsert_session(&session_id, None);
    handler
        .pending_repos
        .insert(session_id.clone(), repo.clone());

    assert_eq!(
        handler.try_url_query_binding(&session_id).as_deref(),
        Some(repo.as_path())
    );
    assert_eq!(handler.session_manager.loaded_repo_count(), 0);
}

#[tokio::test]
async fn bind_workspace_returns_consent_required_for_new_repo() {
    let handler = test_handler();
    let session_id = "bind-new".to_string();
    let repo = tempfile::tempdir().unwrap();
    let repo_path = Utf8PathBuf::from_path_buf(repo.path().to_path_buf()).unwrap();
    handler.upsert_session(&session_id, None);

    let value = handler
        .bind_workspace_path(&session_id, repo_path)
        .await
        .unwrap();

    assert_eq!(value["status"], "consent_required");
    assert_eq!(handler.session_manager.loaded_repo_count(), 0);
}

#[tokio::test]
async fn approve_indexing_starts_real_job_and_returns_id() {
    let data = tempfile::tempdir().unwrap();
    let data_dir = Utf8PathBuf::from_path_buf(data.path().to_path_buf()).unwrap();
    let handler = test_handler_in(data_dir);
    let repo = tempfile::tempdir().unwrap();
    let repo_path = repo.path().to_str().unwrap();

    let value = handler
        .approve_indexing_decision(repo_path, "approve")
        .await
        .unwrap();

    assert_eq!(value["status"], "indexing_started");
    assert!(value["job_id"].as_str().unwrap().starts_with("initial-"));
}
```

Add the gate-disabled behavior to the session coordinator tests, where configuration can be controlled without mocking MCP runtime:

```rust
#[tokio::test]
async fn disabled_consent_gate_auto_authorizes_but_still_starts_index() {
    let (_data, data_dir) = temp_data_dir();
    let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));
    let config = StandaloneConfig {
        data_dir: data_dir.clone(),
        embeddings_backend: crate::config::EmbeddingsBackend::Hash,
        hash_embedding_dim: 64,
        index_consent_required: false,
        ..StandaloneConfig::default()
    };
    let embedder = Arc::new(crate::embeddings::SharedEmbedder::new(Box::new(
        crate::embeddings::hash::HashEmbedder::new(64),
    )));
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
```

- [ ] **Step 7: Run MCP-facing tests and verify GREEN**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features server::consent::tests -- --nocapture
EMBEDDINGS_BACKEND=hash cargo test --no-default-features server::standalone::tests -- --nocapture
EMBEDDINGS_BACKEND=hash cargo test --no-default-features server_instructions -- --nocapture
```

Expected: all selected tests pass.

- [ ] **Step 8: Commit Task 4**

```bash
git add src/server/consent.rs src/server/mod.rs src/server/standalone.rs src/tools/mod.rs
git commit -m "feat: require chat approval for first index"
```

---

### Task 5: Share the lifecycle with HTTP APIs and documentation

**Files:**
- Modify: `src/server/api/consent.rs:18-151`
- Modify: `src/server/api/repos.rs:236-372`
- Modify: `src/server/api/query.rs:443-490`
- Modify: `ui/src/api/consent.ts:25-45`
- Modify: `README.md:50-180,260-275`
- Modify: `npm/README.md:50-180,260-275`
- Modify: `AGENTS.md:60-75`
- Modify: `CLAUDE.md:75-90,215-230`
- Test: `src/server/api/consent.rs:159-220`
- Test: `src/server/api/repos.rs:409-520`

**Interfaces:**
- Consumes: `RepoAccess` and lifecycle payload helpers.
- Produces: HTTP consent approval responses with `job_id`.
- Guarantees: dashboard Add and manual reindex cannot bypass first-index permission.

- [ ] **Step 1: Write failing API behavior tests**

Add this test helper to the API test module so the manager and API use the same job registry:

```rust
async fn test_api_state() -> (tempfile::TempDir, Arc<ApiState>) {
    use crate::config::{EmbeddingsBackend, StandaloneConfig};
    use crate::embeddings::hash::HashEmbedder;
    use crate::embeddings::SharedEmbedder;
    use crate::log_broadcast::LogBroadcaster;
    use crate::registry::RepoRegistry;
    use crate::server::jobs;
    use crate::session::SessionManager;

    let temp = tempfile::tempdir().unwrap();
    let data_dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let config = StandaloneConfig {
        data_dir: data_dir.clone(),
        embeddings_backend: EmbeddingsBackend::Hash,
        hash_embedding_dim: 64,
        ..StandaloneConfig::default()
    };
    let registry = RepoRegistry::new(
        data_dir.join("registry.json"),
        data_dir.join("repos"),
    );
    let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));
    let job_registry = jobs::new_job_registry();
    let session_manager = Arc::new(
        SessionManager::new(
            config,
            registry,
            embedder,
            Some(job_registry.clone()),
            None,
        )
        .await
        .unwrap(),
    );
    let state = Arc::new(ApiState {
        session_manager,
        session_repos: crate::server::standalone::new_session_repos(),
        log_broadcaster: LogBroadcaster::new(),
        job_registry,
        started_at_unix_s: 0,
    });
    (temp, state)
}
```

Add tests for response mapping and state effects:

```rust
#[tokio::test]
async fn add_repo_records_pending_first_index_consent() {
    let (_data, state) = test_api_state().await;
    let repo = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(repo.path().to_path_buf()).unwrap();

    let response = handle_repo_add(
        State(state.clone()),
        Json(AddRepoRequest { path: path.to_string() }),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let repo_id = RepoRegistry::path_hash(path.as_str());
    assert!(state.session_manager.is_pending(&repo_id));
}
```

Add manual-reindex coverage:

```rust
#[tokio::test]
async fn manual_reindex_cannot_bypass_first_index_consent() {
    let (_data, state) = test_api_state().await;
    let repo = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(repo.path().to_path_buf()).unwrap();
    state
        .session_manager
        .registry
        .register(path.as_str())
        .unwrap();
    let repo_id = RepoRegistry::path_hash(path.as_str());

    let response = handle_repo_reindex(
        State(state),
        Path(repo_id),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["status"], "consent_required");
}

#[tokio::test]
async fn ready_repo_manual_reindex_remains_permission_free() {
    let (_data, state) = test_api_state().await;
    let repo = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(repo.path().to_path_buf()).unwrap();
    state
        .session_manager
        .approve_and_start_initial_index(path.as_path())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match state.session_manager.resolve_repo(path.as_path()).await.unwrap() {
                crate::session::RepoAccess::Ready(_) => break,
                crate::session::RepoAccess::Indexing { .. } => tokio::task::yield_now().await,
                _ => panic!("approved repository did not become ready"),
            }
        }
    })
    .await
    .unwrap();
    let repo_id = RepoRegistry::path_hash(path.as_str());

    let response = handle_repo_reindex(
        State(state),
        Path(repo_id),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}
```

- [ ] **Step 2: Run the new API test and verify RED**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features add_repo_records_pending_first_index_consent -- --nocapture
```

Expected: the pending assertion fails because Add currently only writes the registry.

- [ ] **Step 3: Route dashboard consent through `SessionManager`**

In `handle_consent_post`, keep path and decision validation, then call:

```rust
match decision {
    ConsentDecision::Approve => state
        .session_manager
        .approve_and_start_initial_index(repo_path.as_path())
        .await,
    ConsentDecision::Decline => {
        state
            .session_manager
            .decline_initial_index(repo_path.as_path())
            .map_err(|error| ApiError(format!("failed to record decline: {error}")))?;
        return declined HTTP response;
    }
}
```

Map `RepoAccess::Indexing` to HTTP 202 with the lifecycle payload, and `RepoAccess::Ready` to HTTP 200 with `status: "ready"`.

- [ ] **Step 4: Make Add pending and protect manual reindex**

After `registry.register` in `handle_repo_add`, call `session_manager.record_pending(repo_path.as_path())`.

At the start of `handle_repo_reindex`, call `resolve_repo`. Continue with the existing manual background job only for `RepoAccess::Ready`. Return HTTP 409 and the structured payload for `NeedsApproval`, `Declined`, or `Indexing`.

Update `resolve_query_repo` to consume `resolve_repo`. It must return the ready state only after lifecycle completion and surface the blocked lifecycle status in its error message instead of calling `get_or_create_repo` directly.

- [ ] **Step 5: Update frontend response types**

Change `ResolveConsentResponse` in `ui/src/api/consent.ts`, not `ui/src/api/types.ts`, to:

```ts
export type ResolveConsentResponse = {
  ok: boolean;
  status: "ready" | "indexing_started" | "indexing_in_progress" | "declined";
  repo: string;
  repo_id: string;
  job_id?: string;
};
```

No component layout change is required because React Query already invalidates consent, repo, and job queries after the mutation.

- [ ] **Step 6: Update user and agent documentation**

In all four documentation surfaces, replace the explicit-binding bypass description with:

```text
With INDEX_CONSENT_REQUIRED=true, every repository that has never completed a full index returns consent_required, including repositories selected through ?repo= or bind_workspace. The agent must ask in chat and wait for explicit approval. Approval starts a background InitialBind job immediately. Later watcher updates and manual reindexes do not ask again. INDEX_CONSENT_REQUIRED=false keeps the CI and benchmark opt-out but still starts the first index immediately.
```

Keep `README.md` and `npm/README.md` synchronized.

- [ ] **Step 7: Run API and UI verification and verify GREEN**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features server::api::consent::tests -- --nocapture
EMBEDDINGS_BACKEND=hash cargo test --no-default-features server::api::repos::tests -- --nocapture
EMBEDDINGS_BACKEND=hash cargo test --no-default-features server::api::query::tests -- --nocapture
cd ui && bun run lint && bun test
```

Expected: Rust API tests pass, TypeScript reports no errors, and UI tests pass.

- [ ] **Step 8: Commit Task 5**

```bash
git add src/server/api/consent.rs src/server/api/repos.rs src/server/api/query.rs ui/src/api/consent.ts README.md npm/README.md AGENTS.md CLAUDE.md
git commit -m "feat: apply first index lifecycle to HTTP APIs"
```

---

### Task 6: Prove the cold-index gap is closed and run the full regression suite

**Files:**
- Create: `tests/first_index_consent.rs`

**Interfaces:**
- Verifies the complete user-visible sequence from consent through searchable index.

- [ ] **Step 1: Write the end-to-end regression test**

Create a hash-backed temporary repository and exercise this sequence:

```rust
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
    let registry = RepoRegistry::new(
        data_dir.join("registry.json"),
        data_dir.join("repos"),
    );
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
    assert!(
        results
            .response
            .hits
            .iter()
            .any(|hit| hit.name == "consent_index_probe")
    );
}
```

Use condition-based polling with a ten-second timeout. Do not use a fixed sleep.

- [ ] **Step 2: Run the regression test**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features --test first_index_consent -- --nocapture
```

Expected: the complete consent, job, index, and retrieval sequence passes without any filesystem event after approval.

- [ ] **Step 3: Run formatting and lint checks**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets --no-default-features -- -D warnings
cd ui && bun run lint
```

Expected: every command exits successfully with no warnings.

- [ ] **Step 4: Run all Rust and UI tests**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features
cd ui && bun test && bun run build
```

Expected: all Rust tests pass, all UI tests pass, and the production UI build succeeds.

- [ ] **Step 5: Confirm documentation and source contain no stale bypass claims**

Run:

```bash
rg -n 'Explicit binds bypass|explicit.*unaffected|implicitly-bound repo' README.md npm/README.md AGENTS.md CLAUDE.md src
```

Expected: no stale claims that `?repo=` or `bind_workspace` bypass first-index consent.

- [ ] **Step 6: Review the final diff against acceptance criteria**

Run:

```bash
git diff --check
git status --short
```

Confirm the diff contains only consent-aware first-index code, tests, UI types, and documentation. Confirm no unrelated user files changed.

- [ ] **Step 7: Commit Task 6**

```bash
git add tests/first_index_consent.rs
git commit -m "test: cover consent-aware first indexing"
```

use super::SessionManager;
use crate::{
    indexer::pipeline::ExternalIndexTrigger,
    path::{Utf8Path, Utf8PathBuf},
    registry::{IndexConsent, RepoEntry, RepoRegistry},
    server::jobs::{self, Job, JobKind},
    storage::sqlite::SqliteStore,
};
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use std::{sync::Arc, time::SystemTime};

pub enum RepoAccess {
    Ready(Arc<crate::handlers::AppState>),
    NeedsApproval,
    Indexing { job: Job, started: bool },
    Declined,
}

impl SessionManager {
    pub async fn resolve_repo(self: &Arc<Self>, repo_path: &Utf8Path) -> Result<RepoAccess> {
        let canonical = crate::path::canonicalize_existing_dir(repo_path)
            .context("Failed to canonicalize repository path")?;

        let entry = match self.registry.get(canonical.as_str())? {
            Some(entry) => entry,
            None => {
                // A worktree of an indexed repo can reuse that index, which is
                // cheap enough that prompting buys the user nothing.
                if let Some(access) = self.try_seed_worktree(&canonical).await? {
                    return Ok(access);
                }
                if self.standalone_config.index_consent_required {
                    self.record_pending(canonical.as_path());
                    return Ok(RepoAccess::NeedsApproval);
                }
                self.registry.approve_initial_index(canonical.as_str())?
            }
        };

        if entry.consent == IndexConsent::Declined {
            return Ok(RepoAccess::Declined);
        }

        if entry.initial_index_completed_at.is_some() || self.has_persisted_index_run(&entry)? {
            if entry.initial_index_completed_at.is_none() {
                self.registry
                    .mark_initial_index_completed(canonical.as_str())?;
            }
            let runtime = self.get_or_create_runtime(&canonical).await?;
            runtime.ensure_watcher_started();
            return Ok(RepoAccess::Ready(runtime.state.clone()));
        }

        if entry.initial_index_approved_at.is_none() {
            if let Some(access) = self.try_seed_worktree(&canonical).await? {
                return Ok(access);
            }
            if self.standalone_config.index_consent_required {
                self.record_pending(canonical.as_path());
                return Ok(RepoAccess::NeedsApproval);
            }
            self.registry.approve_initial_index(canonical.as_str())?;
        }

        self.start_or_get_initial_index(&canonical).await
    }

    /// Decide whether `worktree` can be seeded from an already-indexed base.
    ///
    /// Every precondition failure returns `None`, which sends the caller down
    /// the normal consent and full-index path.
    fn plan_worktree_seed(
        &self,
        worktree: &Utf8PathBuf,
    ) -> Result<Option<crate::session::worktree::SeedPlan>> {
        let Some(base_path) = crate::session::worktree::resolve_base_repo(worktree.as_path())
        else {
            return Ok(None);
        };

        let Some(base_entry) = self.registry.get(base_path.as_str())? else {
            tracing::debug!(base = %base_path, "worktree base is not registered, not seeding");
            return Ok(None);
        };
        if base_entry.initial_index_completed_at.is_none() {
            tracing::debug!(base = %base_path, "worktree base has no completed index, not seeding");
            return Ok(None);
        }
        let base_db = base_entry.data_dir.join("code-intelligence.db");
        if !base_db.as_std_path().is_file() {
            return Ok(None);
        }

        // A base built by a different extraction format would be cleared and
        // fully rebuilt on first use, so seeding from it saves nothing.
        let base_version = {
            let conn = rusqlite::Connection::open(base_db.as_std_path())
                .with_context(|| format!("Failed to open base index at {base_db}"))?;
            conn.query_row(
                "SELECT value FROM index_metadata WHERE key = 'graph_index_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("Failed to read base graph index version")?
        };
        if base_version.as_deref() != Some(crate::indexer::pipeline::GRAPH_INDEX_VERSION) {
            tracing::info!(
                base = %base_path,
                base_version = ?base_version,
                current_version = %crate::indexer::pipeline::GRAPH_INDEX_VERSION,
                "worktree base index predates the current graph format, not seeding"
            );
            return Ok(None);
        }

        // Derive the per-repo data directory from the base's own, so it always
        // lands where this registry puts things rather than where the config
        // says it should.
        let Some(repos_dir) = base_entry.data_dir.parent() else {
            tracing::warn!(
                base_data_dir = %base_entry.data_dir,
                "base data dir has no parent directory, not seeding"
            );
            return Ok(None);
        };
        // Whether the target directory is free is deliberately NOT checked here:
        // it has to be checked under the locks `try_seed_worktree` takes, or two
        // first binds of the same worktree would both decide to seed it.
        let worktree_data_dir = repos_dir.join(RepoRegistry::path_hash(worktree.as_str()));

        Ok(Some(crate::session::worktree::SeedPlan {
            base_repo_path: base_path.clone(),
            base_repo_id: RepoRegistry::path_hash(base_path.as_str()),
            base_data_dir: base_entry.data_dir.clone(),
            worktree_path: worktree.clone(),
            worktree_data_dir,
        }))
    }

    /// Seed a worktree's index from its base, then start the normal first pass.
    ///
    /// Returns `Ok(None)` when there is nothing to seed from or when seeding
    /// failed, in which case the caller falls through to the consent path.
    ///
    /// Seeding is strictly an optimization, so nothing this function does may fail
    /// a bind: a refused precondition, a clone error, a panic inside the clone, a
    /// registry write that will not land, and a cloned store that will not open
    /// all log a warning, undo the seed, and return `Ok(None)`. The last of those
    /// matters because the seed is what makes those stores exist and get opened:
    /// with consent required the caller would have returned `NeedsApproval`
    /// without opening anything, so propagating that error would strand the
    /// worktree on a data dir nothing re-runs (`index_runs` was cleared, so
    /// `has_persisted_index_run` stays false) and nothing repairs.
    ///
    /// The one error that does propagate comes from the already-populated data
    /// dir branch below, where the index belongs to another bind that has already
    /// registered and approved it. Joining its lifecycle is then exactly what the
    /// caller does for any approved repo, and the failure is not this seed's to
    /// undo.
    async fn try_seed_worktree(
        self: &Arc<Self>,
        canonical: &Utf8PathBuf,
    ) -> Result<Option<RepoAccess>> {
        let plan = match self.plan_worktree_seed(canonical) {
            Ok(Some(plan)) => plan,
            Ok(None) => return Ok(None),
            Err(error) => {
                tracing::warn!(worktree = %canonical, %error, "worktree seed planning failed");
                return Ok(None);
            }
        };

        // Hold the BASE repo's init lock so no other session can initialize its
        // stores while we snapshot its Tantivy and LanceDB directories. It does
        // not gate an already-running watcher pass on the base; a clone torn by
        // one fails `seed_index_from_base`'s validation, and the error path below
        // turns that into a normal full index.
        let base_key = plan.base_repo_path.as_str().to_string();
        let base_lock = self
            .init_locks
            .entry(base_key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let base_guard = base_lock.lock().await;

        // And the worktree's own, so only one bind at a time can decide to write
        // into its data directory. This is the lock `get_or_create_runtime` uses,
        // so holding it also keeps a concurrent bind from opening the half-cloned
        // stores. Both guards go before the first index pass starts, because that
        // path takes this same lock.
        let worktree_lock = self
            .init_locks
            .entry(canonical.as_str().to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let worktree_guard = worktree_lock.lock().await;

        // Now that nothing else can be seeding this worktree, check the target
        // directory. Artifacts here mean another bind seeded it while we waited,
        // or an earlier attempt left something behind. Either way it is not ours
        // to overwrite, and it must not be treated as a seed failure: that path
        // deletes the directory, which would destroy an index already in use.
        if crate::session::worktree::data_dir_has_index_artifacts(&plan.worktree_data_dir) {
            drop(worktree_guard);
            drop(base_guard);
            tracing::debug!(
                worktree = %canonical,
                "worktree data dir already holds index artifacts, not seeding"
            );
            // An approved entry means another bind owns this index: it seeded and
            // approved the worktree while we waited, so join its lifecycle rather
            // than asking the user about a repo that is already indexing.
            // Anything else is not ours to interpret, so hand it back to the
            // caller's normal consent handling.
            let owned_by_another_bind = match self.registry.get(canonical.as_str()) {
                Ok(entry) => entry.is_some_and(|entry| entry.initial_index_approved_at.is_some()),
                Err(error) => {
                    tracing::warn!(
                        worktree = %canonical,
                        %error,
                        "could not read the registry for an already-populated worktree data dir"
                    );
                    return Ok(None);
                }
            };
            // This `?` is the one error the seed path may propagate: nothing here
            // was created by us, so there is nothing to undo, and the entry is
            // already approved, which is the state the caller's own lifecycle
            // handling would have acted on.
            if owned_by_another_bind {
                return Ok(Some(self.start_or_get_initial_index(canonical).await?));
            }
            return Ok(None);
        }

        // `register` creates the (empty) data directory the clone writes into.
        // Whether the entry is ours decides both whether the failure path may
        // drop it and whether the seed may stamp its provenance on it.
        let entry_is_ours = match self.registry.get(canonical.as_str()) {
            Ok(entry) => entry.is_none(),
            Err(error) => {
                tracing::warn!(
                    worktree = %canonical,
                    %error,
                    "could not read the registry before seeding a worktree"
                );
                return Ok(None);
            }
        };
        if let Err(error) = self.registry.register(canonical.as_str()) {
            tracing::warn!(
                worktree = %canonical,
                %error,
                "could not register a worktree for seeding, falling back to a full index"
            );
            self.discard_failed_seed(canonical, &plan.worktree_data_dir, entry_is_ours);
            return Ok(None);
        }

        let blocking_plan = plan.clone();
        let seeded = match tokio::task::spawn_blocking(move || {
            crate::session::worktree::seed_index_from_base(&blocking_plan)
        })
        .await
        {
            Ok(seeded) => seeded,
            // A panic or cancellation inside the seed. The clone writes to raw
            // paths, so treat it exactly like a returned error.
            Err(error) => {
                tracing::warn!(
                    worktree = %canonical,
                    base = %plan.base_repo_path,
                    %error,
                    "worktree seed task did not finish, falling back to a full index"
                );
                self.discard_failed_seed(canonical, &plan.worktree_data_dir, entry_is_ours);
                return Ok(None);
            }
        };

        if let Err(error) = seeded {
            tracing::warn!(
                worktree = %canonical,
                base = %plan.base_repo_path,
                %error,
                "worktree index seeding failed, falling back to a full index"
            );
            // Leave nothing half-built for the fallback path to trip over: a
            // surviving data dir would hold index artifacts and so block every
            // later seed attempt for this worktree.
            self.discard_failed_seed(canonical, &plan.worktree_data_dir, entry_is_ours);
            return Ok(None);
        }

        // Provenance goes only on an entry this seed created. It is what enrolls a
        // repo in the prune sweep, and an entry the user registered by hand must
        // never become something the daemon deletes on its own.
        if entry_is_ours {
            if let Err(error) = self
                .registry
                .mark_seeded_from(canonical.as_str(), &plan.base_repo_id)
            {
                tracing::warn!(
                    worktree = %canonical,
                    %error,
                    "could not record worktree seed provenance, falling back to a full index"
                );
                self.discard_failed_seed(canonical, &plan.worktree_data_dir, entry_is_ours);
                return Ok(None);
            }
        }
        if let Err(error) = self.registry.approve_initial_index(canonical.as_str()) {
            tracing::warn!(
                worktree = %canonical,
                %error,
                "could not auto-approve a seeded worktree, falling back to a full index"
            );
            self.discard_failed_seed(canonical, &plan.worktree_data_dir, entry_is_ours);
            return Ok(None);
        }
        // The worktree may have asked for consent on an earlier bind, before its
        // base was indexed. It is not waiting on the user any more.
        self.clear_pending(&RepoRegistry::path_hash(canonical.as_str()));

        tracing::info!(
            worktree = %canonical,
            base = %plan.base_repo_path,
            "seeded worktree index from base repo, indexing the delta only"
        );

        // Release both before the first pass below, which takes the worktree's
        // own locks.
        drop(worktree_guard);
        drop(base_guard);

        // Opening the cloned stores is the last thing that can go wrong, and it is
        // the seed's failure rather than the caller's: `validate_seeded_index` does
        // not open the cloned `vectors/`, so a Lance dataset copied mid-write first
        // surfaces here, inside the very stores this seed created. Undo the seed so
        // the caller reaches the consent path with a clean slate; propagating would
        // leave an approved entry whose index nothing re-runs and nothing repairs.
        match self.start_or_get_initial_index(canonical).await {
            Ok(access) => Ok(Some(access)),
            Err(error) => {
                tracing::warn!(
                    worktree = %canonical,
                    base = %plan.base_repo_path,
                    %error,
                    "seeded worktree index will not open, discarding the seed and \
                     falling back to a full index"
                );
                self.discard_failed_seed(canonical, &plan.worktree_data_dir, entry_is_ours);
                Ok(None)
            }
        }
    }

    /// Undo a seed that will not be used: remove its data directory, and the
    /// registry entry too when the seed is what created it. Best-effort, since the
    /// caller is already on its way to a full index either way.
    ///
    /// Covers both a clone that failed and a clone that succeeded but could not be
    /// recorded, because an index the registry does not describe as seeded and
    /// approved is one nothing will ever finish or prune.
    ///
    /// The directory always goes: whatever the clone managed to write counts as
    /// index artifacts, which would block every later seed of this worktree.
    /// `init_repo_state` recreates the directory it needs.
    fn discard_failed_seed(
        &self,
        canonical: &Utf8PathBuf,
        data_dir: &Utf8Path,
        entry_is_ours: bool,
    ) {
        match std::fs::remove_dir_all(data_dir.as_std_path()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                data_dir = %data_dir,
                %error,
                "could not remove the data dir left by a failed seed; \
                 this worktree will not be seedable until it is deleted"
            ),
        }
        if !entry_is_ours {
            return;
        }
        if let Err(error) = self.registry.remove(canonical.as_str()) {
            tracing::warn!(
                worktree = %canonical,
                %error,
                "could not drop the registry entry created for a failed seed"
            );
        }
    }

    pub async fn approve_and_start_initial_index(
        self: &Arc<Self>,
        repo_path: &Utf8Path,
    ) -> Result<RepoAccess> {
        let canonical = crate::path::canonicalize_existing_dir(repo_path)
            .context("Failed to canonicalize repository path")?;
        self.registry.approve_initial_index(canonical.as_str())?;
        self.clear_pending(&RepoRegistry::path_hash(canonical.as_str()));
        self.start_or_get_initial_index(&canonical).await
    }

    pub fn decline_initial_index(&self, repo_path: &Utf8Path) -> Result<()> {
        let canonical = crate::path::canonicalize_existing_dir(repo_path)
            .context("Failed to canonicalize repository path")?;
        self.registry
            .set_consent(canonical.as_str(), IndexConsent::Declined)?;
        self.clear_pending(&RepoRegistry::path_hash(canonical.as_str()));
        Ok(())
    }

    fn has_persisted_index_run(&self, entry: &RepoEntry) -> Result<bool> {
        let db_path = entry.data_dir.join("code-intelligence.db");
        if !db_path.as_std_path().exists() {
            return Ok(false);
        }

        let sqlite = SqliteStore::open(&db_path).context("Failed to open persisted index state")?;
        sqlite
            .init()
            .context("Failed to initialize persisted index schema")?;
        Ok(sqlite.latest_index_run()?.is_some())
    }

    async fn start_or_get_initial_index(
        self: &Arc<Self>,
        canonical: &Utf8PathBuf,
    ) -> Result<RepoAccess> {
        let key = canonical.as_str().to_string();
        let lock = self
            .initial_index_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        let entry = self
            .registry
            .get(canonical.as_str())?
            .with_context(|| format!("Repository is not registered: {canonical}"))?;

        if entry.consent == IndexConsent::Declined {
            return Ok(RepoAccess::Declined);
        }

        if entry.initial_index_completed_at.is_some() || self.has_persisted_index_run(&entry)? {
            if entry.initial_index_completed_at.is_none() {
                self.registry
                    .mark_initial_index_completed(canonical.as_str())?;
            }
            let runtime = self.get_or_create_runtime(canonical).await?;
            runtime.ensure_watcher_started();
            return Ok(RepoAccess::Ready(runtime.state.clone()));
        }

        if entry.initial_index_approved_at.is_none() {
            self.record_pending(canonical.as_path());
            return Ok(RepoAccess::NeedsApproval);
        }

        let repo_id = RepoRegistry::path_hash(canonical.as_str());
        if let Some(job) = jobs::most_recent_running_for_repo_kind(
            &self.job_registry,
            &repo_id,
            JobKind::InitialBind,
        ) {
            return Ok(RepoAccess::Indexing {
                job,
                started: false,
            });
        }

        let runtime = self.get_or_create_runtime(canonical).await?;
        let job_id = format!(
            "initial-{}-{}",
            repo_id,
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        );
        let job = jobs::register_running(
            &self.job_registry,
            job_id.clone(),
            JobKind::InitialBind,
            repo_id,
            canonical.as_str().to_string(),
        );

        let worker_runtime = runtime.clone();
        let worker_registry = self.registry.clone();
        let worker_jobs = self.job_registry.clone();
        let worker_job_id = job_id.clone();
        let worker_repo = canonical.clone();
        let task = tokio::spawn(async move {
            let outcome = worker_runtime
                .state
                .indexer
                .index_all_with_external_index(ExternalIndexTrigger::InitialBind)
                .await;

            match outcome {
                Ok(outcome) => {
                    if let Err(error) =
                        worker_registry.mark_initial_index_completed(worker_repo.as_str())
                    {
                        jobs::mark_failed(&worker_jobs, &worker_job_id, error.to_string());
                        return;
                    }
                    worker_runtime.ensure_watcher_started();
                    jobs::mark_succeeded(
                        &worker_jobs,
                        &worker_job_id,
                        serde_json::json!({
                            "stats": outcome.stats,
                            "external_index": outcome.external_index,
                        }),
                    );
                }
                Err(error) => jobs::mark_failed(&worker_jobs, &worker_job_id, error.to_string()),
            }
        });

        let watchdog_jobs = self.job_registry.clone();
        let watchdog_job_id = job_id;
        tokio::spawn(async move {
            if let Err(join_error) = task.await {
                let reason = if join_error.is_panic() {
                    format!("initial index task panicked: {join_error}")
                } else if join_error.is_cancelled() {
                    "initial index task cancelled before completion".to_string()
                } else {
                    format!("initial index task aborted: {join_error}")
                };
                jobs::mark_failed_if_running(&watchdog_jobs, &watchdog_job_id, reason);
            }
        });

        Ok(RepoAccess::Indexing { job, started: true })
    }
}

use super::SessionManager;
use crate::{
    indexer::pipeline::ExternalIndexTrigger,
    path::{Utf8Path, Utf8PathBuf},
    registry::{IndexConsent, RepoEntry, RepoRegistry},
    server::jobs::{self, Job, JobKind},
    storage::sqlite::SqliteStore,
};
use anyhow::{Context, Result};
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
            None if self.standalone_config.index_consent_required => {
                self.record_pending(canonical.as_path());
                return Ok(RepoAccess::NeedsApproval);
            }
            None => self.registry.approve_initial_index(canonical.as_str())?,
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
            if self.standalone_config.index_consent_required {
                self.record_pending(canonical.as_path());
                return Ok(RepoAccess::NeedsApproval);
            }
            self.registry.approve_initial_index(canonical.as_str())?;
        }

        self.start_or_get_initial_index(&canonical).await
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

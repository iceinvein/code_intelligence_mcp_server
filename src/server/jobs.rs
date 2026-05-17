//! In-memory registry of background indexing jobs for the dashboard.
//!
//! Three job kinds are tracked:
//! - `ManualReindex`: triggered by `POST /api/repos/{id}/reindex`.
//! - `InitialBind`: the first index pass after a session binds a repo
//!   (registered from the watch loop's first fire on a fresh AppState).
//! - `WatchReindex`: subsequent incremental reindex runs driven by the
//!   file watcher in `IndexPipeline::spawn_watch_loop`.
//!
//! Coalescing: the watch loop already serialises runs per repo, so we
//! never need to merge two concurrent jobs. Instead each run carries a
//! `coalesced_count` field with the number of filesystem events the
//! debounce window absorbed into it. The dashboard reads that field to
//! show "WatchReindex • 7 events" without one row per fire.
//!
//! There is no intra-job progress %. The indexer emits final
//! `IndexRunStats` only. We record stats on completion and show
//! "running" until the task returns.

use dashmap::DashMap;
use serde::Serialize;
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Keep finished jobs visible in the dashboard for this long before
/// evicting. Running jobs are never evicted by this TTL.
pub const FINISHED_JOB_TTL: Duration = Duration::from_secs(900); // 15 min

/// Cap on total jobs (running + finished) to prevent unbounded memory
/// growth if the eviction loop ever falls behind.
pub const MAX_JOBS: usize = 200;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Triggered by `POST /api/repos/{id}/reindex`.
    ManualReindex,
    /// First index pass for a freshly bound repo (cold scan).
    InitialBind,
    /// Incremental reindex driven by the file watcher.
    WatchReindex,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub id: String,
    pub kind: JobKind,
    pub repo_id: String,
    pub repo_path: String,
    pub status: JobStatus,
    pub started_at_unix_s: u64,
    pub finished_at_unix_s: Option<u64>,
    pub duration_ms: Option<u64>,
    /// JSON of `IndexRunStats` when the run succeeded. None for running or
    /// failed jobs.
    pub stats: Option<Value>,
    /// Error message when the run failed. None for running or succeeded
    /// jobs.
    pub error: Option<String>,
    /// Watcher-only: number of filesystem events absorbed into this run
    /// by the debounce window (0 for manual reindexes).
    #[serde(default)]
    pub coalesced_count: u32,
}

pub type JobRegistry = Arc<DashMap<String, Job>>;

pub fn new_job_registry() -> JobRegistry {
    Arc::new(DashMap::new())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Register a fresh `Running` job. Returns the job id (the caller-provided
/// one is reused so the API response and the dashboard agree).
pub fn register_running(
    registry: &JobRegistry,
    job_id: String,
    kind: JobKind,
    repo_id: String,
    repo_path: String,
) -> Job {
    register_running_with_coalesced(registry, job_id, kind, repo_id, repo_path, 0)
}

/// Variant of [`register_running`] that pre-seeds `coalesced_count`. The
/// watch loop uses this because the debounce window has already drained N
/// filesystem events into the upcoming run before the job is registered.
pub fn register_running_with_coalesced(
    registry: &JobRegistry,
    job_id: String,
    kind: JobKind,
    repo_id: String,
    repo_path: String,
    coalesced_count: u32,
) -> Job {
    let job = Job {
        id: job_id.clone(),
        kind,
        repo_id,
        repo_path,
        status: JobStatus::Running,
        started_at_unix_s: unix_now(),
        finished_at_unix_s: None,
        duration_ms: None,
        stats: None,
        error: None,
        coalesced_count,
    };
    registry.insert(job_id, job.clone());
    job
}

/// Most recent `Running` job for `repo_id`, if any. Used by `/api/repos`
/// to render the per-row live indicator.
pub fn most_recent_running_for_repo(registry: &JobRegistry, repo_id: &str) -> Option<Job> {
    registry
        .iter()
        .filter(|e| e.value().repo_id == repo_id && e.value().status == JobStatus::Running)
        .map(|e| e.value().clone())
        .max_by_key(|j| j.started_at_unix_s)
}

/// Most recent finished (`Succeeded` or `Failed`) job for `repo_id`, if
/// any. Used by `/api/repos` to render the "last reindex Xm ago" hint
/// when no job is currently running.
pub fn most_recent_finished_for_repo(registry: &JobRegistry, repo_id: &str) -> Option<Job> {
    registry
        .iter()
        .filter(|e| {
            e.value().repo_id == repo_id
                && matches!(e.value().status, JobStatus::Succeeded | JobStatus::Failed)
        })
        .map(|e| e.value().clone())
        .max_by_key(|j| j.finished_at_unix_s.unwrap_or(0))
}

/// Mark a job as succeeded with final stats.
pub fn mark_succeeded(registry: &JobRegistry, job_id: &str, stats: Value) {
    if let Some(mut entry) = registry.get_mut(job_id) {
        let now = unix_now();
        entry.status = JobStatus::Succeeded;
        entry.finished_at_unix_s = Some(now);
        entry.duration_ms = Some(
            now.saturating_sub(entry.started_at_unix_s)
                .saturating_mul(1000),
        );
        entry.stats = Some(stats);
    }
}

/// Mark a job as failed with an error string.
pub fn mark_failed(registry: &JobRegistry, job_id: &str, error: String) {
    if let Some(mut entry) = registry.get_mut(job_id) {
        let now = unix_now();
        entry.status = JobStatus::Failed;
        entry.finished_at_unix_s = Some(now);
        entry.duration_ms = Some(
            now.saturating_sub(entry.started_at_unix_s)
                .saturating_mul(1000),
        );
        entry.error = Some(error);
    }
}

/// Mark a job as failed only if it is still `Running`. Used by the
/// panic-watchdog so we do not overwrite a normal `Succeeded`/`Failed`
/// result with a spurious "task panicked" entry when the JoinHandle
/// resolves after the worker has already recorded its outcome.
pub fn mark_failed_if_running(registry: &JobRegistry, job_id: &str, error: String) {
    if let Some(mut entry) = registry.get_mut(job_id) {
        if entry.status == JobStatus::Running {
            let now = unix_now();
            entry.status = JobStatus::Failed;
            entry.finished_at_unix_s = Some(now);
            entry.duration_ms = Some(
                now.saturating_sub(entry.started_at_unix_s)
                    .saturating_mul(1000),
            );
            entry.error = Some(error);
        }
    }
}

/// Snapshot the registry sorted newest-first.
pub fn snapshot(registry: &JobRegistry) -> Vec<Job> {
    let mut items: Vec<Job> = registry.iter().map(|e| e.value().clone()).collect();
    items.sort_by(|a, b| b.started_at_unix_s.cmp(&a.started_at_unix_s));
    items
}

/// Spawn a background task that evicts finished jobs past
/// [`FINISHED_JOB_TTL`] and trims the registry to [`MAX_JOBS`].
pub fn spawn_job_eviction_loop(registry: JobRegistry) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            evict_once(&registry);
        }
    });
}

fn evict_once(registry: &JobRegistry) {
    let now = unix_now();
    let ttl_secs = FINISHED_JOB_TTL.as_secs();
    let stale: Vec<String> = registry
        .iter()
        .filter(|e| {
            let j = e.value();
            matches!(j.status, JobStatus::Succeeded | JobStatus::Failed)
                && j.finished_at_unix_s
                    .map(|t| now.saturating_sub(t) > ttl_secs)
                    .unwrap_or(false)
        })
        .map(|e| e.key().clone())
        .collect();
    for id in stale {
        registry.remove(&id);
    }

    // Hard cap: if we still exceed MAX_JOBS, drop the oldest finished
    // jobs first. Running jobs are preserved unconditionally.
    if registry.len() > MAX_JOBS {
        let mut finished: Vec<(String, u64)> = registry
            .iter()
            .filter(|e| e.value().status != JobStatus::Running)
            .map(|e| (e.key().clone(), e.value().finished_at_unix_s.unwrap_or(0)))
            .collect();
        finished.sort_by_key(|(_, t)| *t);
        let to_drop = registry.len() - MAX_JOBS;
        for (id, _) in finished.into_iter().take(to_drop) {
            registry.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn register_and_complete_succeeded() {
        let reg = new_job_registry();
        let job = register_running(
            &reg,
            "job-1".to_string(),
            JobKind::ManualReindex,
            "repo-hash".to_string(),
            "/abs/path".to_string(),
        );
        assert_eq!(job.status, JobStatus::Running);
        assert!(job.finished_at_unix_s.is_none());

        mark_succeeded(&reg, "job-1", json!({ "files_indexed": 42 }));
        let after = reg.get("job-1").unwrap().clone();
        assert_eq!(after.status, JobStatus::Succeeded);
        assert!(after.finished_at_unix_s.is_some());
        assert_eq!(
            after.stats.unwrap().get("files_indexed").unwrap().as_u64(),
            Some(42)
        );
    }

    #[test]
    fn register_and_complete_failed() {
        let reg = new_job_registry();
        register_running(
            &reg,
            "job-2".to_string(),
            JobKind::ManualReindex,
            "h".to_string(),
            "/p".to_string(),
        );
        mark_failed(&reg, "job-2", "boom".to_string());
        let after = reg.get("job-2").unwrap().clone();
        assert_eq!(after.status, JobStatus::Failed);
        assert_eq!(after.error.as_deref(), Some("boom"));
    }

    #[test]
    fn mark_done_on_unknown_id_is_noop() {
        let reg = new_job_registry();
        // Must not panic.
        mark_succeeded(&reg, "nope", json!({}));
        mark_failed(&reg, "nope", "x".to_string());
        mark_failed_if_running(&reg, "nope", "x".to_string());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn mark_failed_if_running_skips_already_completed_jobs() {
        let reg = new_job_registry();
        register_running(
            &reg,
            "ok".to_string(),
            JobKind::ManualReindex,
            "h".to_string(),
            "/p".to_string(),
        );
        mark_succeeded(&reg, "ok", json!({ "files_indexed": 1 }));

        // Watchdog later resolves; must not overwrite the Succeeded result.
        mark_failed_if_running(&reg, "ok", "watchdog: panic".to_string());

        let after = reg.get("ok").unwrap().clone();
        assert_eq!(after.status, JobStatus::Succeeded);
        assert!(after.error.is_none());
    }

    #[test]
    fn mark_failed_if_running_promotes_running_to_failed() {
        let reg = new_job_registry();
        register_running(
            &reg,
            "stuck".to_string(),
            JobKind::ManualReindex,
            "h".to_string(),
            "/p".to_string(),
        );
        mark_failed_if_running(&reg, "stuck", "task panicked".to_string());

        let after = reg.get("stuck").unwrap().clone();
        assert_eq!(after.status, JobStatus::Failed);
        assert_eq!(after.error.as_deref(), Some("task panicked"));
    }

    #[test]
    fn snapshot_returns_newest_first() {
        let reg = new_job_registry();
        register_running(
            &reg,
            "a".to_string(),
            JobKind::ManualReindex,
            "h".to_string(),
            "/p".to_string(),
        );
        // Force a later timestamp by mutating started_at directly.
        std::thread::sleep(Duration::from_millis(1100));
        register_running(
            &reg,
            "b".to_string(),
            JobKind::ManualReindex,
            "h".to_string(),
            "/p".to_string(),
        );

        let s = snapshot(&reg);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].id, "b");
        assert_eq!(s[1].id, "a");
    }

    #[test]
    fn evict_once_drops_old_finished_jobs() {
        let reg = new_job_registry();
        register_running(
            &reg,
            "old".to_string(),
            JobKind::ManualReindex,
            "h".to_string(),
            "/p".to_string(),
        );
        mark_succeeded(&reg, "old", json!({}));

        // Backdate finished_at so the eviction loop sees it as stale.
        let ttl = FINISHED_JOB_TTL.as_secs();
        if let Some(mut e) = reg.get_mut("old") {
            e.finished_at_unix_s = Some(unix_now().saturating_sub(ttl + 60));
        }

        register_running(
            &reg,
            "fresh-running".to_string(),
            JobKind::ManualReindex,
            "h".to_string(),
            "/p".to_string(),
        );

        evict_once(&reg);
        assert!(!reg.contains_key("old"));
        assert!(reg.contains_key("fresh-running"));
    }

    #[test]
    fn evict_once_preserves_running_under_cap() {
        let reg = new_job_registry();
        register_running(
            &reg,
            "still-running".to_string(),
            JobKind::ManualReindex,
            "h".to_string(),
            "/p".to_string(),
        );
        evict_once(&reg);
        assert!(reg.contains_key("still-running"));
    }

    #[test]
    fn register_running_default_coalesced_count_is_zero() {
        let reg = new_job_registry();
        let job = register_running(
            &reg,
            "j".to_string(),
            JobKind::ManualReindex,
            "h".to_string(),
            "/p".to_string(),
        );
        assert_eq!(job.coalesced_count, 0);
    }

    #[test]
    fn register_running_with_coalesced_preseeds_count() {
        let reg = new_job_registry();
        let job = register_running_with_coalesced(
            &reg,
            "j".to_string(),
            JobKind::WatchReindex,
            "h".to_string(),
            "/p".to_string(),
            7,
        );
        assert_eq!(job.coalesced_count, 7);
        let stored = reg.get("j").unwrap().clone();
        assert_eq!(stored.coalesced_count, 7);
    }

    #[test]
    fn most_recent_running_for_repo_filters_by_repo_and_status() {
        let reg = new_job_registry();
        register_running(
            &reg,
            "other-repo".to_string(),
            JobKind::WatchReindex,
            "other".to_string(),
            "/o".to_string(),
        );
        register_running(
            &reg,
            "old".to_string(),
            JobKind::WatchReindex,
            "target".to_string(),
            "/t".to_string(),
        );
        mark_succeeded(&reg, "old", json!({}));
        std::thread::sleep(Duration::from_millis(1100));
        register_running(
            &reg,
            "new".to_string(),
            JobKind::WatchReindex,
            "target".to_string(),
            "/t".to_string(),
        );

        let found = most_recent_running_for_repo(&reg, "target").unwrap();
        assert_eq!(found.id, "new");
    }

    #[test]
    fn most_recent_running_returns_none_when_only_finished() {
        let reg = new_job_registry();
        register_running(
            &reg,
            "done".to_string(),
            JobKind::WatchReindex,
            "target".to_string(),
            "/t".to_string(),
        );
        mark_succeeded(&reg, "done", json!({}));
        assert!(most_recent_running_for_repo(&reg, "target").is_none());
    }

    #[test]
    fn most_recent_finished_for_repo_returns_latest_completed() {
        let reg = new_job_registry();
        register_running(
            &reg,
            "first".to_string(),
            JobKind::WatchReindex,
            "target".to_string(),
            "/t".to_string(),
        );
        mark_succeeded(&reg, "first", json!({ "files_indexed": 1 }));
        std::thread::sleep(Duration::from_millis(1100));
        register_running(
            &reg,
            "second".to_string(),
            JobKind::WatchReindex,
            "target".to_string(),
            "/t".to_string(),
        );
        mark_failed(&reg, "second", "boom".to_string());

        let found = most_recent_finished_for_repo(&reg, "target").unwrap();
        assert_eq!(found.id, "second");
        assert_eq!(found.status, JobStatus::Failed);
    }
}

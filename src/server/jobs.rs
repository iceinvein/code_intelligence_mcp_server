//! In-memory registry of background indexing jobs for the dashboard.
//!
//! The daemon spawns background tasks for manual reindex (POST
//! `/api/repos/{id}/reindex`). Without tracking, those tasks run silently:
//! the API returns 202 with a `job_id` and the caller has no way to find
//! out when it finished. This module is the lightweight bookkeeping side
//! of that promise — populated by the spawner, consumed by `GET
//! /api/jobs`.
//!
//! Scope notes:
//! - Manual reindexes are tracked. The file-watcher auto-reindex
//!   (`IndexPipeline::spawn_watch_loop`) and initial-index-on-bind are
//!   not tracked here yet; they need plumbing through the indexer.
//! - There is no intra-job progress %. The indexer emits final
//!   `IndexRunStats` only. We record stats on completion and show
//!   "running" until the task returns.

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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Triggered by `POST /api/repos/{id}/reindex`.
    ManualReindex,
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
    };
    registry.insert(job_id, job.clone());
    job
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
        assert_eq!(reg.len(), 0);
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
}

//! Session management for standalone mode: maps repo paths to per-repo AppState instances.

mod initial_index;
mod worktree;

pub use initial_index::RepoAccess;

use crate::{
    config::StandaloneConfig,
    embeddings::SharedEmbedder,
    handlers::AppState,
    indexer::pipeline::IndexPipeline,
    metrics::MetricsRegistry,
    path::{Utf8Path, Utf8PathBuf},
    registry::{RepoEntry, RepoRegistry},
    reranker::Reranker,
    retrieval::Retriever,
    server::jobs::{self, JobRegistry},
    storage::{sqlite::SqliteStore, tantivy::TantivyIndex, vector::LanceDbStore},
};
use anyhow::{Context, Result};
use dashmap::DashMap;
use serde::Serialize;
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Decide whether to spawn the index-time LLM description worker.
///
/// Descriptions are off by default (`descriptions_enabled` false): the backfill
/// is a multi-hour index-time cost with no proven retrieval benefit (R005/R006).
/// They run only when explicitly enabled, an LLM backend is available, and the
/// bench ablation knob is not set.
fn should_spawn_description_worker(
    descriptions_enabled: bool,
    llm_enabled: bool,
    bench_disabled: bool,
) -> bool {
    descriptions_enabled && llm_enabled && !bench_disabled
}

/// One repo awaiting an indexing decision. Held in memory only: pending consent
/// is transient, agent-driven state, so it is intentionally not persisted (that
/// would write a registry entry for every temp/worktree dir the gate exists to
/// skip). Cleared on restart; the gate re-records it when an agent retries.
#[derive(Debug, Clone, Serialize)]
pub struct PendingConsent {
    pub repo_path: String,
    pub repo_id: String,
    pub detected: String,
    pub recommendation: String,
    pub detail: Option<String>,
    pub first_seen_unix_s: i64,
    pub last_seen_unix_s: i64,
    pub occurrences: u32,
}

fn now_unix_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Whether an absent path's absence is explained by something other than a
/// deletion, in which case no deletion countdown may start.
///
/// Walks up to the deepest ancestor that exists. Two answers mean "no evidence":
/// `/Volumes`, which is the macOS mount table root and therefore says the volume
/// is not mounted rather than that the repo is gone; and `/`, which says nothing
/// on the path was ever reachable. Anything else is a live directory whose child
/// vanished, which is a deletion.
///
/// Only called for paths already known to be absent.
fn absence_is_inconclusive(path: &str) -> bool {
    let mut cursor = std::path::Path::new(path);
    while let Some(parent) = cursor.parent() {
        if parent.exists() {
            return parent == std::path::Path::new("/")
                || parent == std::path::Path::new("/Volumes");
        }
        cursor = parent;
    }
    // Relative or empty path: not something the sweep should act on.
    true
}

/// What the sweep should do with a non-seeded entry whose path is absent.
#[derive(Debug, PartialEq, Eq)]
enum GraceVerdict {
    /// No usable stamp: record the current time and wait for a later sweep.
    Stamp,
    /// Stamped and still inside the grace window, or grace is disabled.
    Wait,
    /// Absent for at least the grace period. Delete the index.
    Delete,
}

/// Decide the fate of a non-seeded entry from its stamp alone.
///
/// Pure so the grace arithmetic is testable without touching disk or sleeping.
/// An unparseable stamp yields `Stamp`, which rewrites it: a corrupt value must
/// not be able to trigger a deletion, nor pin an entry forever.
fn grace_verdict(
    missing_since: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
    grace_days: u32,
) -> GraceVerdict {
    let Some(stamp) = missing_since else {
        return GraceVerdict::Stamp;
    };
    let Ok(stamped) = chrono::DateTime::parse_from_rfc3339(stamp) else {
        return GraceVerdict::Stamp;
    };
    if grace_days == 0 {
        return GraceVerdict::Wait;
    }
    let elapsed = now.signed_duration_since(stamped.with_timezone(&chrono::Utc));
    if elapsed >= chrono::Duration::days(i64::from(grace_days)) {
        GraceVerdict::Delete
    } else {
        GraceVerdict::Wait
    }
}

/// How long a data directory must have existed before the orphan sweep may
/// delete it. `register` creates the directory and then saves `registry.json`;
/// a sweep landing between the two would delete a live directory. Seeding has
/// the same window.
const ORPHAN_DIR_MIN_AGE: Duration = Duration::from_secs(3600);

/// Whether a directory name is one this daemon generates, i.e. the 16-character
/// lowercase hex prefix that `RepoRegistry::path_hash` produces. Anything else
/// under `repos/` was put there by something other than us and is left alone.
fn is_repo_hash_dir_name(name: &str) -> bool {
    name.len() == 16
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The purely-decidable half of whether a `repos/` entry may be collected by
/// the orphan sweep: a hash-shaped name, not claimed by any registry entry, an
/// actual directory, and old enough to be past `register`'s create-then-save
/// window. Pure so every guard combination is testable without touching disk.
/// The remaining guards, a running job and the two-sighting count, need
/// `&self` and stay in `sweep_orphan_data_dirs_with_min_age`.
fn is_collectable_orphan_dir(
    name: &str,
    is_dir: bool,
    claimed: bool,
    age: Duration,
    min_age: Duration,
) -> bool {
    is_repo_hash_dir_name(name) && !claimed && is_dir && age >= min_age
}

/// Refuse to delete anything that is not strictly inside the managed repos
/// directory.
///
/// Containment is structural rather than conventional: the sweeps call
/// `remove_dir_all` with nobody watching, so a bug in how a path was derived must
/// not be able to reach outside the tree. `starts_with` compares whole
/// components, so a sibling sharing a name prefix does not pass; the equality
/// check stops `repos/` itself from being taken as its own child.
fn ensure_within_repos_dir(candidate: &Utf8Path, repos_dir: &Utf8Path) -> Result<()> {
    if candidate == repos_dir || !candidate.starts_with(repos_dir) {
        anyhow::bail!(
            "refusing to delete '{candidate}': it is outside the managed repo directory '{repos_dir}'"
        );
    }
    Ok(())
}

/// Total size of a directory tree, for reporting how much a deletion reclaimed.
/// Best-effort: unreadable entries are skipped rather than failing the sweep.
fn dir_size_bytes(dir: &Utf8Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(current.as_std_path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if let Ok(child) = Utf8PathBuf::from_path_buf(entry.path()) {
                    stack.push(child);
                }
            } else {
                total += meta.len();
            }
        }
    }
    total
}

pub struct SessionManager {
    pub standalone_config: Arc<StandaloneConfig>,
    pub registry: Arc<RepoRegistry>,
    embedder: Arc<SharedEmbedder>,
    /// Cross-encoder reranker shared across all repos (loads the ~600MB model
    /// once). `None` when `reranker_enabled` is false. Each repo's `Retriever`
    /// receives a clone of this handle.
    reranker: Option<Arc<dyn Reranker>>,
    /// Keyed by canonical repo path string.
    repos: DashMap<String, Arc<RepoRuntime>>,
    /// Per-key init locks to prevent TOCTOU races when two sessions init the same repo
    init_locks: DashMap<String, Arc<Mutex<()>>>,
    /// Separate locks for first-index readiness checks and job registration.
    initial_index_locks: DashMap<String, Arc<Mutex<()>>>,
    /// Tracks the last time each repo was accessed, for TTL-based eviction
    last_accessed: DashMap<String, Instant>,
    /// Seeded worktree indexes whose checkout was missing on the last prune sweep,
    /// keyed by canonical repo path. Deletion needs two consecutive sightings, so
    /// a checkout that is briefly unreachable is not destroyed. The `Instant` is
    /// diagnostic only. Non-seeded entries use the persisted `missing_since`
    /// stamp instead; nothing writes both for the same repo.
    seeded_absent_once: DashMap<String, Instant>,
    /// Data directories under `repos/` that no registry entry claimed on the last
    /// sweep, keyed by directory name. Like `seeded_absent_once`, deletion needs
    /// two consecutive sightings.
    orphan_dir_seen_once: DashMap<String, Instant>,
    /// Repositories awaiting a user decision about their first full index.
    pending_consent: DashMap<String, PendingConsent>,
    metrics: Arc<MetricsRegistry>,
    /// Shared handle for all background indexing jobs.
    job_registry: JobRegistry,
}

struct RepoRuntime {
    state: Arc<AppState>,
    watch_cancel: CancellationToken,
    watcher_started: AtomicBool,
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

impl SessionManager {
    pub async fn new(
        standalone_config: StandaloneConfig,
        registry: RepoRegistry,
        embedder: Arc<SharedEmbedder>,
        job_registry: Option<JobRegistry>,
        reranker: Option<Arc<dyn Reranker>>,
    ) -> Result<Self> {
        let metrics = Arc::new(MetricsRegistry::new().context("Failed to create MetricsRegistry")?);
        let job_registry = job_registry.unwrap_or_else(jobs::new_job_registry);

        Ok(Self {
            standalone_config: Arc::new(standalone_config),
            registry: Arc::new(registry),
            embedder,
            reranker,
            repos: DashMap::new(),
            init_locks: DashMap::new(),
            initial_index_locks: DashMap::new(),
            last_accessed: DashMap::new(),
            seeded_absent_once: DashMap::new(),
            orphan_dir_seen_once: DashMap::new(),
            pending_consent: DashMap::new(),
            metrics,
            job_registry,
        })
    }

    pub fn job_registry(&self) -> JobRegistry {
        self.job_registry.clone()
    }

    #[cfg(test)]
    pub(crate) fn loaded_repo_count(&self) -> usize {
        self.repos.len()
    }

    pub async fn get_or_create_repo(&self, repo_path: &Utf8PathBuf) -> Result<Arc<AppState>> {
        Ok(self.get_or_create_runtime(repo_path).await?.state.clone())
    }

    async fn get_or_create_runtime(&self, repo_path: &Utf8PathBuf) -> Result<Arc<RepoRuntime>> {
        let repo_path = crate::path::canonicalize_existing_dir(repo_path)
            .context("Failed to canonicalize repository path")?;
        let canonical = repo_path.as_str().to_string();

        // Fast path: check if already exists (no lock needed)
        if let Some(entry) = self.repos.get(&canonical) {
            let _ = self.registry.touch(&canonical);
            self.last_accessed.insert(canonical, Instant::now());
            return Ok(entry.value().clone());
        }

        // Slow path: acquire per-key init lock to prevent TOCTOU race
        // (two sessions binding to the same repo simultaneously)
        let lock = self
            .init_locks
            .entry(canonical.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-check after acquiring lock (another task may have initialized it)
        if let Some(entry) = self.repos.get(&canonical) {
            let _ = self.registry.touch(&canonical);
            self.last_accessed.insert(canonical, Instant::now());
            return Ok(entry.value().clone());
        }

        let repo_entry = self
            .registry
            .register(&canonical)
            .context("Failed to register repository")?;

        let (state, watch_cancel) = self
            .init_repo_state(repo_path.clone(), &repo_entry)
            .await
            .context("Failed to initialize repository state")?;

        let runtime = Arc::new(RepoRuntime {
            state: Arc::new(state),
            watch_cancel,
            watcher_started: AtomicBool::new(false),
        });
        self.repos.insert(canonical.clone(), runtime.clone());
        self.last_accessed.insert(canonical, Instant::now());

        Ok(runtime)
    }

    /// Record (or refresh) a repo awaiting a consent decision. Keyed by repo id
    /// so repeated tool calls on the same repo bump `occurrences`/`last_seen`
    /// instead of duplicating.
    pub fn record_pending(&self, repo_path: &crate::path::Utf8Path) {
        let repo_id = RepoRegistry::path_hash(repo_path.as_str());
        let now = now_unix_s();
        self.pending_consent
            .entry(repo_id.clone())
            .and_modify(|p| {
                p.last_seen_unix_s = now;
                p.occurrences = p.occurrences.saturating_add(1);
            })
            .or_insert_with(|| {
                let class = crate::server::project_check::classify_repo(repo_path);
                PendingConsent {
                    repo_path: repo_path.as_str().to_string(),
                    repo_id,
                    detected: class.kind().to_string(),
                    recommendation: class.recommendation(),
                    detail: class.detail(),
                    first_seen_unix_s: now,
                    last_seen_unix_s: now,
                    occurrences: 1,
                }
            });
    }

    /// Snapshot of pending repos, oldest first.
    pub fn list_pending(&self) -> Vec<PendingConsent> {
        let mut v: Vec<PendingConsent> = self
            .pending_consent
            .iter()
            .map(|e| e.value().clone())
            .collect();
        v.sort_by_key(|p| p.first_seen_unix_s);
        v
    }

    /// Whether a repo id is currently pending.
    pub fn is_pending(&self, repo_id: &str) -> bool {
        self.pending_consent.contains_key(repo_id)
    }

    /// Drop a pending entry once the user has resolved it.
    pub fn clear_pending(&self, repo_id: &str) {
        self.pending_consent.remove(repo_id);
    }

    /// Evict repos that have not been accessed within `warm_ttl_seconds`.
    ///
    /// A TTL of `0` is treated as "never evict" (infinite lifetime).
    pub async fn evict_idle_repos(&self) {
        // Opportunistically drop decline records for ephemeral repos (worktrees,
        // temp copies) whose paths have since been deleted.
        if let Err(e) = self.registry.prune_declined_missing() {
            tracing::debug!(error = %e, "prune_declined_missing failed");
        }

        // Deliberately before the TTL early return below: that gate exists so a
        // deployment configured never to evict a warm repo also never deletes
        // that repo's live index. An unclaimed data dir belongs to no registered
        // repo, so that rationale does not apply to it, and `warm_ttl_seconds =
        // 0` must not silently disable orphan collection forever.
        self.sweep_orphan_data_dirs();

        let ttl_secs = self.standalone_config.warm_ttl_seconds;
        if ttl_secs == 0 {
            return;
        }

        // Deliberately after the TTL early return: a deployment configured never
        // to evict a warm repo must never delete one of its indexes either.
        self.prune_vanished_indexes().await;

        let ttl = Duration::from_secs(ttl_secs);

        // Collect keys to evict without holding DashMap shard locks during async work.
        let to_evict: Vec<String> = self
            .last_accessed
            .iter()
            .filter(|entry| {
                if entry.value().elapsed() <= ttl {
                    return false;
                }

                let repo_id = RepoRegistry::path_hash(entry.key());
                jobs::most_recent_running_for_repo(&self.job_registry, &repo_id).is_none()
            })
            .map(|entry| entry.key().clone())
            .collect();

        for key in to_evict {
            // Cancel the watcher before dropping the AppState.
            if let Some((_, runtime)) = self.repos.remove(&key) {
                runtime.watch_cancel.cancel();
            }
            let repo_id = RepoRegistry::path_hash(&key);
            crate::server::jobs::mark_running_watch_jobs_for_repo_failed(
                &self.job_registry,
                &repo_id,
                "repo evicted while watch job was running".to_string(),
            );
            self.last_accessed.remove(&key);
            self.init_locks.remove(&key);

            tracing::info!(
                repo = %key,
                ttl_secs,
                "Evicted idle repo from session cache"
            );
        }
    }

    /// Delete indexes whose repository folder is gone.
    ///
    /// Two policies, split by what a mistake costs. A seeded worktree index is
    /// seconds to rebuild, so it goes after two consecutive sweeps find the
    /// checkout absent. A full index is a complete GPU pass, so it waits out
    /// `missing_repo_grace_days` measured from a stamp persisted in
    /// `registry.json`, which survives daemon restarts.
    ///
    /// Guards shared by both paths:
    ///
    /// - `absence_is_inconclusive` skips paths whose volume is simply not
    ///   mounted, so an ejected drive costs nothing;
    /// - an entry with a running job is skipped, because its `SqliteStore` and
    ///   Tantivy writer are live and would rebuild a partial data dir right after
    ///   the removal, leaving artifacts that block this repo from being seeded
    ///   again;
    /// - `delete_repo_by_hash` refuses any data dir outside the managed tree.
    ///
    /// The seeded path's absence observations live in memory only. A restart
    /// forgets them, which costs one extra sweep before a dead worktree is
    /// collected.
    async fn prune_vanished_indexes(&self) {
        if let Err(error) = self.registry.clear_missing_since_for_present_paths() {
            tracing::debug!(%error, "clearing missing_since stamps failed");
        }

        let missing = match self.registry.list_missing() {
            Ok(missing) => missing,
            Err(error) => {
                tracing::debug!(%error, "list_missing failed");
                return;
            }
        };

        let actionable: Vec<RepoEntry> = missing
            .into_iter()
            .filter(|entry| !absence_is_inconclusive(&entry.path))
            .collect();

        // Forget observations for checkouts that came back, for entries that are
        // no longer registered at all, and for volumes that went away.
        let still_missing: std::collections::HashSet<&str> =
            actionable.iter().map(|entry| entry.path.as_str()).collect();
        self.seeded_absent_once
            .retain(|path, _| still_missing.contains(path.as_str()));

        let now = chrono::Utc::now();
        let grace_days = self.standalone_config.missing_repo_grace_days;

        for entry in &actionable {
            let hash = RepoRegistry::path_hash(&entry.path);
            if let Some(job) = jobs::most_recent_running_for_repo(&self.job_registry, &hash) {
                tracing::debug!(
                    repo = %entry.path,
                    job = %job.id,
                    "not pruning an index while a job is running against it"
                );
                continue;
            }

            let doomed = if entry.seeded_from.is_some() {
                // First sighting only arms the deletion; the next sweep performs it.
                let armed = self
                    .seeded_absent_once
                    .insert(entry.path.clone(), Instant::now())
                    .is_some();
                if !armed {
                    tracing::debug!(
                        repo = %entry.path,
                        "seeded worktree checkout is missing, pruning it if it is still missing next sweep"
                    );
                }
                armed
            } else {
                match grace_verdict(entry.missing_since.as_deref(), now, grace_days) {
                    GraceVerdict::Stamp => {
                        let stamp = now.to_rfc3339();
                        match self.registry.stamp_missing_since(&entry.path, &stamp) {
                            Ok(_) => tracing::info!(
                                repo = %entry.path,
                                grace_days,
                                "repository folder is gone; its index is deleted if it stays gone"
                            ),
                            Err(error) => tracing::warn!(
                                repo = %entry.path,
                                %error,
                                "could not record when a repository folder went missing"
                            ),
                        }
                        false
                    }
                    GraceVerdict::Wait => false,
                    GraceVerdict::Delete => true,
                }
            };

            if !doomed {
                continue;
            }

            match self.delete_repo_by_hash(&hash).await {
                Ok(Some(_)) => {
                    self.seeded_absent_once.remove(&entry.path);
                    tracing::info!(
                        repo = %entry.path,
                        data_dir = %entry.data_dir,
                        seeded = entry.seeded_from.is_some(),
                        "deleted the index of a repository whose folder was removed"
                    );
                }
                Ok(None) => {
                    self.seeded_absent_once.remove(&entry.path);
                }
                Err(error) => tracing::warn!(
                    repo = %entry.path,
                    %error,
                    "failed to delete the index of a removed repository"
                ),
            }
        }
    }

    /// Delete data directories under `repos/` that no registry entry claims.
    ///
    /// These are invisible to the dashboard, which lists from `registry.json`, so
    /// nothing else will ever collect them. They come from a crash between
    /// `register`'s `create_dir_all` and its save, from discarded seeds, and from
    /// hand-edited registries.
    ///
    /// Ownership here is inferred from the *absence* of a claim, so anything
    /// that makes claims disappear (a deleted `registry.json`, an emptied one)
    /// is amplified into deletion. Two guards exist for exactly that: the
    /// sweep refuses to run at all when `registry.json` itself is missing,
    /// and refuses again when the registry claims nothing but `repos/` holds
    /// hash-shaped directories anyway.
    fn sweep_orphan_data_dirs(&self) {
        self.sweep_orphan_data_dirs_with_min_age(ORPHAN_DIR_MIN_AGE);
    }

    /// The body of [`sweep_orphan_data_dirs`], with the age guard injected so
    /// tests do not have to backdate directory timestamps.
    fn sweep_orphan_data_dirs_with_min_age(&self, min_age: Duration) {
        let repos_dir = self.registry.repos_dir().to_owned();

        // `RepoRegistry::load` returns an empty registry, not an error, when
        // `registry.json` does not exist (deliberate: a brand-new daemon has no
        // file yet). That is indistinguishable from "nothing is registered"
        // unless this sweep checks the file itself: without this guard,
        // `rm registry.json` (or a partial backup restore) makes every claim
        // vanish at once, and every real data directory on the machine looks
        // unclaimed however old and populated it is.
        if !self.registry.registry_path_exists() {
            if repos_dir.as_std_path().is_dir() {
                tracing::warn!(
                    dir = %repos_dir,
                    "registry.json is missing but the repos dir holds data; refusing \
                     to run the orphan sweep rather than treat everything in it as unclaimed"
                );
            }
            return;
        }

        let claimed: std::collections::HashSet<String> = match self.registry.list_all() {
            Ok(entries) => entries
                .iter()
                .map(|e| RepoRegistry::path_hash(&e.path))
                .collect(),
            Err(error) => {
                tracing::debug!(%error, "listing repos for the orphan sweep failed");
                return;
            }
        };

        let dirents: Vec<std::fs::DirEntry> = match std::fs::read_dir(repos_dir.as_std_path()) {
            Ok(entries) => entries.flatten().collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                tracing::debug!(%error, dir = %repos_dir, "reading the repos dir failed");
                return;
            }
        };

        // Belt-and-braces: an empty `claimed` set should mean "nothing is
        // registered", not "the registry lost track of what it owns". If the
        // registry says nobody owns anything but `repos/` still holds
        // hash-shaped directories, something is wrong with the registry
        // rather than with the directories, so refuse rather than collect
        // real data on a guess. The missing-file guard above already covers
        // the exact production repro; this covers the same failure mode by a
        // different, independent signal (e.g. a registry emptied rather than
        // deleted). A corrupt registry file already errors out of `load` and
        // is handled by the `Err` branch above.
        if claimed.is_empty()
            && dirents.iter().any(|d| {
                is_repo_hash_dir_name(&d.file_name().to_string_lossy())
                    && d.metadata().map(|m| m.is_dir()).unwrap_or(false)
            })
        {
            tracing::warn!(
                dir = %repos_dir,
                "no registered repo claims anything, but hash-shaped data directories \
                 exist under it; refusing to run the orphan sweep"
            );
            return;
        }

        let mut seen_now: std::collections::HashSet<String> = std::collections::HashSet::new();

        for dirent in dirents {
            let name = dirent.file_name().to_string_lossy().to_string();
            let Ok(meta) = dirent.metadata() else {
                continue;
            };
            // Unreadable mtime is treated the same as "too young": there is no
            // evidence the create-then-save window has closed.
            let Some(age) = meta.modified().ok().and_then(|m| m.elapsed().ok()) else {
                continue;
            };
            if !is_collectable_orphan_dir(
                &name,
                meta.is_dir(),
                claimed.contains(&name),
                age,
                min_age,
            ) {
                continue;
            }
            if jobs::most_recent_running_for_repo(&self.job_registry, &name).is_some() {
                tracing::debug!(
                    dir = %name,
                    "not collecting an unclaimed data dir while a job is running against it"
                );
                continue;
            }
            // I2: the job guard above covers an active indexing pass. It does
            // not cover a warm runtime sitting idle with open SQLite, Tantivy,
            // and LanceDB handles and a file watcher: nothing else checks that
            // before this sweep runs. Deleting the directory under a live
            // runtime corrupts it immediately, and a repo can go warm without
            // ever starting a job (e.g. plain reads).
            if self
                .repos
                .iter()
                .any(|r| RepoRegistry::path_hash(r.key()) == name)
            {
                tracing::debug!(
                    dir = %name,
                    "not collecting an unclaimed data dir while its repo is loaded in memory"
                );
                continue;
            }

            seen_now.insert(name.clone());

            // First sighting only arms the deletion; the next sweep performs it.
            if self
                .orphan_dir_seen_once
                .insert(name.clone(), Instant::now())
                .is_none()
            {
                tracing::debug!(
                    dir = %name,
                    "no registry entry claims this data dir, collecting it if that holds next sweep"
                );
                continue;
            }

            let path = repos_dir.join(&name);
            if let Err(error) = ensure_within_repos_dir(&path, &repos_dir) {
                tracing::warn!(%error, "refusing to collect an unclaimed data dir");
                continue;
            }
            let reclaimed = dir_size_bytes(&path);
            match std::fs::remove_dir_all(path.as_std_path()) {
                Ok(()) => {
                    self.orphan_dir_seen_once.remove(&name);
                    seen_now.remove(&name);
                    tracing::info!(
                        dir = %path,
                        reclaimed_bytes = reclaimed,
                        "collected a data dir no registry entry claimed"
                    );
                }
                Err(error) => tracing::warn!(
                    dir = %path,
                    %error,
                    "could not collect an unclaimed data dir"
                ),
            }
        }

        // A directory that stopped qualifying (claimed again, job started) has to
        // restart its two-sighting count.
        self.orphan_dir_seen_once
            .retain(|name, _| seen_now.contains(name.as_str()));
    }

    /// Spawn a background task that calls [`evict_idle_repos`] every 60 seconds.
    ///
    /// The loop runs for the lifetime of the process — no cancellation is provided
    /// because the process owns the SessionManager and both die together.
    pub fn spawn_eviction_loop(self: &Arc<Self>) {
        let session = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                session.evict_idle_repos().await;
            }
        });
    }

    async fn init_repo_state(
        &self,
        repo_path: Utf8PathBuf,
        entry: &RepoEntry,
    ) -> Result<(AppState, CancellationToken)> {
        // Build per-repo config
        let config = self
            .standalone_config
            .repo_config(repo_path, &entry.data_dir);
        let config_arc = Arc::new(config);

        // Create storage directories
        std::fs::create_dir_all(&entry.data_dir).context("Failed to create repo data directory")?;
        if let Some(db_parent) = config_arc.db_path.parent() {
            std::fs::create_dir_all(db_parent).context("Failed to create db parent directory")?;
        }
        std::fs::create_dir_all(&config_arc.vector_db_path)
            .context("Failed to create vector db directory")?;
        std::fs::create_dir_all(&config_arc.tantivy_index_path)
            .context("Failed to create tantivy index directory")?;

        // Open SQLite and initialize schema
        let sqlite =
            SqliteStore::open(&config_arc.db_path).context("Failed to open SQLite store")?;
        sqlite
            .init()
            .context("Failed to initialize SQLite schema")?;
        let sqlite_arc = Arc::new(sqlite);

        // Open Tantivy index
        let tantivy = TantivyIndex::open_or_create(&config_arc.tantivy_index_path)
            .context("Failed to open Tantivy index")?;
        let tantivy_arc = Arc::new(tantivy);

        // Get embedder dimension. `SharedEmbedder::dim()` is cached at
        // construction so this requires no lock and no async work.
        let embedding_dim = self.embedder.dim();

        // Connect LanceDB, migrate if embedding dimension changed, then open table
        let lancedb = LanceDbStore::connect(&config_arc.vector_db_path)
            .await
            .context("Failed to connect to LanceDB")?;
        let _migrated = lancedb
            .migrate_vector_table("symbols", embedding_dim)
            .await
            .context("Failed to migrate vector table")?;
        let vectors = lancedb
            .open_or_create_table("symbols", embedding_dim)
            .await
            .context("Failed to open or create LanceDB table")?;
        let vectors_arc = Arc::new(vectors);

        // Create IndexPipeline
        let indexer = IndexPipeline::new_with_jobs(
            config_arc.clone(),
            sqlite_arc.clone(),
            tantivy_arc.clone(),
            vectors_arc.clone(),
            self.embedder.clone(),
            self.metrics.clone(),
            Some(self.job_registry.clone()),
        );

        // Create Retriever. The reranker (if enabled) is shared across all
        // repos; hyde is still unwired.
        let retriever = Retriever::new(
            config_arc.clone(),
            sqlite_arc.clone(),
            tantivy_arc,
            vectors_arc,
            self.embedder.clone(),
            self.reranker.clone(),
            None, // hyde_generator
            self.metrics.clone(),
        );

        let state = AppState {
            config: config_arc,
            indexer,
            retriever,
            sqlite: sqlite_arc,
            mcp_runtime: std::sync::Arc::new(once_cell::sync::OnceCell::new()),
            answer_generator: std::sync::Arc::new(once_cell::sync::OnceCell::new()),
            ask_code_cache: std::sync::Arc::new(Default::default()),
        };

        // The first-index coordinator starts the watcher only after a
        // successful native full index.
        let watch_cancel = CancellationToken::new();

        // Spawn the LLM description worker (gated by BENCH_DISABLE_DESCRIPTIONS env).
        // The worker pulls undescribed symbols from SQLite, runs Qwen to generate
        // descriptions, and re-upserts symbols into Tantivy with the description
        // field populated. Pre-v4 stdio mode wired this in run_embedded(); the v4
        // refactor (4736a0d) dropped the call site. Re-adding here so the daemon
        // exercises descriptions by default; bench arms set the env to ablate.
        let descriptions_disabled =
            std::env::var("BENCH_DISABLE_DESCRIPTIONS").as_deref() == Ok("1");
        if should_spawn_description_worker(
            state.config.descriptions_enabled,
            state.config.llm_enabled,
            descriptions_disabled,
        ) {
            let llm_config = state.config.clone();
            let llm_indexer = state.indexer.clone();
            let desc_cancel = watch_cancel.clone();
            tokio::spawn(async move {
                let generator = match tokio::task::spawn_blocking(move || {
                    crate::llm::create_llm_generator(&llm_config)
                })
                .await
                {
                    Ok(Ok(Some(llm))) => llm,
                    Ok(Ok(None)) => {
                        tracing::debug!(
                            "LLM descriptions unavailable, skipping description worker"
                        );
                        return;
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("Failed to create LLM generator: {}", e);
                        return;
                    }
                    Err(e) => {
                        tracing::warn!("LLM generator task panicked: {}", e);
                        return;
                    }
                };
                let _desc_handle = llm_indexer.spawn_description_worker(generator, desc_cancel);
                tracing::info!("LLM description worker spawned");
            });
        } else {
            tracing::info!(
                descriptions_enabled = state.config.descriptions_enabled,
                bench_disabled = descriptions_disabled,
                "LLM description worker not spawned (descriptions off by default; \
                 set DESCRIPTIONS_ENABLED=1 to generate them)"
            );
        }

        Ok((state, watch_cancel))
    }

    /// Delete a registered repository: cancel its watcher, drop the in-memory
    /// AppState, unregister from `registry.json`, and remove the on-disk
    /// data directory (`~/.code-intelligence/repos/<hash>/`).
    ///
    /// Returns the removed registry entry on success. Returns `Ok(None)` if
    /// the hash is unknown.
    ///
    /// The operation is best-effort: storage handles are released before
    /// the data directory is removed, but a failure to delete the directory
    /// (e.g. permission error) does NOT roll back the registry change.
    /// The caller sees the error and can retry the directory removal
    /// manually; the registry entry stays gone so the daemon does not
    /// attempt to reopen a half-deleted repo.
    pub async fn delete_repo_by_hash(&self, hash: &str) -> Result<Option<RepoEntry>> {
        let entry = match self.registry.get_by_hash(hash)? {
            Some(e) => e,
            None => return Ok(None),
        };

        let canonical = entry.path.clone();

        // 1. Cancel watcher and drop in-memory AppState (releases SQLite,
        //    Tantivy, LanceDB handles before we rmtree the dir).
        if let Some((_, runtime)) = self.repos.remove(&canonical) {
            runtime.watch_cancel.cancel();
        }
        crate::server::jobs::mark_running_watch_jobs_for_repo_failed(
            &self.job_registry,
            hash,
            "repo deleted while watch job was running".to_string(),
        );
        self.last_accessed.remove(&canonical);
        self.init_locks.remove(&canonical);
        self.initial_index_locks.remove(&canonical);

        // 2. Remove on-disk data directory. Missing dirs are treated as
        //    success; permission errors propagate and leave the registry entry
        //    intact so the user can retry from the API/UI.
        let repos_dir = self.registry.repos_dir();
        ensure_within_repos_dir(&entry.data_dir, repos_dir)?;
        let data_dir = entry.data_dir.as_std_path();
        match std::fs::remove_dir_all(data_dir) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("Failed to remove repo data directory: {}", entry.data_dir)
                });
            }
        }

        // 3. Remove from registry.json after the destructive filesystem step
        //    succeeds. This avoids orphaning a registered repo when deletion
        //    fails.
        let removed = self
            .registry
            .remove_by_hash(hash)
            .context("Failed to remove repo from registry")?;

        tracing::info!(
            repo = %canonical,
            hash = %hash,
            data_dir = %entry.data_dir,
            "Deleted repo index"
        );

        Ok(removed)
    }

    /// Resolve a cross-repo symbol synchronously by looking up the target repo's AppState.
    ///
    /// This is a blocking helper used by the `CrossRepoResolver` trait implementation.
    /// It accesses the DashMap directly (no async init), so the target repo must already
    /// be initialized. Returns None if the repo is not currently loaded.
    fn resolve_symbol_in_loaded_repo(
        &self,
        to_repo_hash: &str,
        to_symbol_name: &str,
        to_symbol_file: Option<&str>,
    ) -> anyhow::Result<
        Option<(
            std::sync::Arc<crate::storage::sqlite::SqliteStore>,
            crate::storage::sqlite::SymbolRow,
        )>,
    > {
        // Look up the repo entry by hash to get its canonical path
        let entry = self.registry.get_by_hash(to_repo_hash)?;
        let entry = match entry {
            Some(e) => e,
            None => return Ok(None),
        };

        // Check if the repo is currently loaded
        let state = match self.repos.get(&entry.path) {
            Some(runtime) => runtime.state.clone(),
            None => return Ok(None),
        };

        // Search for the symbol by name, optionally scoped to file
        let symbols =
            state
                .sqlite
                .search_symbols_by_exact_name(to_symbol_name, to_symbol_file, 1)?;

        match symbols.into_iter().next() {
            Some(sym) => Ok(Some((state.sqlite.clone(), sym))),
            None => Ok(None),
        }
    }

    #[cfg(test)]
    pub async fn new_for_test(data_dir: Utf8PathBuf) -> Self {
        use crate::config::EmbeddingsBackend;
        use crate::embeddings::hash::HashEmbedder;

        let standalone_config = StandaloneConfig {
            embeddings_backend: EmbeddingsBackend::Hash,
            hash_embedding_dim: 64,
            ..StandaloneConfig::default()
        };

        let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));

        let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));

        Self::new(standalone_config, registry, embedder, None, None)
            .await
            .expect("Failed to create test SessionManager")
    }

    /// Build a test SessionManager with a custom TTL.
    #[cfg(test)]
    pub async fn new_for_test_with_ttl(data_dir: Utf8PathBuf, warm_ttl_seconds: u64) -> Self {
        use crate::config::EmbeddingsBackend;
        use crate::embeddings::hash::HashEmbedder;

        let standalone_config = StandaloneConfig {
            embeddings_backend: EmbeddingsBackend::Hash,
            hash_embedding_dim: 64,
            warm_ttl_seconds,
            ..StandaloneConfig::default()
        };

        let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));

        let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));

        Self::new(standalone_config, registry, embedder, None, None)
            .await
            .expect("Failed to create test SessionManager")
    }

    /// Build a test SessionManager with a custom missing-repo grace period.
    #[cfg(test)]
    pub async fn new_for_test_with_grace_days(
        data_dir: Utf8PathBuf,
        missing_repo_grace_days: u32,
    ) -> Self {
        use crate::config::EmbeddingsBackend;
        use crate::embeddings::hash::HashEmbedder;

        let standalone_config = StandaloneConfig {
            embeddings_backend: EmbeddingsBackend::Hash,
            hash_embedding_dim: 64,
            missing_repo_grace_days,
            ..StandaloneConfig::default()
        };

        let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));

        let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));

        Self::new(standalone_config, registry, embedder, None, None)
            .await
            .expect("Failed to create test SessionManager")
    }

    /// Build a test SessionManager whose `registry.json` lives *inside*
    /// `repos/`, matching production (`src/main.rs`): every other
    /// `new_for_test*` helper puts it as a sibling of `repos/`, which cannot
    /// reproduce `rm ~/.code-intelligence/repos/registry.json` faithfully.
    #[cfg(test)]
    pub async fn new_for_test_with_production_layout(data_dir: Utf8PathBuf) -> Self {
        use crate::config::EmbeddingsBackend;
        use crate::embeddings::hash::HashEmbedder;

        let standalone_config = StandaloneConfig {
            embeddings_backend: EmbeddingsBackend::Hash,
            hash_embedding_dim: 64,
            ..StandaloneConfig::default()
        };

        let repos_dir = data_dir.join("repos");
        let registry = RepoRegistry::new(repos_dir.join("registry.json"), repos_dir);

        let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));

        Self::new(standalone_config, registry, embedder, None, None)
            .await
            .expect("Failed to create test SessionManager")
    }
}

impl crate::graph::CrossRepoResolver for SessionManager {
    fn resolve_cross_repo_symbol(
        &self,
        to_repo_hash: &str,
        to_symbol_name: &str,
        to_symbol_file: Option<&str>,
    ) -> anyhow::Result<
        Option<(
            std::sync::Arc<crate::storage::sqlite::SqliteStore>,
            crate::storage::sqlite::SymbolRow,
        )>,
    > {
        self.resolve_symbol_in_loaded_repo(to_repo_hash, to_symbol_name, to_symbol_file)
    }

    fn list_cross_repo_edges_from(
        &self,
        sqlite: &crate::storage::sqlite::SqliteStore,
        from_symbol_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<crate::storage::sqlite::CrossRepoEdgeRow>> {
        sqlite.list_cross_repo_edges_from(from_symbol_id, limit)
    }

    fn repo_name_for_hash(&self, repo_hash: &str) -> anyhow::Result<Option<String>> {
        let entry = self.registry.get_by_hash(repo_hash)?;
        Ok(entry.map(|e| e.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── helpers ────────────────────────────────────────────────────────────────

    /// Create a temporary directory that persists until the returned `TempDir` is dropped.
    fn temp_data_dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        (dir, path)
    }

    fn temp_repo_dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        (dir, path)
    }

    fn canonical_key(path: &Utf8PathBuf) -> String {
        crate::path::canonicalize_existing_dir(path)
            .unwrap()
            .to_string()
    }

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

    #[tokio::test]
    async fn unindexed_repo_requires_approval_even_when_explicitly_selected() {
        let (_data, data_dir) = temp_data_dir();
        let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
        let (_repo, repo_path) = temp_repo_dir();

        let access = manager.resolve_repo(repo_path.as_path()).await.unwrap();
        assert!(matches!(access, RepoAccess::NeedsApproval));
        assert_eq!(manager.loaded_repo_count(), 0);
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
        assert!(matches!(access, RepoAccess::Indexing { started: true, .. }));
    }

    #[tokio::test]
    async fn failed_initial_job_retries_without_another_prompt() {
        let (_data, data_dir) = temp_data_dir();
        let manager = Arc::new(SessionManager::new_for_test(data_dir).await);
        let (_repo, repo_path) = temp_repo_dir();
        let canonical = crate::path::canonicalize_existing_dir(&repo_path).unwrap();
        let repo_id = RepoRegistry::path_hash(canonical.as_str());
        manager
            .registry
            .approve_initial_index(canonical.as_str())
            .unwrap();
        crate::server::jobs::register_running(
            &manager.job_registry,
            "failed-initial".to_string(),
            crate::server::jobs::JobKind::InitialBind,
            repo_id,
            canonical.to_string(),
        );
        crate::server::jobs::mark_failed(
            &manager.job_registry,
            "failed-initial",
            "intentional failure".to_string(),
        );

        let access = manager.resolve_repo(canonical.as_path()).await.unwrap();
        match access {
            RepoAccess::Indexing { job, started: true } => {
                assert_ne!(job.id, "failed-initial");
            }
            _ => panic!("persisted approval must start a replacement job"),
        }
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
        assert_eq!(manager.loaded_repo_count(), 0);
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

    #[tokio::test]
    async fn disabled_consent_gate_auto_authorizes_but_still_starts_index() {
        use crate::config::EmbeddingsBackend;
        use crate::embeddings::hash::HashEmbedder;

        let (_data, data_dir) = temp_data_dir();
        let registry = RepoRegistry::new(data_dir.join("registry.json"), data_dir.join("repos"));
        let config = StandaloneConfig {
            data_dir: data_dir.clone(),
            embeddings_backend: EmbeddingsBackend::Hash,
            hash_embedding_dim: 64,
            index_consent_required: false,
            ..StandaloneConfig::default()
        };
        let embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(64))));
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

    // ─── existing tests ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_or_create_returns_same_state_for_same_repo() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;

        let (_repo, repo_path) = temp_repo_dir();

        let state1 = manager.get_or_create_repo(&repo_path).await.unwrap();
        let state2 = manager.get_or_create_repo(&repo_path).await.unwrap();

        // Same Arc — not recreated
        assert!(Arc::ptr_eq(&state1.config, &state2.config));
    }

    // ─── last_accessed tracking ──────────────────────────────────────────────────

    #[tokio::test]
    async fn last_accessed_populated_after_get_or_create() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (_repo, repo_path) = temp_repo_dir();

        assert!(
            manager.last_accessed.is_empty(),
            "no entries before first access"
        );

        manager.get_or_create_repo(&repo_path).await.unwrap();

        let key = canonical_key(&repo_path);
        assert!(
            manager.last_accessed.contains_key(&key),
            "last_accessed should have an entry after get_or_create"
        );
    }

    #[tokio::test]
    async fn last_accessed_updated_on_fast_path() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (_repo, repo_path) = temp_repo_dir();

        // First call — slow path (initialisation)
        manager.get_or_create_repo(&repo_path).await.unwrap();

        let key = canonical_key(&repo_path);
        let first_ts = *manager.last_accessed.get(&key).unwrap();

        // Sleep briefly so the two Instants differ
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Second call — fast path (repo already in map)
        manager.get_or_create_repo(&repo_path).await.unwrap();

        let second_ts = *manager.last_accessed.get(&key).unwrap();
        assert!(
            second_ts >= first_ts,
            "last_accessed should be refreshed on fast-path hit"
        );
    }

    #[tokio::test]
    async fn record_pending_tracks_and_dedupes_by_repo_id() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir =
            crate::path::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("utf8 path");
        let sm = SessionManager::new_for_test(data_dir).await;

        // A plain absolute path (not under TMPDIR, not a worktree) classifies as standard.
        let p = crate::path::Utf8Path::new("/Users/dev/proj");
        sm.record_pending(p);
        sm.record_pending(p);

        let pending = sm.list_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].repo_path, "/Users/dev/proj");
        assert_eq!(pending[0].detected, "standard");
        assert_eq!(pending[0].occurrences, 2);
        assert!(sm.is_pending(&pending[0].repo_id));

        sm.clear_pending(&pending[0].repo_id);
        assert_eq!(sm.list_pending().len(), 0);
        assert!(!sm.is_pending(&pending[0].repo_id));
    }

    #[tokio::test]
    async fn list_pending_is_sorted_by_first_seen() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir =
            crate::path::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("utf8 path");
        let sm = SessionManager::new_for_test(data_dir).await;
        sm.record_pending(crate::path::Utf8Path::new("/Users/dev/a"));
        sm.record_pending(crate::path::Utf8Path::new("/Users/dev/b"));
        let pending = sm.list_pending();
        assert_eq!(pending.len(), 2);
        assert!(pending[0].first_seen_unix_s <= pending[1].first_seen_unix_s);
    }

    // ─── eviction ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn evict_idle_repos_removes_expired_entries() {
        let (_data, data_dir) = temp_data_dir();
        // TTL of 1 second
        let manager = SessionManager::new_for_test_with_ttl(data_dir, 1).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);

        // Overwrite last_accessed with a timestamp far in the past
        manager
            .last_accessed
            .insert(key.clone(), Instant::now() - Duration::from_secs(10));

        manager.evict_idle_repos().await;

        assert!(
            !manager.repos.contains_key(&key),
            "expired repo should be evicted from repos"
        );
        assert!(
            !manager.last_accessed.contains_key(&key),
            "expired repo should be evicted from last_accessed"
        );
        assert!(
            !manager.init_locks.contains_key(&key),
            "expired repo should be evicted from init_locks"
        );
    }

    #[tokio::test]
    async fn evict_idle_repos_preserves_recently_accessed() {
        let (_data, data_dir) = temp_data_dir();
        // TTL of 300 seconds (default)
        let manager = SessionManager::new_for_test_with_ttl(data_dir, 300).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);

        // last_accessed is "just now" — should NOT be evicted
        manager.evict_idle_repos().await;

        assert!(
            manager.repos.contains_key(&key),
            "recently accessed repo must NOT be evicted"
        );
        assert!(
            manager.last_accessed.contains_key(&key),
            "last_accessed entry must survive for recent repos"
        );
    }

    #[tokio::test]
    async fn evict_idle_repos_preserves_repo_with_running_initial_index() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test_with_ttl(data_dir, 1).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);
        let repo_id = RepoRegistry::path_hash(&key);
        let job = crate::server::jobs::register_running(
            &manager.job_registry,
            "long-first-index".to_string(),
            crate::server::jobs::JobKind::InitialBind,
            repo_id,
            key.clone(),
        );
        manager
            .last_accessed
            .insert(key.clone(), Instant::now() - Duration::from_secs(10));

        manager.evict_idle_repos().await;

        assert!(
            manager.repos.contains_key(&key),
            "a repo with an active first index must stay loaded"
        );
        assert_eq!(
            manager.job_registry.get(&job.id).unwrap().status,
            crate::server::jobs::JobStatus::Running,
            "cache eviction must not fail an active first-index job"
        );
    }

    #[tokio::test]
    async fn delete_repo_by_hash_removes_state_registry_and_dir() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);
        let hash = crate::registry::RepoRegistry::path_hash(&key);

        // Capture the data dir BEFORE delete so we can check it was rmtree'd
        let entry = manager.registry.get_by_hash(&hash).unwrap().unwrap();
        let data_dir = entry.data_dir.clone();
        assert!(
            data_dir.as_std_path().exists(),
            "data dir should exist after init"
        );

        let removed = manager.delete_repo_by_hash(&hash).await.unwrap();
        assert!(removed.is_some(), "delete should return the removed entry");

        // In-memory state cleared
        assert!(!manager.repos.contains_key(&key));
        assert!(!manager.last_accessed.contains_key(&key));
        assert!(!manager.init_locks.contains_key(&key));

        // Registry no longer knows the repo
        assert!(manager.registry.get_by_hash(&hash).unwrap().is_none());

        // On-disk data directory is gone
        assert!(
            !data_dir.as_std_path().exists(),
            "data dir should be removed: {}",
            data_dir
        );
    }

    #[tokio::test]
    async fn delete_repo_by_hash_unknown_returns_none() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;

        let removed = manager
            .delete_repo_by_hash("0000000000000000")
            .await
            .unwrap();
        assert!(removed.is_none());
    }

    #[tokio::test]
    async fn delete_repo_by_hash_keeps_registry_when_data_dir_delete_fails() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (_repo, repo_path) = temp_repo_dir();

        let entry = manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
        std::fs::remove_dir_all(entry.data_dir.as_std_path()).unwrap();
        std::fs::write(entry.data_dir.as_std_path(), b"not a directory").unwrap();

        let err = manager.delete_repo_by_hash(&hash).await.unwrap_err();

        assert!(
            err.to_string()
                .contains("Failed to remove repo data directory"),
            "unexpected delete error: {err}"
        );
        assert!(
            manager.registry.get_by_hash(&hash).unwrap().is_some(),
            "registry entry must remain so a failed data-dir deletion can be retried"
        );
    }

    /// The prune sweep deletes data directories on its own, so a `data_dir` that
    /// somehow points outside the managed tree must be refused rather than
    /// rmtree'd. Simulated by editing the persisted entry, which is the only way
    /// such a path could arise.
    #[tokio::test]
    async fn delete_repo_by_hash_refuses_a_data_dir_outside_the_repos_dir() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir.clone()).await;
        let (_repo, repo_path) = temp_repo_dir();
        manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());

        let outside = data_dir.join("not-a-repo-data-dir");
        std::fs::create_dir_all(outside.as_std_path()).unwrap();
        std::fs::write(outside.join("precious").as_std_path(), b"keep me").unwrap();

        let registry_path = data_dir.join("registry.json");
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(registry_path.as_std_path()).unwrap()).unwrap();
        json["repos"][&hash]["data_dir"] = serde_json::Value::String(outside.to_string());
        std::fs::write(registry_path.as_std_path(), json.to_string()).unwrap();

        let err = manager.delete_repo_by_hash(&hash).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("outside the managed repo directory"),
            "unexpected error: {err}"
        );
        assert!(
            outside.join("precious").as_std_path().is_file(),
            "nothing outside the managed tree may be removed"
        );
        assert!(
            manager.registry.get_by_hash(&hash).unwrap().is_some(),
            "the entry must survive so the refusal is visible"
        );
    }

    #[tokio::test]
    async fn delete_repo_by_hash_works_when_not_loaded() {
        // Repo is registered but NOT currently in the session cache
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir.clone()).await;
        let (_repo, repo_path) = temp_repo_dir();

        // Register directly via registry — never call get_or_create_repo
        let entry = manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());

        assert!(entry.data_dir.as_std_path().exists());

        let removed = manager.delete_repo_by_hash(&hash).await.unwrap();
        assert!(removed.is_some());
        assert!(!entry.data_dir.as_std_path().exists());
        assert!(manager.registry.get_by_hash(&hash).unwrap().is_none());
    }

    #[tokio::test]
    async fn description_worker_skipped_when_bench_disable_descriptions_set() {
        // This test confirms init_repo_state does not panic when the env is set
        // and the worker-spawn branch is the disabled path. It's a weak test by
        // design - tokio::spawn is fire-and-forget. The strong assertion that
        // descriptions are not written lives in storage::tantivy tests.
        static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _guard = ENV_LOCK.lock().await;

        std::env::set_var("BENCH_DISABLE_DESCRIPTIONS", "1");

        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir.clone()).await;
        let repo_path = Utf8PathBuf::from(data_dir.as_str()).join("fake-repo");
        std::fs::create_dir_all(&repo_path).unwrap();
        std::fs::write(
            repo_path.join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.0.0\"\n",
        )
        .unwrap();

        let _state = manager.get_or_create_repo(&repo_path).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        std::env::remove_var("BENCH_DISABLE_DESCRIPTIONS");
    }

    #[test]
    fn description_worker_gate() {
        // Off by default: descriptions_enabled=false means no worker, even with
        // the LLM available.
        assert!(!should_spawn_description_worker(false, true, false));
        // Opted in, LLM available, not bench-disabled → spawn.
        assert!(should_spawn_description_worker(true, true, false));
        // Bench ablation overrides the opt-in.
        assert!(!should_spawn_description_worker(true, true, true));
        // No LLM backend → nothing to spawn.
        assert!(!should_spawn_description_worker(true, false, false));
    }

    #[test]
    fn absence_under_a_live_directory_is_conclusive() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let gone = root.join("deleted-repo");

        // The parent exists and the child does not: a real deletion.
        assert!(!absence_is_inconclusive(gone.as_str()));

        // A whole parent folder of worktrees being deleted still counts, because
        // its own parent (the tempdir) is alive.
        let nested = root.join("worktrees").join("feature");
        assert!(!absence_is_inconclusive(nested.as_str()));
    }

    #[test]
    fn absence_under_an_unmounted_volume_is_inconclusive() {
        // /Volumes exists on every macOS system; this volume does not. The
        // deepest existing ancestor is the mount table root, so the daemon has
        // no evidence that anything was deleted.
        assert!(absence_is_inconclusive(
            "/Volumes/definitely-not-mounted-9f3a/project"
        ));
    }

    #[test]
    fn absence_below_a_missing_top_level_directory_is_inconclusive() {
        // Deepest existing ancestor is "/". Nothing about this path was ever
        // reachable, so it is not evidence of a deletion either.
        assert!(absence_is_inconclusive("/no-such-top-level-9f3a/project"));
    }

    #[test]
    fn grace_verdict_covers_every_state() {
        use chrono::{Duration as ChronoDuration, Utc};
        let now = Utc::now();
        let long_ago = (now - ChronoDuration::days(10)).to_rfc3339();
        let recent = (now - ChronoDuration::days(2)).to_rfc3339();

        // No stamp yet: record one and wait.
        assert_eq!(grace_verdict(None, now, 7), GraceVerdict::Stamp);
        // Inside the window.
        assert_eq!(grace_verdict(Some(&recent), now, 7), GraceVerdict::Wait);
        // Past the window.
        assert_eq!(grace_verdict(Some(&long_ago), now, 7), GraceVerdict::Delete);
        // Exactly at the boundary counts as expired.
        let exactly = (now - ChronoDuration::days(7)).to_rfc3339();
        assert_eq!(grace_verdict(Some(&exactly), now, 7), GraceVerdict::Delete);
        // Grace disabled: stamp for the dashboard, never delete.
        assert_eq!(grace_verdict(None, now, 0), GraceVerdict::Stamp);
        assert_eq!(grace_verdict(Some(&long_ago), now, 0), GraceVerdict::Wait);
        // Corrupt stamp heals by being rewritten.
        assert_eq!(
            grace_verdict(Some("not a date"), now, 7),
            GraceVerdict::Stamp
        );
    }

    #[tokio::test]
    async fn sweep_stamps_a_vanished_repo_before_deleting_it() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (repo, repo_path) = temp_repo_dir();

        let entry = manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
        drop(repo); // deletes the checkout

        manager.evict_idle_repos().await;

        let stamped = manager.registry.get_by_hash(&hash).unwrap().unwrap();
        assert!(
            stamped.missing_since.is_some(),
            "the first sweep records when the path went missing"
        );
        assert!(
            entry.data_dir.as_std_path().exists(),
            "and deletes nothing yet"
        );
    }

    #[tokio::test]
    async fn sweep_deletes_a_vanished_repo_once_the_grace_period_expires() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (repo, repo_path) = temp_repo_dir();

        let entry = manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
        drop(repo);

        // Backdate the stamp past the 7-day default instead of waiting.
        let old = (chrono::Utc::now() - chrono::Duration::days(8)).to_rfc3339();
        manager
            .registry
            .stamp_missing_since(repo_path.as_str(), &old)
            .unwrap();

        manager.evict_idle_repos().await;

        assert!(manager.registry.get_by_hash(&hash).unwrap().is_none());
        assert!(!entry.data_dir.as_std_path().exists());
    }

    #[tokio::test]
    async fn sweep_clears_the_stamp_when_the_checkout_returns() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
        let old = (chrono::Utc::now() - chrono::Duration::days(3)).to_rfc3339();
        manager
            .registry
            .stamp_missing_since(repo_path.as_str(), &old)
            .unwrap();

        // The path exists throughout; the stamp is stale state from an earlier
        // absence.
        manager.evict_idle_repos().await;

        assert_eq!(
            manager
                .registry
                .get_by_hash(&hash)
                .unwrap()
                .unwrap()
                .missing_since,
            None
        );
    }

    #[tokio::test]
    async fn sweep_never_deletes_when_grace_is_disabled() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test_with_grace_days(data_dir, 0).await;
        let (repo, repo_path) = temp_repo_dir();

        let entry = manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
        drop(repo);

        let old = (chrono::Utc::now() - chrono::Duration::days(400)).to_rfc3339();
        manager
            .registry
            .stamp_missing_since(repo_path.as_str(), &old)
            .unwrap();

        manager.evict_idle_repos().await;

        assert!(
            manager.registry.get_by_hash(&hash).unwrap().is_some(),
            "grace_days=0 must never delete"
        );
        assert!(entry.data_dir.as_std_path().exists());
    }

    #[tokio::test]
    async fn sweep_spares_a_vanished_repo_while_a_job_runs_against_it() {
        use crate::server::jobs::{register_running, JobKind};

        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (repo, repo_path) = temp_repo_dir();

        let entry = manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
        drop(repo);

        let old = (chrono::Utc::now() - chrono::Duration::days(8)).to_rfc3339();
        manager
            .registry
            .stamp_missing_since(repo_path.as_str(), &old)
            .unwrap();

        // A live job holds the SQLite and Tantivy writers open; deleting the data
        // dir under it leaves artifacts that block this repo from being seeded.
        register_running(
            &manager.job_registry,
            "job-1".to_string(),
            JobKind::ManualReindex,
            hash.clone(),
            repo_path.to_string(),
        );

        manager.evict_idle_repos().await;

        assert!(manager.registry.get_by_hash(&hash).unwrap().is_some());
        assert!(entry.data_dir.as_std_path().exists());
    }

    #[tokio::test]
    async fn sweep_leaves_seeded_entries_on_the_two_sweep_rule() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        let (repo, repo_path) = temp_repo_dir();

        manager.registry.register(repo_path.as_str()).unwrap();
        manager
            .registry
            .mark_seeded_from(repo_path.as_str(), "basehash12345678")
            .unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
        drop(repo);

        // First sweep arms, second sweep deletes. No stamp is ever written.
        manager.evict_idle_repos().await;
        let armed = manager.registry.get_by_hash(&hash).unwrap().unwrap();
        assert!(
            armed.missing_since.is_none(),
            "seeded entries are not stamped"
        );

        manager.evict_idle_repos().await;
        assert!(manager.registry.get_by_hash(&hash).unwrap().is_none());
    }

    #[test]
    fn repo_hash_dir_names_are_sixteen_lowercase_hex_characters() {
        assert!(is_repo_hash_dir_name("083f007c6f2d63cc"));
        assert!(!is_repo_hash_dir_name("registry.json"));
        assert!(
            !is_repo_hash_dir_name("083F007C6F2D63CC"),
            "uppercase is not ours"
        );
        assert!(!is_repo_hash_dir_name("083f007c6f2d63c"), "15 chars");
        assert!(!is_repo_hash_dir_name("083f007c6f2d63ccx"), "17 chars");
        assert!(!is_repo_hash_dir_name("083f007c6f2d63cg"), "g is not hex");
    }

    #[test]
    fn is_collectable_orphan_dir_covers_every_purely_decidable_guard() {
        let old = Duration::from_secs(7200);
        let young = Duration::from_secs(1);
        let min_age = Duration::from_secs(3600);
        let hash_name = "0123456789abcdef";

        assert!(
            !is_collectable_orphan_dir(hash_name, true, true, old, min_age),
            "a claimed name is never collectable"
        );
        assert!(
            !is_collectable_orphan_dir(hash_name, false, false, old, min_age),
            "a non-directory is never collectable"
        );
        assert!(
            !is_collectable_orphan_dir("my-notes", true, false, old, min_age),
            "not 16 lowercase hex characters"
        );
        assert!(
            !is_collectable_orphan_dir("0123456789ABCDEF", true, false, old, min_age),
            "uppercase hex is not ours"
        );
        assert!(
            !is_collectable_orphan_dir(hash_name, true, false, young, min_age),
            "younger than the minimum age"
        );
        assert!(
            is_collectable_orphan_dir(hash_name, true, false, old, min_age),
            "a hash-shaped, unclaimed, old-enough directory is collectable"
        );
    }

    #[test]
    fn ensure_within_repos_dir_rejects_the_root_itself_and_outsiders() {
        let repos = Utf8PathBuf::from("/data/repos");
        assert!(ensure_within_repos_dir(&repos.join("abc"), &repos).is_ok());
        assert!(
            ensure_within_repos_dir(&repos, &repos).is_err(),
            "the repos dir itself must never be deleted"
        );
        assert!(ensure_within_repos_dir(&Utf8PathBuf::from("/data/other"), &repos).is_err());
        assert!(
            ensure_within_repos_dir(&Utf8PathBuf::from("/data/repos-backup"), &repos).is_err(),
            "a sibling sharing a name prefix is not inside"
        );
    }

    #[tokio::test]
    async fn orphan_sweep_deletes_an_unclaimed_data_dir_on_the_second_sighting() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir.clone()).await;
        // An unrelated, normally-registered repo: creates `registry.json` and
        // keeps `claimed` non-empty, so the C1 guards (missing/empty registry)
        // do not also explain the result this test is checking.
        let (_anchor_repo, anchor_path) = temp_repo_dir();
        manager.registry.register(anchor_path.as_str()).unwrap();
        let repos_dir = data_dir.join("repos");
        let orphan = repos_dir.join("0123456789abcdef");
        std::fs::create_dir_all(orphan.as_std_path()).unwrap();
        std::fs::write(orphan.join("code-intelligence.db").as_std_path(), b"stale").unwrap();

        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);
        assert!(
            orphan.as_std_path().exists(),
            "the first sighting only arms the deletion"
        );

        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);
        assert!(!orphan.as_std_path().exists());
    }

    #[tokio::test]
    async fn orphan_sweep_keeps_dirs_that_are_claimed_or_not_ours() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir.clone()).await;
        let (_repo, repo_path) = temp_repo_dir();

        // A live registry entry's data dir.
        let claimed = manager
            .registry
            .register(repo_path.as_str())
            .unwrap()
            .data_dir;

        let repos_dir = data_dir.join("repos");
        // Not a 16-hex name.
        let foreign = repos_dir.join("my-notes");
        std::fs::create_dir_all(foreign.as_std_path()).unwrap();
        // A hex-named *file* rather than a directory, which the is_dir guard
        // must skip. In production `registry.json` lives here too and is skipped
        // by the name check.
        let stray_file = repos_dir.join("aaaaaaaaaaaaaaaa");
        std::fs::write(stray_file.as_std_path(), b"not a data dir").unwrap();

        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);
        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);

        assert!(
            claimed.as_std_path().exists(),
            "a registered repo's dir stays"
        );
        assert!(foreign.as_std_path().exists(), "an unrecognised name stays");
        assert!(
            stray_file.as_std_path().is_file(),
            "a file is never removed"
        );
    }

    #[tokio::test]
    async fn orphan_sweep_spares_a_dir_while_a_job_runs_under_its_id() {
        use crate::server::jobs::{register_running, JobKind};

        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir.clone()).await;
        // An unrelated, normally-registered repo: creates `registry.json` and
        // keeps `claimed` non-empty, so the C1 guards (missing/empty registry)
        // do not also explain the result this test is checking.
        let (_anchor_repo, anchor_path) = temp_repo_dir();
        manager.registry.register(anchor_path.as_str()).unwrap();
        let repos_dir = data_dir.join("repos");
        let busy_hash = "00112233445566aa";
        let busy = repos_dir.join(busy_hash);
        std::fs::create_dir_all(busy.as_std_path()).unwrap();

        register_running(
            &manager.job_registry,
            "job-2".to_string(),
            JobKind::ManualReindex,
            busy_hash.to_string(),
            "/some/repo".to_string(),
        );

        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);
        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);

        assert!(busy.as_std_path().exists());
    }

    #[tokio::test]
    async fn orphan_sweep_spares_directories_younger_than_the_minimum_age() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir.clone()).await;
        // An unrelated, normally-registered repo: creates `registry.json` and
        // keeps `claimed` non-empty, so the C1 guards (missing/empty registry)
        // do not also explain the result this test is checking.
        let (_anchor_repo, anchor_path) = temp_repo_dir();
        manager.registry.register(anchor_path.as_str()).unwrap();
        let repos_dir = data_dir.join("repos");
        std::fs::create_dir_all(repos_dir.as_std_path()).unwrap();
        let fresh = repos_dir.join("fedcba9876543210");
        std::fs::create_dir_all(fresh.as_std_path()).unwrap();

        // The real entry point uses ORPHAN_DIR_MIN_AGE. A directory created
        // moments ago is inside register()'s create-then-save window.
        manager.sweep_orphan_data_dirs();
        manager.sweep_orphan_data_dirs();

        assert!(fresh.as_std_path().exists());
    }

    /// `warm_ttl_seconds = 0` means "never evict a warm repo"; it must not also
    /// mean "never collect an orphan data dir", since an unclaimed dir belongs
    /// to no registered repo and the TTL's rationale does not cover it. Pins
    /// the call to `sweep_orphan_data_dirs` above the TTL early return in
    /// `evict_idle_repos`. Goes through the real entry point rather than
    /// calling the sweep directly, since the placement is what is under test,
    /// so the directory's age is backdated on disk instead of injected.
    #[tokio::test]
    async fn evict_idle_repos_collects_orphan_dirs_even_when_ttl_is_zero() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test_with_ttl(data_dir.clone(), 0).await;
        // An unrelated, normally-registered repo: creates `registry.json` and
        // keeps `claimed` non-empty, so the C1 guards (missing/empty registry)
        // do not also explain the result this test is checking.
        let (_anchor_repo, anchor_path) = temp_repo_dir();
        manager.registry.register(anchor_path.as_str()).unwrap();
        let repos_dir = data_dir.join("repos");
        let orphan = repos_dir.join("1122334455667788");
        std::fs::create_dir_all(orphan.as_std_path()).unwrap();

        let two_hours_ago = std::time::SystemTime::now() - Duration::from_secs(7200);
        filetime::set_file_mtime(
            orphan.as_std_path(),
            filetime::FileTime::from_system_time(two_hours_ago),
        )
        .unwrap();

        manager.evict_idle_repos().await;
        assert!(
            orphan.as_std_path().exists(),
            "the first sighting only arms the deletion"
        );

        manager.evict_idle_repos().await;
        assert!(
            !orphan.as_std_path().exists(),
            "a TTL of zero must not disable orphan collection"
        );
    }

    #[tokio::test]
    async fn evict_idle_repos_prunes_declined_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let manager = SessionManager::new_for_test(data_dir).await;

        // A declined repo whose path does not exist.
        manager
            .registry
            .set_consent("/no/such/declined", crate::registry::IndexConsent::Declined)
            .unwrap();

        manager.evict_idle_repos().await;

        assert_eq!(
            manager
                .registry
                .consent_status("/no/such/declined")
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn evict_idle_repos_zero_ttl_never_evicts() {
        let (_data, data_dir) = temp_data_dir();
        // TTL of 0 means infinite — never evict
        let manager = SessionManager::new_for_test_with_ttl(data_dir, 0).await;
        let (_repo, repo_path) = temp_repo_dir();

        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);

        // Backdate to look very old
        manager
            .last_accessed
            .insert(key.clone(), Instant::now() - Duration::from_secs(86400));

        manager.evict_idle_repos().await;

        assert!(
            manager.repos.contains_key(&key),
            "TTL=0 must never evict any repo"
        );
    }

    // ─── C1: losing registry.json must not be read as "nothing is registered" ──

    /// `RepoRegistry::load` returns an empty registry, not an error, when
    /// `registry.json` is missing. Before the fix this made the orphan sweep
    /// treat every existing data directory as unclaimed, so losing the
    /// registry file (`rm registry.json`, or a partial backup restore)
    /// deleted every index on the machine within two sweeps, even though the
    /// repo checkout itself was untouched. Reproduces that exact scenario,
    /// using the production layout (`registry.json` inside `repos/`).
    #[tokio::test]
    async fn orphan_sweep_refuses_to_run_when_registry_json_is_missing() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test_with_production_layout(data_dir.clone()).await;
        let (_repo, repo_path) = temp_repo_dir();

        // A live, registered repo with a real, populated data directory.
        let entry = manager.registry.register(repo_path.as_str()).unwrap();
        std::fs::write(
            entry.data_dir.join("code-intelligence.db").as_std_path(),
            b"real index",
        )
        .unwrap();

        let registry_path = data_dir.join("repos").join("registry.json");
        assert!(
            registry_path.as_std_path().exists(),
            "sanity: register() must have created registry.json"
        );
        std::fs::remove_file(registry_path.as_std_path()).unwrap();

        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);
        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);

        assert!(
            entry.data_dir.as_std_path().exists(),
            "losing registry.json must never be read as \"nothing is \
             registered\"; a live, populated index must survive"
        );
    }

    /// Belt-and-braces: even when `registry.json` exists, an empty registry
    /// (no entries at all) is indistinguishable from "everything was just
    /// unregistered" by the guard above. If `repos/` still holds hash-shaped
    /// directories, that is independent evidence something is wrong, so the
    /// sweep refuses rather than trusting an empty registry that might itself
    /// be the bug (e.g. a backup restore that wrote a syntactically valid but
    /// empty file).
    #[tokio::test]
    async fn orphan_sweep_refuses_when_registry_is_present_but_claims_nothing() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test_with_production_layout(data_dir.clone()).await;
        let repos_dir = data_dir.join("repos");
        std::fs::create_dir_all(repos_dir.as_std_path()).unwrap();

        std::fs::write(
            repos_dir.join("registry.json").as_std_path(),
            br#"{"repos":{}}"#,
        )
        .unwrap();
        let orphan = repos_dir.join("0123456789abcdef");
        std::fs::create_dir_all(orphan.as_std_path()).unwrap();

        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);
        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);

        assert!(
            orphan.as_std_path().exists(),
            "an empty registry sitting next to real data directories must not \
             be trusted to mean \"nothing is registered\""
        );
    }

    // ─── C2: declining an already-indexed repo must not skip its grace period ──

    /// `prune_declined_missing` used to drop any `Declined` entry whose path
    /// was gone, including one with a real, populated data directory, because
    /// `set_consent`'s update branch (reachable via `decline_initial_index` on
    /// an already-indexed repo) preserves `data_dir` rather than clearing it.
    /// That dropped the registry's only claim on the directory one tick
    /// before the orphan sweep's own two-sighting rule finished arming it, so
    /// a declined, already-indexed repo whose folder was then deleted was
    /// fully collected in about two ticks (roughly two minutes) instead of
    /// waiting out `missing_repo_grace_days` (7 days by default).
    #[tokio::test]
    async fn declining_an_indexed_repo_then_deleting_its_folder_respects_the_grace_period() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;
        // An unrelated, normally-registered repo so `claimed` stays non-empty
        // even after the declined entry below is (wrongly, pre-fix) dropped;
        // isolates this test to the C2 mechanism rather than the C1 guards.
        let (_anchor_repo, anchor_path) = temp_repo_dir();
        manager.registry.register(anchor_path.as_str()).unwrap();
        let (repo, repo_path) = temp_repo_dir();
        // `decline_initial_index` canonicalizes internally; registering with
        // the same canonical form up front keeps this test on one registry
        // entry rather than creating a second one under a different hash.
        let canonical = crate::path::canonicalize_existing_dir(&repo_path).unwrap();

        let entry = manager.registry.register(canonical.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(canonical.as_str());
        std::fs::write(
            entry.data_dir.join("code-intelligence.db").as_std_path(),
            b"real index",
        )
        .unwrap();

        // Decline an already-indexed repo (the update branch of set_consent,
        // reachable via decline_initial_index / approve_indexing / POST
        // /api/consent), then delete its checkout.
        manager.decline_initial_index(canonical.as_path()).unwrap();
        drop(repo);

        // Backdate the data dir so it is old enough for the orphan sweep to
        // even consider it, isolating this test to the C2 mechanism rather
        // than the create-then-save age guard.
        let two_hours_ago = std::time::SystemTime::now() - Duration::from_secs(7200);
        filetime::set_file_mtime(
            entry.data_dir.as_std_path(),
            filetime::FileTime::from_system_time(two_hours_ago),
        )
        .unwrap();

        manager.evict_idle_repos().await; // tick N
        manager.evict_idle_repos().await; // tick N+1: two sightings would have collected an orphan

        assert!(
            manager.registry.get_by_hash(&hash).unwrap().is_some(),
            "a declined repo that was actually indexed must stay registered so \
             it can be graced, not dropped as a dead decline record"
        );
        assert!(
            entry.data_dir.as_std_path().exists(),
            "its data directory must survive well inside the 7-day grace period"
        );
    }

    // ─── I1: a corrupt missing_since stamp must heal, not pin forever ──────────

    /// `grace_verdict`'s doc comment promises a corrupt `missing_since` is
    /// healed by being rewritten rather than pinning the entry forever or
    /// triggering an immediate deletion. Drives that through the real sweep
    /// entry point across several ticks.
    #[tokio::test]
    async fn evict_idle_repos_heals_a_corrupt_missing_since_stamp_across_sweeps() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir.clone()).await;
        let (repo, repo_path) = temp_repo_dir();

        let entry = manager.registry.register(repo_path.as_str()).unwrap();
        let hash = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
        drop(repo);

        // A corrupt stamp, however it got there (a hand edit, a future format
        // change, disk corruption).
        manager
            .registry
            .stamp_missing_since(repo_path.as_str(), "not a date")
            .unwrap();

        // First sweep after the corruption: grace_verdict sees an unparseable
        // stamp as "no usable stamp", and the registry now overwrites it
        // instead of leaving "not a date" in place forever.
        manager.evict_idle_repos().await;
        let healed = manager
            .registry
            .get_by_hash(&hash)
            .unwrap()
            .unwrap()
            .missing_since
            .expect("a stamp must exist after the sweep");
        assert!(
            chrono::DateTime::parse_from_rfc3339(&healed).is_ok(),
            "the corrupt stamp must be healed to a valid RFC3339 value, got {healed}"
        );

        // Directly backdate the now-healed stamp past the grace period (the
        // same way other tests in this module simulate an old stamp), and
        // confirm the entry is not pinned forever: it still reaches deletion
        // normally.
        let registry_path = data_dir.join("registry.json");
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(registry_path.as_std_path()).unwrap()).unwrap();
        let old = (chrono::Utc::now() - chrono::Duration::days(8)).to_rfc3339();
        json["repos"][&hash]["missing_since"] = serde_json::Value::String(old);
        std::fs::write(registry_path.as_std_path(), json.to_string()).unwrap();

        manager.evict_idle_repos().await;

        assert!(
            manager.registry.get_by_hash(&hash).unwrap().is_none(),
            "once healed, the entry must reach its grace deadline like any \
             other, not stay pinned by the earlier corruption"
        );
        assert!(!entry.data_dir.as_std_path().exists());
    }

    // ─── I2: a warm runtime must not be collected out from under itself ───────

    /// The running-job guard covers an active indexing pass. It does not
    /// cover a warm runtime sitting idle with open SQLite, Tantivy, and
    /// LanceDB handles and a file watcher. Simulates the amplification
    /// scenario directly: the registry's claim on a loaded repo disappears
    /// (as in C1, or any other bug) while the runtime stays warm in
    /// `self.repos`.
    #[tokio::test]
    async fn orphan_sweep_spares_a_dir_whose_repo_is_still_warm_in_memory() {
        let (_data, data_dir) = temp_data_dir();
        let manager = SessionManager::new_for_test(data_dir).await;

        // An unrelated, normally-registered repo so `claimed` stays non-empty
        // and the C1 guards do not also explain the result.
        let (_anchor_repo, anchor_path) = temp_repo_dir();
        manager.registry.register(anchor_path.as_str()).unwrap();

        let (_repo, repo_path) = temp_repo_dir();
        manager.get_or_create_repo(&repo_path).await.unwrap();
        let key = canonical_key(&repo_path);
        let hash = crate::registry::RepoRegistry::path_hash(&key);
        let data_dir_path = manager
            .registry
            .get_by_hash(&hash)
            .unwrap()
            .unwrap()
            .data_dir;

        // The registry entry disappears while the runtime stays warm.
        manager.registry.remove_by_hash(&hash).unwrap();
        assert_eq!(
            manager.loaded_repo_count(),
            1,
            "the warm runtime must still be held, independent of the registry"
        );

        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);
        manager.sweep_orphan_data_dirs_with_min_age(Duration::ZERO);

        assert!(
            data_dir_path.as_std_path().exists(),
            "a directory whose repo is warm in memory must never be \
             collected, even if the registry no longer claims it"
        );
    }
}

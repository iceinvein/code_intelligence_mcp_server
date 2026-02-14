//! Per-repo leader election using POSIX flock().
//!
//! When multiple embedded MCP server instances run against the same repository,
//! only one should perform writes (indexing, file watching, LLM descriptions).
//! This module uses `flock()` (via the `fs2` crate) to elect a single leader
//! per repository data directory. Followers operate in read-only mode.

use anyhow::{anyhow, Context, Result};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::path::Utf8Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Leader,
    Follower,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::Leader => write!(f, "leader"),
            Role::Follower => write!(f, "follower"),
        }
    }
}

#[derive(Debug)]
pub struct Heartbeat {
    pub pid: u32,
    pub timestamp_unix_s: u64,
}

/// Leader election coordinator for a single repository.
///
/// Holds the lock file handle (keeping the flock alive), heartbeat path,
/// and a watch channel for role change notifications.
pub struct LeaderElection {
    lock_file: Option<File>,
    lock_path: String,
    heartbeat_path: String,
    is_leader: Arc<AtomicBool>,
    role_tx: watch::Sender<Role>,
    role_rx: watch::Receiver<Role>,
    heartbeat_interval_ms: u64,
    leader_ttl_seconds: u64,
}

impl LeaderElection {
    /// Create a new leader election coordinator. Does NOT acquire the lock.
    pub fn new(
        repo_data_dir: &Utf8Path,
        heartbeat_interval_ms: u64,
        leader_ttl_seconds: u64,
    ) -> Self {
        let lock_path = repo_data_dir.join("leader.lock").to_string();
        let heartbeat_path = repo_data_dir.join("leader.heartbeat").to_string();
        let (role_tx, role_rx) = watch::channel(Role::Follower);

        Self {
            lock_file: None,
            lock_path,
            heartbeat_path,
            is_leader: Arc::new(AtomicBool::new(false)),
            role_tx,
            role_rx,
            heartbeat_interval_ms,
            leader_ttl_seconds,
        }
    }

    /// Attempt to acquire the leader lock.
    ///
    /// On success: writes PID, writes initial heartbeat, returns `Role::Leader`.
    /// On failure (lock held by another process): returns `Role::Follower`.
    pub fn try_acquire(&mut self) -> Result<Role> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&self.lock_path)
            .with_context(|| format!("Failed to open lock file: {}", self.lock_path))?;

        match file.try_lock_exclusive() {
            Ok(()) => {
                // We got the lock — write our PID
                let mut f = &file;
                f.set_len(0)?;
                write!(f, "{}", std::process::id())?;

                self.lock_file = Some(file);
                self.is_leader.store(true, Ordering::SeqCst);
                let _ = self.role_tx.send(Role::Leader);

                // Write initial heartbeat
                self.write_heartbeat()?;

                tracing::info!(
                    pid = std::process::id(),
                    lock_path = %self.lock_path,
                    "Acquired leader lock"
                );
                Ok(Role::Leader)
            }
            Err(_) => {
                // Lock is held by another process
                tracing::info!(
                    lock_path = %self.lock_path,
                    "Leader lock held by another process, starting as follower"
                );
                Ok(Role::Follower)
            }
        }
    }

    /// Write the current PID and timestamp to the heartbeat file.
    pub fn write_heartbeat(&self) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| anyhow!("System clock before UNIX epoch"))?
            .as_secs();

        let content = format!("{}:{}", std::process::id(), now);
        fs::write(&self.heartbeat_path, content)
            .with_context(|| format!("Failed to write heartbeat: {}", self.heartbeat_path))?;
        Ok(())
    }

    /// Read and parse the heartbeat file.
    pub fn read_heartbeat(&self) -> Option<Heartbeat> {
        let content = fs::read_to_string(&self.heartbeat_path).ok()?;
        let parts: Vec<&str> = content.trim().split(':').collect();
        if parts.len() != 2 {
            return None;
        }
        let pid = parts[0].parse::<u32>().ok()?;
        let timestamp_unix_s = parts[1].parse::<u64>().ok()?;
        Some(Heartbeat {
            pid,
            timestamp_unix_s,
        })
    }

    /// Check if the heartbeat is older than the TTL.
    pub fn is_heartbeat_stale(&self) -> bool {
        let Some(hb) = self.read_heartbeat() else {
            return true; // No heartbeat file = stale
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(hb.timestamp_unix_s) > self.leader_ttl_seconds
    }

    /// Get the current role.
    pub fn role(&self) -> Role {
        if self.is_leader.load(Ordering::SeqCst) {
            Role::Leader
        } else {
            Role::Follower
        }
    }

    /// Get a watch receiver for role transitions.
    pub fn role_receiver(&self) -> watch::Receiver<Role> {
        self.role_rx.clone()
    }

    /// Get a shared flag for fast leader checks.
    pub fn is_leader_flag(&self) -> Arc<AtomicBool> {
        self.is_leader.clone()
    }

    /// Spawn a background task that periodically writes heartbeats (leader only).
    pub fn spawn_heartbeat_writer(&self, cancel: CancellationToken) -> JoinHandle<()> {
        let heartbeat_path = self.heartbeat_path.clone();
        let interval_ms = self.heartbeat_interval_ms;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(
                tokio::time::Duration::from_millis(interval_ms),
            );
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::debug!("Heartbeat writer cancelled");
                        break;
                    }
                    _ = interval.tick() => {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let content = format!("{}:{}", std::process::id(), now);
                        if let Err(e) = fs::write(&heartbeat_path, content) {
                            tracing::warn!("Failed to write heartbeat: {}", e);
                        }
                    }
                }
            }
        })
    }

    /// Spawn a background task that monitors for a stale leader heartbeat (follower only).
    ///
    /// Checks at TTL/3 intervals. If the heartbeat is stale, attempts to acquire the lock.
    /// On success, sends `Role::Leader` on the watch channel.
    pub fn spawn_follower_monitor(&mut self, cancel: CancellationToken) -> JoinHandle<()> {
        let lock_path = self.lock_path.clone();
        let heartbeat_path = self.heartbeat_path.clone();
        let is_leader = self.is_leader.clone();
        let role_tx = self.role_tx.clone();
        let ttl_seconds = self.leader_ttl_seconds;
        let check_interval = tokio::time::Duration::from_secs(
            (ttl_seconds / 3).max(1),
        );

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(check_interval);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::debug!("Follower monitor cancelled");
                        break;
                    }
                    _ = interval.tick() => {
                        // Check heartbeat staleness
                        let stale = match fs::read_to_string(&heartbeat_path) {
                            Ok(content) => {
                                let parts: Vec<&str> = content.trim().split(':').collect();
                                if parts.len() == 2 {
                                    if let Ok(ts) = parts[1].parse::<u64>() {
                                        let now = SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .map(|d| d.as_secs())
                                            .unwrap_or(0);
                                        now.saturating_sub(ts) > ttl_seconds
                                    } else {
                                        true
                                    }
                                } else {
                                    true
                                }
                            }
                            Err(_) => true,
                        };

                        if !stale {
                            continue;
                        }

                        tracing::info!("Leader heartbeat is stale, attempting lock acquisition");

                        // Try to acquire the lock
                        let file = match OpenOptions::new()
                            .create(true)
                            .write(true)
                            .truncate(false)
                            .open(&lock_path)
                        {
                            Ok(f) => f,
                            Err(e) => {
                                tracing::warn!("Failed to open lock file for promotion: {}", e);
                                continue;
                            }
                        };

                        match file.try_lock_exclusive() {
                            Ok(()) => {
                                // Write PID to lock file
                                let mut f = &file;
                                let _ = f.set_len(0);
                                let _ = write!(f, "{}", std::process::id());

                                // Write heartbeat
                                let now = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);
                                let content = format!("{}:{}", std::process::id(), now);
                                let _ = fs::write(&heartbeat_path, content);

                                is_leader.store(true, Ordering::SeqCst);
                                let _ = role_tx.send(Role::Leader);

                                tracing::warn!(
                                    pid = std::process::id(),
                                    "Promoted to leader after stale heartbeat detection"
                                );

                                // Keep the file handle alive by leaking it —
                                // the lock must persist for the process lifetime
                                std::mem::forget(file);
                                break;
                            }
                            Err(_) => {
                                // Another follower beat us to it
                                tracing::debug!("Lock acquisition failed, another instance promoted");
                            }
                        }
                    }
                }
            }
        })
    }
}

impl Drop for LeaderElection {
    fn drop(&mut self) {
        // Clean up heartbeat file if we're the leader
        if self.is_leader.load(Ordering::SeqCst) {
            let _ = fs::remove_file(&self.heartbeat_path);
            tracing::debug!("Cleaned up heartbeat file on leader drop");
        }
        // Lock file is released automatically when self.lock_file is dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_dir() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let c = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cimcp-leader-test-{nanos}-{c}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn test_first_acquire_returns_leader() {
        let dir = tmp_dir();
        let dir_path = Utf8Path::new(&dir);
        let mut election = LeaderElection::new(dir_path, 10_000, 30);

        let role = election.try_acquire().unwrap();
        assert_eq!(role, Role::Leader);
        assert!(election.is_leader.load(Ordering::SeqCst));
    }

    #[test]
    fn test_second_acquire_returns_follower() {
        let dir = tmp_dir();
        let dir_path = Utf8Path::new(&dir);

        let mut election1 = LeaderElection::new(dir_path, 10_000, 30);
        let role1 = election1.try_acquire().unwrap();
        assert_eq!(role1, Role::Leader);

        let mut election2 = LeaderElection::new(dir_path, 10_000, 30);
        let role2 = election2.try_acquire().unwrap();
        assert_eq!(role2, Role::Follower);
    }

    #[test]
    fn test_heartbeat_roundtrip() {
        let dir = tmp_dir();
        let dir_path = Utf8Path::new(&dir);
        let mut election = LeaderElection::new(dir_path, 10_000, 30);
        election.try_acquire().unwrap();

        election.write_heartbeat().unwrap();

        let hb = election.read_heartbeat().unwrap();
        assert_eq!(hb.pid, std::process::id());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(hb.timestamp_unix_s <= now);
        assert!(now - hb.timestamp_unix_s < 5);
    }

    #[test]
    fn test_stale_heartbeat_detected() {
        let dir = tmp_dir();
        let dir_path = Utf8Path::new(&dir);
        let election = LeaderElection::new(dir_path, 10_000, 1);

        // Write a heartbeat with timestamp 0 (very old)
        fs::write(&election.heartbeat_path, "99999:0").unwrap();
        assert!(election.is_heartbeat_stale());
    }

    #[test]
    fn test_lock_released_on_drop() {
        let dir = tmp_dir();
        let dir_path = Utf8Path::new(&dir);

        {
            let mut election = LeaderElection::new(dir_path, 10_000, 30);
            let role = election.try_acquire().unwrap();
            assert_eq!(role, Role::Leader);
        }
        // election dropped, lock released

        let mut election2 = LeaderElection::new(dir_path, 10_000, 30);
        let role2 = election2.try_acquire().unwrap();
        assert_eq!(role2, Role::Leader);
    }

    #[test]
    fn test_role_display() {
        assert_eq!(format!("{}", Role::Leader), "leader");
        assert_eq!(format!("{}", Role::Follower), "follower");
    }

    #[test]
    fn test_missing_heartbeat_is_stale() {
        let dir = tmp_dir();
        let dir_path = Utf8Path::new(&dir);
        let election = LeaderElection::new(dir_path, 10_000, 30);
        // No heartbeat file exists
        assert!(election.is_heartbeat_stale());
    }
}

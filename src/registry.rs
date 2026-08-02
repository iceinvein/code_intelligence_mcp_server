//! Repo registry for standalone mode — tracks registered repos and their storage locations

use crate::path::Utf8PathBuf;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Per-repo indexing-consent decision, persisted in `registry.json`.
///
/// Only `Approved` and `Declined` are ever written: a brand-new repo with no
/// entry is treated as pending by callers (`consent_status` returns `None`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexConsent {
    Approved,
    Declined,
}

/// Default for the `consent` field when an on-disk entry predates it. Such
/// entries were written for repos that were already being indexed, so they are
/// grandfathered as approved.
fn default_consent() -> IndexConsent {
    IndexConsent::Approved
}

/// Classify catastrophic repository roots that must never be registered.
/// Returns a human-readable reason, or `None` when the path is acceptable.
/// A registry entry like "/" (observed after R017: an errant bind wrote one)
/// makes the per-repo watcher watch the whole disk; the first file event
/// anywhere starts an index run over the entire filesystem that starves every
/// session sharing the daemon.
fn forbidden_repo_root(repo_path: &str) -> Option<&'static str> {
    let trimmed = repo_path.trim_end_matches('/');
    if trimmed.is_empty() {
        // "" or "/"
        return Some("filesystem root");
    }
    if matches!(
        trimmed,
        "/Users"
            | "/home"
            | "/tmp"
            | "/private/tmp"
            | "/var"
            | "/private/var"
            | "/etc"
            | "/usr"
            | "/opt"
            | "/System"
            | "/Library"
            | "/Applications"
            | "/Volumes"
    ) {
        return Some("system directory");
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() && trimmed == home.trim_end_matches('/') {
            return Some("home directory");
        }
    }
    None
}

/// Extract the repo name (last path component) for logging/display.
fn repo_name_from_path(repo_path: &str) -> String {
    repo_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("unknown")
        .to_string()
}

/// Information about a registered repository
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoEntry {
    pub path: String,          // canonical absolute path of the repo
    pub name: String,          // last path component (for logs)
    pub data_dir: Utf8PathBuf, // where this repo's indexes live
    pub created_at: String,    // RFC3339 timestamp
    pub last_accessed: String, // RFC3339 timestamp
    #[serde(default = "default_consent")]
    pub consent: IndexConsent,
    #[serde(default)]
    pub initial_index_approved_at: Option<String>,
    #[serde(default)]
    pub initial_index_completed_at: Option<String>,
    /// Repo id of the base repository this index was seeded from, when it was
    /// created by cloning a base index rather than by a full index pass. Only
    /// entries carrying this are eligible for automatic pruning.
    #[serde(default)]
    pub seeded_from: Option<String>,
    /// RFC3339 timestamp of the first sweep that found this repo's path absent,
    /// cleared as soon as the path comes back. Drives grace-period deletion of
    /// non-seeded indexes.
    ///
    /// A seeded entry never carries this: its two-sweep in-memory rule reaches a
    /// decision in about two minutes, well before a persisted stamp would matter.
    #[serde(default)]
    pub missing_since: Option<String>,
}

/// Internal structure for JSON serialization
#[derive(Serialize, Deserialize)]
struct RegistryFile {
    repos: HashMap<String, RepoEntry>, // hash → entry
}

/// Registry for tracking registered repositories and their storage locations
pub struct RepoRegistry {
    registry_path: Utf8PathBuf, // path to registry.json
    repos_dir: Utf8PathBuf,     // parent dir for per-repo storage
}

impl RepoRegistry {
    /// Create a new registry with the given paths
    pub fn new(registry_path: Utf8PathBuf, repos_dir: Utf8PathBuf) -> Self {
        Self {
            registry_path,
            repos_dir,
        }
    }

    /// The directory every per-repo data directory lives under. Exposed so
    /// callers that delete a data directory can check containment first.
    pub(crate) fn repos_dir(&self) -> &crate::path::Utf8Path {
        self.repos_dir.as_path()
    }

    /// Whether `registry.json` exists on disk, as distinct from "loaded and
    /// empty". `load` treats a missing file as an empty registry so a
    /// brand-new daemon starts cleanly; a caller that must not confuse
    /// "nothing is registered" with "the registry file is gone" (the orphan
    /// sweep) checks this first.
    pub(crate) fn registry_path_exists(&self) -> bool {
        self.registry_path.as_std_path().exists()
    }

    /// Compute deterministic 16-character hash of a repository path
    pub fn path_hash(path: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(path.as_bytes());
        let result = hasher.finalize();
        let full_hash = hex::encode(result);
        full_hash[..16].to_string()
    }

    /// Register a repository, creating its data directory and persisting to JSON.
    /// If already registered, updates last_accessed and returns existing entry.
    pub fn register(&self, repo_path: &str) -> Result<RepoEntry> {
        if let Some(kind) = forbidden_repo_root(repo_path) {
            anyhow::bail!(
                "refusing to register '{repo_path}' as a repository root ({kind}). \
                 A watcher on such a root scans the entire tree on any file event; \
                 an errant '/' entry starved every bench session in R017."
            );
        }
        let hash = Self::path_hash(repo_path);
        let mut registry = self.load()?;

        let now = chrono::Utc::now().to_rfc3339();

        // Check if already exists
        if let Some(existing) = registry.repos.get_mut(&hash) {
            // Touch last_accessed
            existing.last_accessed = now;
            let entry = existing.clone();
            self.save(&registry)?;
            return Ok(entry);
        }

        // Extract repo name from path
        let name = repo_name_from_path(repo_path);

        // Create data directory path
        let data_dir = self.repos_dir.join(&hash);

        // Create the data directory
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("Failed to create data directory: {}", data_dir))?;

        let entry = RepoEntry {
            path: repo_path.to_string(),
            name,
            data_dir,
            created_at: now.clone(),
            last_accessed: now,
            consent: IndexConsent::Approved,
            initial_index_approved_at: None,
            initial_index_completed_at: None,
            seeded_from: None,
            missing_since: None,
        };

        registry.repos.insert(hash, entry.clone());
        self.save(&registry)?;

        Ok(entry)
    }

    /// Persist one-time authorization for a repository's first full index.
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

    /// Persist successful completion of a repository's first full index.
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

    /// Look up a repository by path
    pub fn get(&self, repo_path: &str) -> Result<Option<RepoEntry>> {
        let hash = Self::path_hash(repo_path);
        let registry = self.load()?;
        Ok(registry.repos.get(&hash).cloned())
    }

    /// Reverse lookup: find a repository entry by its hash.
    ///
    /// This is used for cross-repo dependency resolution, where edges store
    /// the target repo hash and we need to find the corresponding repo path
    /// and data directory.
    pub fn get_by_hash(&self, hash: &str) -> Result<Option<RepoEntry>> {
        let registry = self.load()?;
        Ok(registry.repos.get(hash).cloned())
    }

    /// List all registered repositories, sorted by most recently accessed first
    pub fn list_all(&self) -> Result<Vec<RepoEntry>> {
        let registry = self.load()?;
        let mut entries: Vec<RepoEntry> = registry.repos.into_values().collect();
        entries.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
        Ok(entries)
    }

    /// Remove a registered repository by its canonical path.
    ///
    /// Drops the entry from `registry.json`. Caller is responsible for
    /// deleting the on-disk data directory (the returned entry exposes
    /// `data_dir` for that). Returns `Ok(None)` if no such repo was
    /// registered.
    pub fn remove(&self, repo_path: &str) -> Result<Option<RepoEntry>> {
        let hash = Self::path_hash(repo_path);
        self.remove_by_hash(&hash)
    }

    /// Remove a registered repository by its 16-character hash.
    pub fn remove_by_hash(&self, hash: &str) -> Result<Option<RepoEntry>> {
        let mut registry = self.load()?;
        let removed = registry.repos.remove(hash);
        if removed.is_some() {
            self.save(&registry)?;
        }
        Ok(removed)
    }

    /// Update the last_accessed timestamp for a repository
    pub fn touch(&self, repo_path: &str) -> Result<()> {
        let hash = Self::path_hash(repo_path);
        let mut registry = self.load()?;

        if let Some(entry) = registry.repos.get_mut(&hash) {
            entry.last_accessed = chrono::Utc::now().to_rfc3339();
            self.save(&registry)?;
        }

        Ok(())
    }

    /// Record a consent decision for a repo WITHOUT creating its on-disk data
    /// directory, and without touching `last_accessed`. Intended for persisting
    /// a `Declined` choice for a repo we will not index; the `data_dir` path is
    /// computed but not created. (Approved repos get their entry and data dir
    /// from `register()` at index time.)
    pub fn set_consent(&self, repo_path: &str, consent: IndexConsent) -> Result<RepoEntry> {
        let hash = Self::path_hash(repo_path);
        let mut registry = self.load()?;
        let now = chrono::Utc::now().to_rfc3339();

        if let Some(existing) = registry.repos.get_mut(&hash) {
            existing.consent = consent;
            if consent == IndexConsent::Declined && existing.initial_index_completed_at.is_none() {
                existing.initial_index_approved_at = None;
            }
            let entry = existing.clone();
            self.save(&registry)?;
            return Ok(entry);
        }

        let name = repo_name_from_path(repo_path);
        let entry = RepoEntry {
            path: repo_path.to_string(),
            name,
            data_dir: self.repos_dir.join(&hash), // path only; not created
            created_at: now.clone(),
            last_accessed: now,
            consent,
            initial_index_approved_at: None,
            initial_index_completed_at: None,
            seeded_from: None,
            missing_since: None,
        };
        registry.repos.insert(hash, entry.clone());
        self.save(&registry)?;
        Ok(entry)
    }

    /// Return the recorded consent decision for a repo, or `None` if the repo
    /// has never been registered (a brand-new repo, treated as pending).
    pub fn consent_status(&self, repo_path: &str) -> Result<Option<IndexConsent>> {
        Ok(self.get(repo_path)?.map(|e| e.consent))
    }

    /// Drop `Declined` entries whose path no longer exists on disk and which
    /// never had a data directory created for them.
    ///
    /// `set_consent`'s insert branch (a repo declined before it was ever
    /// indexed, e.g. an ephemeral worktree or temp copy) computes `data_dir`
    /// but never creates it, so those entries are dead decline records with
    /// nothing on disk to lose. This only reclaims those, keeping
    /// `registry.json` from accumulating them.
    ///
    /// An entry whose `data_dir` exists on disk is kept even when its path is
    /// gone and its consent is `Declined`, because `set_consent`'s update
    /// branch (an already-indexed repo declined later, reachable via
    /// `decline_initial_index`) preserves `data_dir` in place rather than
    /// clearing it. Such an entry carries a real index, so it is left for
    /// `SessionManager::prune_vanished_indexes` to stamp and grace like any
    /// other non-seeded index, instead of being dropped here with its data
    /// directory orphaned underneath it.
    /// Returns the number of entries pruned.
    pub fn prune_declined_missing(&self) -> Result<usize> {
        let mut registry = self.load()?;
        let before = registry.repos.len();
        registry.repos.retain(|_hash, e| {
            e.consent != IndexConsent::Declined
                || std::path::Path::new(&e.path).exists()
                || e.data_dir.as_std_path().exists()
        });
        let pruned = before - registry.repos.len();
        if pruned > 0 {
            self.save(&registry)?;
        }
        Ok(pruned)
    }

    /// Record that this repo's index was seeded from `base_repo_id`.
    ///
    /// The sole writer of `RepoEntry::seeded_from`, and therefore the only way
    /// an entry takes the two-sweep in-memory path in
    /// `SessionManager::prune_vanished_indexes` instead of the persisted
    /// grace-period path that every other entry `list_missing` returns takes.
    pub fn mark_seeded_from(&self, repo_path: &str, base_repo_id: &str) -> Result<RepoEntry> {
        let hash = Self::path_hash(repo_path);
        let mut registry = self.load()?;
        let entry = registry.repos.get_mut(&hash).with_context(|| {
            format!("Cannot mark unregistered repository as seeded: {repo_path}")
        })?;
        entry.seeded_from = Some(base_repo_id.to_string());
        let updated = entry.clone();
        self.save(&registry)?;
        Ok(updated)
    }

    /// Every registered repository whose path no longer exists on disk, seeded
    /// or not.
    ///
    /// Read-only; the caller classifies and deletes. Wider than the
    /// `list_seeded_missing` it replaces, because the grace sweep needs
    /// hand-registered entries too and branches on `seeded_from` itself.
    pub fn list_missing(&self) -> Result<Vec<RepoEntry>> {
        let registry = self.load()?;
        Ok(registry
            .repos
            .into_values()
            .filter(|e| !std::path::Path::new(&e.path).exists())
            .collect())
    }

    /// Record `now` as the moment this repo's path was first seen absent,
    /// unless a valid stamp is already present.
    ///
    /// Returns the effective stamp, newly written or pre-existing, so a sweep can
    /// compute the deadline without a second load. Returns `Ok(None)` when the
    /// repo is not registered. Idempotent for a stamp that parses as RFC3339: a
    /// repeat call must never push the deletion deadline out, or a repo absent
    /// across many sweeps would never reach it. A stamp that fails to parse is
    /// treated as though none were present and is overwritten, so a corrupt
    /// value heals on the next sweep instead of pinning the entry forever.
    pub fn stamp_missing_since(&self, repo_path: &str, now: &str) -> Result<Option<String>> {
        let hash = Self::path_hash(repo_path);
        let mut registry = self.load()?;
        let Some(entry) = registry.repos.get_mut(&hash) else {
            return Ok(None);
        };
        if let Some(existing) = entry.missing_since.clone() {
            if chrono::DateTime::parse_from_rfc3339(&existing).is_ok() {
                return Ok(Some(existing));
            }
        }
        entry.missing_since = Some(now.to_string());
        self.save(&registry)?;
        Ok(Some(now.to_string()))
    }

    /// Clear `missing_since` on every entry whose path is present again.
    ///
    /// Returns how many stamps were cleared. One load and, when nothing changed,
    /// no save at all: this runs on every 60-second sweep and must not rewrite
    /// `registry.json` in the steady state.
    pub fn clear_missing_since_for_present_paths(&self) -> Result<usize> {
        let mut registry = self.load()?;
        let mut cleared = 0;
        for entry in registry.repos.values_mut() {
            if entry.missing_since.is_some() && std::path::Path::new(&entry.path).exists() {
                entry.missing_since = None;
                cleared += 1;
            }
        }
        if cleared > 0 {
            self.save(&registry)?;
        }
        Ok(cleared)
    }

    /// Load registry from disk, or return empty registry if file doesn't exist
    fn load(&self) -> Result<RegistryFile> {
        if !self.registry_path.as_std_path().exists() {
            return Ok(RegistryFile {
                repos: HashMap::new(),
            });
        }

        let contents = std::fs::read_to_string(&self.registry_path)
            .with_context(|| format!("Failed to read registry file: {}", self.registry_path))?;

        serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse registry file: {}", self.registry_path))
    }

    /// Save registry to disk atomically (write to temp file, then rename)
    fn save(&self, file: &RegistryFile) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.registry_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create registry parent directory: {}", parent)
            })?;
        }

        let tmp_path = self.registry_path.with_extension("json.tmp");

        let contents =
            serde_json::to_string_pretty(file).context("Failed to serialize registry to JSON")?;

        std::fs::write(&tmp_path, contents)
            .with_context(|| format!("Failed to write temporary registry file: {}", tmp_path))?;

        std::fs::rename(&tmp_path, &self.registry_path)
            .with_context(|| format!("Failed to rename {} to {}", tmp_path, self.registry_path))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_hash_is_deterministic_and_16_chars() {
        let hash = RepoRegistry::path_hash("/Users/dev/my-project");
        assert_eq!(hash.len(), 16);
        assert_eq!(hash, RepoRegistry::path_hash("/Users/dev/my-project"));
        // Different paths → different hashes
        assert_ne!(hash, RepoRegistry::path_hash("/Users/dev/other-project"));
    }

    #[test]
    fn register_and_lookup_repo() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let registry_path = dir_path.join("registry.json");
        let repos_dir = dir_path.join("repos");

        let reg = RepoRegistry::new(registry_path.clone(), repos_dir.clone());
        let entry = reg.register("/Users/dev/my-project").unwrap();

        assert_eq!(entry.name, "my-project");
        assert!(entry.data_dir.starts_with(repos_dir.as_str()));

        // Storage dir was created
        assert!(entry.data_dir.as_std_path().exists());

        // Persists to disk — new instance can read it back
        let reg2 = RepoRegistry::new(registry_path, repos_dir);
        let entry2 = reg2.get("/Users/dev/my-project").unwrap();
        assert!(entry2.is_some());
        assert_eq!(entry2.unwrap().name, "my-project");
    }

    #[test]
    fn register_refuses_catastrophic_roots() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));

        for path in ["/", "", "/Users", "/tmp", "/System", "/Volumes"] {
            let err = reg.register(path).unwrap_err();
            assert!(
                err.to_string().contains("refusing to register"),
                "{path} must be refused: {err}"
            );
        }
        if let Ok(home) = std::env::var("HOME") {
            assert!(reg.register(&home).is_err(), "home dir must be refused");
        }
        // Normal project paths still register.
        assert!(reg.register("/Users/dev/project").is_ok());
    }

    #[test]
    fn register_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));

        let e1 = reg.register("/Users/dev/project").unwrap();
        let e2 = reg.register("/Users/dev/project").unwrap();
        assert_eq!(e1.path, e2.path);
        assert_eq!(e1.data_dir, e2.data_dir);
    }

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

    #[test]
    fn list_all_returns_sorted_by_last_accessed() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));

        // Register three repos (register sets last_accessed = now)
        reg.register("/a/first").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        reg.register("/b/second").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        reg.register("/c/third").unwrap();

        let all = reg.list_all().unwrap();
        assert_eq!(all.len(), 3);
        // Most recently accessed first
        assert_eq!(all[0].name, "third");
        assert_eq!(all[1].name, "second");
        assert_eq!(all[2].name, "first");
    }

    #[test]
    fn get_by_hash_returns_registered_repo() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));

        reg.register("/Users/dev/my-project").unwrap();
        let hash = RepoRegistry::path_hash("/Users/dev/my-project");

        let entry = reg.get_by_hash(&hash).unwrap();
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().name, "my-project");
    }

    #[test]
    fn get_by_hash_returns_none_for_unknown_hash() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));

        let entry = reg.get_by_hash("0000000000000000").unwrap();
        assert!(entry.is_none());
    }

    #[test]
    fn remove_drops_entry_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let registry_path = dir_path.join("registry.json");
        let repos_dir = dir_path.join("repos");
        let reg = RepoRegistry::new(registry_path.clone(), repos_dir.clone());

        reg.register("/Users/dev/keep").unwrap();
        reg.register("/Users/dev/drop").unwrap();

        let removed = reg.remove("/Users/dev/drop").unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().name, "drop");

        // No longer findable
        assert!(reg.get("/Users/dev/drop").unwrap().is_none());

        // Persists across instances
        let reg2 = RepoRegistry::new(registry_path, repos_dir);
        assert!(reg2.get("/Users/dev/drop").unwrap().is_none());
        assert!(reg2.get("/Users/dev/keep").unwrap().is_some());
    }

    #[test]
    fn remove_unknown_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));
        assert!(reg.remove("/no/such/repo").unwrap().is_none());
    }

    #[test]
    fn remove_by_hash_drops_entry() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));

        reg.register("/Users/dev/by-hash").unwrap();
        let hash = RepoRegistry::path_hash("/Users/dev/by-hash");

        let removed = reg.remove_by_hash(&hash).unwrap();
        assert!(removed.is_some());
        assert!(reg.get_by_hash(&hash).unwrap().is_none());
    }

    #[test]
    fn list_all_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));
        let all = reg.list_all().unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn new_entry_defaults_to_approved() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));
        let entry = reg.register("/Users/dev/proj").unwrap();
        assert_eq!(entry.consent, IndexConsent::Approved);
    }

    #[test]
    fn missing_consent_field_deserializes_as_approved() {
        // A registry.json written before the consent field existed must
        // grandfather its repos as Approved (they were already being indexed).
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let registry_path = dir_path.join("registry.json");
        // Use the real sha256-based hash for "/x/y" so get() can find the entry.
        let hash = RepoRegistry::path_hash("/x/y");
        let json = format!(
            r#"{{"repos":{{"{hash}":{{"path":"/x/y","name":"y","data_dir":"/d","created_at":"t","last_accessed":"t"}}}}}}"#
        );
        std::fs::write(&registry_path, json).unwrap();
        let reg = RepoRegistry::new(registry_path, dir_path.join("repos"));
        let entry = reg.get("/x/y").unwrap().unwrap();
        assert_eq!(entry.consent, IndexConsent::Approved);
    }

    #[test]
    fn set_consent_records_decline_without_creating_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let repos_dir = dir_path.join("repos");
        let reg = RepoRegistry::new(dir_path.join("registry.json"), repos_dir.clone());

        let entry = reg
            .set_consent("/Users/dev/declined", IndexConsent::Declined)
            .unwrap();
        assert_eq!(entry.consent, IndexConsent::Declined);
        // No data directory was created on disk.
        assert!(!entry.data_dir.as_std_path().exists());
        // Persisted and readable.
        assert_eq!(
            reg.consent_status("/Users/dev/declined").unwrap(),
            Some(IndexConsent::Declined)
        );
    }

    #[test]
    fn consent_status_is_none_for_unregistered_repo() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));
        assert_eq!(reg.consent_status("/never/seen").unwrap(), None);
    }

    #[test]
    fn registration_preserves_decline_until_explicit_approval() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));
        reg.set_consent("/Users/dev/project", IndexConsent::Declined)
            .unwrap();

        let registered = reg.register("/Users/dev/project").unwrap();
        assert_eq!(registered.consent, IndexConsent::Declined);
        assert!(registered.initial_index_approved_at.is_none());

        let approved = reg.approve_initial_index("/Users/dev/project").unwrap();
        assert_eq!(approved.consent, IndexConsent::Approved);
        assert!(approved.initial_index_approved_at.is_some());
    }

    #[test]
    fn prune_declined_missing_drops_only_absent_declines() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));

        // A declined repo whose path no longer exists.
        reg.set_consent("/no/such/worktree", IndexConsent::Declined)
            .unwrap();
        // A declined repo whose path DOES exist (the tempdir itself).
        let alive = dir_path.as_str().to_string();
        reg.set_consent(&alive, IndexConsent::Declined).unwrap();
        // An approved repo whose path is missing must NOT be pruned.
        reg.set_consent("/no/such/approved", IndexConsent::Approved)
            .unwrap();

        let pruned = reg.prune_declined_missing().unwrap();
        assert_eq!(pruned, 1);
        assert_eq!(reg.consent_status("/no/such/worktree").unwrap(), None);
        assert_eq!(
            reg.consent_status(&alive).unwrap(),
            Some(IndexConsent::Declined)
        );
        assert_eq!(
            reg.consent_status("/no/such/approved").unwrap(),
            Some(IndexConsent::Approved)
        );
    }

    /// C2: `set_consent`'s update branch (an already-indexed repo declined
    /// later, e.g. via `decline_initial_index`) preserves `data_dir` on an
    /// existing entry. If the checkout is then deleted, that entry must not
    /// be dropped by `prune_declined_missing`: it carries a real index, and
    /// dropping it here would orphan the data directory and skip the grace
    /// period entirely.
    #[test]
    fn prune_declined_missing_retains_an_entry_whose_data_dir_exists() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));

        // Register first: this creates a real data directory, simulating an
        // indexed repo.
        let repo = dir_path.join("indexed-then-declined");
        std::fs::create_dir_all(repo.as_std_path()).unwrap();
        let entry = reg.register(repo.as_str()).unwrap();
        assert!(entry.data_dir.as_std_path().exists());

        // Decline it via the update branch of set_consent, which preserves
        // data_dir rather than clearing it.
        reg.set_consent(repo.as_str(), IndexConsent::Declined)
            .unwrap();

        // The checkout is deleted.
        std::fs::remove_dir_all(repo.as_std_path()).unwrap();

        let pruned = reg.prune_declined_missing().unwrap();
        assert_eq!(
            pruned, 0,
            "an entry whose data dir still holds a real index must not be pruned here"
        );
        assert_eq!(
            reg.consent_status(repo.as_str()).unwrap(),
            Some(IndexConsent::Declined),
            "the entry must survive so prune_vanished_indexes can grace it"
        );
        assert!(
            entry.data_dir.as_std_path().exists(),
            "the data directory itself must be untouched by this method"
        );
    }

    #[test]
    fn mark_seeded_from_records_the_base_repo_id() {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let registry = RepoRegistry::new(root.join("registry.json"), root.join("repos"));
        let repo = root.join("feature");
        std::fs::create_dir_all(repo.as_std_path()).unwrap();

        registry.register(repo.as_str()).unwrap();
        assert_eq!(
            registry.get(repo.as_str()).unwrap().unwrap().seeded_from,
            None
        );

        registry
            .mark_seeded_from(repo.as_str(), "basehash1234567")
            .unwrap();
        assert_eq!(
            registry
                .get(repo.as_str())
                .unwrap()
                .unwrap()
                .seeded_from
                .as_deref(),
            Some("basehash1234567")
        );
    }

    #[test]
    fn list_missing_reports_seeded_and_unseeded_absent_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let registry = RepoRegistry::new(root.join("registry.json"), root.join("repos"));

        // Present: never reported.
        let live = root.join("live");
        std::fs::create_dir_all(live.as_std_path()).unwrap();
        registry.register(live.as_str()).unwrap();

        // Absent and seeded.
        let seeded = root.join("seeded-gone");
        std::fs::create_dir_all(seeded.as_std_path()).unwrap();
        registry.register(seeded.as_str()).unwrap();
        registry.mark_seeded_from(seeded.as_str(), "base1").unwrap();
        std::fs::remove_dir_all(seeded.as_std_path()).unwrap();

        // Absent and hand-registered. The earlier list_seeded_missing excluded
        // this; the grace sweep needs it.
        let manual = root.join("manual-gone");
        std::fs::create_dir_all(manual.as_std_path()).unwrap();
        registry.register(manual.as_str()).unwrap();
        std::fs::remove_dir_all(manual.as_std_path()).unwrap();

        let mut paths: Vec<String> = registry
            .list_missing()
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect();
        paths.sort();
        assert_eq!(paths, vec![manual.to_string(), seeded.to_string()]);

        // Read-only: the caller performs the deletion.
        assert!(registry.get(seeded.as_str()).unwrap().is_some());
        assert!(registry.get(manual.as_str()).unwrap().is_some());
    }

    #[test]
    fn stamp_missing_since_is_idempotent_and_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let registry = RepoRegistry::new(root.join("registry.json"), root.join("repos"));
        let repo = root.join("project");
        std::fs::create_dir_all(repo.as_std_path()).unwrap();
        registry.register(repo.as_str()).unwrap();

        assert_eq!(
            registry.get(repo.as_str()).unwrap().unwrap().missing_since,
            None
        );

        let first = registry
            .stamp_missing_since(repo.as_str(), "2026-08-01T00:00:00Z")
            .unwrap();
        assert_eq!(first.as_deref(), Some("2026-08-01T00:00:00Z"));

        // A later sweep must not push the deadline out.
        let second = registry
            .stamp_missing_since(repo.as_str(), "2026-08-05T00:00:00Z")
            .unwrap();
        assert_eq!(second.as_deref(), Some("2026-08-01T00:00:00Z"));

        // Survives a reload from disk.
        let reopened = RepoRegistry::new(root.join("registry.json"), root.join("repos"));
        assert_eq!(
            reopened
                .get(repo.as_str())
                .unwrap()
                .unwrap()
                .missing_since
                .as_deref(),
            Some("2026-08-01T00:00:00Z")
        );
    }

    /// I1: an unparseable stamp (a hand edit, a future format change, disk
    /// corruption) must heal rather than pin the entry forever. The doc
    /// comment on `grace_verdict` in `src/session.rs` has always claimed this;
    /// this is the test that actually holds it to that claim.
    #[test]
    fn stamp_missing_since_heals_a_corrupt_stamp_instead_of_pinning_it_forever() {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let registry = RepoRegistry::new(root.join("registry.json"), root.join("repos"));
        let repo = root.join("project");
        std::fs::create_dir_all(repo.as_std_path()).unwrap();
        registry.register(repo.as_str()).unwrap();

        // A corrupt stamp, however it got there.
        registry
            .stamp_missing_since(repo.as_str(), "not a date")
            .unwrap();
        assert_eq!(
            registry.get(repo.as_str()).unwrap().unwrap().missing_since,
            Some("not a date".to_string())
        );

        // The next sweep must overwrite it rather than treat it as a stamp to
        // preserve.
        let healed = registry
            .stamp_missing_since(repo.as_str(), "2026-08-01T00:00:00Z")
            .unwrap();
        assert_eq!(healed.as_deref(), Some("2026-08-01T00:00:00Z"));
        assert_eq!(
            registry.get(repo.as_str()).unwrap().unwrap().missing_since,
            Some("2026-08-01T00:00:00Z".to_string())
        );

        // Once healed with a valid stamp, idempotence resumes: a later sweep
        // must not push the deadline out again.
        let unchanged = registry
            .stamp_missing_since(repo.as_str(), "2026-09-01T00:00:00Z")
            .unwrap();
        assert_eq!(unchanged.as_deref(), Some("2026-08-01T00:00:00Z"));
    }

    #[test]
    fn stamp_missing_since_ignores_unregistered_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let registry = RepoRegistry::new(root.join("registry.json"), root.join("repos"));

        let stamped = registry
            .stamp_missing_since("/not/registered", "2026-08-01T00:00:00Z")
            .unwrap();
        assert_eq!(stamped, None);
    }

    #[test]
    fn clear_missing_since_only_touches_paths_that_came_back() {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let registry = RepoRegistry::new(root.join("registry.json"), root.join("repos"));

        // Stamped and back on disk: cleared.
        let returned = root.join("returned");
        std::fs::create_dir_all(returned.as_std_path()).unwrap();
        registry.register(returned.as_str()).unwrap();
        registry
            .stamp_missing_since(returned.as_str(), "2026-08-01T00:00:00Z")
            .unwrap();

        // Stamped and still absent: stamp preserved.
        let gone = root.join("gone");
        std::fs::create_dir_all(gone.as_std_path()).unwrap();
        registry.register(gone.as_str()).unwrap();
        registry
            .stamp_missing_since(gone.as_str(), "2026-08-01T00:00:00Z")
            .unwrap();
        std::fs::remove_dir_all(gone.as_std_path()).unwrap();

        let cleared = registry.clear_missing_since_for_present_paths().unwrap();
        assert_eq!(cleared, 1);
        assert_eq!(
            registry
                .get(returned.as_str())
                .unwrap()
                .unwrap()
                .missing_since,
            None
        );
        assert_eq!(
            registry
                .get(gone.as_str())
                .unwrap()
                .unwrap()
                .missing_since
                .as_deref(),
            Some("2026-08-01T00:00:00Z")
        );

        // Nothing to clear on a second pass, and therefore no write.
        assert_eq!(registry.clear_missing_since_for_present_paths().unwrap(), 0);
    }

    #[test]
    fn legacy_registry_json_without_seeded_from_still_loads() {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        let registry_path = root.join("registry.json");
        std::fs::write(
            registry_path.as_std_path(),
            r#"{"repos":{"abc123":{"path":"/tmp/x","name":"x","data_dir":"/tmp/d","created_at":"2026-01-01T00:00:00Z","last_accessed":"2026-01-01T00:00:00Z"}}}"#,
        )
        .unwrap();
        let registry = RepoRegistry::new(registry_path, root.join("repos"));
        let all = registry.list_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].seeded_from, None);
        assert_eq!(all[0].missing_since, None);
    }
}

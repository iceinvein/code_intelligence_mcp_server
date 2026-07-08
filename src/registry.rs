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
            // register() is only reached when we are about to index this repo,
            // so registering implies approval (and flips a prior decline).
            existing.consent = IndexConsent::Approved;
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
        };

        registry.repos.insert(hash, entry.clone());
        self.save(&registry)?;

        Ok(entry)
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

    /// Drop `Declined` entries whose path no longer exists on disk. Ephemeral
    /// folders (git worktrees, temp copies) get unique paths and are deleted
    /// after use; this keeps `registry.json` from accumulating dead declines.
    /// Returns the number of entries pruned.
    pub fn prune_declined_missing(&self) -> Result<usize> {
        let mut registry = self.load()?;
        let before = registry.repos.len();
        registry.repos.retain(|_hash, e| {
            e.consent != IndexConsent::Declined || std::path::Path::new(&e.path).exists()
        });
        let pruned = before - registry.repos.len();
        if pruned > 0 {
            self.save(&registry)?;
        }
        Ok(pruned)
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
    fn register_flips_declined_entry_back_to_approved() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));
        reg.set_consent("/Users/dev/p", IndexConsent::Declined)
            .unwrap();
        // register() is only called when we are about to index, so it asserts approval.
        let entry = reg.register("/Users/dev/p").unwrap();
        assert_eq!(entry.consent, IndexConsent::Approved);
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
}

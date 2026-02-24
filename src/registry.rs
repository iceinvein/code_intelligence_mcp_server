//! Repo registry for standalone mode — tracks registered repos and their storage locations

use crate::path::Utf8PathBuf;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Information about a registered repository
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoEntry {
    pub path: String,          // canonical absolute path of the repo
    pub name: String,          // last path component (for logs)
    pub data_dir: Utf8PathBuf, // where this repo's indexes live
    pub created_at: String,    // RFC3339 timestamp
    pub last_accessed: String, // RFC3339 timestamp
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
        let name = repo_path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("unknown")
            .to_string();

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

    /// List all registered repositories, sorted by most recently accessed first
    pub fn list_all(&self) -> Result<Vec<RepoEntry>> {
        let registry = self.load()?;
        let mut entries: Vec<RepoEntry> = registry.repos.into_values().collect();
        entries.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
        Ok(entries)
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

        let contents = serde_json::to_string_pretty(file)
            .context("Failed to serialize registry to JSON")?;

        std::fs::write(&tmp_path, contents)
            .with_context(|| format!("Failed to write temporary registry file: {}", tmp_path))?;

        std::fs::rename(&tmp_path, &self.registry_path).with_context(|| {
            format!(
                "Failed to rename {} to {}",
                tmp_path, self.registry_path
            )
        })?;

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
        let dir_path =
            crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
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
    fn register_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path =
            crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));

        let e1 = reg.register("/Users/dev/project").unwrap();
        let e2 = reg.register("/Users/dev/project").unwrap();
        assert_eq!(e1.path, e2.path);
        assert_eq!(e1.data_dir, e2.data_dir);
    }

    #[test]
    fn list_all_returns_sorted_by_last_accessed() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path =
            crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
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
    fn list_all_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path =
            crate::path::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let reg = RepoRegistry::new(dir_path.join("registry.json"), dir_path.join("repos"));
        let all = reg.list_all().unwrap();
        assert!(all.is_empty());
    }
}

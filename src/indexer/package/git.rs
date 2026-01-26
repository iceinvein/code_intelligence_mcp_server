//! Git repository detection using git2.
//!
//! This module provides functionality to discover git repository roots
//! and extract repository metadata including remote URLs.

use anyhow::{Context, Result};
use git2::Repository;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use crate::path::Utf8PathBuf;

/// Information about a discovered git repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryInfo {
    /// SHA-256 hash of the root_path (used as unique ID)
    pub id: String,
    /// Repository name (directory name)
    pub name: String,
    /// Absolute path to the repository root
    pub root_path: String,
    /// Version control system type (always "git" for now)
    pub vcs_type: &'static str,
    /// Remote URL from origin, if available
    pub remote_url: Option<String>,
}

impl RepositoryInfo {
    /// Create a new RepositoryInfo from a root path.
    pub fn from_root_path(root_path: Utf8PathBuf, remote_url: Option<String>) -> Self {
        // Generate SHA-256 hash of root path string for stable ID
        let path_str = root_path.as_str();
        let mut hasher = Sha256::new();
        hasher.update(path_str.as_bytes());
        let hash = hasher.finalize();
        let id = format!("{:x}", hash);

        // Extract name from directory name
        let name = root_path
            .file_name()
            .unwrap_or("unknown")
            .to_string();

        RepositoryInfo {
            id,
            name,
            root_path: path_str.to_string(),
            vcs_type: "git",
            remote_url,
        }
    }
}

/// Discover git repositories by analyzing manifest paths.
///
/// For each manifest path, uses `git2::Repository::discover()` to find the
/// git repository root. Deduplicates roots using a HashSet and extracts
/// repository metadata including remote URL from the "origin" remote.
///
/// # Arguments
///
/// * `manifests` - Slice of manifest file paths to analyze
///
/// # Returns
///
/// Vector of deduplicated `RepositoryInfo` structs
///
/// # Errors
///
/// Returns an error if all manifests fail to discover repositories.
/// Individual failures are logged and skipped.
///
/// # Examples
///
/// ```no_run
/// use anyhow::Result;
/// use code_intelligence_mcp_server::path::Utf8PathBuf;
/// use code_intelligence_mcp_server::indexer::package::git::discover_git_roots;
///
/// fn main() -> Result<()> {
///     let manifests = vec![
///         Utf8PathBuf::from("/path/to/repo/package.json"),
///         Utf8PathBuf::from("/path/to/repo/subdir/Cargo.toml"),
///     ];
///
///     let repos = discover_git_roots(&manifests)?;
///     // Returns single RepositoryInfo if both manifests are in the same git repo
///
///     Ok(())
/// }
/// ```
pub fn discover_git_roots(manifests: &[Utf8PathBuf]) -> Result<Vec<RepositoryInfo>> {
    let mut roots = HashSet::new();
    let mut results = Vec::new();
    let mut error_count = 0;

    for manifest_path in manifests {
        match discover_single_root(manifest_path) {
            Ok(Some(repo_info)) => {
                let root_path = repo_info.root_path.clone();
                if roots.insert(root_path) {
                    results.push(repo_info);
                }
            }
            Ok(None) => {
                // Not a git repository, skip silently
            }
            Err(e) => {
                error_count += 1;
                tracing::debug!(
                    "Failed to discover git repository for {:?}: {}",
                    manifest_path,
                    e
                );
            }
        }
    }

    // If all manifests failed, return an error
    if error_count > 0 && results.is_empty() {
        return Err(anyhow::anyhow!(
            "Failed to discover any git repositories ({} errors)",
            error_count
        ));
    }

    // Sort by root path for deterministic output
    results.sort_by(|a, b| a.root_path.cmp(&b.root_path));

    Ok(results)
}

/// Discover git repository for a single manifest path.
fn discover_single_root(manifest_path: &Utf8PathBuf) -> Result<Option<RepositoryInfo>> {
    // Convert Utf8PathBuf to std::path::Path for git2 compatibility
    let std_path = manifest_path.as_std_path();

    // Use git2::Repository::discover to find the git root
    let repo = match Repository::discover(std_path) {
        Ok(r) => r,
        Err(e) if e.class() == git2::ErrorClass::Repository => {
            // Not in a git repository
            return Ok(None);
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "Failed to discover git repository: manifest_path={}",
                    manifest_path
                )
            });
        }
    };

    // Get the workdir (repository root)
    let workdir = repo
        .workdir()
        .with_context(|| {
            format!(
                "Repository is bare, cannot determine root path: manifest_path={}",
                manifest_path
            )
        })?;

    // Convert PathBuf back to Utf8PathBuf, fall back to string if path is not valid UTF-8
    let root_path = Utf8PathBuf::from_path_buf(workdir.to_path_buf())
        .map_err(|_| anyhow::anyhow!("Repository root path is not valid UTF-8"))?;

    // Try to get the remote URL from "origin"
    let remote_url = repo.find_remote("origin").ok().and_then(|remote| {
        let bytes = remote.url_bytes();
        std::str::from_utf8(bytes).ok().map(|s| s.to_string())
    });

    Ok(Some(RepositoryInfo::from_root_path(root_path, remote_url)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::TempDir;

    /// Initialize a git repository in the given directory.
    fn init_git_repo(dir: &PathBuf) -> Result<()> {
        let status = Command::new("git")
            .arg("init")
            .current_dir(dir)
            .status()
            .with_context(|| {
                format!(
                    "Failed to run git init: dir={}",
                    dir.display()
                )
            })?;

        if !status.success() {
            return Err(anyhow::anyhow!("git init failed"));
        }

        // Configure user for commits
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir)
            .status()?;

        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(dir)
            .status()?;

        Ok(())
    }

    /// Add a remote to the git repository.
    fn add_remote(dir: &PathBuf, name: &str, url: &str) -> Result<()> {
        let status = Command::new("git")
            .args(["remote", "add", name, url])
            .current_dir(dir)
            .status()
            .with_context(|| {
                format!(
                    "Failed to add git remote: dir={}, remote_name={}, remote_url={}",
                    dir.display(),
                    name,
                    url
                )
            })?;

        if !status.success() {
            return Err(anyhow::anyhow!("git remote add failed"));
        }

        Ok(())
    }

    #[test]
    fn test_repository_info_from_root_path() {
        let root_path = Utf8PathBuf::from("/test/repo");
        let remote_url = Some("https://github.com/test/repo.git".to_string());

        let info = RepositoryInfo::from_root_path(root_path.clone(), remote_url.clone());

        assert_eq!(info.name, "repo");
        assert_eq!(info.root_path, "/test/repo");
        assert_eq!(info.vcs_type, "git");
        assert_eq!(info.remote_url, remote_url);
        assert!(!info.id.is_empty());
        assert_eq!(info.id.len(), 64); // SHA-256 hex string
    }

    #[test]
    fn test_repository_info_id_stability() {
        let root_path = Utf8PathBuf::from("/test/repo");

        let info1 = RepositoryInfo::from_root_path(root_path.clone(), None);
        let info2 = RepositoryInfo::from_root_path(root_path, None);

        // Same path should generate same ID
        assert_eq!(info1.id, info2.id);
    }

    #[test]
    fn test_repository_info_different_paths_different_ids() {
        let path1 = Utf8PathBuf::from("/test/repo1");
        let path2 = Utf8PathBuf::from("/test/repo2");

        let info1 = RepositoryInfo::from_root_path(path1, None);
        let info2 = RepositoryInfo::from_root_path(path2, None);

        // Different paths should generate different IDs
        assert_ne!(info1.id, info2.id);
    }

    #[test]
    fn test_discover_git_roots_deduplicates() {
        // Create a temporary git repository
        let temp_dir = TempDir::new().unwrap();
        let repo_root = temp_dir.path();

        init_git_repo(&repo_root.to_path_buf()).unwrap();

        // Create multiple manifest paths in the same repository
        let path_manifests = vec![
            repo_root.join("package.json"),
            repo_root.join("subdir").join("Cargo.toml"),
            repo_root.join("deep").join("nested").join("go.mod"),
        ];

        // Convert to Utf8PathBuf for our API
        let manifests: Vec<Utf8PathBuf> = path_manifests
            .iter()
            .filter_map(|p| Utf8PathBuf::from_path_buf(p.clone()).ok())
            .collect();

        // Create the directories and files
        fs::create_dir_all(repo_root.join("subdir")).unwrap();
        fs::create_dir_all(repo_root.join("deep").join("nested")).unwrap();
        for manifest in &path_manifests {
            fs::write(manifest, b"{}").unwrap();
        }

        let results = discover_git_roots(&manifests).unwrap();

        // Should return only one repository despite 3 manifests
        assert_eq!(results.len(), 1);

        // Canonicalize both paths for comparison (handles macOS /var -> /private symlinks)
        let expected = repo_root.canonicalize().unwrap();
        let actual = PathBuf::from(&results[0].root_path);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_discover_git_roots_extracts_remote_url() {
        let temp_dir = TempDir::new().unwrap();
        let repo_root = temp_dir.path();

        init_git_repo(&repo_root.to_path_buf()).unwrap();
        add_remote(
            &repo_root.to_path_buf(),
            "origin",
            "https://github.com/test/example.git",
        )
        .unwrap();

        let manifest = repo_root.join("package.json");
        fs::write(&manifest, b"{}").unwrap();

        let utf8_manifest = Utf8PathBuf::from_path_buf(manifest).unwrap();
        let results = discover_git_roots(&[utf8_manifest]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].remote_url,
            Some("https://github.com/test/example.git".to_string())
        );
    }

    #[test]
    fn test_discover_git_roots_no_remote() {
        let temp_dir = TempDir::new().unwrap();
        let repo_root = temp_dir.path();

        init_git_repo(&repo_root.to_path_buf()).unwrap();

        let manifest = repo_root.join("package.json");
        fs::write(&manifest, b"{}").unwrap();

        let utf8_manifest = Utf8PathBuf::from_path_buf(manifest).unwrap();
        let results = discover_git_roots(&[utf8_manifest]).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].remote_url, None);
    }

    #[test]
    fn test_discover_git_roots_non_git_directory() {
        let temp_dir = TempDir::new().unwrap();

        // Create a manifest without initializing git
        let manifest = temp_dir.path().join("package.json");
        fs::write(&manifest, b"{}").unwrap();

        let utf8_manifest = Utf8PathBuf::from_path_buf(manifest).unwrap();
        let results = discover_git_roots(&[utf8_manifest]).unwrap();

        // Should return empty results, not error
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_discover_git_roots_multiple_repos() {
        // Create two separate git repositories
        let temp_dir = TempDir::new().unwrap();

        let repo1 = temp_dir.path().join("repo1");
        let repo2 = temp_dir.path().join("repo2");

        fs::create_dir_all(&repo1).unwrap();
        fs::create_dir_all(&repo2).unwrap();

        init_git_repo(&repo1).unwrap();
        init_git_repo(&repo2).unwrap();

        let manifest1 = repo1.join("package.json");
        let manifest2 = repo2.join("Cargo.toml");

        fs::write(&manifest1, b"{}").unwrap();
        fs::write(&manifest2, b"[package]").unwrap();

        let utf8_manifest1 = Utf8PathBuf::from_path_buf(manifest1).unwrap();
        let utf8_manifest2 = Utf8PathBuf::from_path_buf(manifest2).unwrap();
        let results = discover_git_roots(&[utf8_manifest1, utf8_manifest2]).unwrap();

        // Should find both repositories
        assert_eq!(results.len(), 2);

        // Results should be sorted by root path
        assert!(results[0].root_path < results[1].root_path);
    }
}

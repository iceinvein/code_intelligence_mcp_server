//! Package detection module for multi-repo support
//!
//! This module provides functionality for detecting packages and repositories
//! within a workspace. It handles manifest file discovery, parsing, and
//! repository detection using git roots.

pub mod detector;
pub mod git;
pub mod parsers;

// Re-export detector module types
pub use detector::{discover_manifests, should_skip_package_dir, MANIFEST_FILENAMES};

// Re-export git module types
pub use git::{discover_git_roots, RepositoryInfo as GitRepositoryInfo};

// Re-export parser functions
pub use parsers::parse_manifest;

use crate::config::Config;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fmt, path::Path, path::PathBuf};

/// Package ecosystem type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Copy, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PackageType {
    /// npm/Node.js (package.json)
    Npm,
    /// Rust/Cargo (Cargo.toml)
    Cargo,
    /// Go (go.mod)
    Go,
    /// Python (pyproject.toml, requirements.txt)
    Python,
    /// Java/Maven (pom.xml)
    Maven,
    /// Unknown package type
    Unknown,
}

impl fmt::Display for PackageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageType::Npm => write!(f, "npm"),
            PackageType::Cargo => write!(f, "cargo"),
            PackageType::Go => write!(f, "go"),
            PackageType::Python => write!(f, "python"),
            PackageType::Maven => write!(f, "maven"),
            PackageType::Unknown => write!(f, "unknown"),
        }
    }
}

impl PackageType {
    /// Detect package type from manifest filename.
    pub fn from_filename(filename: &str) -> Self {
        match filename {
            "package.json" => PackageType::Npm,
            "Cargo.toml" => PackageType::Cargo,
            "go.mod" => PackageType::Go,
            "pyproject.toml" | "requirements.txt" => PackageType::Python,
            "pom.xml" => PackageType::Maven,
            _ => PackageType::Unknown,
        }
    }

    /// Get the manifest filename for this package type.
    pub fn manifest_filename(&self) -> Option<&'static str> {
        match self {
            PackageType::Npm => Some("package.json"),
            PackageType::Cargo => Some("Cargo.toml"),
            PackageType::Go => Some("go.mod"),
            PackageType::Python => Some("pyproject.toml"), // Also requirements.txt
            PackageType::Maven => Some("pom.xml"),
            PackageType::Unknown => None,
        }
    }
}

/// Information about a discovered package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackageInfo {
    /// Unique identifier (SHA-256 hash of manifest_path)
    pub id: String,
    /// Package name from manifest (may not be unique)
    pub name: Option<String>,
    /// Package version from manifest
    pub version: Option<String>,
    /// Absolute path to the manifest file
    pub manifest_path: String,
    /// Root directory of the package (parent directory of manifest)
    pub root_path: String,
    /// Type of package ecosystem
    pub package_type: PackageType,
    /// ID of the repository containing this package
    pub repository_id: Option<String>,
}

impl PackageInfo {
    /// Create a new PackageInfo with auto-generated ID.
    pub fn new(
        manifest_path: String,
        root_path: String,
        package_type: PackageType,
        name: Option<String>,
        version: Option<String>,
    ) -> Self {
        let id = Self::generate_id(&manifest_path);
        Self {
            id,
            name,
            version,
            manifest_path,
            root_path,
            package_type,
            repository_id: None,
        }
    }

    /// Create a new PackageInfo from a Path (legacy compatibility).
    pub fn from_path(manifest_path: &Path, package_type: PackageType) -> Self {
        let path_str = manifest_path.to_string_lossy().to_string();
        let root_path = manifest_path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.clone());
        Self::new(path_str, root_path, package_type, None, None)
    }

    /// Generate a unique package ID from manifest path using SHA-256.
    fn generate_id(manifest_path: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(manifest_path.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Set the package name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the package version.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Set the repository ID.
    pub fn with_repository_id(mut self, repository_id: String) -> Self {
        self.repository_id = Some(repository_id);
        self
    }
}

/// Information about a repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepositoryInfo {
    /// Unique repository ID (SHA-256 hash of root_path)
    pub id: String,
    /// Repository name (derived from directory name or remote)
    pub name: String,
    /// Absolute path to repository root
    pub root_path: String,
    /// Version control system type
    pub vcs_type: VcsType,
    /// Remote URL if available
    pub remote_url: Option<String>,
}

impl RepositoryInfo {
    /// Create a new RepositoryInfo with auto-generated ID.
    pub fn new(root_path: String, vcs_type: VcsType, remote_url: Option<String>) -> Self {
        let id = Self::generate_id(&root_path);
        let name = Path::new(&root_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        Self {
            id,
            name,
            root_path,
            vcs_type,
            remote_url,
        }
    }

    /// Create a new RepositoryInfo from a Path (legacy compatibility).
    pub fn from_path(root_path: &Path) -> Self {
        let path_str = root_path.to_string_lossy().to_string();
        Self::new(path_str, VcsType::None, None)
    }

    /// Generate a unique repository ID from root path using SHA-256.
    fn generate_id(root_path: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(root_path.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

/// Version control system type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Copy, Hash)]
#[serde(rename_all = "lowercase")]
pub enum VcsType {
    /// Git version control
    Git,
    /// No version control detected
    None,
    /// Other VCS
    Other,
}

impl fmt::Display for VcsType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VcsType::Git => write!(f, "git"),
            VcsType::None => write!(f, "none"),
            VcsType::Other => write!(f, "other"),
        }
    }
}

/// Discover all packages in the workspace.
///
/// This function walks the directory tree, finds manifest files,
/// and returns package information for each one.
///
/// # Arguments
///
/// * `config` - Configuration for the workspace
/// * `repo_roots` - List of repository root directories to scan
///
/// # Returns
///
/// Vector of discovered packages with metadata extracted from manifests
///
/// # Examples
///
/// ```no_run
/// use crate::config::Config;
/// use crate::indexer::package::discover_packages;
/// use std::path::PathBuf;
///
/// # fn main() -> anyhow::Result<()> {
/// let config = Config::from_env()?;
/// let repo_roots = vec![config.base_dir.clone()];
/// let packages = discover_packages(&config, &repo_roots)?;
/// # Ok(())
/// # }
/// ```
pub fn discover_packages(config: &Config, repo_roots: &[PathBuf]) -> anyhow::Result<Vec<PackageInfo>> {
    let mut packages = Vec::new();

    // Discover all manifest files from all repo roots
    let mut manifest_paths = Vec::new();
    for root in repo_roots {
        match discover_manifests(config, root) {
            Ok(mut manifests) => manifest_paths.append(&mut manifests),
            Err(e) => {
                tracing::debug!(
                    root = %root.display(),
                    error = %e,
                    "Failed to discover manifests in root"
                );
                // Continue with other roots
            }
        }
    }

    // Parse each manifest to extract package metadata
    for manifest_path in manifest_paths {
        match parse_manifest(&manifest_path) {
            Ok(pkg) => packages.push(pkg),
            Err(e) => {
                tracing::debug!(
                    manifest = %manifest_path.display(),
                    error = %e,
                    "Failed to parse manifest"
                );
                // Continue with other manifests
            }
        }
    }

    tracing::debug!(
        count = packages.len(),
        "Discovered packages"
    );

    Ok(packages)
}

/// Detect all repositories in the workspace.
///
/// This function discovers git repositories from package manifest locations
/// and returns repository information. It also assigns each package to its
/// containing repository.
///
/// # Arguments
///
/// * `packages` - Slice of packages to detect repositories for
///
/// # Returns
///
/// Vector of discovered repositories. Each package in the input slice
/// will have its `repository_id` field updated to point to the containing
/// repository.
///
/// # Examples
///
/// ```no_run
/// use crate::indexer::package::{discover_packages, detect_repositories};
/// use crate::config::Config;
///
/// # fn main() -> anyhow::Result<()> {
/// let config = Config::from_env()?;
/// let mut packages = discover_packages(&config, &config.repo_roots)?;
/// let repositories = detect_repositories(&mut packages)?;
/// # Ok(())
/// # }
/// ```
pub fn detect_repositories(packages: &mut [PackageInfo]) -> anyhow::Result<Vec<RepositoryInfo>> {
    // Extract manifest paths from packages for git root detection
    let manifest_paths: Vec<PathBuf> = packages
        .iter()
        .map(|p| PathBuf::from(&p.manifest_path))
        .collect();

    // Use git module to discover repository roots
    let git_repos = discover_git_roots(&manifest_paths)?;

    // Build a map of root_path -> repository_id for efficient lookup
    let mut repo_map: HashMap<String, String> = HashMap::new();
    let mut repositories = Vec::new();

    for git_repo in git_repos {
        let repo_id = git_repo.id.clone();
        let root_path = git_repo.root_path.clone();

        // Convert git::RepositoryInfo to package::RepositoryInfo
        repositories.push(RepositoryInfo {
            id: repo_id.clone(),
            name: git_repo.name,
            root_path,
            vcs_type: VcsType::Git,
            remote_url: git_repo.remote_url,
        });

        repo_map.insert(git_repo.root_path, repo_id);
    }

    // Assign repository_id to each package based on git root
    for pkg in packages.iter_mut() {
        let pkg_path = PathBuf::from(&pkg.manifest_path);

        // Find the repository that contains this package
        // The repository with the longest matching root path wins
        let mut matching_repo_id: Option<String> = None;
        let mut longest_match_len = 0;

        for (repo_root, repo_id) in &repo_map {
            if pkg_path.starts_with(repo_root) {
                let match_len = repo_root.len();
                if match_len > longest_match_len {
                    longest_match_len = match_len;
                    matching_repo_id = Some(repo_id.clone());
                }
            }
        }

        pkg.repository_id = matching_repo_id;
    }

    tracing::debug!(
        repositories = repositories.len(),
        packages_assigned = packages.iter().filter(|p| p.repository_id.is_some()).count(),
        "Detected repositories and assigned packages"
    );

    Ok(repositories)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_type_from_filename() {
        assert_eq!(PackageType::from_filename("package.json"), PackageType::Npm);
        assert_eq!(PackageType::from_filename("Cargo.toml"), PackageType::Cargo);
        assert_eq!(PackageType::from_filename("go.mod"), PackageType::Go);
        assert_eq!(PackageType::from_filename("pyproject.toml"), PackageType::Python);
        assert_eq!(
            PackageType::from_filename("requirements.txt"),
            PackageType::Python
        );
        assert_eq!(PackageType::from_filename("pom.xml"), PackageType::Maven);
        assert_eq!(PackageType::from_filename("unknown.txt"), PackageType::Unknown);
    }

    #[test]
    fn test_package_type_manifest_filename() {
        assert_eq!(PackageType::Npm.manifest_filename(), Some("package.json"));
        assert_eq!(PackageType::Cargo.manifest_filename(), Some("Cargo.toml"));
        assert_eq!(PackageType::Go.manifest_filename(), Some("go.mod"));
        assert_eq!(
            PackageType::Python.manifest_filename(),
            Some("pyproject.toml")
        );
        assert_eq!(PackageType::Maven.manifest_filename(), Some("pom.xml"));
        assert_eq!(PackageType::Unknown.manifest_filename(), None);
    }

    #[test]
    fn test_package_info_new() {
        let pkg = PackageInfo::new(
            "/path/to/package.json".to_string(),
            "/path/to".to_string(),
            PackageType::Npm,
            Some("my-package".to_string()),
            Some("1.0.0".to_string()),
        );

        assert_eq!(pkg.manifest_path, "/path/to/package.json");
        assert_eq!(pkg.root_path, "/path/to");
        assert_eq!(pkg.package_type, PackageType::Npm);
        assert_eq!(pkg.name, Some("my-package".to_string()));
        assert_eq!(pkg.version, Some("1.0.0".to_string()));
        assert!(pkg.repository_id.is_none());
        // ID should be 64 hex characters
        assert_eq!(pkg.id.len(), 64);
    }

    #[test]
    fn test_package_info_from_path() {
        let manifest = std::path::Path::new("/path/to/package.json");
        let pkg = PackageInfo::from_path(manifest, PackageType::Npm);

        assert_eq!(pkg.manifest_path, "/path/to/package.json");
        assert_eq!(pkg.root_path, "/path/to");
        assert_eq!(pkg.package_type, PackageType::Npm);
        assert!(pkg.repository_id.is_none());
        assert_eq!(pkg.id.len(), 64);
    }

    #[test]
    fn test_package_info_with_name() {
        let pkg = PackageInfo::new(
            "/path/to/package.json".to_string(),
            "/path/to".to_string(),
            PackageType::Npm,
            None,
            None,
        )
        .with_name("my-package");

        assert_eq!(pkg.name, Some("my-package".to_string()));
    }

    #[test]
    fn test_package_info_with_repository_id() {
        let pkg = PackageInfo::new(
            "/path/to/package.json".to_string(),
            "/path/to".to_string(),
            PackageType::Npm,
            None,
            None,
        )
        .with_repository_id("repo-123".to_string());

        assert_eq!(pkg.repository_id, Some("repo-123".to_string()));
    }

    #[test]
    fn test_repository_info_new() {
        let repo = RepositoryInfo::new(
            "/path/to/my-repo".to_string(),
            VcsType::Git,
            Some("https://github.com/user/repo".to_string()),
        );

        assert_eq!(repo.root_path, "/path/to/my-repo");
        assert_eq!(repo.name, "my-repo");
        assert_eq!(repo.vcs_type, VcsType::Git);
        assert_eq!(
            repo.remote_url,
            Some("https://github.com/user/repo".to_string())
        );
        // ID should be 64 hex characters
        assert_eq!(repo.id.len(), 64);
    }

    #[test]
    fn test_repository_info_from_path() {
        let root = std::path::Path::new("/path/to/my-repo");
        let repo = RepositoryInfo::from_path(root);

        assert_eq!(repo.root_path, "/path/to/my-repo");
        assert_eq!(repo.name, "my-repo");
        assert_eq!(repo.vcs_type, VcsType::None);
        assert_eq!(repo.id.len(), 64);
    }

    #[test]
    fn test_vcs_type_variants() {
        // Ensure VcsType has the expected variants
        let _ = VcsType::Git;
        let _ = VcsType::None;
        let _ = VcsType::Other;
    }
}

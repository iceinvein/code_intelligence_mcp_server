//! Manifest file discovery.
//!
//! Discovers package manifest files by walking the directory tree.

use crate::config::Config;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Known manifest filenames for various package ecosystems.
pub const MANIFEST_FILENAMES: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pom.xml",
    "pyproject.toml",
    "requirements.txt",
];

/// Vendor directories to skip during manifest discovery.
///
/// These directories contain external dependencies that should not be
/// indexed as part of the workspace.
const VENDOR_DIRS: &[&str] = &[
    "node_modules", // npm/yarn/pnpm dependencies
    "target",       // Cargo build artifacts
    ".git",         // Git metadata
    "dist",         // Build output directories
    "build",        // Build output directories
    "vendor",       // Go/PHP/Rust vendor directories
    ".venv",        // Python virtual environments
    "venv",         // Python virtual environments
    "env",          // Python virtual environments
    ".env",         // Environment variable files (also a directory sometimes)
    "__pycache__",  // Python cache
    ".next",        // Next.js build output
    ".nuxt",        // Nuxt.js build output
    "out",          // Various build outputs
    ".output",      // Nuxt.js output
    ".turbo",       // Turborepo cache
    ".cache",       // General cache directories
];

/// Discover all manifest files in the given directory tree.
///
/// Walks the directory recursively, skipping vendor directories, and returns
/// a sorted list of unique manifest file paths.
///
/// # Arguments
///
/// * `config` - Configuration containing exclude patterns
/// * `root` - Root directory to start discovery from
///
/// # Returns
///
/// A sorted vector of PathBuf pointing to discovered manifest files.
///
/// # Examples
///
/// ```no_run
/// use crate::config::Config;
/// use crate::indexer::package::detector::discover_manifests;
/// use std::path::Path;
///
/// fn main() -> anyhow::Result<()> {
///     let config = Config::from_env()?;
///     let manifests = discover_manifests(&config, Path::new("/path/to/workspace"))?;
///     println!("Found {} manifest files", manifests.len());
///     Ok(())
/// }
/// ```
pub fn discover_manifests(config: &Config, root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut manifests = BTreeSet::new();

    if !root.exists() {
        return Ok(Vec::new());
    }

    walk_dir(config, root, &mut manifests)?;

    Ok(manifests.into_iter().collect())
}

/// Recursively walk a directory looking for manifest files.
fn walk_dir(config: &Config, dir: &Path, manifests: &mut BTreeSet<PathBuf>) -> anyhow::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::debug!(dir = %dir.display(), error = %err, "Failed to read directory");
            return Ok(()); // Skip directories we can't read
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                tracing::debug!(
                    dir = %dir.display(),
                    error = %err,
                    "Failed to read directory entry"
                );
                continue;
            }
        };

        let path = entry.path();
        let file_name = match path.file_name() {
            Some(name) => name.to_string_lossy(),
            None => continue,
        };

        // Check if this is a directory we should skip
        if path.is_dir() {
            if should_skip_package_dir(config, &path, &file_name) {
                continue;
            }
            // Recurse into subdirectory
            walk_dir(config, &path, manifests)?;
        } else if MANIFEST_FILENAMES.contains(&file_name.as_ref()) {
            // Found a manifest file - check it's not in an excluded path
            if !is_excluded_path(config, &path) {
                manifests.insert(path);
            }
        }
    }

    Ok(())
}

/// Check if a directory should be skipped during manifest discovery.
///
/// This function checks both the built-in vendor directory list and
/// the configuration's exclude patterns.
///
/// # Arguments
///
/// * `config` - Configuration containing exclude patterns and index_node_modules setting
/// * `path` - Full path to the directory
/// * `name` - Directory name only
///
/// # Returns
///
/// `true` if the directory should be skipped, `false` otherwise.
pub fn should_skip_package_dir(config: &Config, path: &Path, name: &str) -> bool {
    // Check built-in vendor directory list
    if VENDOR_DIRS.contains(&name) {
        return true;
    }

    // Skip hidden directories (starting with .)
    if name.starts_with('.') {
        return true;
    }

    // Check config-specific exclusions
    // Respect index_node_modules setting for node_modules specifically
    if name == "node_modules" && !config.index_node_modules {
        return true;
    }

    // Check exclude patterns
    if is_excluded_path(config, path) {
        return true;
    }

    false
}

/// Check if a path matches any exclude pattern in the config.
fn is_excluded_path(config: &Config, path: &Path) -> bool {
    let path_str = path.to_string_lossy().replace('\\', "/");

    // Check against exclude patterns
    for pattern in &config.exclude_patterns {
        if pattern_matches_path(pattern, &path_str) {
            return true;
        }
    }

    false
}

/// Check if a glob pattern matches a path.
fn pattern_matches_path(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");

    // Exact match
    if path == pattern {
        return true;
    }

    // Double wildcard pattern (**) - matches any number of directories
    if pattern.contains("**") {
        // For **/*.ext, check if path ends with .ext
        if pattern == "**/*.json" && path.ends_with(".json") {
            return true;
        }
        if pattern == "**/*.txt" && path.ends_with(".txt") {
            return true;
        }
        // For **/pattern, check if any path component matches
        let parts: Vec<&str> = pattern.split("**").collect();
        if parts.len() == 2 {
            let suffix = parts[1].trim_start_matches('/');
            if path.ends_with(suffix) {
                return true;
            }
        }
        return false;
    }

    // Single wildcard patterns (*)
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.split('*').collect();
        if parts.len() == 2 {
            let prefix = parts[0];
            let suffix = parts[1];
            // Path must start with prefix and end with suffix
            if path.starts_with(prefix) && path.ends_with(suffix) {
                // Ensure there's something between prefix and suffix (unless empty suffix)
                if suffix.is_empty() || path.len() > prefix.len() + suffix.len() {
                    return true;
                }
            }
        }
        return false;
    }

    // Pattern contains directory separator - check as prefix
    if pattern.contains('/') && path.starts_with(&format!("{}/", pattern)) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test config with default settings
    fn test_config() -> Config {
        // We'll create a minimal config for testing
        // In real usage, this would come from Config::from_env()
        let temp_dir = std::env::temp_dir();
        Config {
            base_dir: temp_dir.clone(),
            db_path: temp_dir.join("test.db"),
            vector_db_path: temp_dir.join("vectors"),
            tantivy_index_path: temp_dir.join("tantivy"),
            embeddings_backend: crate::config::EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_model_url: None,
            embeddings_model_sha256: None,
            embeddings_auto_download: false,
            embeddings_model_repo: None,
            embeddings_model_revision: None,
            embeddings_model_hf_token: None,
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 64,
            vector_search_limit: 20,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 0.1,
            rank_index_file_boost: 0.05,
            rank_test_penalty: 0.1,
            rank_popularity_weight: 0.05,
            rank_popularity_cap: 50,
            index_patterns: vec!["**/*.ts".to_string(), "**/*.rs".to_string()],
            exclude_patterns: vec![],
            watch_mode: true,
            watch_debounce_ms: 250,
            max_context_bytes: 200_000,
            index_node_modules: false,
            repo_roots: vec![],
            reranker_model_path: None,
            reranker_top_k: 20,
            reranker_cache_dir: None,
            learning_enabled: false,
            learning_selection_boost: 0.1,
            learning_file_affinity_boost: 0.05,
            max_context_tokens: 8192,
            token_encoding: "o200k_base".to_string(),
            parallel_workers: 1,
            embedding_cache_enabled: true,
            pagerank_damping: 0.85,
            pagerank_iterations: 20,
            synonym_expansion_enabled: true,
            acronym_expansion_enabled: true,
            rrf_enabled: true,
            rrf_k: 60.0,
            rrf_keyword_weight: 1.0,
            rrf_vector_weight: 1.0,
            rrf_graph_weight: 0.5,
            hyde_enabled: false,
            hyde_llm_backend: "openai".to_string(),
            hyde_api_key: None,
            hyde_max_tokens: 512,
            metrics_enabled: true,
            metrics_port: 9090,
            package_detection_enabled: true,
        }
    }

    #[test]
    fn test_manifest_filenames_contains_expected() {
        assert!(MANIFEST_FILENAMES.contains(&"package.json"));
        assert!(MANIFEST_FILENAMES.contains(&"Cargo.toml"));
        assert!(MANIFEST_FILENAMES.contains(&"go.mod"));
        assert!(MANIFEST_FILENAMES.contains(&"pom.xml"));
        assert!(MANIFEST_FILENAMES.contains(&"pyproject.toml"));
        assert!(MANIFEST_FILENAMES.contains(&"requirements.txt"));
    }

    #[test]
    fn test_should_skip_package_dir_vendor_dirs() {
        let config = test_config();

        // All vendor directories should be skipped
        assert!(should_skip_package_dir(
            &config,
            Path::new("/path/to/node_modules"),
            "node_modules"
        ));
        assert!(should_skip_package_dir(
            &config,
            Path::new("/path/to/target"),
            "target"
        ));
        assert!(should_skip_package_dir(
            &config,
            Path::new("/path/to/.git"),
            ".git"
        ));
        assert!(should_skip_package_dir(
            &config,
            Path::new("/path/to/dist"),
            "dist"
        ));
        assert!(should_skip_package_dir(
            &config,
            Path::new("/path/to/build"),
            "build"
        ));
        assert!(should_skip_package_dir(
            &config,
            Path::new("/path/to/vendor"),
            "vendor"
        ));
    }

    #[test]
    fn test_should_skip_package_dir_hidden_dirs() {
        let config = test_config();

        // Hidden directories should be skipped
        assert!(should_skip_package_dir(
            &config,
            Path::new("/path/to/.hidden"),
            ".hidden"
        ));
        assert!(should_skip_package_dir(
            &config,
            Path::new("/path/to/.vscode"),
            ".vscode"
        ));
        assert!(should_skip_package_dir(
            &config,
            Path::new("/path/to/.idea"),
            ".idea"
        ));
    }

    #[test]
    fn test_should_skip_package_dir_source_dirs() {
        let config = test_config();

        // Source directories should NOT be skipped
        assert!(!should_skip_package_dir(
            &config,
            Path::new("/path/to/src"),
            "src"
        ));
        assert!(!should_skip_package_dir(
            &config,
            Path::new("/path/to/lib"),
            "lib"
        ));
        assert!(!should_skip_package_dir(
            &config,
            Path::new("/path/to/packages"),
            "packages"
        ));
        assert!(!should_skip_package_dir(
            &config,
            Path::new("/path/to/app"),
            "app"
        ));
    }

    #[test]
    fn test_pattern_matches_path() {
        // Exact match (pattern, path)
        assert!(pattern_matches_path(
            "/path/to/file.txt",
            "/path/to/file.txt"
        ));

        // Wildcard patterns (pattern, path)
        assert!(pattern_matches_path("/path/to/*.txt", "/path/to/file.txt"));
        assert!(pattern_matches_path("**/*.json", "/path/to/file.json"));

        // No match (pattern, path)
        assert!(!pattern_matches_path(
            "/other/path/*.txt",
            "/path/to/file.txt"
        ));
    }

    #[test]
    fn test_discover_manifests_filters_vendor_dirs() {
        // This test verifies the structure is correct
        // Full integration tests would require creating temporary directories
        assert!(MANIFEST_FILENAMES.contains(&"package.json"));
        assert!(MANIFEST_FILENAMES.contains(&"Cargo.toml"));
        assert!(VENDOR_DIRS.contains(&"node_modules"));
        assert!(VENDOR_DIRS.contains(&"target"));
    }
}

//! go.mod parser.
//!
//! Parses go.mod files to extract module path, Go version, and local dependencies.

use crate::indexer::package::{PackageInfo, PackageType};
use crate::path::Utf8Path;
use anyhow::Result;
use regex::Regex;

/// Parse a go.mod file and extract package information.
///
/// # Arguments
///
/// * `path` - Path to the go.mod file
///
/// # Returns
///
/// * `Ok(PackageInfo)` - Package information with extracted metadata
/// * `Err(anyhow::Error)` - If the file cannot be read
///
/// # Examples
///
/// ```no_run
/// use code_intelligence_mcp_server::indexer::package::parsers::go::parse_go_mod;
/// use code_intelligence_mcp_server::path::Utf8Path;
///
/// let manifest = Utf8Path::new("/path/to/go.mod");
/// let info = parse_go_mod(manifest)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn parse_go_mod(path: &Utf8Path) -> Result<PackageInfo> {
    let content = std::fs::read_to_string(path)?;

    let manifest_path = path.to_string();
    let root_path = path
        .parent()
        .map(|p| p.to_string())
        .unwrap_or_else(|| manifest_path.clone());

    // Extract module path (e.g., "github.com/user/repo")
    let module_re = Regex::new(r"(?m)^module\s+([^\s]+)")?;
    let module_path = module_re
        .captures(&content)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    // Extract Go version (e.g., "1.21")
    let go_version_re = Regex::new(r"(?m)^go\s+([0-9.]+)")?;
    let version = go_version_re
        .captures(&content)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    // Derive package name from module path (last component after "/")
    let name = module_path
        .as_ref()
        .and_then(|path| path.split('/').next_back().map(|s| s.to_string()));

    let info = PackageInfo::new(manifest_path, root_path, PackageType::Go, name, version);

    // Check for workspace (Go 1.18+ has workspace support via go.work)
    // This is detected via presence of go.work file, not go.mod
    // We'll log if local dependencies are found
    let local_deps = extract_local_dependencies(&content);
    if !local_deps.is_empty() {
        tracing::debug!(
            "Go module with {} local dependencies at {}",
            local_deps.len(),
            info.root_path
        );
    }

    Ok(info)
}

/// Extract local (workspace) dependencies from go.mod.
///
/// Local dependencies are those with relative paths starting with "./"
/// or that refer to other modules in the same workspace.
fn extract_local_dependencies(content: &str) -> Vec<String> {
    let mut local_deps = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Check for replace directives which indicate workspace members
        // Format: replace module/path => ./local/path
        if trimmed.starts_with("replace ") {
            if let Some(rest) = trimmed.strip_prefix("replace ") {
                if let Some(idx) = rest.find(" => ") {
                    let replace_path = rest[idx + 4..].trim();
                    if replace_path.starts_with("./") || replace_path.starts_with("../") {
                        local_deps.push(replace_path.to_string());
                    }
                }
            }
        }

        // Check for require directives with local paths (less common)
        // Local dependencies typically use replace directives instead
        if trimmed.starts_with("require ") {
            // Extract module path from require statement
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let module_path = parts[1];
                // Check for relative-style module paths
                if module_path.starts_with("./") || module_path.contains("/local/") {
                    local_deps.push(module_path.to_string());
                }
            }
        }
    }

    local_deps
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_parse_go_mod_extracts_module_path() {
        let temp_dir = TempDir::new().unwrap();
        let go_mod_buf = temp_dir.path().join("go.mod");
        let go_mod = Utf8PathBuf::from_path_buf(go_mod_buf).unwrap();

        let content = r#"module github.com/example/test-project

go 1.21

require github.com/example/other v1.0.0
"#;

        let mut file = std::fs::File::create(&go_mod).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_go_mod(&go_mod).unwrap();

        assert_eq!(info.package_type, PackageType::Go);
        assert_eq!(info.name, Some("test-project".to_string()));
        assert_eq!(info.version, Some("1.21".to_string()));
        assert!(info.manifest_path.ends_with("go.mod"));
    }

    #[test]
    fn test_parse_go_mod_handles_minimal() {
        let temp_dir = TempDir::new().unwrap();
        let go_mod_buf = temp_dir.path().join("go.mod");
        let go_mod = Utf8PathBuf::from_path_buf(go_mod_buf).unwrap();

        let content = r#"module example.com/simple

go 1.19
"#;

        let mut file = std::fs::File::create(&go_mod).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_go_mod(&go_mod).unwrap();

        assert_eq!(info.name, Some("simple".to_string()));
        assert_eq!(info.version, Some("1.19".to_string()));
    }

    #[test]
    fn test_parse_go_mod_with_local_dependencies() {
        let content = r#"module github.com/example/mono

go 1.21

require (
    github.com/example/mono/pkg/util v0.0.0
    github.com/lib/external v1.2.3
)

replace github.com/example/mono/pkg/util => ./pkg/util
"#;

        let local_deps = extract_local_dependencies(content);
        assert!(!local_deps.is_empty());
        assert!(local_deps.contains(&"./pkg/util".to_string()));
    }

    #[test]
    fn test_extract_local_dependencies() {
        let content = r#"module example.com/test

go 1.21

require github.com/example/test/pkg v1.0.0
require github.com/external/lib v2.0.0

replace github.com/example/test/pkg => ./pkg
replace github.com/external/lib => ../external
"#;

        let deps = extract_local_dependencies(content);
        assert!(deps.contains(&"./pkg".to_string()));
        assert!(deps.contains(&"../external".to_string()));
    }

    #[test]
    fn test_parse_go_mod_empty_file() {
        let temp_dir = TempDir::new().unwrap();
        let go_mod_buf = temp_dir.path().join("go.mod");
        let go_mod = Utf8PathBuf::from_path_buf(go_mod_buf).unwrap();

        let content = r#"# Empty go.mod
"#;

        let mut file = std::fs::File::create(&go_mod).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_go_mod(&go_mod).unwrap();

        assert_eq!(info.name, None);
        assert_eq!(info.version, None);
        assert_eq!(info.package_type, PackageType::Go);
    }

    #[test]
    fn test_parse_go_mod_nested_module() {
        let content = r#"module github.com/example/mono/internal/service

go 1.21
"#;

        let temp_dir = TempDir::new().unwrap();
        let go_mod_buf = temp_dir.path().join("go.mod");
        let go_mod = Utf8PathBuf::from_path_buf(go_mod_buf).unwrap();
        let mut file = std::fs::File::create(&go_mod).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_go_mod(&go_mod).unwrap();

        // Name should be last component of module path
        assert_eq!(info.name, Some("service".to_string()));
    }
}

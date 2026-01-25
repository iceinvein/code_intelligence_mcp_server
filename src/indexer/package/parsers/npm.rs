//! npm package.json parser.
//!
//! Parses package.json files to extract package name, version, and workspace configuration.

use crate::indexer::package::{PackageInfo, PackageType};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

/// Parse a package.json file and extract package information.
///
/// # Arguments
///
/// * `path` - Path to the package.json file
///
/// # Returns
///
/// * `Ok(PackageInfo)` - Package information with extracted metadata
/// * `Err(anyhow::Error)` - If the file cannot be read or parsed
///
/// # Examples
///
/// ```no_run
/// use code_intelligence_mcp_server::indexer::package::parsers::npm::parse_package_json;
/// use std::path::Path;
///
/// let manifest = Path::new("/path/to/package.json");
/// let info = parse_package_json(manifest)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn parse_package_json(path: &Path) -> Result<PackageInfo> {
    let content = std::fs::read_to_string(path)?;
    let json: Value = serde_json::from_str(&content)?;

    let manifest_path = path.to_string_lossy().to_string();
    let root_path = path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| manifest_path.clone());

    // Extract package name
    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Extract version
    let version = json
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let info = PackageInfo::new(manifest_path, root_path, PackageType::Npm, name, version);

    // Check for workspace configuration (monorepo detection)
    if has_workspaces(&json) {
        tracing::debug!("npm workspace detected at {}", info.root_path);
        // Workspace packages will be handled by the detector
    }

    Ok(info)
}

/// Check if a package.json has workspace configuration.
///
/// npm workspaces can be specified as:
/// - Array: "workspaces": ["packages/*"]
/// - Object: "workspaces": { "packages": ["packages/*"] }
fn has_workspaces(json: &Value) -> bool {
    if let Some(workspaces) = json.get("workspaces") {
        // Check if it's an array (direct workspace packages list)
        if workspaces.is_array() {
            return true;
        }
        // Check if it's an object with "packages" field
        if let Some(obj) = workspaces.as_object() {
            return obj.contains_key("packages");
        }
    }

    // Also check for older "packages" array (Yarn workspaces v1)
    json.get("packages").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_parse_package_json_extracts_name_version() {
        let temp_dir = TempDir::new().unwrap();
        let package_json = temp_dir.path().join("package.json");

        let content = r#"{
            "name": "test-package",
            "version": "1.0.0",
            "description": "Test package"
        }"#;

        let mut file = std::fs::File::create(&package_json).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_package_json(&package_json).unwrap();

        assert_eq!(info.package_type, PackageType::Npm);
        assert_eq!(info.name, Some("test-package".to_string()));
        assert_eq!(info.version, Some("1.0.0".to_string()));
        assert!(info.manifest_path.ends_with("package.json"));
    }

    #[test]
    fn test_parse_package_json_handles_missing_fields() {
        let temp_dir = TempDir::new().unwrap();
        let package_json = temp_dir.path().join("package.json");

        let content = r#"{
            "name": "minimal-package"
        }"#;

        let mut file = std::fs::File::create(&package_json).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_package_json(&package_json).unwrap();

        assert_eq!(info.name, Some("minimal-package".to_string()));
        assert_eq!(info.version, None);
    }

    #[test]
    fn test_parse_package_json_detects_workspaces() {
        // Array-style workspaces
        let json_array: Value =
            serde_json::from_str(r#"{"name": "mono", "workspaces": ["packages/*"]}"#).unwrap();
        assert!(has_workspaces(&json_array));

        // Object-style workspaces
        let json_object: Value =
            serde_json::from_str(r#"{"name": "mono", "workspaces": {"packages": ["packages/*"]}}"#)
                .unwrap();
        assert!(has_workspaces(&json_object));

        // No workspaces
        let json_none: Value = serde_json::from_str(r#"{"name": "single"}"#).unwrap();
        assert!(!has_workspaces(&json_none));
    }

    #[test]
    fn test_parse_package_json_empty_file() {
        let temp_dir = TempDir::new().unwrap();
        let package_json = temp_dir.path().join("package.json");

        let content = r#"{}"#;

        let mut file = std::fs::File::create(&package_json).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_package_json(&package_json).unwrap();

        assert_eq!(info.name, None);
        assert_eq!(info.version, None);
        assert_eq!(info.package_type, PackageType::Npm);
    }

    #[test]
    fn test_has_workspaces_yarn_v1() {
        let json: Value =
            serde_json::from_str(r#"{"name": "yarn-mono", "packages": ["packages/*"]}"#).unwrap();
        assert!(has_workspaces(&json));
    }
}

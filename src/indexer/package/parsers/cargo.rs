//! Cargo.toml parser.
//!
//! Parses Cargo.toml files to extract package name, version, and workspace configuration.

use crate::indexer::package::{PackageInfo, PackageType};
use anyhow::Result;
use std::path::Path;

/// Parse a Cargo.toml file and extract package information.
///
/// # Arguments
///
/// * `path` - Path to the Cargo.toml file
///
/// # Returns
///
/// * `Ok(PackageInfo)` - Package information with extracted metadata
/// * `Err(anyhow::Error)` - If the file cannot be read or parsed
///
/// # Examples
///
/// ```no_run
/// use crate::indexer::package::parsers::cargo::parse_cargo_toml;
/// use std::path::Path;
///
/// let manifest = Path::new("/path/to/Cargo.toml");
/// let info = parse_cargo_toml(manifest)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn parse_cargo_toml(path: &Path) -> Result<PackageInfo> {
    let content = std::fs::read_to_string(path)?;
    let toml: toml::Value = toml::from_str(&content)?;

    let manifest_path = path.to_string_lossy().to_string();
    let root_path = path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| manifest_path.clone());

    // Extract package name from [package.name]
    let name = toml
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Extract version from [package.version]
    let version = toml
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let info = PackageInfo::new(manifest_path, root_path, PackageType::Cargo, name, version);

    // Check for workspace configuration
    if has_workspace_members(&toml) {
        tracing::debug!("Cargo workspace detected at {}", info.root_path);
        // Workspace members will be handled by the detector
    }

    Ok(info)
}

/// Check if a Cargo.toml has workspace members configuration.
///
/// Cargo workspaces can be specified as:
/// - [workspace.members] array: members = ["crate1", "crate2"]
/// - [workspace] table presence (workspace root)
fn has_workspace_members(toml: &toml::Value) -> bool {
    if let Some(workspace) = toml.get("workspace") {
        // Check for members array
        if let Some(members) = workspace.get("members") {
            if members.is_array() {
                let arr = members.as_array().unwrap();
                return !arr.is_empty();
            }
        }
        // Presence of [workspace] table indicates workspace root
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_parse_cargo_toml_extracts_name_version() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        let content = r#"[package]
name = "test-crate"
version = "1.0.0"
description = "Test crate"
"#;

        let mut file = std::fs::File::create(&cargo_toml).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_cargo_toml(&cargo_toml).unwrap();

        assert_eq!(info.package_type, PackageType::Cargo);
        assert_eq!(info.name, Some("test-crate".to_string()));
        assert_eq!(info.version, Some("1.0.0".to_string()));
        assert!(info.manifest_path.ends_with("Cargo.toml"));
    }

    #[test]
    fn test_parse_cargo_toml_handles_missing_fields() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        let content = r#"[package]
name = "minimal-crate"
"#;

        let mut file = std::fs::File::create(&cargo_toml).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_cargo_toml(&cargo_toml).unwrap();

        assert_eq!(info.name, Some("minimal-crate".to_string()));
        assert_eq!(info.version, None);
    }

    #[test]
    fn test_parse_cargo_toml_empty_file() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        let content = r#"# Empty Cargo.toml
"#;

        let mut file = std::fs::File::create(&cargo_toml).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_cargo_toml(&cargo_toml).unwrap();

        assert_eq!(info.name, None);
        assert_eq!(info.version, None);
        assert_eq!(info.package_type, PackageType::Cargo);
    }

    #[test]
    fn test_has_workspace_members() {
        // Workspace with members array
        let toml_with_members: toml::Value = toml::from_str(
            r#"[workspace]
members = ["crate1", "crate2"]
"#,
        )
        .unwrap();
        assert!(has_workspace_members(&toml_with_members));

        // Workspace table without members
        let toml_workspace: toml::Value = toml::from_str(
            r#"[workspace]
resolver = "2"
"#,
        )
        .unwrap();
        assert!(has_workspace_members(&toml_workspace));

        // Regular package (no workspace)
        let toml_package: toml::Value = toml::from_str(
            r#"[package]
name = "single"
version = "0.1.0"
"#,
        )
        .unwrap();
        assert!(!has_workspace_members(&toml_package));
    }

    #[test]
    fn test_parse_cargo_toml_workspace_root() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        let content = r#"[workspace]
members = ["member1", "member2"]

[workspace.package]
version = "0.1.0"
"#;

        let mut file = std::fs::File::create(&cargo_toml).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_cargo_toml(&cargo_toml).unwrap();

        // Workspace root doesn't have [package] table, so name/version are None
        assert_eq!(info.name, None);
        assert_eq!(info.version, None);
        assert_eq!(info.package_type, PackageType::Cargo);
    }

    #[test]
    fn test_parse_cargo_toml_virtual_workspace() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        // Virtual workspace with [workspace.metadata]
        let content = r#"[workspace]
members = ["crates/*"]

[workspace.metadata.example]
key = "value"
"#;

        let mut file = std::fs::File::create(&cargo_toml).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_cargo_toml(&cargo_toml).unwrap();

        assert_eq!(info.name, None);
        assert_eq!(info.version, None);
        assert_eq!(info.package_type, PackageType::Cargo);
    }
}

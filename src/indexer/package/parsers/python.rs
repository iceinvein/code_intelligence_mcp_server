//! pyproject.toml parser.
//!
//! Parses pyproject.toml files to extract package name, version, and dependencies.
//! Supports both PEP 621 and Poetry formats.

use crate::indexer::package::{PackageInfo, PackageType};
use anyhow::Result;
use std::path::Path;

/// Parse a pyproject.toml file and extract package information.
///
/// Supports both PEP 621 (standard) and Poetry formats.
///
/// # Arguments
///
/// * `path` - Path to the pyproject.toml file
///
/// # Returns
///
/// * `Ok(PackageInfo)` - Package information with extracted metadata
/// * `Err(anyhow::Error)` - If the file cannot be read or parsed
///
/// # Examples
///
/// ```no_run
/// use code_intelligence_mcp_server::indexer::package::parsers::python::parse_pyproject_toml;
/// use std::path::Path;
///
/// let manifest = Path::new("/path/to/pyproject.toml");
/// let info = parse_pyproject_toml(manifest)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn parse_pyproject_toml(path: &Path) -> Result<PackageInfo> {
    let content = std::fs::read_to_string(path)?;
    let toml: toml::Value = toml::from_str(&content)?;

    let manifest_path = path.to_string_lossy().to_string();
    let root_path = path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| manifest_path.clone());

    // Try PEP 621 format first: [project.name], [project.version]
    let (name, version) = if let Some(project) = toml.get("project") {
        let name = project
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let version = project
            .get("version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        (name, version)
    } else {
        // Fall back to Poetry format: [tool.poetry.name], [tool.poetry.version]
        let name = toml
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let version = toml
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        (name, version)
    };

    let info = PackageInfo::new(manifest_path, root_path, PackageType::Python, name, version);

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_parse_pyproject_toml_pep621() {
        let temp_dir = TempDir::new().unwrap();
        let pyproject = temp_dir.path().join("pyproject.toml");

        let content = r#"[project]
name = "test-package"
version = "1.0.0"
description = "Test package"
requires-python = ">=3.8"
"#;

        let mut file = std::fs::File::create(&pyproject).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_pyproject_toml(&pyproject).unwrap();

        assert_eq!(info.package_type, PackageType::Python);
        assert_eq!(info.name, Some("test-package".to_string()));
        assert_eq!(info.version, Some("1.0.0".to_string()));
        assert!(info.manifest_path.ends_with("pyproject.toml"));
    }

    #[test]
    fn test_parse_pyproject_toml_poetry() {
        let temp_dir = TempDir::new().unwrap();
        let pyproject = temp_dir.path().join("pyproject.toml");

        let content = r#"[tool.poetry]
name = "poetry-package"
version = "2.0.0"
description = "Poetry package"
authors = ["Test Author"]

[tool.poetry.dependencies]
python = "^3.9"
"#;

        let mut file = std::fs::File::create(&pyproject).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_pyproject_toml(&pyproject).unwrap();

        assert_eq!(info.package_type, PackageType::Python);
        assert_eq!(info.name, Some("poetry-package".to_string()));
        assert_eq!(info.version, Some("2.0.0".to_string()));
    }

    #[test]
    fn test_parse_pyproject_toml_handles_missing_fields() {
        let temp_dir = TempDir::new().unwrap();
        let pyproject = temp_dir.path().join("pyproject.toml");

        let content = r#"[project]
name = "minimal-package"
"#;

        let mut file = std::fs::File::create(&pyproject).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_pyproject_toml(&pyproject).unwrap();

        assert_eq!(info.name, Some("minimal-package".to_string()));
        assert_eq!(info.version, None);
    }

    #[test]
    fn test_parse_pyproject_toml_empty_file() {
        let temp_dir = TempDir::new().unwrap();
        let pyproject = temp_dir.path().join("pyproject.toml");

        let content = r#"# Empty pyproject.toml
"#;

        let mut file = std::fs::File::create(&pyproject).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_pyproject_toml(&pyproject).unwrap();

        assert_eq!(info.name, None);
        assert_eq!(info.version, None);
        assert_eq!(info.package_type, PackageType::Python);
    }

    #[test]
    fn test_parse_pyproject_toml_pep621_with_dependencies() {
        let temp_dir = TempDir::new().unwrap();
        let pyproject = temp_dir.path().join("pyproject.toml");

        let content = r#"[project]
name = "dep-package"
version = "1.0.0"

dependencies = [
    "requests>=2.28.0",
    "pydantic>=2.0.0",
]

[project.optional-dependencies]
dev = ["pytest>=7.0.0"]
"#;

        let mut file = std::fs::File::create(&pyproject).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_pyproject_toml(&pyproject).unwrap();

        assert_eq!(info.name, Some("dep-package".to_string()));
        assert_eq!(info.version, Some("1.0.0".to_string()));
    }

    #[test]
    fn test_parse_pyproject_toml_poetry_with_workspace() {
        let temp_dir = TempDir::new().unwrap();
        let pyproject = temp_dir.path().join("pyproject.toml");

        // Poetry doesn't have native workspace support, but use tools like
        // poetry-workspace-plugin or multi-project repos
        let content = r#"[tool.poetry]
name = "mono-package"
version = "1.0.0"

[tool.poetry.dependencies]
python = "^3.9"

# Local package dependency
local-pkg = {path = "../local-pkg", develop = true}
"#;

        let mut file = std::fs::File::create(&pyproject).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let info = parse_pyproject_toml(&pyproject).unwrap();

        assert_eq!(info.name, Some("mono-package".to_string()));
        assert_eq!(info.version, Some("1.0.0".to_string()));
    }
}

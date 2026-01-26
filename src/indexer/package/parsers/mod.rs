//! Language-specific manifest parsers.
//!
//! This module contains parsers for different package manifest formats:
//! - npm (package.json)
//! - Rust/Cargo (Cargo.toml)
//! - Go (go.mod)
//! - Python (pyproject.toml)

pub mod cargo;
pub mod go;
pub mod npm;
pub mod python;

// Re-export types from parent module
pub use crate::indexer::package::{PackageInfo, PackageType};
use crate::path::Utf8Path;

// Re-export parser functions for public use
pub use cargo::parse_cargo_toml;
pub use go::parse_go_mod;
pub use npm::parse_package_json;
pub use python::parse_pyproject_toml;

/// Parse a package manifest file and return package information.
///
/// This dispatcher function routes to the appropriate language-specific parser
/// based on the manifest filename.
///
/// # Arguments
///
/// * `path` - Path to the manifest file
///
/// # Returns
///
/// * `Ok(PackageInfo)` - Package information with extracted metadata
/// * `Err(anyhow::Error)` - If the file cannot be read or a critical error occurs
///
/// # Examples
///
/// ```no_run
/// use code_intelligence_mcp_server::indexer::package::parsers::parse_manifest;
/// use std::path::Path;
///
/// let manifest = Path::new("/path/to/package.json");
/// let info = parse_manifest(manifest)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn parse_manifest(path: &Utf8Path) -> anyhow::Result<PackageInfo> {
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid manifest path: no filename"))?;

    let package_type = PackageType::from_filename(file_name);

    // Route to appropriate parser based on filename
    let result = match file_name {
        "package.json" => parse_package_json(path)?,
        "Cargo.toml" => parse_cargo_toml(path)?,
        "go.mod" => parse_go_mod(path)?,
        "pyproject.toml" => parse_pyproject_toml(path)?,
        _ => {
            // Unknown manifest type - return basic PackageInfo
            tracing::debug!("Unknown manifest type: {}", file_name);
            PackageInfo::from_path(path, package_type)
        }
    };

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_manifest_dispatches_correctly() {
        // Test that PackageType::from_filename works correctly
        assert_eq!(PackageType::from_filename("package.json"), PackageType::Npm);
        assert_eq!(PackageType::from_filename("Cargo.toml"), PackageType::Cargo);
        assert_eq!(PackageType::from_filename("go.mod"), PackageType::Go);
        assert_eq!(
            PackageType::from_filename("pyproject.toml"),
            PackageType::Python
        );
        assert_eq!(PackageType::from_filename("pom.xml"), PackageType::Maven);
        assert_eq!(
            PackageType::from_filename("unknown.txt"),
            PackageType::Unknown
        );
    }
}

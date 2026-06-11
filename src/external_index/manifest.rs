use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProducerManifest {
    pub schema_version: u32,
    pub producers: Vec<ProducerManifestEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProducerManifestEntry {
    pub id: String,
    pub language: String,
    pub executable: String,
    pub tier: String,
    pub output_file: String,
    pub requires_project_toolchain: bool,
    pub description: String,
}

pub fn bundled_manifest() -> Result<ProducerManifest> {
    serde_json::from_str(include_str!("../../producers/manifest.json"))
        .context("Failed to parse bundled external producer manifest")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_lists_every_supported_producer() {
        let manifest = bundled_manifest().expect("manifest parses");
        let ids = manifest
            .producers
            .iter()
            .map(|producer| producer.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(
            ids,
            [
                "c",
                "cpp",
                "csharp",
                "go",
                "java",
                "kotlin",
                "python",
                "ruby",
                "rust",
                "swift",
                "typescript"
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
        );
    }

    #[test]
    fn manifest_executable_names_use_code_intelligence_prefix() {
        let manifest = bundled_manifest().expect("manifest parses");

        for producer in manifest.producers {
            assert!(
                producer.executable.starts_with("code-intelligence-external-"),
                "unexpected executable for {}: {}",
                producer.id,
                producer.executable
            );
        }
    }
}

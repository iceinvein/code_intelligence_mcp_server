use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProducerAvailability {
    pub id: String,
    pub language: String,
    pub tier: String,
    pub executable: String,
    pub availability: String,
}

pub fn bundled_manifest() -> Result<ProducerManifest> {
    serde_json::from_str(include_str!("../../producers/manifest.json"))
        .context("Failed to parse bundled external producer manifest")
}

pub fn producer_availability() -> Result<Vec<ProducerAvailability>> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
    producer_availability_for_dir(exe_dir.as_deref())
}

pub fn producer_availability_for_dir(exe_dir: Option<&Path>) -> Result<Vec<ProducerAvailability>> {
    let manifest = bundled_manifest()?;
    Ok(manifest
        .producers
        .into_iter()
        .map(|producer| {
            let bundled_path = exe_dir.map(|dir| dir.join(&producer.executable));
            let bundled_executable = bundled_path
                .as_deref()
                .filter(|path| super::producers::is_executable(path));
            let (executable, availability) = match bundled_executable {
                Some(path) => (path.to_string_lossy().into_owned(), "bundled"),
                None => (producer.executable, "missing"),
            };

            ProducerAvailability {
                id: producer.id,
                language: producer.language,
                tier: producer.tier,
                executable,
                availability: availability.to_string(),
            }
        })
        .collect())
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
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec![
                "typescript",
                "rust",
                "python",
                "go",
                "java",
                "kotlin",
                "csharp",
                "swift",
                "c",
                "cpp",
                "ruby"
            ]
        );
    }

    #[test]
    fn manifest_executable_names_use_code_intelligence_prefix() {
        let manifest = bundled_manifest().expect("manifest parses");

        for producer in manifest.producers {
            assert!(
                producer
                    .executable
                    .starts_with("code-intelligence-external-"),
                "unexpected executable for {}: {}",
                producer.id,
                producer.executable
            );
        }
    }

    #[test]
    fn producer_availability_marks_missing_when_executable_absent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let availability = producer_availability_for_dir(Some(temp.path())).expect("availability");
        let rust = availability
            .iter()
            .find(|producer| producer.id == "rust")
            .expect("rust producer");

        assert_eq!(rust.availability, "missing");
        assert_eq!(rust.executable, "code-intelligence-external-rust");
    }
}

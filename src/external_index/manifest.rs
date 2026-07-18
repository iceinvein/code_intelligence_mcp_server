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
    pub readiness: String,
    pub output_file: String,
    pub requires_project_toolchain: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProducerAvailability {
    pub id: String,
    pub language: String,
    pub tier: String,
    pub readiness: String,
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
            let (executable, availability) = match (producer.readiness.as_str(), bundled_executable)
            {
                ("adapter_only", Some(path)) => {
                    (path.to_string_lossy().into_owned(), "adapter_only")
                }
                ("adapter_only", None) => (producer.executable, "adapter_only"),
                (_, Some(path)) => (path.to_string_lossy().into_owned(), "bundled"),
                (_, None) => (producer.executable, "missing"),
            };

            ProducerAvailability {
                id: producer.id,
                language: producer.language,
                tier: producer.tier,
                readiness: producer.readiness,
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
        assert_eq!(manifest.schema_version, 2);
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

    #[test]
    fn manifest_and_availability_expose_adapter_only_producers() {
        let manifest = bundled_manifest().expect("manifest parses");
        assert_eq!(
            manifest
                .producers
                .iter()
                .filter(|producer| producer.readiness == "integrated")
                .map(|producer| producer.id.as_str())
                .collect::<Vec<_>>(),
            vec!["typescript", "rust", "python", "go"]
        );

        let temp = tempfile::tempdir().expect("tempdir");
        let availability = producer_availability_for_dir(Some(temp.path())).expect("availability");
        let java = availability
            .iter()
            .find(|producer| producer.id == "java")
            .expect("java producer");
        assert_eq!(java.readiness, "adapter_only");
        assert_eq!(java.availability, "adapter_only");
    }
}

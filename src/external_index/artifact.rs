use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct NormalizedExternalIndex {
    pub source_kind: String,
    pub producer: String,
    pub language: String,
    pub root_path: String,
    pub symbols: Vec<NormalizedExternalSymbol>,
    pub references: Vec<NormalizedExternalReference>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NormalizedExternalSymbol {
    pub external_symbol: String,
    pub display_name: String,
    pub kind: String,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub start_byte: Option<u32>,
    pub end_byte: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NormalizedExternalReference {
    pub from_external_symbol: Option<String>,
    pub to_external_symbol: Option<String>,
    pub relationship: String,
    pub file_path: String,
    pub line: u32,
    pub column: Option<u32>,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
    pub confidence: Option<f32>,
    pub provenance: Option<String>,
}

pub fn read_normalized_artifact(path: &Path) -> Result<NormalizedExternalIndex> {
    let bytes = fs::read(path).with_context(|| {
        format!(
            "Failed to read normalized external index: {}",
            path.display()
        )
    })?;
    parse_normalized_artifact_from_slice(&bytes, path)
}

pub fn parse_normalized_artifact_from_slice(
    bytes: &[u8],
    path_for_context: &Path,
) -> Result<NormalizedExternalIndex> {
    serde_json::from_slice(bytes).with_context(|| {
        format!(
            "Failed to parse normalized external index: {}",
            path_for_context.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn parses_normalized_artifact_from_slice() {
        let artifact = parse_normalized_artifact_from_slice(
            br#"{
              "source_kind": "normalized_json",
              "producer": "unit-test",
              "language": "typescript",
              "root_path": "/fixture/repo",
              "symbols": [],
              "references": []
            }"#,
            Path::new("unit-fixture.json"),
        )
        .expect("parse artifact");

        assert_eq!(artifact.source_kind, "normalized_json");
        assert_eq!(artifact.producer, "unit-test");
        assert!(artifact.symbols.is_empty());
        assert!(artifact.references.is_empty());
    }

    #[test]
    fn parse_error_includes_context_path() {
        let error = parse_normalized_artifact_from_slice(b"{", Path::new("broken-artifact.json"))
            .expect_err("parse should fail");

        assert!(error.to_string().contains("broken-artifact.json"));
    }
}

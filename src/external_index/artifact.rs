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
    let json = fs::read_to_string(path).with_context(|| {
        format!(
            "Failed to read normalized external index: {}",
            path.display()
        )
    })?;
    serde_json::from_str(&json).with_context(|| {
        format!(
            "Failed to parse normalized external index: {}",
            path.display()
        )
    })
}

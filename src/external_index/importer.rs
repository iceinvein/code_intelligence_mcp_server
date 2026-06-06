use std::{collections::HashMap, fs, path::Path};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::external_index::artifact::{read_normalized_artifact, NormalizedExternalSymbol};
use crate::storage::sqlite::queries::external::{
    ExternalIndexInsert, ExternalReferenceInsert, ExternalSymbolInsert, SymbolMappingInsert,
};
use crate::storage::sqlite::{SqliteStore, SymbolRow};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub index_id: String,
    pub symbols_imported: usize,
    pub references_imported: usize,
    pub symbols_mapped: usize,
    pub symbols_unmapped: usize,
}

pub fn import_external_index(
    store: &SqliteStore,
    repo_root: &str,
    artifact_path: &Path,
) -> Result<ImportReport> {
    let artifact_bytes = fs::read(artifact_path).with_context(|| {
        format!(
            "Failed to read normalized external index artifact: {}",
            artifact_path.display()
        )
    })?;
    let artifact_hash = sha256_hex(&artifact_bytes);
    let index_hash = first_16(&artifact_hash);
    let index_id = format!("external:{index_hash}");
    let artifact = read_normalized_artifact(artifact_path)?;

    store.upsert_external_index(&ExternalIndexInsert {
        id: &index_id,
        source_kind: &artifact.source_kind,
        producer: &artifact.producer,
        language: &artifact.language,
        root_path: if repo_root.is_empty() {
            &artifact.root_path
        } else {
            repo_root
        },
        artifact_path: &artifact_path.to_string_lossy(),
        artifact_hash: &artifact_hash,
        status: "ready",
        diagnostics_json: "{}",
    })?;

    let mut symbol_ids = HashMap::with_capacity(artifact.symbols.len());
    let mut symbols_mapped = 0;
    let mut symbols_unmapped = 0;

    for symbol in &artifact.symbols {
        let external_symbol_id = stable_external_symbol_id(index_hash, &symbol.external_symbol);
        let normalized_file_path = symbol
            .file_path
            .as_deref()
            .map(normalize_repo_relative_path)
            .transpose()?;

        store.upsert_external_symbol(&ExternalSymbolInsert {
            id: &external_symbol_id,
            external_index_id: &index_id,
            external_symbol: &symbol.external_symbol,
            display_name: &symbol.display_name,
            language: &artifact.language,
            kind: &symbol.kind,
            file_path: normalized_file_path.as_deref(),
            start_line: symbol.start_line,
            end_line: symbol.end_line,
            start_byte: symbol.start_byte,
            end_byte: symbol.end_byte,
            metadata_json: "{}",
        })?;

        if let Some((internal_symbol_id, mapping_kind, confidence)) =
            find_internal_symbol_mapping(store, symbol, normalized_file_path.as_deref())?
        {
            store.upsert_symbol_mapping(&SymbolMappingInsert {
                external_symbol_id: &external_symbol_id,
                internal_symbol_id: &internal_symbol_id,
                mapping_kind,
                confidence,
            })?;
            symbols_mapped += 1;
        } else {
            symbols_unmapped += 1;
        }

        symbol_ids.insert(symbol.external_symbol.clone(), external_symbol_id);
    }

    let provenance_default = artifact.source_kind.as_str();
    let mut references_imported = 0;
    for reference in &artifact.references {
        let normalized_file_path = normalize_repo_relative_path(&reference.file_path)?;
        let from_external_symbol_id = reference
            .from_external_symbol
            .as_deref()
            .and_then(|symbol| symbol_ids.get(symbol))
            .map(String::as_str);
        let to_external_symbol_id = reference
            .to_external_symbol
            .as_deref()
            .and_then(|symbol| symbol_ids.get(symbol))
            .map(String::as_str);
        let provenance = reference
            .provenance
            .as_deref()
            .unwrap_or(provenance_default);

        store.upsert_external_reference(&ExternalReferenceInsert {
            external_index_id: &index_id,
            from_external_symbol_id,
            to_external_symbol_id,
            relationship: &reference.relationship,
            file_path: &normalized_file_path,
            line: reference.line,
            column: reference.column,
            end_line: reference.end_line,
            end_column: reference.end_column,
            confidence: reference.confidence.unwrap_or(1.0),
            provenance,
            metadata_json: "{}",
        })?;
        references_imported += 1;
    }

    Ok(ImportReport {
        index_id,
        symbols_imported: artifact.symbols.len(),
        references_imported,
        symbols_mapped,
        symbols_unmapped,
    })
}

fn find_internal_symbol_mapping(
    store: &SqliteStore,
    external_symbol: &NormalizedExternalSymbol,
    normalized_file_path: Option<&str>,
) -> Result<Option<(String, &'static str, f32)>> {
    let Some(file_path) = normalized_file_path else {
        return Ok(None);
    };

    let candidates =
        store.search_symbols_by_exact_name(&external_symbol.display_name, Some(file_path), 50)?;
    if let Some(exact) = candidates
        .iter()
        .find(|candidate| exact_range(candidate, external_symbol))
    {
        return Ok(Some((exact.id.clone(), "exact_range", 1.0)));
    }
    Ok(candidates
        .first()
        .map(|candidate| (candidate.id.clone(), "same_file_name", 0.8)))
}

fn exact_range(candidate: &SymbolRow, external_symbol: &NormalizedExternalSymbol) -> bool {
    Some(candidate.start_line) == external_symbol.start_line
        && Some(candidate.end_line) == external_symbol.end_line
}

fn stable_external_symbol_id(index_hash: &str, external_symbol: &str) -> String {
    let symbol_hash = sha256_hex(external_symbol.as_bytes());
    format!("external:{index_hash}:{}", first_16(&symbol_hash))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn first_16(hash: &str) -> &str {
    &hash[..16]
}

fn normalize_repo_relative_path(path: &str) -> Result<String> {
    let normalized_separators = path.replace('\\', "/");
    if normalized_separators.starts_with('/') {
        bail!("External index path must be repo-relative: {path}");
    }

    let mut parts = Vec::new();
    for part in normalized_separators.split('/') {
        match part {
            "" | "." => {}
            ".." => bail!("External index path escapes the repository: {path}"),
            part => parts.push(part),
        }
    }

    if parts.is_empty() {
        bail!("External index path must not be empty");
    }
    Ok(parts.join("/"))
}

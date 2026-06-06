use std::{borrow::Cow, collections::HashMap, fs, path::Path};

use anyhow::{bail, Context, Result};
use rusqlite::params;
use sha2::{Digest, Sha256};

use crate::external_index::artifact::{
    parse_normalized_artifact_from_slice, NormalizedExternalSymbol,
};
use crate::storage::sqlite::queries::external::{
    self, ExternalIndexInsert, ExternalReferenceInsert, ExternalSymbolInsert, SymbolMappingInsert,
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
    let artifact = parse_normalized_artifact_from_slice(&artifact_bytes, artifact_path)?;
    let artifact_path_display = artifact_path.to_string_lossy();
    let root_path = if repo_root.is_empty() {
        Cow::Borrowed(artifact.root_path.as_str())
    } else {
        Cow::Borrowed(repo_root)
    };

    let mut symbol_ids = HashMap::new();
    let mut prepared_symbols = Vec::with_capacity(artifact.symbols.len());
    let mut symbols_mapped = 0;
    let mut symbols_unmapped = 0;

    for symbol in &artifact.symbols {
        let external_symbol_id = stable_external_symbol_id(index_hash, &symbol.external_symbol);
        let normalized_file_path = symbol
            .file_path
            .as_deref()
            .map(normalize_repo_relative_path)
            .transpose()?;

        let mapping = if let Some((internal_symbol_id, mapping_kind, confidence)) =
            find_internal_symbol_mapping(store, symbol, normalized_file_path.as_deref())?
        {
            symbols_mapped += 1;
            Some(PreparedMapping {
                external_symbol_id: external_symbol_id.clone(),
                internal_symbol_id,
                mapping_kind,
                confidence,
            })
        } else {
            symbols_unmapped += 1;
            None
        };

        prepared_symbols.push(PreparedSymbol {
            id: external_symbol_id.clone(),
            external_symbol: symbol.external_symbol.clone(),
            display_name: symbol.display_name.clone(),
            kind: symbol.kind.clone(),
            file_path: normalized_file_path,
            start_line: symbol.start_line,
            end_line: symbol.end_line,
            start_byte: symbol.start_byte,
            end_byte: symbol.end_byte,
            metadata_json: "{}".to_string(),
            mapping,
        });

        symbol_ids.insert(symbol.external_symbol.clone(), external_symbol_id);
    }

    let provenance_default = artifact.source_kind.as_str();
    let mut prepared_references = Vec::with_capacity(artifact.references.len());
    for reference in &artifact.references {
        let normalized_file_path = normalize_repo_relative_path(&reference.file_path)?;
        let from_external_symbol_id = external_symbol_id_for_endpoint(
            &mut symbol_ids,
            &mut prepared_symbols,
            index_hash,
            reference.from_external_symbol.as_deref(),
        );
        let to_external_symbol_id = external_symbol_id_for_endpoint(
            &mut symbol_ids,
            &mut prepared_symbols,
            index_hash,
            reference.to_external_symbol.as_deref(),
        );
        let provenance = reference
            .provenance
            .as_deref()
            .unwrap_or(provenance_default);

        prepared_references.push(PreparedReference {
            from_external_symbol_id,
            to_external_symbol_id,
            relationship: reference.relationship.clone(),
            file_path: normalized_file_path,
            line: reference.line,
            column: reference.column,
            end_line: reference.end_line,
            end_column: reference.end_column,
            confidence: reference.confidence.unwrap_or(1.0),
            provenance: provenance.to_string(),
        });
    }

    {
        let mut conn = store.write()?;
        let tx = conn
            .transaction()
            .context("Failed to start external index import transaction")?;

        external::upsert_external_index(
            &tx,
            &ExternalIndexInsert {
                id: &index_id,
                source_kind: &artifact.source_kind,
                producer: &artifact.producer,
                language: &artifact.language,
                root_path: root_path.as_ref(),
                artifact_path: artifact_path_display.as_ref(),
                artifact_hash: &artifact_hash,
                status: "imported",
                diagnostics_json: "{}",
            },
        )?;

        tx.execute(
            r#"
DELETE FROM symbol_mappings
WHERE external_symbol_id IN (
  SELECT id FROM external_symbols WHERE external_index_id = ?1
)
"#,
            params![&index_id],
        )
        .with_context(|| {
            format!("Failed to clear stale external symbol mappings: index_id={index_id}")
        })?;

        for symbol in &prepared_symbols {
            external::upsert_external_symbol(
                &tx,
                &ExternalSymbolInsert {
                    id: &symbol.id,
                    external_index_id: &index_id,
                    external_symbol: &symbol.external_symbol,
                    display_name: &symbol.display_name,
                    language: &artifact.language,
                    kind: &symbol.kind,
                    file_path: symbol.file_path.as_deref(),
                    start_line: symbol.start_line,
                    end_line: symbol.end_line,
                    start_byte: symbol.start_byte,
                    end_byte: symbol.end_byte,
                    metadata_json: &symbol.metadata_json,
                },
            )?;

            if let Some(mapping) = &symbol.mapping {
                external::upsert_symbol_mapping(
                    &tx,
                    &SymbolMappingInsert {
                        external_symbol_id: &mapping.external_symbol_id,
                        internal_symbol_id: &mapping.internal_symbol_id,
                        mapping_kind: mapping.mapping_kind,
                        confidence: mapping.confidence,
                    },
                )?;
            }
        }

        for reference in &prepared_references {
            external::upsert_external_reference(
                &tx,
                &ExternalReferenceInsert {
                    external_index_id: &index_id,
                    from_external_symbol_id: reference.from_external_symbol_id.as_deref(),
                    to_external_symbol_id: reference.to_external_symbol_id.as_deref(),
                    relationship: &reference.relationship,
                    file_path: &reference.file_path,
                    line: reference.line,
                    column: reference.column,
                    end_line: reference.end_line,
                    end_column: reference.end_column,
                    confidence: reference.confidence,
                    provenance: &reference.provenance,
                    metadata_json: "{}",
                },
            )?;
        }

        tx.commit()
            .context("Failed to commit external index import transaction")?;
    }

    Ok(ImportReport {
        index_id,
        symbols_imported: artifact.symbols.len(),
        references_imported: prepared_references.len(),
        symbols_mapped,
        symbols_unmapped,
    })
}

fn external_symbol_id_for_endpoint(
    symbol_ids: &mut HashMap<String, String>,
    prepared_symbols: &mut Vec<PreparedSymbol>,
    index_hash: &str,
    raw_external_symbol: Option<&str>,
) -> Option<String> {
    let raw_external_symbol = raw_external_symbol?;
    if let Some(id) = symbol_ids.get(raw_external_symbol) {
        return Some(id.clone());
    }

    let external_symbol_id = stable_external_symbol_id(index_hash, raw_external_symbol);
    symbol_ids.insert(raw_external_symbol.to_string(), external_symbol_id.clone());
    prepared_symbols.push(PreparedSymbol {
        id: external_symbol_id.clone(),
        external_symbol: raw_external_symbol.to_string(),
        display_name: placeholder_display_name(raw_external_symbol),
        kind: "unknown".to_string(),
        file_path: None,
        start_line: None,
        end_line: None,
        start_byte: None,
        end_byte: None,
        metadata_json: r#"{"placeholder":true}"#.to_string(),
        mapping: None,
    });
    Some(external_symbol_id)
}

fn placeholder_display_name(raw_external_symbol: &str) -> String {
    let tail = raw_external_symbol
        .trim()
        .trim_end_matches('.')
        .rsplit_once(' ')
        .map(|(_, tail)| tail)
        .unwrap_or(raw_external_symbol)
        .trim();
    let name = tail.split_once('(').map(|(name, _)| name).unwrap_or(tail);
    if name.is_empty() {
        raw_external_symbol.to_string()
    } else {
        name.to_string()
    }
}

#[derive(Debug, Clone)]
struct PreparedSymbol {
    id: String,
    external_symbol: String,
    display_name: String,
    kind: String,
    file_path: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
    start_byte: Option<u32>,
    end_byte: Option<u32>,
    metadata_json: String,
    mapping: Option<PreparedMapping>,
}

#[derive(Debug, Clone)]
struct PreparedMapping {
    external_symbol_id: String,
    internal_symbol_id: String,
    mapping_kind: &'static str,
    confidence: f32,
}

#[derive(Debug, Clone)]
struct PreparedReference {
    from_external_symbol_id: Option<String>,
    to_external_symbol_id: Option<String>,
    relationship: String,
    file_path: String,
    line: u32,
    column: Option<u32>,
    end_line: Option<u32>,
    end_column: Option<u32>,
    confidence: f32,
    provenance: String,
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

    let exact_candidates = candidates
        .iter()
        .filter(|candidate| {
            exact_range(candidate, external_symbol) && compatible_kind(candidate, external_symbol)
        })
        .collect::<Vec<_>>();
    if exact_candidates.len() == 1 {
        return Ok(Some((exact_candidates[0].id.clone(), "exact_range", 1.0)));
    }
    if exact_candidates.len() > 1 {
        return Ok(None);
    }

    let compatible_candidates = candidates
        .iter()
        .filter(|candidate| compatible_kind(candidate, external_symbol))
        .collect::<Vec<_>>();
    if compatible_candidates.len() == 1 {
        let candidate = compatible_candidates[0];
        return Ok(Some((candidate.id.clone(), "same_file_name", 0.8)));
    }
    Ok(None)
}

fn exact_range(candidate: &SymbolRow, external_symbol: &NormalizedExternalSymbol) -> bool {
    Some(candidate.start_line) == external_symbol.start_line
        && Some(candidate.end_line) == external_symbol.end_line
}

fn compatible_kind(candidate: &SymbolRow, external_symbol: &NormalizedExternalSymbol) -> bool {
    candidate.kind.eq_ignore_ascii_case(&external_symbol.kind)
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

use std::cmp::Ordering;
use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::storage::sqlite::{EdgeRow, ExternalReferenceRow, SqliteStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceSource {
    Native,
    External,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergedReference {
    pub to_symbol_id: String,
    pub from_symbol_id: Option<String>,
    pub from_symbol_name: Option<String>,
    pub from_symbol_file: Option<String>,
    pub reference_type: String,
    pub at_file: Option<String>,
    pub at_line: Option<u32>,
    pub source: ReferenceSource,
    pub confidence: f32,
    pub external_index_id: Option<String>,
    pub provenance: Option<String>,
    pub metadata_json: Option<String>,
}

pub fn merged_references_to_internal_symbol(
    sqlite: &SqliteStore,
    internal_symbol_id: &str,
    relationship: Option<&str>,
    limit: usize,
) -> Result<Vec<MergedReference>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let relationship = normalized_relationship(relationship);
    let fetch_limit = limit.saturating_mul(3).max(50);

    let native = sqlite
        .list_edges_to(internal_symbol_id, fetch_limit)?
        .into_iter()
        .filter(|edge| relationship_matches(relationship, &edge.edge_type))
        .map(|edge| native_reference(sqlite, edge))
        .collect::<Result<Vec<_>>>()?;

    let external = sqlite
        .list_external_references_to_internal_symbol(internal_symbol_id, relationship, fetch_limit)?
        .into_iter()
        .map(|reference| external_reference(internal_symbol_id, reference))
        .collect::<Vec<_>>();

    let mut deduped: HashMap<DedupeKey, MergedReference> = HashMap::new();
    for reference in native.into_iter().chain(external) {
        let key = DedupeKey::from(&reference);
        match deduped.get(&key) {
            Some(existing) if prefer_reference(&reference, existing) => {
                deduped.insert(key, reference);
            }
            None => {
                deduped.insert(key, reference);
            }
            _ => {}
        }
    }

    let mut references = deduped.into_values().collect::<Vec<_>>();
    references.sort_by(compare_references);
    references.truncate(limit);
    Ok(references)
}

fn native_reference(sqlite: &SqliteStore, edge: EdgeRow) -> Result<MergedReference> {
    let from_symbol = sqlite.get_symbol_by_id(&edge.from_symbol_id)?;
    Ok(MergedReference {
        to_symbol_id: edge.to_symbol_id,
        from_symbol_id: Some(edge.from_symbol_id),
        from_symbol_name: from_symbol.as_ref().map(|symbol| symbol.name.clone()),
        from_symbol_file: from_symbol.map(|symbol| symbol.file_path),
        reference_type: edge.edge_type,
        at_file: edge.at_file,
        at_line: edge.at_line,
        source: ReferenceSource::Native,
        confidence: edge.confidence,
        external_index_id: None,
        provenance: None,
        metadata_json: None,
    })
}

fn external_reference(
    internal_symbol_id: &str,
    reference: ExternalReferenceRow,
) -> MergedReference {
    MergedReference {
        to_symbol_id: internal_symbol_id.to_string(),
        from_symbol_id: None,
        from_symbol_name: None,
        from_symbol_file: None,
        reference_type: reference.relationship,
        at_file: Some(reference.file_path),
        at_line: Some(reference.line),
        source: ReferenceSource::External,
        confidence: reference.confidence,
        external_index_id: Some(reference.external_index_id),
        provenance: Some(reference.provenance),
        metadata_json: Some(reference.metadata_json),
    }
}

fn normalized_relationship(relationship: Option<&str>) -> Option<&str> {
    match relationship {
        Some(value) if value.eq_ignore_ascii_case("all") => None,
        other => other,
    }
}

fn relationship_matches(filter: Option<&str>, reference_type: &str) -> bool {
    filter.is_none_or(|expected| reference_type == expected)
}

fn prefer_reference(candidate: &MergedReference, existing: &MergedReference) -> bool {
    match candidate
        .confidence
        .partial_cmp(&existing.confidence)
        .unwrap_or(Ordering::Equal)
    {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => source_rank(candidate.source) > source_rank(existing.source),
    }
}

fn compare_references(left: &MergedReference, right: &MergedReference) -> Ordering {
    right
        .confidence
        .partial_cmp(&left.confidence)
        .unwrap_or(Ordering::Equal)
        .then_with(|| source_rank(right.source).cmp(&source_rank(left.source)))
        .then_with(|| left.at_file.cmp(&right.at_file))
        .then_with(|| left.at_line.cmp(&right.at_line))
        .then_with(|| left.reference_type.cmp(&right.reference_type))
        .then_with(|| left.to_symbol_id.cmp(&right.to_symbol_id))
        .then_with(|| left.from_symbol_id.cmp(&right.from_symbol_id))
        .then_with(|| left.external_index_id.cmp(&right.external_index_id))
}

fn source_rank(source: ReferenceSource) -> u8 {
    match source {
        ReferenceSource::Native => 0,
        ReferenceSource::External => 1,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DedupeKey {
    to_symbol_id: String,
    reference_type: String,
    at_file: Option<String>,
    at_line: Option<u32>,
}

impl From<&MergedReference> for DedupeKey {
    fn from(reference: &MergedReference) -> Self {
        Self {
            to_symbol_id: reference.to_symbol_id.clone(),
            reference_type: reference.reference_type.clone(),
            at_file: reference.at_file.clone(),
            at_line: reference.at_line,
        }
    }
}

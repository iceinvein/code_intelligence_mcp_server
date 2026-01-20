use crate::retrieval::RankedHit;
use crate::storage::sqlite::SqliteStore;
use anyhow::Result;

/// Expand results with related symbols via edges
pub fn expand_with_edges(
    sqlite: &SqliteStore,
    hits: Vec<RankedHit>,
    limit: usize,
) -> Result<(Vec<RankedHit>, std::collections::HashSet<String>)> {
    if hits.is_empty() {
        return Ok((hits, std::collections::HashSet::new()));
    }

    let mut out = hits.clone();
    let mut seen: std::collections::HashSet<String> = hits.iter().map(|h| h.id.clone()).collect();
    let mut expanded_ids = std::collections::HashSet::new();
    let expand_candidates = hits.iter().take(3).cloned().collect::<Vec<_>>();

    for h in expand_candidates {
        let (is_func, is_type) = match h.kind.as_str() {
            "function" | "method" => (true, false),
            "struct" | "enum" | "class" | "interface" | "trait" => (false, true),
            _ => (false, false),
        };

        if is_func {
            // Find callees (implementation details)
            let edges = sqlite.list_edges_from(&h.id, 5)?;
            for edge in edges {
                if edge.edge_type != "call" {
                    continue;
                }
                if seen.insert(edge.to_symbol_id.clone()) {
                    if let Some(row) = sqlite.get_symbol_by_id(&edge.to_symbol_id)? {
                        let evidence_boost =
                            (1.0 + (edge.evidence_count as f32).ln_1p() * 0.25).clamp(1.0, 1.75);
                        let resolution_multiplier = match edge.resolution.as_str() {
                            "local" => 1.0,
                            "import" => 0.9,
                            "heuristic" => 0.75,
                            _ => 0.8,
                        };
                        out.push(RankedHit {
                            id: row.id.clone(),
                            score: h.score
                                * 0.8
                                * edge.confidence
                                * evidence_boost
                                * resolution_multiplier,
                            name: row.name,
                            kind: row.kind,
                            file_path: row.file_path,
                            exported: row.exported,
                            language: row.language,
                        });
                        expanded_ids.insert(row.id);
                    }
                }
            }
        } else if is_type {
            // Find usages (references TO this symbol)
            let edges = sqlite.list_edges_to(&h.id, 5)?;
            for edge in edges {
                if edge.edge_type != "reference"
                    && edge.edge_type != "type"
                    && edge.edge_type != "extends"
                    && edge.edge_type != "implements"
                    && edge.edge_type != "alias"
                {
                    continue;
                }
                if seen.insert(edge.from_symbol_id.clone()) {
                    if let Some(row) = sqlite.get_symbol_by_id(&edge.from_symbol_id)? {
                        let evidence_boost =
                            (1.0 + (edge.evidence_count as f32).ln_1p() * 0.25).clamp(1.0, 1.75);
                        let resolution_multiplier = match edge.resolution.as_str() {
                            "local" => 1.0,
                            "import" => 0.9,
                            "heuristic" => 0.75,
                            _ => 0.8,
                        };
                        out.push(RankedHit {
                            id: row.id.clone(),
                            score: h.score
                                * 0.8
                                * edge.confidence
                                * evidence_boost
                                * resolution_multiplier,
                            name: row.name,
                            kind: row.kind,
                            file_path: row.file_path,
                            exported: row.exported,
                            language: row.language,
                        });
                        expanded_ids.insert(row.id);
                    }
                }
            }
        }
    }

    // Re-sort and truncate
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
    });
    if out.len() > limit {
        out.truncate(limit);
    }

    Ok((out, expanded_ids))
}

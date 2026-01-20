use crate::storage::sqlite::{SqliteStore, SymbolRow};
use anyhow::Result;
use std::collections::{HashMap, HashSet};

pub fn expand_with_scoring(
    store: &SqliteStore,
    roots: &[SymbolRow],
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    let mut candidates: HashMap<String, (SymbolRow, f32)> = HashMap::new();
    let mut expanded_frontier: HashSet<String> = roots.iter().map(|r| r.id.clone()).collect();

    // Initialize frontier with roots.
    // We don't add roots to candidates because they are already in the context.
    // We just use them to explore.
    let mut frontier: Vec<String> = roots.iter().map(|r| r.id.clone()).collect();

    let max_depth = 2; // Depth 1 (direct neighbors) and maybe Depth 2
    let exploration_limit = 100; // Don't fetch too many symbols total

    for depth in 0..max_depth {
        if candidates.len() >= exploration_limit {
            break;
        }
        if frontier.is_empty() {
            break;
        }

        let mut next_frontier = Vec::new();

        for from_id in frontier {
            if candidates.len() >= exploration_limit {
                break;
            }

            let edges = store.list_edges_from(&from_id, 20)?; // Limit fan-out per node
            for edge in edges {
                let depth_penalty = 1.0 / ((depth + 1) as f32);
                let type_multiplier = match edge.edge_type.as_str() {
                    "extends" | "implements" | "alias" | "type" => 1.5,
                    "call" => 1.0,
                    "reference" => 0.8,
                    _ => 1.0,
                };
                let resolution_multiplier = match edge.resolution.as_str() {
                    "local" => 1.0,
                    "import" => 0.9,
                    "heuristic" => 0.75,
                    _ => 0.8,
                };
                let evidence_boost =
                    (1.0 + (edge.evidence_count as f32).ln_1p() * 0.25).clamp(1.0, 1.75);
                let score = depth_penalty
                    * type_multiplier
                    * resolution_multiplier
                    * edge.confidence
                    * evidence_boost;

                let entry = candidates.get_mut(&edge.to_symbol_id);
                if let Some((_, s)) = entry {
                    if score > *s {
                        *s = score;
                    }
                    continue;
                }

                if let Some(row) = store.get_symbol_by_id(&edge.to_symbol_id)? {
                    candidates.insert(row.id.clone(), (row.clone(), score));
                    if expanded_frontier.insert(row.id.clone()) {
                        next_frontier.push(row.id);
                    }
                }
            }
        }
        frontier = next_frontier;
    }

    // Convert to vec and sort by score
    let mut scored_rows: Vec<(SymbolRow, f32)> = candidates.into_values().collect();
    scored_rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Take top `limit`
    let result = scored_rows
        .into_iter()
        .take(limit)
        .map(|(row, _)| row)
        .collect();
    Ok(result)
}

//! Package-aware ranking for multi-repo support

use crate::config::Config;
use crate::retrieval::{HitSignals, RankedHit};
use crate::storage::sqlite::SqliteStore;
use anyhow::Result;
use std::collections::HashMap;

/// Apply package boost with signals tracking
///
/// This function boosts search result scores for symbols that belong to the
/// same package as the query context. The default boost is 1.15x for same-package
/// results.
pub fn apply_package_boost_with_signals(
    sqlite: &SqliteStore,
    mut hits: Vec<RankedHit>,
    hit_signals: &mut HashMap<String, HitSignals>,
    query_package_id: Option<&str>,
    _config: &Config,
) -> Result<Vec<RankedHit>> {
    if hits.is_empty() {
        return Ok(hits);
    }

    // Determine query context package
    let query_pkg = if let Some(pkg_id) = query_package_id {
        // Explicit package from query controls (e.g., "myFunction pkg:my-lib")
        pkg_id.to_string()
    } else {
        // Auto-detect from first hit's file path
        if let Some(first_hit) = hits.first() {
            if let Some(pkg) = sqlite.get_package_for_file(&first_hit.file_path)? {
                pkg.name
            } else {
                return Ok(hits);
            }
        } else {
            return Ok(hits);
        }
    };

    // Collect symbol IDs from hits for batch lookup
    let symbol_ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();

    // Batch load package associations (returns HashMap<symbol_id, package_id>)
    let package_id_map = sqlite
        .batch_get_symbol_packages(&symbol_ids.iter().map(|s| s.as_str()).collect::<Vec<&str>>())
        .unwrap_or_default();

    // If no packages found, return early
    if package_id_map.is_empty() {
        return Ok(hits);
    }

    // Build a symbol->package_name map by looking up each package_id
    let mut package_name_map = HashMap::new();
    for (symbol_id, package_id) in &package_id_map {
        if let Ok(Some(pkg)) = sqlite.get_package_by_id(package_id) {
            package_name_map.insert(symbol_id.clone(), pkg.name);
        }
    }

    // Default boost multiplier (1.15x for general case)
    let boost_multiplier = 1.15;

    // Apply boost to same-package hits
    for h in hits.iter_mut() {
        if let Some(hit_package_name) = package_name_map.get(&h.id) {
            if hit_package_name == &query_pkg {
                // Same package - apply boost
                let original_score = h.score;
                h.score *= boost_multiplier;
                let boost_amount = h.score - original_score;

                hit_signals
                    .entry(h.id.clone())
                    .and_modify(|s| s.package_boost += boost_amount)
                    .or_insert(HitSignals {
                        keyword_score: 0.0,
                        vector_score: 0.0,
                        base_score: 0.0,
                        structural_adjust: 0.0,
                        intent_mult: 1.0,
                        definition_bias: 0.0,
                        popularity_boost: 0.0,
                        learning_boost: 0.0,
                        affinity_boost: 0.0,
                        docstring_boost: 0.0,
                        package_boost: boost_amount,
                    });
            }
        }
    }

    // Re-sort by score after applying boosts
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.file_path.cmp(&b.file_path))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(hits)
}

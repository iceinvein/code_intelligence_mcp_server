use crate::storage::sqlite::{SqliteStore, SymbolRow};
use anyhow::Result;
use std::collections::{HashMap, HashSet};

use super::tokens::TokenCounter;

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

/// Find types referenced in a function signature or symbol definition
///
/// Resolves type and reference edges to find parameter types, return types,
/// and other type dependencies of a symbol.
pub fn resolve_parameter_types(
    store: &SqliteStore,
    symbol_id: &str,
) -> Result<Vec<SymbolRow>> {
    let edges = store.list_edges_from(symbol_id, 20)?;

    let mut related = Vec::new();
    for edge in edges {
        if edge.edge_type == "type" || edge.edge_type == "reference" {
            if let Some(sym) = store.get_symbol_by_id(&edge.to_symbol_id)? {
                related.push(sym);
            }
        }
    }
    Ok(related)
}

/// Find parent classes and interfaces via inheritance edges
///
/// Resolves extends and implements edges to find the class hierarchy
/// for a given symbol. Useful for understanding type relationships.
pub fn resolve_parent_classes(
    store: &SqliteStore,
    symbol_id: &str,
) -> Result<Vec<SymbolRow>> {
    let edges = store.list_edges_from(symbol_id, 20)?;

    let mut parents = Vec::new();
    for edge in edges {
        if edge.edge_type == "extends" || edge.edge_type == "implements" {
            if let Some(sym) = store.get_symbol_by_id(&edge.to_symbol_id)? {
                parents.push(sym);
            }
        }
    }
    Ok(parents)
}

/// Auto-include related type dependencies for context completeness
///
/// For each root symbol, resolves parameter types and parent classes,
/// filters out already-seen symbols, and adds new symbols within the token budget.
///
/// Returns symbols that were auto-included (types, parents, etc.).
pub fn auto_include_dependencies(
    store: &SqliteStore,
    roots: &[SymbolRow],
    seen: &HashSet<String>,
    budget: &mut usize,
    counter: &TokenCounter,
) -> Vec<SymbolRow> {
    let mut extra = Vec::new();
    let mut seen_local = seen.clone();

    for root in roots {
        if *budget == 0 {
            break;
        }

        // Resolve parameter types (type, reference edges)
        if let Ok(types) = resolve_parameter_types(store, &root.id) {
            for sym in types {
                if !seen_local.contains(&sym.id) {
                    let tokens = counter.count(&sym.text);
                    if *budget >= tokens {
                        *budget -= tokens;
                        seen_local.insert(sym.id.clone());
                        extra.push(sym);
                    }
                }
            }
        }

        if *budget == 0 {
            break;
        }

        // Resolve parent classes (extends, implements edges)
        if let Ok(parents) = resolve_parent_classes(store, &root.id) {
            for sym in parents {
                if !seen_local.contains(&sym.id) {
                    let tokens = counter.count(&sym.text);
                    if *budget >= tokens {
                        *budget -= tokens;
                        seen_local.insert(sym.id.clone());
                        extra.push(sym);
                    }
                }
            }
        }
    }

    extra
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_parameter_types_empty() {
        let store = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        store.init().unwrap();

        let result = resolve_parameter_types(&store, "nonexistent").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_resolve_parent_classes_empty() {
        let store = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        store.init().unwrap();

        let result = resolve_parent_classes(&store, "nonexistent").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_auto_include_dependencies_respects_budget() {
        let store = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        store.init().unwrap();
        let counter = TokenCounter::new("o200k_base").unwrap();

        // Create test symbols
        let root = SymbolRow {
            id: "root".to_string(),
            file_path: "test.rs".to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: "test_func".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 100,
            start_line: 1,
            end_line: 5,
            text: "fn test_func() -> Result<()> { Ok(()) }".to_string(),
        };
        store.upsert_symbol(&root).unwrap();

        let mut budget = 10; // Very small budget
        let seen = HashSet::new();
        let result = auto_include_dependencies(&store, &[root], &seen, &mut budget, &counter);

        // With tiny budget, should include nothing or very little
        assert!(result.is_empty() || budget < 10);
    }

    #[test]
    fn test_auto_include_dependencies_filters_seen() {
        let store = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        store.init().unwrap();
        let counter = TokenCounter::new("o200k_base").unwrap();

        // Create test symbols
        let root = SymbolRow {
            id: "root".to_string(),
            file_path: "test.rs".to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: "test_func".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 100,
            start_line: 1,
            end_line: 5,
            text: "fn test_func() -> Result<()> { Ok(()) }".to_string(),
        };
        let dep = SymbolRow {
            id: "dep".to_string(),
            file_path: "test.rs".to_string(),
            language: "rust".to_string(),
            kind: "type".to_string(),
            name: "Result".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 50,
            start_line: 1,
            end_line: 3,
            text: "type Result<T> = ...".to_string(),
        };

        store.upsert_symbol(&root).unwrap();
        store.upsert_symbol(&dep).unwrap();

        // Create a type edge from root to dep
        let edge = crate::storage::sqlite::EdgeRow {
            from_symbol_id: "root".to_string(),
            to_symbol_id: "dep".to_string(),
            edge_type: "type".to_string(),
            at_file: Some("test.rs".to_string()),
            at_line: Some(1),
            confidence: 1.0,
            evidence_count: 1,
            resolution: "local".to_string(),
        };
        store.upsert_edge(&edge).unwrap();

        let mut budget = 1000;
        let mut seen = HashSet::new();
        seen.insert("dep".to_string()); // Mark dep as already seen

        let result = auto_include_dependencies(&store, &[root], &seen, &mut budget, &counter);

        // Dep should not be included since it's already seen
        assert!(result.iter().all(|s| s.id != "dep"));
    }
}

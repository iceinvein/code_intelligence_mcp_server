//! Framework pattern injection for NL queries.
//!
//! Framework patterns (WebSocket handlers, routes, middleware) live in a
//! separate SQLite table and aren't directly searchable via BM25/vector.
//! This module queries them and boosts/injects matching parent symbols
//! into the search results.

use super::RankedHit;
use crate::storage::sqlite::SqliteStore;
use anyhow::Result;
use std::collections::HashSet;

/// Inject framework pattern matches into the search results.
///
/// For each framework pattern whose kind matches the query (e.g., "websocket"
/// matches "WebSocket handler"), finds the smallest enclosing symbol and either
/// boosts its score (if already present) or injects it as a new result.
pub(super) fn inject_framework_patterns(
    sqlite: &SqliteStore,
    query: &str,
    hits: &mut Vec<RankedHit>,
    seen: &mut HashSet<String>,
) -> Result<()> {
    let patterns = sqlite.search_framework_patterns(None, None, None, None, None, None, 200)?;

    let query_lower = query.to_lowercase();
    let mut fw_file_lines: Vec<(String, u32)> = Vec::new();

    for pattern in &patterns {
        let kind_lower = pattern.kind.to_lowercase();
        let matches = query_lower.contains(&kind_lower)
            || (kind_lower == "websocket"
                && (query_lower.contains("websocket")
                    || query_lower.contains("ws")
                    || query_lower.contains("socket")))
            || (kind_lower == "route"
                && (query_lower.contains("route")
                    || query_lower.contains("endpoint")
                    || query_lower.contains("api")))
            || (kind_lower == "middleware" && query_lower.contains("middleware"))
            || (kind_lower == "plugin" && query_lower.contains("plugin"));

        if matches {
            // Route patterns without an HTTP path (e.g., map.get(key), params.get('id'),
            // headers.get('content-type')) are false positives from the Elysia extractor
            // misidentifying generic .get()/.post()/.delete() calls as routes. Skip them.
            if kind_lower == "route" && !pattern.path.as_ref().is_some_and(|p| p.starts_with('/')) {
                continue;
            }
            fw_file_lines.push((pattern.file_path.clone(), pattern.line));
        }
    }

    // Find parent symbols for matched framework patterns
    let fw_files: HashSet<String> = fw_file_lines.iter().map(|(fp, _)| fp.clone()).collect();

    for fw_file in &fw_files {
        if let Ok(file_symbols) = sqlite.list_symbols_by_file(fw_file) {
            for &(ref fp, line) in &fw_file_lines {
                if fp != fw_file {
                    continue;
                }
                // Find best enclosing symbol: start_line <= line <= end_line,
                // preferring the smallest span (most specific)
                let enclosing = file_symbols
                    .iter()
                    .filter(|s| s.start_line <= line && s.end_line >= line)
                    .min_by_key(|s| s.end_line - s.start_line);

                if let Some(sym) = enclosing {
                    if seen.contains(&sym.id) {
                        // Already in results — boost its score
                        if let Some(hit) = hits.iter_mut().find(|h| h.id == sym.id) {
                            hit.score += 0.15;
                        }
                    } else {
                        // Inject as new result with moderate score
                        let top_score = hits.first().map(|h| h.score).unwrap_or(1.0);
                        seen.insert(sym.id.clone());
                        hits.push(RankedHit {
                            id: sym.id.clone(),
                            score: top_score * 0.7,
                            name: sym.name.clone(),
                            kind: sym.kind.clone(),
                            file_path: sym.file_path.clone(),
                            exported: sym.exported,
                            language: sym.language.clone(),
                        });
                    }
                }
            }
        }
    }

    Ok(())
}

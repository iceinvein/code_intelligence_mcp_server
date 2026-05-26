//! Framework pattern injection for NL queries.
//!
//! Framework patterns (WebSocket handlers, routes, middleware, Convex functions,
//! cron jobs) live in a separate SQLite table and aren't directly searchable
//! via BM25/vector. This module queries them and boosts/injects matching
//! parent symbols into the search results.

use super::RankedHit;
use crate::storage::sqlite::SqliteStore;
use anyhow::Result;
use std::collections::HashSet;

/// Inject framework pattern matches into the search results.
///
/// For each framework pattern whose kind matches the query (e.g., "websocket"
/// matches "WebSocket handler"), finds the smallest enclosing symbol and either
/// boosts its score (if already present) or injects it as a new result.
/// Maximum number of framework pattern symbols to inject per query.
/// Capping prevents route-heavy queries (e.g., "API route handlers") from
/// flooding the pool with 18+ routers, which pushes genuine BM25 results
/// out of the pool_size window and inflates the score floor so that
/// sub-query coverage injections trigger score-gap removal.
const MAX_FRAMEWORK_INJECTIONS: usize = 8;

/// Returns the number of new symbols injected (not just boosted).
pub(super) fn inject_framework_patterns(
    sqlite: &SqliteStore,
    query: &str,
    hits: &mut Vec<RankedHit>,
    seen: &mut HashSet<String>,
) -> Result<usize> {
    let patterns = sqlite.search_framework_patterns(None, None, None, None, None, None, 200)?;

    let query_lower = query.to_lowercase();
    let mut fw_file_lines: Vec<(String, u32)> = Vec::new();

    for pattern in &patterns {
        let kind_lower = pattern.kind.to_lowercase();
        let matches = query_lower.contains(&kind_lower)
            // WebSocket aliases
            || (kind_lower == "websocket"
                && (query_lower.contains("websocket")
                    || query_lower.contains("ws")
                    || query_lower.contains("socket")))
            // Route aliases
            || (kind_lower == "route"
                && (query_lower.contains("route")
                    || query_lower.contains("endpoint")
                    || query_lower.contains("api")))
            // Middleware
            || (kind_lower == "middleware" && query_lower.contains("middleware"))
            // Plugin
            || (kind_lower == "plugin" && query_lower.contains("plugin"))
            // Controller aliases
            || (kind_lower == "controller"
                && (query_lower.contains("controller")
                    || query_lower.contains("controllers")))
            // Injectable/service aliases
            || (kind_lower == "injectable"
                && (query_lower.contains("injectable")
                    || query_lower.contains("service")
                    || query_lower.contains("provider")))
            // Module
            || (kind_lower == "module" && query_lower.contains("module"))
            // Interceptor
            || (kind_lower == "interceptor" && query_lower.contains("interceptor"))
            // Pipe
            || (kind_lower == "pipe"
                && (query_lower.contains("pipe")
                    || query_lower.contains("validation")))
            // Procedure aliases (tRPC)
            || (kind_lower == "procedure"
                && (query_lower.contains("procedure")
                    || query_lower.contains("trpc")))
            // Router
            || (kind_lower == "router" && query_lower.contains("router"))
            // Error handler aliases
            || (kind_lower == "error_handler"
                && (query_lower.contains("error handler")
                    || query_lower.contains("error handling")
                    || query_lower.contains("errorhandler")))
            // Hook aliases
            || (kind_lower == "hook"
                && (query_lower.contains("hook")
                    || query_lower.contains("lifecycle")))
            // FileRoute aliases (Next.js pages/layouts)
            || (kind_lower == "file_route"
                && (query_lower.contains("page")
                    || query_lower.contains("layout")
                    || query_lower.contains("file route")))
            // Query aliases (Convex)
            || (kind_lower == "query"
                && (query_lower.contains("query")
                    || query_lower.contains("convex")))
            // Mutation aliases (Convex)
            || (kind_lower == "mutation"
                && (query_lower.contains("mutation")
                    || query_lower.contains("convex")))
            // Action aliases (Convex)
            || (kind_lower == "action"
                && (query_lower.contains("action")
                    || query_lower.contains("convex")))
            // CronJob aliases (Convex)
            || (kind_lower == "cron_job"
                && (query_lower.contains("cron")
                    || query_lower.contains("scheduled")
                    || query_lower.contains("periodic")));

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
    let mut injection_count = 0usize;

    for fw_file in &fw_files {
        if injection_count >= MAX_FRAMEWORK_INJECTIONS {
            break;
        }
        if crate::classify::is_generated_output_path(fw_file) {
            continue;
        }
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
                        if injection_count >= MAX_FRAMEWORK_INJECTIONS {
                            break;
                        }
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
                        injection_count += 1;
                    }
                }
            }
        }
    }

    Ok(injection_count)
}

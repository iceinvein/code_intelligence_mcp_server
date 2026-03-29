use crate::retrieval::RankedHit;
use crate::retrieval::query::Intent;
use crate::retrieval::ranking::score::{intent_adjustment, is_test_file, is_test_symbol, symbol_importance_adjustment};
use crate::storage::sqlite::SqliteStore;
use crate::text;
use anyhow::Result;

/// Penalty multiplier for const/variable symbols with short, all-lowercase names.
/// These are almost always local variables that shouldn't surface via edge expansion.
fn local_variable_discount(kind: &str, name: &str) -> f32 {
    if !matches!(kind, "const" | "variable") {
        return 1.0;
    }
    if name.starts_with('{') || name.starts_with('[') {
        return 0.3; // destructured bindings
    }
    let is_all_lower = name.chars().all(|c| c.is_lowercase() || c.is_ascii_digit());
    if is_all_lower && name.len() <= 9 {
        return 0.5; // short single-word locals
    }
    1.0
}

/// Query relevance discount for edge-expanded symbols.
///
/// Edge expansion follows call/reference edges from top results, which can
/// pull in completely unrelated symbols (e.g., ActivityLogs UI component from
/// an API router query). This checks if the expanded symbol has ANY overlap
/// with the query terms via its name or file path. Symbols with zero overlap
/// get a heavy discount (0.2x).
fn query_relevance_discount(name: &str, file_path: &str, query_terms: &[String]) -> f32 {
    if query_terms.is_empty() {
        return 1.0;
    }

    let name_lower = name.to_lowercase();
    let name_parts = text::split_identifier_like(&name_lower);
    let path_lower = file_path.to_lowercase();

    for term in query_terms {
        // Check name (including CamelCase-split parts)
        if name_lower.contains(term.as_str()) || name_parts.contains(term.as_str()) {
            return 1.0;
        }
        // Check file path segments
        if path_lower.contains(term.as_str()) {
            return 1.0;
        }
        // Check synonym overlap — "handler" in name matches "route" query via callback synonym
        let synonyms = text::get_related_terms(term.as_str());
        for syn in &synonyms {
            if name_lower.contains(syn) || path_lower.contains(syn) {
                return 0.8; // Synonym match — mild discount (not as strong as direct)
            }
        }
    }

    // Zero overlap with any query term — heavy discount
    0.2
}

/// Extract significant query terms (3+ chars, no stopwords) for relevance checking.
fn extract_query_terms(query: &str) -> Vec<String> {
    static STOPWORDS: &[&str] = &[
        "the", "and", "for", "how", "does", "what", "this", "that", "with", "from",
        "are", "was", "were", "has", "have", "had", "not", "but", "can", "will",
        "would", "should", "could", "its", "all", "any", "each", "which", "when",
        "where", "who", "why", "work", "works",
    ];
    query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() >= 3 && !STOPWORDS.contains(&t.as_str()))
        .collect()
}

/// Expand results with related symbols via edges.
///
/// Parent scores already have intent multipliers baked in from the scoring loop.
/// To prevent edge-expanded children from inheriting inflated scores (e.g., a
/// 75x Schema-boosted TodoRow producing 108-score children in todos.rs), we
/// strip the parent's intent multiplier before deriving child scores.
pub fn expand_with_edges(
    sqlite: &SqliteStore,
    hits: Vec<RankedHit>,
    limit: usize,
    intent: &Option<Intent>,
    query: &str,
) -> Result<(Vec<RankedHit>, std::collections::HashSet<String>)> {
    let query_terms = extract_query_terms(query);
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

        // Strip the parent's intent multiplier so children aren't inflated.
        // E.g., TodoRow in schema.rs gets 75x Schema boost → score 291.
        // Without stripping, children inherit 291 * 0.8 ≈ 233.
        // With stripping, children inherit (291/75) * 0.8 ≈ 3.1 (the base relevance).
        let parent_intent = intent_adjustment(intent, &h.kind, &h.file_path, h.exported, &h.name);
        let base_score = if parent_intent > 0.0 && parent_intent != 1.0 {
            h.score / parent_intent
        } else {
            h.score
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
                        // Skip test symbols/files — they were already penalized and
                        // truncated by the scoring pipeline. Re-adding them via edge
                        // expansion would bypass intent penalties.
                        if is_test_file(&row.file_path) || is_test_symbol(&row.name) {
                            continue;
                        }
                        let line_count = row.end_line.saturating_sub(row.start_line) + 1;
                        let si = symbol_importance_adjustment(line_count, row.exported);
                        // Skip small private helpers — they are implementation details
                        // that shouldn't surface just because their caller matched.
                        // E.g., repo_name (5-line private fn) called by
                        // generate_embeddings_for_parallel_indexed_files.
                        // Exempt exported symbols — they represent the API surface
                        // and should survive regardless of size (Q13: PathNormalizer::new
                        // is 3 lines but IS the constructor).
                        if si < -1.0 && !row.exported {
                            continue;
                        }
                        let evidence_boost =
                            (1.0 + (edge.evidence_count as f32).ln_1p() * 0.25).clamp(1.0, 1.75);
                        let resolution_multiplier = match edge.resolution.as_str() {
                            "local" => 1.0,
                            "import" => 0.9,
                            "heuristic" => 0.75,
                            _ => 0.8,
                        };
                        let lv_disc = local_variable_discount(&row.kind, &row.name);
                        let qr_disc = query_relevance_discount(&row.name, &row.file_path, &query_terms);
                        out.push(RankedHit {
                            id: row.id.clone(),
                            score: base_score
                                * 0.8
                                * edge.confidence
                                * evidence_boost
                                * resolution_multiplier
                                * lv_disc
                                * qr_disc,
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
                        // Skip test symbols/files — same rationale as above
                        if is_test_file(&row.file_path) || is_test_symbol(&row.name) {
                            continue;
                        }
                        let line_count = row.end_line.saturating_sub(row.start_line) + 1;
                        let si = symbol_importance_adjustment(line_count, row.exported);
                        if si < -1.0 && !row.exported {
                            continue;
                        }
                        let evidence_boost =
                            (1.0 + (edge.evidence_count as f32).ln_1p() * 0.25).clamp(1.0, 1.75);
                        let resolution_multiplier = match edge.resolution.as_str() {
                            "local" => 1.0,
                            "import" => 0.9,
                            "heuristic" => 0.75,
                            _ => 0.8,
                        };
                        let lv_disc = local_variable_discount(&row.kind, &row.name);
                        let qr_disc = query_relevance_discount(&row.name, &row.file_path, &query_terms);
                        out.push(RankedHit {
                            id: row.id.clone(),
                            score: base_score
                                * 0.8
                                * edge.confidence
                                * evidence_boost
                                * resolution_multiplier
                                * lv_disc
                                * qr_disc,
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

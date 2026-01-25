//! Reciprocal Rank Fusion (RRF) for multi-source score combination
//!
//! RRF provides a principled method to combine ranked lists from keyword,
//! vector, and graph sources without score calibration issues.

use crate::retrieval::RankedHit;
use crate::storage::sqlite::SqliteStore;
use anyhow::Result;
use std::collections::HashMap;

const DEFAULT_RRF_K: f32 = 60.0;

/// RRF source for tracking where a rank came from
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum RankSource {
    Keyword,
    Vector,
    Graph,
}

/// Combine multiple ranked lists using Reciprocal Rank Fusion
///
/// RRF formula: score = sum(weight / (k + rank))
/// where k is a constant (default 60) to prevent division by zero
/// and weight is the source-specific weight
///
/// # Arguments
/// * `keyword_hits` - Ranked results from keyword search (Tantivy)
/// * `vector_hits` - Ranked results from vector search (LanceDB)
/// * `graph_hits` - Ranked results by PageRank scores
/// * `weights` - Tuple of (keyword_weight, vector_weight, graph_weight)
///
/// # Returns
/// Combined and sorted results with RRF scores
pub fn reciprocal_rank_fusion(
    keyword_hits: &[RankedHit],
    vector_hits: &[RankedHit],
    graph_hits: &[RankedHit],
    weights: (f32, f32, f32), // (keyword_weight, vector_weight, graph_weight)
) -> Vec<RankedHit> {
    let (w_kw, w_vec, w_graph) = weights;
    let k = DEFAULT_RRF_K;

    let mut rrf_scores: HashMap<String, f32> = HashMap::new();
    let mut hit_data: HashMap<String, RankedHit> = HashMap::new();

    // Process keyword hits
    for (rank, hit) in keyword_hits.iter().enumerate() {
        let score = w_kw / (k + rank as f32 + 1.0);
        *rrf_scores.entry(hit.id.clone()).or_insert(0.0) += score;
        hit_data
            .entry(hit.id.clone())
            .or_insert_with(|| hit.clone());
    }

    // Process vector hits
    for (rank, hit) in vector_hits.iter().enumerate() {
        let score = w_vec / (k + rank as f32 + 1.0);
        *rrf_scores.entry(hit.id.clone()).or_insert(0.0) += score;
        hit_data
            .entry(hit.id.clone())
            .or_insert_with(|| hit.clone());
    }

    // Process graph hits (sorted by PageRank)
    for (rank, hit) in graph_hits.iter().enumerate() {
        let score = w_graph / (k + rank as f32 + 1.0);
        *rrf_scores.entry(hit.id.clone()).or_insert(0.0) += score;
        hit_data
            .entry(hit.id.clone())
            .or_insert_with(|| hit.clone());
    }

    // Convert to sorted results
    let mut results: Vec<RankedHit> = hit_data
        .into_values()
        .map(|mut hit| {
            hit.score = rrf_scores.get(&hit.id).copied().unwrap_or(0.0);
            hit
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
            .then_with(|| a.name.cmp(&b.name))
    });

    results
}

/// Get graph-ranked hits based on PageRank scores
///
/// Takes a set of hits and reorders them by their PageRank scores.
/// This provides a third ranking dimension for RRF fusion.
///
/// # Arguments
/// * `hits` - The hits to reorder
/// * `sqlite` - Database connection for loading PageRank metrics
///
/// # Returns
/// Hits sorted by PageRank (highest first)
pub fn get_graph_ranked_hits(hits: &[RankedHit], sqlite: &SqliteStore) -> Result<Vec<RankedHit>> {
    if hits.is_empty() {
        return Ok(vec![]);
    }

    // Batch load PageRank scores
    let symbol_ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();
    let pagerank_map = sqlite.batch_get_symbol_metrics(&symbol_ids)?;

    // Sort by PageRank (highest first)
    let mut graph_hits = hits.to_vec();
    graph_hits.sort_by(|a, b| {
        let pr_a = pagerank_map.get(&a.id).copied().unwrap_or(0.0);
        let pr_b = pagerank_map.get(&b.id).copied().unwrap_or(0.0);
        pr_b.partial_cmp(&pr_a).unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(graph_hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hit(id: &str, score: f32, exported: bool) -> RankedHit {
        RankedHit {
            id: id.to_string(),
            score,
            name: id.to_string(),
            kind: "function".to_string(),
            file_path: format!("src/{}.rs", id),
            exported,
            language: "rust".to_string(),
        }
    }

    #[test]
    fn test_rrf_combines_multiple_sources() {
        let keyword_hits = vec![
            make_hit("a", 0.9, true),
            make_hit("b", 0.8, true),
            make_hit("c", 0.7, false),
        ];
        let vector_hits = vec![
            make_hit("b", 0.95, true),
            make_hit("d", 0.85, true),
            make_hit("a", 0.75, true),
        ];
        let graph_hits = vec![
            make_hit("c", 0.5, false),
            make_hit("a", 0.4, true),
            make_hit("b", 0.3, true),
        ];

        let results =
            reciprocal_rank_fusion(&keyword_hits, &vector_hits, &graph_hits, (1.0, 1.0, 0.5));

        // Results should be sorted by RRF score
        assert!(!results.is_empty());

        // Top result should have highest RRF score
        // (appears in multiple sources gets boost)
        let top_id = &results[0].id;
        assert!(top_id == "a" || top_id == "b"); // Both appear in multiple sources
    }

    #[test]
    fn test_rrf_with_empty_sources() {
        let keyword_hits = vec![make_hit("a", 0.9, true)];
        let vector_hits: Vec<RankedHit> = vec![];
        let graph_hits: Vec<RankedHit> = vec![];

        let results =
            reciprocal_rank_fusion(&keyword_hits, &vector_hits, &graph_hits, (1.0, 1.0, 0.5));

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn test_rrf_with_all_empty() {
        let keyword_hits: Vec<RankedHit> = vec![];
        let vector_hits: Vec<RankedHit> = vec![];
        let graph_hits: Vec<RankedHit> = vec![];

        let results =
            reciprocal_rank_fusion(&keyword_hits, &vector_hits, &graph_hits, (1.0, 1.0, 0.5));

        assert!(results.is_empty());
    }

    #[test]
    fn test_rrf_respects_weights() {
        let keyword_hits = vec![make_hit("a", 0.9, true)];
        let vector_hits = vec![make_hit("b", 0.9, true)];
        let graph_hits: Vec<RankedHit> = vec![];

        // With higher keyword weight, 'a' should win
        let results_high_kw =
            reciprocal_rank_fusion(&keyword_hits, &vector_hits, &graph_hits, (2.0, 0.5, 0.0));
        assert_eq!(results_high_kw[0].id, "a");

        // With higher vector weight, 'b' should win
        let results_high_vec =
            reciprocal_rank_fusion(&keyword_hits, &vector_hits, &graph_hits, (0.5, 2.0, 0.0));
        assert_eq!(results_high_vec[0].id, "b");
    }

    #[test]
    fn test_rrf_sorts_stable() {
        let hits = vec![
            make_hit("a", 0.5, true),
            make_hit("b", 0.5, true),
            make_hit("c", 0.5, false),
        ];

        let results = reciprocal_rank_fusion(&hits, &[], &[], (1.0, 0.0, 0.0));

        // All have same score, but exported should come first
        assert_eq!(results[0].exported, true);
        assert_eq!(results[1].exported, true);
        assert_eq!(results[2].exported, false);
    }
}

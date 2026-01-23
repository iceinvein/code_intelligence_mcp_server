//! Integration between reranker and search pipeline

use crate::retrieval::RankedHit;
use crate::reranker::Reranker;
use anyhow::Result;
use std::collections::HashMap;

/// Apply reranker scores to ranked hits
pub fn apply_reranker_scores(
    hits: &[RankedHit],
    reranker_scores: &[f32],
    weight: f32,
) -> Vec<RankedHit> {
    let mut result = hits.to_vec();

    for (i, hit) in result.iter_mut().enumerate() {
        if i < reranker_scores.len() {
            let rerank_score = reranker_scores[i];
            // Blend reranker score with existing score
            hit.score = hit.score * (1.0 - weight) + rerank_score * weight * 10.0;
        }
    }

    // Re-sort by updated scores
    result.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
    });

    result
}

/// Prepare documents for reranking from RankedHit
pub fn prepare_rerank_docs(
    hits: &[RankedHit],
    texts: &HashMap<String, String>,
) -> Vec<crate::reranker::RerankDocument> {
    hits.iter()
        .take(20) // Limit for performance
        .map(|hit| {
            let text = texts
                .get(&hit.id)
                .cloned()
                .unwrap_or_else(|| format!("{}: {}", hit.kind, hit.name));

            crate::reranker::RerankDocument {
                id: hit.id.clone(),
                text,
                name: hit.name.clone(),
            }
        })
        .collect()
}

/// Check if reranking should be applied
pub fn should_rerank(result_count: usize, min_results: usize) -> bool {
    result_count >= min_results
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
            file_path: format!("{}.ts", id),
            exported,
            language: "typescript".to_string(),
        }
    }

    #[test]
    fn test_apply_reranker_scores_basic() {
        let hits = vec![
            make_hit("a", 0.5, true),
            make_hit("b", 0.4, true),
            make_hit("c", 0.3, true),
        ];
        let rerank_scores = vec![0.9, 0.2, 0.5]; // c should move up

        let result = apply_reranker_scores(&hits, &rerank_scores, 0.3);

        // With weight 0.3, reranker has some influence
        // a: 0.5 * 0.7 + 0.9 * 0.3 * 10 = 0.35 + 2.7 = 3.05
        // c: 0.3 * 0.7 + 0.5 * 0.3 * 10 = 0.21 + 1.5 = 1.71
        // b: 0.4 * 0.7 + 0.2 * 0.3 * 10 = 0.28 + 0.6 = 0.88
        assert_eq!(result[0].id, "a"); // Highest reranker score
        assert_eq!(result[1].id, "c"); // Medium reranker score
        assert_eq!(result[2].id, "b"); // Lowest reranker score
    }

    #[test]
    fn test_apply_reranker_scores_empty() {
        let hits = vec![];
        let rerank_scores = vec![];
        let result = apply_reranker_scores(&hits, &rerank_scores, 0.3);
        assert!(result.is_empty());
    }

    #[test]
    fn test_should_rerank() {
        assert!(should_rerank(5, 3));
        assert!(should_rerank(3, 3));
        assert!(!should_rerank(2, 3));
    }

    #[test]
    fn test_prepare_rerank_docs() {
        let hits = vec![
            make_hit("a", 0.5, true),
            make_hit("b", 0.4, false),
        ];
        let mut texts = HashMap::new();
        texts.insert("a".to_string(), "text for a".to_string());

        let docs = prepare_rerank_docs(&hits, &texts);

        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].id, "a");
        assert_eq!(docs[0].text, "text for a");
        assert_eq!(docs[1].id, "b");
        // b uses fallback format
        assert!(docs[1].text.contains("function"));
    }
}

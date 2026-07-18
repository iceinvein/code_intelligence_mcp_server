//! Deterministic information-retrieval and set-quality metrics for integration gates.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankingMetrics {
    pub recall_at_k: f64,
    pub reciprocal_rank: f64,
    pub ndcg_at_k: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetMetrics {
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
}

pub fn ranking_metrics(
    ranked_ids: &[String],
    relevance: &HashMap<String, u8>,
    k: usize,
) -> RankingMetrics {
    let relevant_total = relevance.values().filter(|&&grade| grade > 0).count();
    let cutoff = ranked_ids.len().min(k);
    let relevant_retrieved = ranked_ids[..cutoff]
        .iter()
        .filter(|id| relevance.get(*id).copied().unwrap_or(0) > 0)
        .count();
    let recall_at_k = ratio(relevant_retrieved, relevant_total);
    let reciprocal_rank = ranked_ids[..cutoff]
        .iter()
        .position(|id| relevance.get(id).copied().unwrap_or(0) > 0)
        .map(|rank| 1.0 / (rank + 1) as f64)
        .unwrap_or(0.0);
    let dcg = ranked_ids[..cutoff]
        .iter()
        .enumerate()
        .map(|(rank, id)| discounted_gain(relevance.get(id).copied().unwrap_or(0), rank))
        .sum::<f64>();
    let mut ideal_grades = relevance.values().copied().collect::<Vec<_>>();
    ideal_grades.sort_unstable_by(|a, b| b.cmp(a));
    let ideal_dcg = ideal_grades
        .into_iter()
        .take(k)
        .enumerate()
        .map(|(rank, grade)| discounted_gain(grade, rank))
        .sum::<f64>();

    RankingMetrics {
        recall_at_k,
        reciprocal_rank,
        ndcg_at_k: if ideal_dcg == 0.0 {
            1.0
        } else {
            dcg / ideal_dcg
        },
    }
}

pub fn set_metrics<T>(predicted: &HashSet<T>, expected: &HashSet<T>) -> SetMetrics
where
    T: Eq + std::hash::Hash,
{
    let true_positives = predicted.intersection(expected).count();
    let precision = ratio(true_positives, predicted.len());
    let recall = ratio(true_positives, expected.len());
    let f1 = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };
    SetMetrics {
        precision,
        recall,
        f1,
    }
}

fn discounted_gain(grade: u8, zero_based_rank: usize) -> f64 {
    let gain = 2_f64.powi(i32::from(grade)) - 1.0;
    gain / ((zero_based_rank + 2) as f64).log2()
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        if numerator == 0 {
            1.0
        } else {
            0.0
        }
    } else {
        numerator as f64 / denominator as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_metrics_reward_early_relevant_results() {
        let ranked = vec!["noise".to_string(), "best".to_string(), "other".to_string()];
        let relevance = HashMap::from([("best".to_string(), 2), ("other".to_string(), 1)]);
        let metrics = ranking_metrics(&ranked, &relevance, 3);
        assert_eq!(metrics.recall_at_k, 1.0);
        assert_eq!(metrics.reciprocal_rank, 0.5);
        assert!(metrics.ndcg_at_k > 0.6 && metrics.ndcg_at_k < 1.0);
    }

    #[test]
    fn set_metrics_count_false_positives_and_false_negatives() {
        let predicted = HashSet::from(["a", "noise"]);
        let expected = HashSet::from(["a", "missing"]);
        let metrics = set_metrics(&predicted, &expected);
        assert_eq!(metrics.precision, 0.5);
        assert_eq!(metrics.recall, 0.5);
        assert_eq!(metrics.f1, 0.5);
    }
}

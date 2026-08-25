pub mod diversify;
pub mod expansion;
pub mod package;
pub mod reranker;
pub mod rrf;
pub mod score;

pub(crate) use crate::classify::is_test_file;
pub use diversify::{diversify_by_cluster, diversify_by_file, diversify_by_kind};
pub use expansion::expand_with_edges;

/// Demote document hits whose file carries a superseded/deprecated status in
/// `doc_metadata` (docs-indexing design, Phase 3). Applied as a multiplicative
/// penalty so a superseded ADR can still be found — just not above current
/// material. Code hits pass through untouched.
pub fn apply_doc_status_demotion_with_signals(
    sqlite: &crate::storage::sqlite::SqliteStore,
    mut hits: Vec<super::RankedHit>,
    hit_signals: &mut std::collections::HashMap<String, super::HitSignals>,
) -> anyhow::Result<Vec<super::RankedHit>> {
    const SUPERSEDED_PENALTY: f32 = 0.5;

    let doc_files: Vec<String> = hits
        .iter()
        .filter(|h| h.kind == "document")
        .map(|h| h.file_path.clone())
        .collect();
    if doc_files.is_empty() {
        return Ok(hits);
    }
    let meta = sqlite.get_doc_meta_for_paths(&doc_files)?;
    if meta.is_empty() {
        return Ok(hits);
    }
    for h in hits.iter_mut() {
        if h.kind != "document" {
            continue;
        }
        let Some(m) = meta.get(&h.file_path) else {
            continue;
        };
        let suppressed = matches!(m.status.as_deref(), Some("superseded") | Some("deprecated"));
        if !suppressed {
            continue;
        }
        h.score *= SUPERSEDED_PENALTY;
        hit_signals
            .entry(h.id.clone())
            .and_modify(|s| s.doc_status_penalty = SUPERSEDED_PENALTY)
            .or_default()
            .doc_status_penalty = SUPERSEDED_PENALTY;
    }
    Ok(hits)
}
pub use package::apply_package_boost_with_signals;
pub use reranker::{apply_reranker_scores, prepare_rerank_docs, should_rerank};
pub use rrf::{get_graph_ranked_hits, reciprocal_rank_fusion};
pub use score::{
    apply_docstring_boost_with_signals, apply_file_affinity_boost_with_signals,
    apply_popularity_boost_with_signals, apply_selection_boost_with_signals,
    rank_hits_with_signals,
};
pub(crate) use score::{
    definition_bias, intent_adjustment, structural_adjustment, symbol_importance_adjustment,
    term_coverage_adjustment, test_symbol_penalty,
};

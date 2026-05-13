pub mod diversify;
pub mod expansion;
pub mod package;
pub mod reranker;
pub mod rrf;
pub mod score;

pub(crate) use crate::classify::is_test_file;
pub use diversify::{diversify_by_cluster, diversify_by_file, diversify_by_kind};
pub use expansion::expand_with_edges;
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

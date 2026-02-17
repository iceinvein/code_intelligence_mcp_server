//! Cross-encoder reranking for improved search result precision

pub mod cache;

use anyhow::Result;
use async_trait::async_trait;
use crate::path::Utf8Path;
use std::sync::Arc;

/// Trait for reranking search results
#[async_trait]
pub trait Reranker: Send + Sync {
    /// Rerank documents based on relevance to query
    /// Returns scores for each document (higher = more relevant)
    async fn rerank(&self, query: &str, documents: &[RerankDocument]) -> Result<Vec<f32>>;

    /// Get the top-k limit for this reranker
    fn top_k(&self) -> usize;
}

/// Document representation for reranking
#[derive(Debug, Clone)]
pub struct RerankDocument {
    pub id: String,
    pub text: String,
    pub name: String,
}

/// Create a reranker based on config.
///
/// Currently always returns `None` — the reranker is not enabled.
/// The trait and types are kept since they're referenced by the retrieval module.
pub fn create_reranker(
    _model_path: Option<&Utf8Path>,
    _cache_dir: Option<&Utf8Path>,
    _top_k: usize,
) -> Result<Option<Arc<dyn Reranker>>> {
    Ok(None)
}

//! Cross-encoder reranking for improved search result precision

pub mod onnx;
pub mod cache;

use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

/// Trait for reranking search results
#[async_trait]
pub trait Reranker: Send + Sync {
    /// Rerank documents based on relevance to query
    /// Returns scores for each document (higher = more relevant)
    async fn rerank(
        &self,
        query: &str,
        documents: &[RerankDocument],
    ) -> Result<Vec<f32>>;

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

/// Create a reranker based on config
pub fn create_reranker(
    model_path: Option<&Path>,
    cache_dir: Option<&Path>,
    top_k: usize,
) -> Result<Option<Arc<dyn Reranker>>> {
    let model_path = match model_path {
        Some(p) if p.exists() => p.to_path_buf(),
        Some(p) => {
            tracing::warn!("Reranker model path not found: {}, reranking disabled", p.display());
            return Ok(None);
        }
        None => {
            tracing::info!("No reranker model path specified, reranking disabled");
            return Ok(None);
        }
    };

    Ok(Some(Arc::new(onnx::CrossEncoderReranker::new(
        &model_path,
        cache_dir,
        top_k,
    )?)))
}

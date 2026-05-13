//! Cross-encoder reranking for improved search result precision

pub mod cache;
pub mod llamacpp;

use crate::path::{Utf8Path, Utf8PathBuf};
use anyhow::Result;
use async_trait::async_trait;
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
/// Returns `Ok(None)` when:
/// - `enabled` is false
/// - `model_path` is `None`
/// - The model file does not exist and auto-download fails
///
/// When `enabled` is true, creates a [`llamacpp::LlamaCppReranker`]
/// (bge-reranker-v2-m3 BERT cross-encoder) wrapped in a [`cache::CachedReranker`].
///
/// # Arguments
/// * `enabled` - Whether reranking is enabled (from `RERANKER_ENABLED` env var)
/// * `model_path` - Optional explicit path to the `.gguf` model file
/// * `cache_dir` - Optional directory for the caching wrapper (unused if
///   `model_path` is `None`)
/// * `top_k` - Maximum number of documents scored per rerank call
pub fn create_reranker(
    enabled: bool,
    model_path: Option<&Utf8Path>,
    cache_dir: Option<&Utf8Path>,
    top_k: usize,
) -> Result<Option<Arc<dyn Reranker>>> {
    if !enabled {
        tracing::debug!("Reranker disabled (RERANKER_ENABLED=false)");
        return Ok(None);
    }

    // Resolve the model file path: use explicit path or fall back to the
    // default reranker model directory (bge-reranker-v2-m3, separate from
    // the LLM description model).
    let resolved_model_path: Utf8PathBuf = match model_path {
        Some(p) => p.to_owned(),
        None => {
            let data_dir = crate::config::get_data_dir();
            data_dir
                .join("models/bge-reranker-v2-m3-gguf")
                .join(llamacpp::HF_MODEL_FILE)
        }
    };

    // Auto-download if model file not found
    if !resolved_model_path.exists() {
        let model_dir = resolved_model_path
            .parent()
            .map(|p| p.to_owned())
            .unwrap_or_else(|| {
                crate::config::get_data_dir().join("models/bge-reranker-v2-m3-gguf")
            });

        tracing::info!(
            model_path = %resolved_model_path,
            "Reranker model not found, attempting auto-download"
        );

        match llamacpp::download_reranker_model(&model_dir) {
            Ok(()) => {
                tracing::info!("Reranker model downloaded successfully");
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to download reranker model. Set RERANKER_ENABLED=false to suppress."
                );
                return Ok(None);
            }
        }
    }

    // Build the inner reranker
    let inner = llamacpp::LlamaCppReranker::new(&resolved_model_path, top_k)?;

    // Determine cache size: 256 entries by default, respecting cache_dir
    // presence as an opt-in signal (the cache itself is in-memory).
    let cache_size = if cache_dir.is_some() { 256 } else { 64 };

    let cached = cache::CachedReranker::new(Box::new(inner), cache_size);

    tracing::info!(
        model_path = %resolved_model_path,
        top_k,
        cache_size,
        "Reranker initialised"
    );

    Ok(Some(Arc::new(cached)))
}

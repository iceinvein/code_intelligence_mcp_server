pub mod hash;
pub mod llamacpp;
pub mod shared;

use anyhow::Result;
use std::sync::Arc;

pub use shared::SharedEmbedder;

/// L2-normalize a vector in place.
///
/// If the norm is zero (all-zeros vector) the vector is left unchanged
/// to avoid a division-by-zero NaN.
pub(crate) fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

/// Backend trait for sync embedding implementations.
///
/// All implementations must be `Send + Sync` so a single instance can be
/// shared between the index pipeline and the query path without an external
/// `AsyncMutex`. The contract is interior-immutable: `embed` takes `&self`
/// and any state required for the forward pass (e.g. a fresh `LlamaContext`)
/// must be created inside the call.
///
/// The async/concurrency-cap front-end is [`SharedEmbedder`], which wraps an
/// `Arc<dyn Embedder>` with a `tokio::Semaphore` and `spawn_blocking` so the
/// blocking CPU/GPU work does not stall the tokio runtime.
pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Embed query texts with model-specific instruction prefix for retrieval.
    ///
    /// jina-code-1.5b uses symmetric embeddings, so queries and documents share
    /// the same embedding space. Default implementation falls back to `embed()`.
    fn query_embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed(texts)
    }
}

/// Known embedding dimension for jina-code-1.5b (the LlamaCpp backend model).
const JINA_CODE_DIM: usize = 1536;

/// Returns the embedding dimension for a given backend without loading the model.
///
/// Priority: `dim_override` > `truncate_dim` cap > backend default.
/// Set `dim_override` via `EMBEDDING_DIM` when evaluating a model with a
/// different native dimension than jina-code-1.5b (1536).
/// If `truncate_dim` is `Some(d)`, the returned dimension is capped at `d`
/// (Matryoshka truncation).
pub fn default_embedding_dim(
    backend: crate::config::EmbeddingsBackend,
    hash_dim: usize,
    truncate_dim: Option<usize>,
    dim_override: Option<usize>,
) -> usize {
    let full = match dim_override {
        Some(d) => d,
        None => match backend {
            crate::config::EmbeddingsBackend::LlamaCpp => JINA_CODE_DIM,
            crate::config::EmbeddingsBackend::Hash => hash_dim,
        },
    };
    match truncate_dim {
        Some(d) if d < full => d,
        _ => full,
    }
}

/// A deferred embedder that starts without a loaded model and allows a real
/// embedder to be "slotted in" later from a background task.
///
/// Before the real embedder is set, `dim()` returns a pre-configured dimension
/// and `embed()`/`query_embed()` return errors. The hybrid search pipeline
/// already handles embedding errors gracefully — it degrades to BM25-only.
pub struct DeferredEmbedder {
    dim: usize,
    inner: Arc<std::sync::Mutex<Option<Box<dyn Embedder>>>>,
}

impl DeferredEmbedder {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            inner: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Slot in the real embedder once it's downloaded and loaded.
    pub fn set_inner(&self, embedder: Box<dyn Embedder>) {
        let mut guard = self.inner.lock().expect("DeferredEmbedder mutex poisoned");
        *guard = Some(embedder);
    }

    /// Check whether the real embedder has been loaded.
    pub fn is_ready(&self) -> bool {
        let guard = self.inner.lock().expect("DeferredEmbedder mutex poisoned");
        guard.is_some()
    }

    /// Get a clone of the inner Arc for sharing with a background task.
    pub fn inner_slot(&self) -> Arc<std::sync::Mutex<Option<Box<dyn Embedder>>>> {
        Arc::clone(&self.inner)
    }
}

impl Embedder for DeferredEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let guard = self.inner.lock().expect("DeferredEmbedder mutex poisoned");
        match guard.as_ref() {
            Some(embedder) => embedder.embed(texts),
            None => anyhow::bail!(
                "Embedding model is still loading — search will use BM25-only until ready"
            ),
        }
    }

    fn query_embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let guard = self.inner.lock().expect("DeferredEmbedder mutex poisoned");
        match guard.as_ref() {
            Some(embedder) => embedder.query_embed(texts),
            None => anyhow::bail!(
                "Embedding model is still loading — search will use BM25-only until ready"
            ),
        }
    }
}

/// Wrapper that truncates Matryoshka embeddings to a smaller dimension.
///
/// Jina Code v2 supports Matryoshka Representation Learning (MRL), meaning the
/// first N dimensions of the full 1536-dim vector retain meaningful semantic
/// structure. Truncating to N dimensions and L2 re-normalizing gives a smaller,
/// faster vector with minimal quality loss — useful for reducing storage costs
/// and speeding up approximate nearest-neighbour search.
///
/// # Example
///
/// ```no_run
/// use code_intelligence_mcp_server::embeddings::{TruncatingEmbedder, Embedder};
/// use code_intelligence_mcp_server::embeddings::hash::HashEmbedder;
///
/// let base = Box::new(HashEmbedder::new(128));
/// let mut truncating = TruncatingEmbedder::new(base, 64).unwrap();
/// assert_eq!(truncating.dim(), 64);
/// let vecs = truncating.embed(&["hello world".to_string()]).unwrap();
/// assert_eq!(vecs[0].len(), 64);
/// ```
pub struct TruncatingEmbedder {
    inner: Box<dyn Embedder>,
    target_dim: usize,
}

impl TruncatingEmbedder {
    /// Create a new `TruncatingEmbedder` that wraps `inner` and truncates its
    /// output to `target_dim` dimensions followed by L2 re-normalization.
    ///
    /// # Errors
    ///
    /// Returns an error if `target_dim` is zero or exceeds the inner embedder's
    /// native dimension.
    pub fn new(inner: Box<dyn Embedder>, target_dim: usize) -> Result<Self> {
        let full_dim = inner.dim();
        anyhow::ensure!(target_dim > 0, "Target dimension must be > 0");
        anyhow::ensure!(
            target_dim <= full_dim,
            "Target dimension ({target_dim}) exceeds model dimension ({full_dim})"
        );
        Ok(Self { inner, target_dim })
    }
}

impl Embedder for TruncatingEmbedder {
    fn dim(&self) -> usize {
        self.target_dim
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let full = self.inner.embed(texts)?;
        Ok(full
            .into_iter()
            .map(|v| {
                let mut truncated = v[..self.target_dim].to_vec();
                l2_normalize(&mut truncated);
                truncated
            })
            .collect())
    }

    fn query_embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let full = self.inner.query_embed(texts)?;
        Ok(full
            .into_iter()
            .map(|v| {
                let mut truncated = v[..self.target_dim].to_vec();
                l2_normalize(&mut truncated);
                truncated
            })
            .collect())
    }
}

/// HuggingFace repository for the GGUF-format jina-code embedding model.
const EMBEDDING_HF_REPO: &str = "jinaai/jina-code-embeddings-1.5b-GGUF";
/// Q8_0 quantized GGUF file (~1.6 GB). Higher precision than Q4 because
/// embedding quality degrades more from quantization than generation quality.
const EMBEDDING_HF_MODEL_FILE: &str = "jina-code-embeddings-1.5b-Q8_0.gguf";

/// Factory function to create an embedder based on the backend configuration.
///
/// # Arguments
/// * `backend` - The embeddings backend to use
/// * `model_dir` - Optional path to model directory (required for LlamaCpp)
/// * `device` - Device to use for inference (CPU/Metal)
/// * `hash_dim` - Dimension for hash embedder (only used if backend is Hash)
pub fn create_embedder(
    backend: crate::config::EmbeddingsBackend,
    model_dir: Option<&crate::path::Utf8Path>,
    _device: crate::config::EmbeddingsDevice,
    hash_dim: usize,
) -> Result<Box<dyn Embedder>> {
    match backend {
        crate::config::EmbeddingsBackend::LlamaCpp => {
            let model_dir = model_dir
                .ok_or_else(|| anyhow::anyhow!("LlamaCpp embedder requires a model directory"))?;

            let model_file = model_dir.join(EMBEDDING_HF_MODEL_FILE);

            // Auto-download if model not found
            if !model_file.exists() {
                tracing::info!(
                    "Embedding model not found at {}, attempting auto-download...",
                    model_file
                );
                download_embedding_model(model_dir)?;
            }

            Ok(Box::new(llamacpp::LlamaCppEmbedder::new(&model_file)?))
        }
        crate::config::EmbeddingsBackend::Hash => Ok(Box::new(hash::HashEmbedder::new(hash_dim))),
    }
}

/// Download the jina-code-embeddings-1.5b GGUF model from HuggingFace.
///
/// Downloads a single GGUF file (~1.6 GB) into the HuggingFace cache,
/// then creates a symlink in `target_dir`. Uses the same pattern as
/// the LLM model download in `src/llm/mod.rs`.
fn download_embedding_model(target_dir: &crate::path::Utf8Path) -> Result<()> {
    use anyhow::Context;

    tracing::info!(
        "Downloading embedding model from huggingface.co/{}",
        EMBEDDING_HF_REPO
    );

    let api = hf_hub::api::sync::Api::new().context("Failed to initialize HuggingFace Hub API")?;
    let repo = api.model(EMBEDDING_HF_REPO.to_string());

    tracing::info!(
        "Downloading {} (~1.6 GB, this may take a few minutes)...",
        EMBEDDING_HF_MODEL_FILE
    );
    let model_cached = repo
        .get(EMBEDDING_HF_MODEL_FILE)
        .context("Failed to download embedding GGUF model file")?;

    // Create target directory
    std::fs::create_dir_all(target_dir.as_std_path())
        .context("Failed to create embedding model directory")?;

    // Symlink from HF cache into our model directory
    let target_model = target_dir.join(EMBEDDING_HF_MODEL_FILE);
    symlink_or_copy(&model_cached, target_model.as_std_path())
        .context("Failed to link embedding GGUF model file")?;

    tracing::info!("Embedding model ready at {}", target_dir);
    Ok(())
}

/// Create a symlink from `source` to `target`.
fn symlink_or_copy(source: &std::path::Path, target: &std::path::Path) -> Result<()> {
    if target.exists() || target.symlink_metadata().is_ok() {
        std::fs::remove_file(target).ok();
    }
    std::os::unix::fs::symlink(source, target)?;
    Ok(())
}

#[cfg(test)]
mod truncating_tests {
    use super::*;
    use crate::embeddings::hash::HashEmbedder;

    /// Wrapping a 64-dim HashEmbedder with target=32 should report dim=32
    /// and produce 32-element vectors.
    #[test]
    fn truncating_embedder_reduces_dimension() {
        let base = Box::new(HashEmbedder::new(64));
        let truncating = TruncatingEmbedder::new(base, 32).unwrap();
        assert_eq!(truncating.dim(), 32);

        let result = truncating.embed(&["hello".to_string()]).unwrap();
        assert_eq!(result[0].len(), 32);
    }

    /// Requesting a target dimension larger than the model dimension must fail.
    #[test]
    fn truncating_embedder_rejects_larger_dim() {
        let base = Box::new(HashEmbedder::new(64));
        let result = TruncatingEmbedder::new(base, 128);
        assert!(result.is_err());
    }

    /// Requesting target_dim=0 must fail.
    #[test]
    fn truncating_embedder_rejects_zero_dim() {
        let base = Box::new(HashEmbedder::new(64));
        let result = TruncatingEmbedder::new(base, 0);
        assert!(result.is_err());
    }

    /// Truncated vectors must be L2-normalized (unit norm within floating-point
    /// tolerance).
    ///
    /// Uses a large diverse text so that enough tokens hash into the first
    /// `target_dim` slots, guaranteeing a non-zero truncated vector.
    #[test]
    fn truncated_vectors_are_l2_normalized() {
        // Use many tokens across a wide vocabulary so that the probability of
        // all of them hashing exclusively into the upper half of the vector
        // is negligible.  HashEmbedder with dim=8 and target=4 makes this
        // deterministically safe: with 8 buckets and multiple tokens, at least
        // one will fall into the first 4 slots by the pigeonhole principle.
        let base = Box::new(HashEmbedder::new(8));
        let truncating = TruncatingEmbedder::new(base, 4).unwrap();

        let texts = vec!["alpha beta gamma delta epsilon zeta".to_string()];
        let result = truncating.embed(&texts).unwrap();
        let norm: f32 = result[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "Truncated vector should be L2-normalized, got norm={norm}"
        );
    }

    /// `query_embed` must also truncate and re-normalize.
    #[test]
    fn query_embed_also_truncates() {
        let base = Box::new(HashEmbedder::new(64));
        let truncating = TruncatingEmbedder::new(base, 16).unwrap();

        let result = truncating.query_embed(&["query".to_string()]).unwrap();
        assert_eq!(result[0].len(), 16);
    }

    /// `dim()` must equal the target dimension, not the inner model's dimension.
    #[test]
    fn dim_reports_target_not_inner() {
        let base = Box::new(HashEmbedder::new(64));
        let truncating = TruncatingEmbedder::new(base, 8).unwrap();
        assert_eq!(truncating.dim(), 8);
    }

    /// When target_dim equals the inner dim, output should still be valid and
    /// L2-normalized (effectively a no-op truncation, but still re-normalizes).
    #[test]
    fn truncating_at_full_dim_is_valid() {
        let base = Box::new(HashEmbedder::new(32));
        let truncating = TruncatingEmbedder::new(base, 32).unwrap();

        let result = truncating.embed(&["full dimension".to_string()]).unwrap();
        assert_eq!(result[0].len(), 32);
        let norm: f32 = result[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "Full-dim truncated vector should be L2-normalized, got norm={norm}"
        );
    }
}

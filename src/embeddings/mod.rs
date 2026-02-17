pub mod hash;
pub mod llamacpp;

use anyhow::Result;

pub trait Embedder {
    fn dim(&self) -> usize;
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Embed query texts with model-specific instruction prefix for retrieval.
    ///
    /// jina-code-0.5b uses symmetric embeddings, so queries and documents share
    /// the same embedding space. Default implementation falls back to `embed()`.
    fn query_embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed(texts)
    }
}

/// HuggingFace repository for the GGUF-format jina-code embedding model.
const EMBEDDING_HF_REPO: &str = "jinaai/jina-code-embeddings-0.5b-GGUF";
/// Q8_0 quantized GGUF file (~531 MB). Higher precision than Q4 because
/// embedding quality degrades more from quantization than generation quality.
const EMBEDDING_HF_MODEL_FILE: &str = "jina-code-embeddings-0.5b-Q8_0.gguf";

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
) -> Result<Box<dyn Embedder + Send>> {
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

/// Download the jina-code-embeddings-0.5b GGUF model from HuggingFace.
///
/// Downloads a single GGUF file (~531 MB) into the HuggingFace cache,
/// then creates a symlink in `target_dir`. Uses the same pattern as
/// the LLM model download in `src/llm/mod.rs`.
fn download_embedding_model(target_dir: &crate::path::Utf8Path) -> Result<()> {
    use anyhow::Context;

    tracing::info!(
        "Downloading embedding model from huggingface.co/{}",
        EMBEDDING_HF_REPO
    );

    let api =
        hf_hub::api::sync::Api::new().context("Failed to initialize HuggingFace Hub API")?;
    let repo = api.model(EMBEDDING_HF_REPO.to_string());

    tracing::info!(
        "Downloading {} (~531 MB, this may take a few minutes)...",
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

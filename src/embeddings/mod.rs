pub mod fastembed;
pub mod hash;

use anyhow::Result;

pub trait Embedder {
    fn dim(&self) -> usize;
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// Factory function to create an embedder based on the backend configuration.
///
/// This function handles the initialization of all embedder variants and
/// returns a boxed trait object for runtime polymorphism.
///
/// # Arguments
/// * `backend` - The embeddings backend to use
/// * `model_dir` - Optional path to model directory (required for JinaCode and FastEmbed)
/// * `model_repo` - Model repository name (e.g., "jinaai/jina-embeddings-v2-base-code")
/// * `device` - Device to use for inference (CPU/Metal)
/// * `hash_dim` - Dimension for hash embedder (only used if backend is Hash)
///
/// # Returns
/// A boxed embedder implementing the Embedder trait
///
/// # Errors
/// Returns error if:
/// - Model files are missing for JinaCode
/// - FastEmbed initialization fails
/// - Invalid backend specified
pub fn create_embedder(
    backend: crate::config::EmbeddingsBackend,
    model_dir: Option<&std::path::Path>,
    model_repo: Option<&str>,
    device: crate::config::EmbeddingsDevice,
    hash_dim: usize,
) -> Result<Box<dyn Embedder + Send>> {
    match backend {
        crate::config::EmbeddingsBackend::FastEmbed => {
            let model_repo = model_repo.unwrap_or("BAAI/bge-base-en-v1.5");
            let cache_dir = model_dir;

            Ok(Box::new(fastembed::FastEmbedder::new(
                model_repo,
                cache_dir,
                device,
            )?))
        }
        crate::config::EmbeddingsBackend::Hash => Ok(Box::new(hash::HashEmbedder::new(hash_dim))),
        crate::config::EmbeddingsBackend::JinaCode => {
            // JinaCode uses FastEmbed with the Jina Code model
            // FastEmbed's JinaEmbeddingsV2BaseCode supports jina-embeddings-v2-base-code
            let model_repo = model_repo.unwrap_or("jinaai/jina-embeddings-v2-base-code");
            let cache_dir = model_dir;

            Ok(Box::new(fastembed::FastEmbedder::new(
                model_repo,
                cache_dir,
                device,
            )?))
        }
    }
}

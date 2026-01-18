use crate::config::EmbeddingsDevice;
use crate::embeddings::Embedder;
use anyhow::{anyhow, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::path::Path;

pub struct FastEmbedder {
    model: TextEmbedding,
}

impl FastEmbedder {
    pub fn new(
        model_name: &str,
        cache_dir: Option<&Path>,
        _device: EmbeddingsDevice, // FastEmbed handles device selection via ORT execution providers if configured, but currently defaults to CPU/Accelerate
    ) -> Result<Self> {
        // Map string model name to enum if possible, or error if not supported
        // FastEmbed uses an enum for supported models.
        // We'll try to match common names or default to BGE-Base.

        let model_enum = match model_name {
            "BAAI/bge-base-en-v1.5" => EmbeddingModel::BGEBaseENV15,
            "BAAI/bge-small-en-v1.5" => EmbeddingModel::BGESmallENV15,
            "sentence-transformers/all-MiniLM-L6-v2" => EmbeddingModel::AllMiniLML6V2,
            "jinaai/jina-embeddings-v2-base-en" => EmbeddingModel::JinaEmbeddingsV2BaseCode,
            // Fallback or error? Let's error to be safe, or default to BGE-Base
            _ => return Err(anyhow!("Unsupported model for FastEmbed: {}. Supported: BAAI/bge-base-en-v1.5, BAAI/bge-small-en-v1.5, sentence-transformers/all-MiniLM-L6-v2, jinaai/jina-embeddings-v2-base-en", model_name)),
        };

        let mut options = InitOptions::new(model_enum);

        if let Some(path) = cache_dir {
            options = options.with_cache_dir(path.to_path_buf());
        }

        // FastEmbed defaults to ONNX Runtime.
        // On macOS, it typically uses CoreML or CPU with Accelerate.
        // Explicit execution provider configuration might require 'ort' crate types which we might not want to expose directly yet.
        // For now, default settings are usually optimal.

        let model = TextEmbedding::try_new(options)
            .map_err(|e| anyhow!("Failed to initialize FastEmbed: {}", e))?;

        Ok(Self { model })
    }
}

impl Embedder for FastEmbedder {
    fn dim(&self) -> usize {
        // FastEmbed doesn't expose dim() directly on the struct easily without digging into the model info,
        // but we can infer it or run a dummy embedding?
        // Actually, TextEmbedding::get_embedding_model_info() -> EmbeddingModelInfo
        // But that might require holding the model info.
        // Let's check the model enum.
        // For now, we can run a dummy embedding once at startup if needed, or hardcode based on known models.
        // Or better, let's just run a dummy embedding of an empty string or "a" to get the dimension.
        // It's a bit hacky but robust.

        // Optimization: Lazy load or cache?
        // Let's try to get it from the model itself if possible.
        // Looking at docs, `model.get_model_info()` might exist.
        // If not, we'll dummy embed.

        // For the sake of implementation speed:
        match self.model.embed(vec!["a"], None) {
            Ok(vecs) => vecs.first().map(|v| v.len()).unwrap_or(0),
            Err(_) => 0,
        }
    }

    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.model
            .embed(texts.to_vec(), None) // batch_size default is usually 256
            .map_err(|e| anyhow!("Embedding failed: {}", e))
    }
}

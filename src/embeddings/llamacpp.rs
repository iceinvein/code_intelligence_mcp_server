//! llama.cpp embedding backend (jina-code-embeddings-0.5b via GGUF)
//!
//! Uses llama-cpp-2 Rust bindings with Metal GPU acceleration on Apple Silicon.
//! Shares the process-wide `LlamaBackend` singleton with the LLM generator.

use anyhow::{anyhow, Result};
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use std::num::NonZeroU32;

use crate::embeddings::Embedder;
use crate::path::Utf8Path;

/// Maximum tokens per text before truncation.
/// jina-code-0.5b supports 32K but code symbols rarely exceed 8K tokens.
const MAX_TOKENS: usize = 8192;

/// llama.cpp-based embedder for jina-code-embeddings-0.5b.
///
/// Shares the `&'static LlamaBackend` singleton with `LlamaCppGenerator`.
/// Owns its own `LlamaModel` (loaded into GPU memory). A fresh `LlamaContext`
/// is created per `embed()` call since it is `!Send`.
pub struct LlamaCppEmbedder {
    backend: &'static LlamaBackend,
    model: LlamaModel,
    dim: usize,
}

impl LlamaCppEmbedder {
    /// Load an embedding GGUF model with Metal GPU offloading.
    pub fn new(model_path: &Utf8Path) -> Result<Self> {
        tracing::info!("Loading embedding model from: {}", model_path);

        let backend = crate::llm::get_or_init_backend()?;

        // Offload all layers to Metal GPU (99 > actual layer count).
        let model_params = LlamaModelParams::default().with_n_gpu_layers(99);

        let model =
            LlamaModel::load_from_file(backend, model_path.as_std_path(), &model_params)
                .map_err(|e| anyhow!("Failed to load embedding GGUF model: {:?}", e))?;

        let dim = model.n_embd() as usize;

        tracing::info!(
            "Embedding model loaded (dim={}, vocab={}, params={})",
            dim,
            model.n_vocab(),
            model.n_params(),
        );

        Ok(Self {
            backend,
            model,
            dim,
        })
    }

    /// Embed a single text and return the L2-normalized vector.
    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        // Tokenize with AddBos::Always — embedding models need the BOS
        // token for proper sequence start signaling (unlike chat LLMs
        // where the chat template already includes it).
        let tokens = self
            .model
            .str_to_token(text, AddBos::Always)
            .map_err(|e| anyhow!("Tokenization failed: {:?}", e))?;

        if tokens.is_empty() {
            return Ok(vec![0.0; self.dim]);
        }

        // Truncate to MAX_TOKENS
        let tokens = if tokens.len() > MAX_TOKENS {
            &tokens[..MAX_TOKENS]
        } else {
            &tokens
        };

        // Create a fresh context with embeddings enabled.
        // jina-code-0.5b uses last-token (EOS) pooling.
        let n_ctx = (tokens.len() as u32).max(64);
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(n_ctx)
            .with_embeddings(true)
            .with_pooling_type(LlamaPoolingType::Last);

        let mut ctx = self
            .model
            .new_context(self.backend, ctx_params)
            .map_err(|e| anyhow!("Failed to create embedding context: {:?}", e))?;

        // Fill batch with all tokens, seq_id = 0
        let mut batch = LlamaBatch::new(tokens.len(), 1);
        for (i, token) in tokens.iter().enumerate() {
            // For pooled embeddings, we need logits enabled on the last token
            let is_last = i == tokens.len() - 1;
            batch
                .add(*token, i as i32, &[0], is_last)
                .map_err(|e| anyhow!("Failed to add token to batch: {:?}", e))?;
        }

        // Decode (forward pass)
        ctx.decode(&mut batch)
            .map_err(|e| anyhow!("Embedding decode failed: {:?}", e))?;

        // Extract the pooled embedding for sequence 0
        let embedding = ctx
            .embeddings_seq_ith(0)
            .map_err(|e| anyhow!("Failed to extract embedding: {:?}", e))?;

        // L2 normalize (required for cosine similarity in LanceDB)
        let mut vec = embedding.to_vec();
        l2_normalize(&mut vec);

        Ok(vec)
    }
}

impl Embedder for LlamaCppEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed_one(text)?);
        }
        Ok(results)
    }

    // query_embed() uses the default impl (calls embed()).
    // jina-code-0.5b uses symmetric embeddings — no query prefix needed.
}

/// L2 normalize a vector in place.
fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

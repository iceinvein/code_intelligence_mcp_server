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
use llama_cpp_2::token::LlamaToken;
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

    /// Maximum total tokens across all sequences in one sub-batch.
    /// The KV cache is allocated as n_ctx * n_seq_max, so this must be
    /// conservative. 4096 works for both large Rust symbols and many short
    /// TypeScript symbols.
    const SUB_BATCH_MAX_TOKENS: usize = 4096;

    /// Maximum sequences per sub-batch. With n_seq_max sequences, the KV cache
    /// grows proportionally. 32 keeps GPU memory bounded while still giving
    /// ~10-20x speedup over single-sequence embedding.
    const SUB_BATCH_MAX_SEQS: usize = 32;

    /// Embed a single text and return the L2-normalized vector.
    /// Used as fallback for texts that exceed SUB_BATCH_MAX_TOKENS alone.
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

    /// Embed multiple texts in a single GPU forward pass using multi-sequence batching.
    ///
    /// Tokenizes all texts upfront, packs them into sub-batches that fit within
    /// `SUB_BATCH_MAX_TOKENS` total tokens, and processes each sub-batch with
    /// one `LlamaContext` + one `decode()` call. This amortizes GPU context
    /// creation cost and improves Metal GPU utilization.
    ///
    /// Texts exceeding `SUB_BATCH_MAX_TOKENS` alone fall back to single-sequence
    /// `embed_one()`. Empty texts get zero vectors.
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if texts.len() == 1 {
            return Ok(vec![self.embed_one(&texts[0])?]);
        }

        // Phase 1: Tokenize all texts upfront
        let tokenized: Vec<Vec<LlamaToken>> = texts
            .iter()
            .map(|text| {
                let mut tokens = self
                    .model
                    .str_to_token(text, AddBos::Always)
                    .map_err(|e| anyhow!("Tokenization failed: {:?}", e))?;
                if tokens.len() > MAX_TOKENS {
                    tokens.truncate(MAX_TOKENS);
                }
                Ok(tokens)
            })
            .collect::<Result<Vec<_>>>()?;

        // Phase 2: Pack into sub-batches by token budget and process
        let mut results: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        let mut sub_batch_indices: Vec<usize> = Vec::new();
        let mut sub_batch_token_count: usize = 0;

        for (i, tokens) in tokenized.iter().enumerate() {
            let token_len = tokens.len();

            // Empty text: zero vector, no GPU work needed
            if token_len == 0 {
                results[i] = Some(vec![0.0; self.dim]);
                continue;
            }

            // Oversized text: flush current sub-batch, then embed solo via fallback
            if token_len > Self::SUB_BATCH_MAX_TOKENS {
                self.flush_sub_batch(&sub_batch_indices, &tokenized, &mut results)?;
                sub_batch_indices.clear();
                sub_batch_token_count = 0;
                results[i] = Some(self.embed_one(&texts[i])?);
                continue;
            }

            // Would exceed token or sequence budget: flush current sub-batch first
            if (sub_batch_token_count + token_len > Self::SUB_BATCH_MAX_TOKENS
                || sub_batch_indices.len() >= Self::SUB_BATCH_MAX_SEQS)
                && !sub_batch_indices.is_empty()
            {
                self.flush_sub_batch(&sub_batch_indices, &tokenized, &mut results)?;
                sub_batch_indices.clear();
                sub_batch_token_count = 0;
            }

            sub_batch_indices.push(i);
            sub_batch_token_count += token_len;
        }

        // Flush remaining sub-batch
        self.flush_sub_batch(&sub_batch_indices, &tokenized, &mut results)?;

        // Unwrap Options -- all should be Some at this point
        results
            .into_iter()
            .enumerate()
            .map(|(i, opt)| opt.ok_or_else(|| anyhow!("Missing embedding for text index {}", i)))
            .collect()
    }

    /// Process a sub-batch of texts through a single GPU forward pass.
    ///
    /// Creates one `LlamaContext` sized to the total token count, packs all
    /// sequences into one `LlamaBatch` via `add_sequence`, runs a single
    /// `decode()`, and extracts per-sequence embeddings via `embeddings_seq_ith`.
    fn flush_sub_batch(
        &self,
        indices: &[usize],
        tokenized: &[Vec<LlamaToken>],
        results: &mut [Option<Vec<f32>>],
    ) -> Result<()> {
        if indices.is_empty() {
            return Ok(());
        }

        let total_tokens: usize = indices.iter().map(|&i| tokenized[i].len()).sum();
        let max_seq_len = indices.iter().map(|&i| tokenized[i].len()).max().unwrap_or(0);
        let n_seqs = indices.len();

        // n_ctx must be max_seq_len * n_seqs because llama.cpp divides KV cache
        // as n_ctx / n_seq_max per sequence. Using just total_tokens would
        // under-allocate when one sequence is much longer than average.
        let n_ctx = ((max_seq_len * n_seqs) as u32).max(64);

        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(total_tokens as u32)
            .with_n_seq_max(n_seqs as u32)
            .with_embeddings(true)
            .with_pooling_type(LlamaPoolingType::Last);

        let mut ctx = self
            .model
            .new_context(self.backend, ctx_params)
            .map_err(|e| anyhow!("Failed to create batch embedding context: {:?}", e))?;

        let mut batch = LlamaBatch::new(total_tokens, indices.len() as i32);

        for (seq_idx, &text_idx) in indices.iter().enumerate() {
            let tokens = &tokenized[text_idx];
            // add_sequence sets logits=true on the last token automatically,
            // which is what Last pooling needs.
            batch
                .add_sequence(tokens, seq_idx as i32, false)
                .map_err(|e| {
                    anyhow!(
                        "Failed to add sequence {} ({} tokens) to batch: {:?}",
                        seq_idx,
                        tokens.len(),
                        e
                    )
                })?;
        }

        ctx.decode(&mut batch)
            .map_err(|e| anyhow!("Batch embedding decode failed: {:?}", e))?;

        for (seq_idx, &text_idx) in indices.iter().enumerate() {
            let embedding = ctx.embeddings_seq_ith(seq_idx as i32).map_err(|e| {
                anyhow!(
                    "Failed to extract embedding for seq {}: {:?}",
                    seq_idx,
                    e
                )
            })?;
            let mut vec = embedding.to_vec();
            l2_normalize(&mut vec);
            results[text_idx] = Some(vec);
        }

        Ok(())
    }
}

impl Embedder for LlamaCppEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_batch(texts)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that multi-sequence batch embedding produces the same vectors
    /// as single-sequence embedding (within floating-point tolerance).
    ///
    /// Requires the real embedding model (~531MB). Run with:
    ///   cargo test --lib embeddings::llamacpp::tests::batch_matches_single -- --ignored
    #[test]
    #[ignore]
    fn batch_matches_single() {
        let home = std::env::var("HOME").expect("HOME not set");
        let model_path = crate::path::Utf8PathBuf::from(format!(
            "{}/.code-intelligence/models/jina-code-embeddings-0.5b-gguf/jina-code-embeddings-0.5b-Q8_0.gguf",
            home
        ));
        if !model_path.exists() {
            eprintln!("Skipping: embedding model not found at {}", model_path);
            return;
        }

        let mut embedder = LlamaCppEmbedder::new(&model_path).expect("failed to load model");

        let texts: Vec<String> = vec![
            "fn hello() { println!(\"hello\"); }".to_string(),
            "struct Config { port: u16, host: String }".to_string(),
            "async fn fetch_data(url: &str) -> Result<Response> { reqwest::get(url).await }"
                .to_string(),
            "/// Parse a TOML configuration file.\nfn parse_config(path: &Path) -> Config { todo!() }"
                .to_string(),
        ];

        // Get single-sequence embeddings (one at a time via embed_one)
        let single: Vec<Vec<f32>> = texts
            .iter()
            .map(|t| embedder.embed_one(t).expect("embed_one failed"))
            .collect();

        // Get batch embeddings (multi-sequence via embed_batch)
        let batch = embedder.embed(&texts).expect("batch embed failed");

        assert_eq!(single.len(), batch.len());
        for (i, (s, b)) in single.iter().zip(&batch).enumerate() {
            assert_eq!(s.len(), b.len(), "Dimension mismatch for text {}", i);
            let max_diff: f32 = s
                .iter()
                .zip(b)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(
                max_diff < 1e-3,
                "Text {} differs by {:.6} (threshold 1e-3)",
                i,
                max_diff
            );
        }
    }
}

//! llama.cpp LLM generation backend (Qwen2.5-Coder-1.5B-Instruct via GGUF)
//!
//! Uses llama-cpp-2 Rust bindings with Metal GPU acceleration on Apple Silicon.
//! Per-call context creation avoids Send/Sync issues with LlamaContext while
//! keeping the model weights loaded in GPU memory across calls.

use anyhow::{anyhow, Result};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel, Special};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;

use crate::path::Utf8Path;

use super::LlmGenerator;

/// llama.cpp-based LLM generator for Qwen2.5-Coder-1.5B-Instruct.
///
/// Borrows the shared `&'static LlamaBackend` singleton and owns the model
/// weights (loaded into GPU memory). A fresh `LlamaContext` is created per
/// `generate()` call since it is `!Send`. Context creation is cheap
/// (~microseconds) compared to inference (~300ms per symbol).
pub struct LlamaCppGenerator {
    backend: &'static LlamaBackend,
    model: LlamaModel,
}

impl LlamaCppGenerator {
    /// Load a GGUF model with Metal GPU offloading.
    ///
    /// # Arguments
    /// * `model_path` - Path to the `.gguf` model file
    pub fn new(model_path: &Utf8Path) -> Result<Self> {
        tracing::info!("Loading LLM from: {}", model_path);

        let backend = super::get_or_init_backend()?;

        // Offload all 28 transformer layers to Metal GPU.
        // 99 > actual layer count (28), so llama.cpp caps at the model's max.
        let model_params = LlamaModelParams::default().with_n_gpu_layers(99);

        let model =
            LlamaModel::load_from_file(backend, model_path.as_std_path(), &model_params)
                .map_err(|e| anyhow!("Failed to load GGUF model: {:?}", e))?;

        tracing::info!(
            "LLM loaded successfully (vocab={}, params={}, ctx_train={})",
            model.n_vocab(),
            model.n_params(),
            model.n_ctx_train(),
        );

        Ok(Self { backend, model })
    }
}

impl LlmGenerator for LlamaCppGenerator {
    fn generate(&self, prompt: &str, max_tokens: u32) -> Result<String> {
        // Tokenize with AddBos::Never — Qwen2.5 chat template (<|im_start|>)
        // already includes the sequence start token.
        let tokens = self
            .model
            .str_to_token(prompt, AddBos::Never)
            .map_err(|e| anyhow!("Tokenization failed: {:?}", e))?;

        if tokens.is_empty() {
            return Ok(String::new());
        }

        // Create a fresh context per call (LlamaContext is !Send).
        // n_ctx=512 fits our prompts (~275-425 tokens + 30 generated).
        let ctx_params =
            LlamaContextParams::default().with_n_ctx(NonZeroU32::new(512));
        let mut ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .map_err(|e| anyhow!("Failed to create llama context: {:?}", e))?;

        // Fill batch with prompt tokens
        let n_prompt = tokens.len();
        let mut batch = LlamaBatch::new(512, 1);
        for (i, token) in tokens.iter().enumerate() {
            let is_last = i == n_prompt - 1;
            batch
                .add(*token, i as i32, &[0], is_last)
                .map_err(|e| anyhow!("Failed to add token to batch: {:?}", e))?;
        }

        // Decode prompt
        ctx.decode(&mut batch)
            .map_err(|e| anyhow!("Prompt decode failed: {:?}", e))?;

        // Greedy sampling (deterministic, same as previous ORT argmax)
        let mut sampler = LlamaSampler::greedy();

        // Sample first token
        let mut new_token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(new_token);

        if self.model.is_eog_token(new_token) {
            return Ok(String::new());
        }

        let mut output_tokens = vec![new_token];
        let mut pos = n_prompt as i32;

        // Generate remaining tokens
        for _ in 1..max_tokens {
            batch.clear();
            batch
                .add(new_token, pos, &[0], true)
                .map_err(|e| anyhow!("Failed to add token to batch: {:?}", e))?;
            pos += 1;

            ctx.decode(&mut batch)
                .map_err(|e| anyhow!("Token decode failed: {:?}", e))?;

            new_token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(new_token);

            if self.model.is_eog_token(new_token) {
                break;
            }

            output_tokens.push(new_token);
        }

        // Detokenize
        let output = self
            .model
            .tokens_to_str(&output_tokens, Special::Tokenize)
            .map_err(|e| anyhow!("Detokenization failed: {:?}", e))?;

        Ok(output.trim().to_string())
    }
}


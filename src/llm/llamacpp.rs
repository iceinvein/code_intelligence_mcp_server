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
    n_ctx: u32,
}

impl LlamaCppGenerator {
    /// Load a GGUF model with Metal GPU offloading.
    ///
    /// `n_ctx` must accommodate the largest expected prompt plus the desired
    /// `max_tokens` output. Description-pipeline callers use 512 (small,
    /// fixed-shape prompts). `ask_code` uses several thousand because each
    /// answer prompt embeds retrieved code evidence.
    pub fn new(model_path: &Utf8Path, n_ctx: u32) -> Result<Self> {
        assert!(n_ctx >= 32, "n_ctx must be at least 32 tokens");
        tracing::info!("Loading LLM from: {} (n_ctx={})", model_path, n_ctx);

        let backend = super::get_or_init_backend()?;

        // Offload all 28 transformer layers to Metal GPU.
        // 99 > actual layer count (28), so llama.cpp caps at the model's max.
        let model_params = LlamaModelParams::default().with_n_gpu_layers(99);

        let model = LlamaModel::load_from_file(backend, model_path.as_std_path(), &model_params)
            .map_err(|e| anyhow!("Failed to load GGUF model: {:?}", e))?;

        tracing::info!(
            "LLM loaded successfully (vocab={}, params={}, ctx_train={})",
            model.n_vocab(),
            model.n_params(),
            model.n_ctx_train(),
        );

        Ok(Self {
            backend,
            model,
            n_ctx,
        })
    }

    pub fn n_ctx(&self) -> u32 {
        self.n_ctx
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
        // n_ctx is set per-generator: description path uses 512 (fast,
        // ~275-425 prompt tokens), ask_code uses tens of thousands to fit
        // retrieved code evidence. Generation extends past the prompt by
        // `max_tokens`, so the prompt+output must both fit inside n_ctx;
        // otherwise llama.cpp aborts inside `llama_decode`.
        let n_prompt = tokens.len();
        let total_budget = n_prompt
            .saturating_add(max_tokens as usize)
            .saturating_add(8); // small safety for special tokens
        if total_budget > self.n_ctx as usize {
            return Err(anyhow!(
                "ask_code prompt ({} tokens) + max_tokens ({}) exceeds n_ctx ({}); raise ANSWER_LLM_N_CTX or trim evidence",
                n_prompt,
                max_tokens,
                self.n_ctx,
            ));
        }
        // n_batch defaults to 512 in llama.cpp; if the prompt has more tokens
        // than that, `llama_decode` asserts `n_tokens_all <= cparams.n_batch`.
        // Size n_batch to the full context so any prompt that fits in n_ctx
        // can be decoded. n_ubatch stays at its default (512) for memory.
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(self.n_ctx))
            .with_n_batch(self.n_ctx);
        let mut ctx = self
            .model
            .new_context(self.backend, ctx_params)
            .map_err(|e| anyhow!("Failed to create llama context: {:?}", e))?;

        // Fill batch with prompt tokens. Batch capacity must cover the full
        // prompt; we size it to n_ctx so it can also hold subsequent decode
        // steps if a caller ever batches generation (we currently do not).
        let mut batch = LlamaBatch::new(self.n_ctx as usize, 1);
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
        // Generate remaining tokens
        for pos in n_prompt as i32..(n_prompt as i32 + max_tokens as i32 - 1) {
            batch.clear();
            batch
                .add(new_token, pos, &[0], true)
                .map_err(|e| anyhow!("Failed to add token to batch: {:?}", e))?;

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

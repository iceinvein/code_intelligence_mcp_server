//! Chat LLM backend — Qwen2.5-Coder-14B-Instruct with streaming generation.
//!
//! Downloads and loads the 14B GGUF model from HuggingFace, with full Metal GPU
//! offloading on Apple Silicon. Provides both streaming (token-by-token) and
//! non-streaming generation interfaces for use in the RAG chatbot pipeline.
//!
//! The `LlamaContext` type is `!Send`, so each call to `generate_stream` or
//! `generate` creates a fresh context. Context creation is cheap relative to
//! inference and amortises to nothing across a multi-turn conversation.

use anyhow::{anyhow, Result};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel, Special};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use tokio::sync::mpsc;

use crate::path::{Utf8Path, Utf8PathBuf};

/// HuggingFace repository for the 14B GGUF model.
const HF_REPO: &str = "Qwen/Qwen2.5-Coder-14B-Instruct-GGUF";

/// Q4_K_M quantized GGUF model file (~9 GB). GGUF embeds the tokenizer,
/// so no separate download is required.
const HF_MODEL_FILE: &str = "qwen2.5-coder-14b-instruct-q4_k_m.gguf";

/// Context window size in tokens.
const CTX_SIZE: u32 = 8192;


/// Download the Qwen2.5-Coder-14B-Instruct GGUF model from HuggingFace.
///
/// Downloads the GGUF file into the HuggingFace cache
/// (`~/.cache/huggingface/hub/`), then creates a symlink at
/// `target_dir/qwen2.5-coder-14b-instruct-q4_k_m.gguf`.
///
/// # Arguments
/// * `target_dir` - Directory in which to place the symlink
///
/// # Returns
/// Path to the symlink (which resolves to the cached model file)
pub fn download_chat_model(target_dir: &Utf8Path) -> Result<Utf8PathBuf> {
    use anyhow::Context;

    tracing::info!("Downloading chat LLM model from huggingface.co/{}", HF_REPO);

    let api = hf_hub::api::sync::Api::new()
        .context("Failed to initialize HuggingFace Hub API")?;
    let repo = api.model(HF_REPO.to_string());

    tracing::info!(
        "Downloading {} (~9 GB, this may take several minutes)...",
        HF_MODEL_FILE
    );
    let model_cached = repo
        .get(HF_MODEL_FILE)
        .context("Failed to download GGUF model file")?;

    // Ensure the target directory exists
    std::fs::create_dir_all(target_dir.as_std_path())
        .context("Failed to create model directory")?;

    let target_model = target_dir.join(HF_MODEL_FILE);
    symlink_model(&model_cached, target_model.as_std_path())
        .context("Failed to link GGUF model file")?;

    tracing::info!("Chat LLM model ready at {}", target_model);
    Ok(target_model)
}

/// Create a Unix symlink from `source` to `target`, removing any existing
/// file or stale symlink at `target` first.
fn symlink_model(source: &std::path::Path, target: &std::path::Path) -> Result<()> {
    // Remove existing target (stale symlink or old file)
    if target.exists() || target.symlink_metadata().is_ok() {
        std::fs::remove_file(target).ok();
    }

    std::os::unix::fs::symlink(source, target)?;
    Ok(())
}

/// Chat LLM backed by Qwen2.5-Coder-14B-Instruct loaded via llama.cpp.
///
/// Owns the model weights (resident in Metal GPU VRAM) and borrows the
/// process-wide `LlamaBackend` singleton. A fresh `LlamaContext` is created
/// on every `generate` / `generate_stream` call because `LlamaContext` is
/// `!Send`. This is safe: the shared weights are never mutated after load,
/// and each context holds only per-call KV-cache state.
pub struct ChatLlm {
    backend: &'static LlamaBackend,
    model: LlamaModel,
}

// SAFETY: `LlamaModel` contains a raw pointer to immutable C++ model weights.
// The weights are never mutated after `load_from_file` returns. All mutable
// per-call state lives in `LlamaContext`, which is created and destroyed
// within each `generate` / `generate_stream` call (never shared across
// threads). Therefore `ChatLlm` is safe to send across threads and to hold
// behind a shared reference from multiple threads simultaneously.
unsafe impl Send for ChatLlm {}
unsafe impl Sync for ChatLlm {}

impl ChatLlm {
    /// Load a GGUF chat model with full Metal GPU offloading.
    ///
    /// # Arguments
    /// * `model_path` - Path to the `.gguf` model file
    ///
    /// # Errors
    /// Returns an error if the backend cannot be initialised or the model
    /// file cannot be loaded (e.g. file not found, corrupt GGUF).
    pub fn new(model_path: &Utf8Path) -> Result<Self> {
        tracing::info!("Loading chat LLM from: {}", model_path);

        let backend = crate::llm::get_or_init_backend()?;

        // Offload all transformer layers to Metal GPU.
        // 99 exceeds the actual layer count; llama.cpp caps at the model max.
        let model_params = LlamaModelParams::default().with_n_gpu_layers(99);

        let model =
            LlamaModel::load_from_file(backend, model_path.as_std_path(), &model_params)
                .map_err(|e| anyhow!("Failed to load chat GGUF model: {:?}", e))?;

        tracing::info!(
            "Chat LLM loaded (vocab={}, params={}, ctx_train={})",
            model.n_vocab(),
            model.n_params(),
            model.n_ctx_train(),
        );

        Ok(Self { backend, model })
    }

    /// Stream generated tokens to a channel, one decoded string per token.
    ///
    /// This method runs synchronously and is intended to be called from a
    /// `tokio::task::spawn_blocking` context. Tokens are sent via the returned
    /// [`mpsc::Receiver`] as soon as they are sampled. Generation stops when:
    /// - An end-of-generation token is produced, or
    /// - `max_tokens` tokens have been generated, or
    /// - The receiver is dropped (client disconnected).
    ///
    /// # Arguments
    /// * `prompt` - Full formatted prompt (including chat template markers)
    /// * `max_tokens` - Upper bound on generated tokens
    ///
    /// # Returns
    /// A receiver that yields token strings in order.
    ///
    /// # Errors
    /// Returns an error if tokenisation or context creation fails.
    pub fn generate_stream(
        &self,
        prompt: &str,
        max_tokens: u32,
    ) -> Result<mpsc::Receiver<String>> {
        let (tx, rx) = mpsc::channel::<String>(64);

        let tokens = self
            .model
            .str_to_token(prompt, AddBos::Never)
            .map_err(|e| anyhow!("Tokenisation failed: {:?}", e))?;

        if tokens.is_empty() {
            return Ok(rx);
        }

        let n_prompt = tokens.len();

        // Context window must hold prompt + generated tokens.
        let ctx_params =
            LlamaContextParams::default().with_n_ctx(NonZeroU32::new(CTX_SIZE));
        let mut ctx = self
            .model
            .new_context(self.backend, ctx_params)
            .map_err(|e| anyhow!("Failed to create llama context: {:?}", e))?;

        // Fill batch with prompt tokens; request logits only for the last one.
        let mut batch = LlamaBatch::new(CTX_SIZE as usize, 1);
        for (i, token) in tokens.iter().enumerate() {
            let is_last = i == n_prompt - 1;
            batch
                .add(*token, i as i32, &[0], is_last)
                .map_err(|e| anyhow!("Failed to add token to batch: {:?}", e))?;
        }

        // Decode the prompt
        ctx.decode(&mut batch)
            .map_err(|e| anyhow!("Prompt decode failed: {:?}", e))?;

        // Temperature sampling: temp → dist provides stochastic but coherent output
        let mut sampler =
            LlamaSampler::chain_simple([LlamaSampler::temp(0.7), LlamaSampler::dist(0)]);

        // Sample first generated token
        let mut new_token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(new_token);

        if self.model.is_eog_token(new_token) {
            return Ok(rx);
        }

        // Send first token; return early if receiver already dropped
        let token_str = self
            .model
            .token_to_str(new_token, Special::Tokenize)
            .unwrap_or_default();
        if tx.blocking_send(token_str).is_err() {
            return Ok(rx);
        }

        let mut pos = n_prompt as i32;

        // Autoregressive generation loop
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

            let token_str = self
                .model
                .token_to_str(new_token, Special::Tokenize)
                .unwrap_or_default();

            // Stop if receiver was dropped (client disconnected)
            if tx.blocking_send(token_str).is_err() {
                break;
            }
        }

        Ok(rx)
    }

    /// Generate a complete response, returning the full output string.
    ///
    /// Equivalent to collecting all tokens from [`generate_stream`] but
    /// without channel overhead. Suitable for tool-call rounds where the
    /// full response must be parsed before acting.
    ///
    /// # Arguments
    /// * `prompt` - Full formatted prompt (including chat template markers)
    /// * `max_tokens` - Upper bound on generated tokens
    ///
    /// # Returns
    /// The generated text, trimmed of leading/trailing whitespace.
    ///
    /// # Errors
    /// Returns an error if tokenisation or context creation fails.
    pub fn generate(&self, prompt: &str, max_tokens: u32) -> Result<String> {
        let tokens = self
            .model
            .str_to_token(prompt, AddBos::Never)
            .map_err(|e| anyhow!("Tokenisation failed: {:?}", e))?;

        if tokens.is_empty() {
            return Ok(String::new());
        }

        let n_prompt = tokens.len();

        let ctx_params =
            LlamaContextParams::default().with_n_ctx(NonZeroU32::new(CTX_SIZE));
        let mut ctx = self
            .model
            .new_context(self.backend, ctx_params)
            .map_err(|e| anyhow!("Failed to create llama context: {:?}", e))?;

        let mut batch = LlamaBatch::new(CTX_SIZE as usize, 1);
        for (i, token) in tokens.iter().enumerate() {
            let is_last = i == n_prompt - 1;
            batch
                .add(*token, i as i32, &[0], is_last)
                .map_err(|e| anyhow!("Failed to add token to batch: {:?}", e))?;
        }

        ctx.decode(&mut batch)
            .map_err(|e| anyhow!("Prompt decode failed: {:?}", e))?;

        let mut sampler =
            LlamaSampler::chain_simple([LlamaSampler::temp(0.7), LlamaSampler::dist(0)]);

        // Sample first generated token
        let mut new_token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(new_token);

        if self.model.is_eog_token(new_token) {
            return Ok(String::new());
        }

        let mut output_tokens = vec![new_token];
        let mut pos = n_prompt as i32;

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

        let output = self
            .model
            .tokens_to_str(&output_tokens, Special::Tokenize)
            .map_err(|e| anyhow!("Detokenisation failed: {:?}", e))?;

        Ok(output.trim().to_string())
    }
}

/// Build a Qwen2.5 chat template prompt for the chatbot.
///
/// # Arguments
/// * `system` - System instruction (role/behaviour for the assistant)
/// * `user` - User message content
///
/// # Returns
/// Formatted prompt string ready for tokenisation.
///
/// # Examples
/// ```
/// use code_intelligence_mcp_server::chat::llm::build_chat_prompt;
///
/// let prompt = build_chat_prompt(
///     "You are a helpful coding assistant.",
///     "Explain what a BTreeMap is.",
/// );
/// assert!(prompt.contains("<|im_start|>system"));
/// assert!(prompt.contains("You are a helpful coding assistant."));
/// assert!(prompt.contains("<|im_start|>user"));
/// assert!(prompt.contains("Explain what a BTreeMap is."));
/// assert!(prompt.contains("<|im_start|>assistant"));
/// ```
pub fn build_chat_prompt(system: &str, user: &str) -> String {
    format!(
        "<|im_start|>system\n{}<|im_end|>\n\
         <|im_start|>user\n{}<|im_end|>\n\
         <|im_start|>assistant\n",
        system, user
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_chat_prompt_structure() {
        let prompt = build_chat_prompt(
            "You are a helpful coding assistant.",
            "What does HashMap do?",
        );

        assert!(prompt.contains("<|im_start|>system\n"));
        assert!(prompt.contains("You are a helpful coding assistant."));
        assert!(prompt.contains("<|im_end|>\n<|im_start|>user\n"));
        assert!(prompt.contains("What does HashMap do?"));
        assert!(prompt.contains("<|im_end|>\n<|im_start|>assistant\n"));
    }

    #[test]
    fn test_build_chat_prompt_ends_with_assistant_marker() {
        let prompt = build_chat_prompt("system msg", "user msg");
        assert!(
            prompt.ends_with("<|im_start|>assistant\n"),
            "Prompt must end with assistant marker so the model continues generation"
        );
    }

    #[test]
    fn test_chat_llm_constants() {
        assert_eq!(HF_REPO, "Qwen/Qwen2.5-Coder-14B-Instruct-GGUF");
        assert_eq!(HF_MODEL_FILE, "qwen2.5-coder-14b-instruct-q4_k_m.gguf");
        assert_eq!(CTX_SIZE, 8192);
    }
}

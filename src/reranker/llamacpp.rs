//! llama.cpp cross-encoder reranker backend (Qwen2.5-Coder-1.5B-Instruct via GGUF)
//!
//! Implements zero-shot relevance scoring by prompting the LLM to rate
//! query–document relevance on a 0–10 scale. The model is the same GGUF
//! file used for symbol description generation, so no extra download is
//! required.
//!
//! # Scoring approach
//!
//! Each (query, document) pair is formatted as a Qwen2.5 chat prompt asking
//! the model to rate relevance from 0–10. We generate at most 2 tokens and
//! parse the first digit character as the raw score, then normalise it to
//! the 0.0–1.0 range. When the model produces a non-digit or empty output
//! the document receives a neutral score of 0.5.
//!
//! # Thread safety
//!
//! `LlamaContext` is `!Send`, so inference runs inside
//! `tokio::task::spawn_blocking`. The shared `&'static LlamaBackend` and
//! the owned `LlamaModel` are `Send + Sync`; only the context created per
//! call is single-threaded.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel, Special},
    model::params::LlamaModelParams,
    sampling::LlamaSampler,
};
use std::num::NonZeroU32;
use std::sync::Arc;

use crate::path::Utf8Path;
use super::{RerankDocument, Reranker};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// HuggingFace repo for the reranker model (same as LLM descriptions).
pub const HF_REPO: &str = "Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF";
/// GGUF model filename used both for LLM descriptions and reranking.
pub const HF_MODEL_FILE: &str = "qwen2.5-coder-1.5b-instruct-q4_k_m.gguf";

/// Context window for reranking prompts.
///
/// Each prompt is: system (~60 tokens) + query (~50 tokens) + document
/// (~300 tokens) + template overhead. 1024 tokens fits comfortably.
const RERANK_N_CTX: u32 = 1024;

/// Maximum tokens to generate per scoring call. We only need the single digit
/// character ("0"–"9" or "10"), so 3 tokens is more than sufficient.
const RERANK_MAX_TOKENS: u32 = 3;

/// Score returned when the model produces unparseable output (neutral).
const FALLBACK_SCORE: f32 = 0.5;

// ---------------------------------------------------------------------------
// Prompt builder
// ---------------------------------------------------------------------------

/// Build the Qwen2.5 chat prompt for relevance scoring.
///
/// The system prompt instructs the model to output only a single digit on
/// the 0–10 scale. The assistant turn is left open so the model completes it.
///
/// # Arguments
/// * `query` - The search query (e.g., "how does authentication work?")
/// * `document` - The code document text (symbol body + name context)
pub fn build_rerank_prompt(query: &str, document: &str) -> String {
    // Truncate document to avoid exceeding context window.
    // At ~3.5 chars/token and a 1024-token budget, 700 chars is conservative.
    const MAX_DOC_CHARS: usize = 700;
    let truncated_doc: String = if document.len() <= MAX_DOC_CHARS {
        document.to_string()
    } else {
        let mut end = MAX_DOC_CHARS;
        while end > 0 && !document.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &document[..end])
    };

    format!(
        "<|im_start|>system\n\
         Rate the relevance of the code to the search query on a scale of 0-10. \
         Respond with only the number.<|im_end|>\n\
         <|im_start|>user\n\
         Query: {query}\n\
         Code: {truncated_doc}<|im_end|>\n\
         <|im_start|>assistant\n"
    )
}

/// Parse a relevance score from the raw model output.
///
/// Extracts the first ASCII digit sequence and normalises to [0.0, 1.0].
/// Returns `FALLBACK_SCORE` if no digit is found.
fn parse_score(raw: &str) -> f32 {
    let trimmed = raw.trim();

    // Accept "10" as a special case before single-digit extraction
    if trimmed.starts_with("10") {
        return 1.0;
    }

    // Take the first digit character
    if let Some(c) = trimmed.chars().next() {
        if let Some(d) = c.to_digit(10) {
            return d as f32 / 10.0;
        }
    }

    FALLBACK_SCORE
}

// ---------------------------------------------------------------------------
// LlamaCppReranker
// ---------------------------------------------------------------------------

/// Cross-encoder reranker backed by Qwen2.5-Coder-1.5B via llama.cpp.
///
/// The model weights are shared with the LLM description generator via
/// `&'static LlamaBackend`. A fresh `LlamaContext` is created per inference
/// call because `LlamaContext` is `!Send`.
pub struct LlamaCppReranker {
    backend: &'static llama_cpp_2::llama_backend::LlamaBackend,
    model: Arc<LlamaModel>,
    top_k: usize,
}

impl LlamaCppReranker {
    /// Load a GGUF model for cross-encoder reranking with Metal GPU offload.
    ///
    /// # Arguments
    /// * `model_path` - Path to the `.gguf` model file
    /// * `top_k` - Maximum number of documents to rerank
    pub fn new(model_path: &Utf8Path, top_k: usize) -> Result<Self> {
        tracing::info!(
            model_path = %model_path,
            top_k,
            "Loading reranker model"
        );

        let backend = crate::llm::get_or_init_backend()?;

        // Offload all transformer layers to Metal GPU.
        // 99 exceeds any model's actual layer count; llama.cpp caps at maximum.
        let model_params = LlamaModelParams::default().with_n_gpu_layers(99);

        let model =
            LlamaModel::load_from_file(backend, model_path.as_std_path(), &model_params)
                .map_err(|e| anyhow!("Failed to load reranker GGUF model: {:?}", e))?;

        tracing::info!(
            vocab = model.n_vocab(),
            params = model.n_params(),
            ctx_train = model.n_ctx_train(),
            "Reranker model loaded"
        );

        Ok(Self {
            backend,
            model: Arc::new(model),
            top_k,
        })
    }

    /// Score a single (query, document) pair synchronously.
    ///
    /// Creates a fresh context per call since `LlamaContext` is `!Send`.
    /// Called from `spawn_blocking` to avoid blocking the async executor.
    fn score_pair(
        backend: &'static llama_cpp_2::llama_backend::LlamaBackend,
        model: &LlamaModel,
        query: &str,
        document: &str,
    ) -> Result<f32> {
        let prompt = build_rerank_prompt(query, document);

        let tokens = model
            .str_to_token(&prompt, AddBos::Never)
            .map_err(|e| anyhow!("Reranker tokenization failed: {:?}", e))?;

        if tokens.is_empty() {
            return Ok(FALLBACK_SCORE);
        }

        // Reranking prompts are short; 1024 tokens is the maximum we need.
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(RERANK_N_CTX));

        let mut ctx = model
            .new_context(backend, ctx_params)
            .map_err(|e| anyhow!("Failed to create reranker context: {:?}", e))?;

        let n_prompt = tokens.len();
        let mut batch = LlamaBatch::new(RERANK_N_CTX as usize, 1);

        for (i, token) in tokens.iter().enumerate() {
            let is_last = i == n_prompt - 1;
            batch
                .add(*token, i as i32, &[0], is_last)
                .map_err(|e| anyhow!("Failed to add token to reranker batch: {:?}", e))?;
        }

        ctx.decode(&mut batch)
            .map_err(|e| anyhow!("Reranker prompt decode failed: {:?}", e))?;

        let mut sampler = LlamaSampler::greedy();
        let mut output_tokens = Vec::new();
        let mut pos = n_prompt as i32;

        // Sample the first token
        let first_token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(first_token);

        if model.is_eog_token(first_token) {
            // Empty generation — return fallback
            return Ok(FALLBACK_SCORE);
        }

        output_tokens.push(first_token);

        // Generate up to RERANK_MAX_TOKENS (we only need 1–2 for "0"–"10")
        for _ in 1..RERANK_MAX_TOKENS {
            batch.clear();
            let last = *output_tokens.last().unwrap();
            batch
                .add(last, pos, &[0], true)
                .map_err(|e| anyhow!("Failed to add token to reranker batch: {:?}", e))?;
            pos += 1;

            ctx.decode(&mut batch)
                .map_err(|e| anyhow!("Reranker token decode failed: {:?}", e))?;

            let next = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(next);

            if model.is_eog_token(next) {
                break;
            }
            output_tokens.push(next);
        }

        let raw = model
            .tokens_to_str(&output_tokens, Special::Tokenize)
            .map_err(|e| anyhow!("Reranker detokenization failed: {:?}", e))?;

        Ok(parse_score(&raw))
    }
}

#[async_trait]
impl Reranker for LlamaCppReranker {
    async fn rerank(&self, query: &str, documents: &[RerankDocument]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }

        let backend = self.backend;
        let model = Arc::clone(&self.model);
        let query_owned = query.to_string();

        // Limit to top_k documents to bound latency
        let docs_to_score: Vec<RerankDocument> =
            documents.iter().take(self.top_k).cloned().collect();

        // Run inference in a blocking thread — LlamaContext is !Send
        let scores = tokio::task::spawn_blocking(move || {
            let mut scores = Vec::with_capacity(docs_to_score.len());
            for doc in &docs_to_score {
                let text = format!("{}: {}", doc.name, doc.text);
                let score = match Self::score_pair(backend, &model, &query_owned, &text) {
                    Ok(s) => {
                        tracing::debug!(doc_name = %doc.name, score = s, "Reranker scored document");
                        s
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, doc_name = %doc.name, "Reranker scoring failed, using fallback");
                        FALLBACK_SCORE
                    }
                };
                scores.push(score);
            }
            scores
        })
        .await
        .map_err(|e| anyhow!("Reranker task panicked: {:?}", e))?;

        // Pad with fallback scores for documents beyond top_k
        let mut result = scores;
        result.resize(documents.len(), FALLBACK_SCORE);

        Ok(result)
    }

    fn top_k(&self) -> usize {
        self.top_k
    }
}

// ---------------------------------------------------------------------------
// Auto-download
// ---------------------------------------------------------------------------

/// Download the Qwen2.5-Coder-1.5B-Instruct GGUF model for reranking.
///
/// Reuses the same GGUF file as the LLM description generator, so this is a
/// no-op when the LLM model is already present. The model is fetched from
/// HuggingFace into the system cache and symlinked into `target_dir`.
///
/// # Arguments
/// * `target_dir` - Directory where the symlink should be placed
pub fn download_reranker_model(target_dir: &Utf8Path) -> Result<()> {
    use anyhow::Context;

    tracing::info!("Downloading reranker model from huggingface.co/{}", HF_REPO);

    let api = hf_hub::api::sync::Api::new()
        .context("Failed to initialize HuggingFace Hub API")?;
    let repo = api.model(HF_REPO.to_string());

    tracing::info!(
        "Downloading {} (~1.1 GB)...",
        HF_MODEL_FILE
    );
    let cached = repo
        .get(HF_MODEL_FILE)
        .context("Failed to download reranker GGUF model file")?;

    std::fs::create_dir_all(target_dir.as_std_path())
        .context("Failed to create reranker model directory")?;

    let target_file = target_dir.join(HF_MODEL_FILE);
    symlink_or_copy(&cached, target_file.as_std_path())
        .context("Failed to link reranker GGUF model file")?;

    tracing::info!("Reranker model ready at {}", target_dir);
    Ok(())
}

/// Create a symlink from `source` to `target`, replacing any existing entry.
fn symlink_or_copy(source: &std::path::Path, target: &std::path::Path) -> Result<()> {
    if target.exists() || target.symlink_metadata().is_ok() {
        std::fs::remove_file(target).ok();
    }
    std::os::unix::fs::symlink(source, target)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── prompt builder ───────────────────────────────────────────────────────

    #[test]
    fn build_rerank_prompt_contains_required_sections() {
        let prompt = build_rerank_prompt("authentication flow", "fn verify_token() {}");

        assert!(prompt.contains("<|im_start|>system\n"));
        assert!(prompt.contains("Rate the relevance"));
        assert!(prompt.contains("0-10"));
        assert!(prompt.contains("<|im_start|>user\n"));
        assert!(prompt.contains("Query: authentication flow"));
        assert!(prompt.contains("Code: fn verify_token() {}"));
        assert!(prompt.contains("<|im_start|>assistant\n"));
    }

    #[test]
    fn build_rerank_prompt_truncates_long_documents() {
        // Document significantly longer than MAX_DOC_CHARS (700)
        let long_doc = "x".repeat(2000);
        let prompt = build_rerank_prompt("query", &long_doc);

        // Prompt must not embed the full 2000-char document
        assert!(
            prompt.len() < 2000 + 200,
            "Prompt should be truncated; got length {}",
            prompt.len()
        );
        // Truncation marker must be present
        assert!(prompt.contains("..."));
    }

    #[test]
    fn build_rerank_prompt_short_document_not_truncated() {
        let short_doc = "fn add(a: i32, b: i32) -> i32 { a + b }";
        let prompt = build_rerank_prompt("addition", short_doc);

        // No truncation marker for short documents
        assert!(!prompt.contains("..."));
        assert!(prompt.contains(short_doc));
    }

    // ── score parser ─────────────────────────────────────────────────────────

    #[test]
    fn parse_score_single_digits() {
        for d in 0u32..=9 {
            let raw = d.to_string();
            let score = parse_score(&raw);
            let expected = d as f32 / 10.0;
            assert!(
                (score - expected).abs() < 1e-6,
                "parse_score({raw:?}) = {score}, expected {expected}"
            );
        }
    }

    #[test]
    fn parse_score_ten() {
        assert!((parse_score("10") - 1.0).abs() < 1e-6);
        assert!((parse_score("10\n") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parse_score_with_surrounding_whitespace() {
        assert!((parse_score("  7  ") - 0.7).abs() < 1e-6);
        assert!((parse_score("\n8\n") - 0.8).abs() < 1e-6);
    }

    #[test]
    fn parse_score_non_digit_returns_fallback() {
        assert!((parse_score("") - FALLBACK_SCORE).abs() < 1e-6);
        assert!((parse_score("  ") - FALLBACK_SCORE).abs() < 1e-6);
        assert!((parse_score("N/A") - FALLBACK_SCORE).abs() < 1e-6);
        assert!((parse_score("unknown") - FALLBACK_SCORE).abs() < 1e-6);
    }

    #[test]
    fn parse_score_digit_followed_by_text() {
        // "7 out of 10" — should take the first digit
        assert!((parse_score("7 out of 10") - 0.7).abs() < 1e-6);
    }

    // ── mock reranker (no model) ─────────────────────────────────────────────

    /// A stub reranker that returns pre-computed scores for testing the
    /// pipeline integration without loading a model.
    struct StubReranker {
        scores: Vec<f32>,
        top_k: usize,
    }

    #[async_trait]
    impl Reranker for StubReranker {
        async fn rerank(
            &self,
            _query: &str,
            documents: &[RerankDocument],
        ) -> Result<Vec<f32>> {
            let mut out = Vec::with_capacity(documents.len());
            for i in 0..documents.len() {
                out.push(self.scores.get(i).copied().unwrap_or(FALLBACK_SCORE));
            }
            Ok(out)
        }

        fn top_k(&self) -> usize {
            self.top_k
        }
    }

    #[tokio::test]
    async fn stub_reranker_returns_scores_in_order() {
        let reranker = StubReranker {
            scores: vec![0.9, 0.3, 0.7],
            top_k: 10,
        };

        let docs = vec![
            RerankDocument {
                id: "a".to_string(),
                text: "fn foo() {}".to_string(),
                name: "foo".to_string(),
            },
            RerankDocument {
                id: "b".to_string(),
                text: "fn bar() {}".to_string(),
                name: "bar".to_string(),
            },
            RerankDocument {
                id: "c".to_string(),
                text: "fn baz() {}".to_string(),
                name: "baz".to_string(),
            },
        ];

        let scores = reranker.rerank("foo function", &docs).await.unwrap();

        assert_eq!(scores.len(), 3);
        assert!((scores[0] - 0.9).abs() < 1e-6);
        assert!((scores[1] - 0.3).abs() < 1e-6);
        assert!((scores[2] - 0.7).abs() < 1e-6);
    }

    #[tokio::test]
    async fn stub_reranker_pads_missing_scores_with_fallback() {
        let reranker = StubReranker {
            scores: vec![0.8], // Only one score for two documents
            top_k: 10,
        };

        let docs = vec![
            RerankDocument {
                id: "a".to_string(),
                text: "text".to_string(),
                name: "a".to_string(),
            },
            RerankDocument {
                id: "b".to_string(),
                text: "text".to_string(),
                name: "b".to_string(),
            },
        ];

        let scores = reranker.rerank("q", &docs).await.unwrap();

        assert_eq!(scores.len(), 2);
        assert!((scores[0] - 0.8).abs() < 1e-6);
        assert!((scores[1] - FALLBACK_SCORE).abs() < 1e-6);
    }

    #[tokio::test]
    async fn stub_reranker_empty_documents() {
        let reranker = StubReranker {
            scores: vec![],
            top_k: 5,
        };

        let scores = reranker.rerank("query", &[]).await.unwrap();
        assert!(scores.is_empty());
    }
}

//! llama.cpp cross-encoder reranker backend (bge-reranker-v2-m3 via GGUF)
//!
//! Uses a BERT-based cross-encoder model to score (query, document) relevance.
//! The model processes `[CLS] query [SEP] document [SEP]` through all attention
//! layers jointly, producing a single classification logit via rank pooling.
//! Sigmoid maps the raw logit to a [0, 1] relevance score.
//!
//! This replaces the previous generative approach (prompting Qwen2.5-Coder-1.5B
//! to output a digit 0–10). The BERT cross-encoder is both faster (single forward
//! pass vs. multi-token generation) and more accurate (trained specifically on
//! relevance judgments vs. zero-shot generation).
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
    context::params::{LlamaContextParams, LlamaPoolingType},
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel},
    model::params::LlamaModelParams,
};
use std::num::NonZeroU32;
use std::sync::Arc;

use crate::path::Utf8Path;
use super::{RerankDocument, Reranker};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// HuggingFace repo for the cross-encoder reranker model.
pub const HF_REPO: &str = "gpustack/bge-reranker-v2-m3-GGUF";
/// GGUF model filename. Q8_0 for best classification quality (636 MB).
pub const HF_MODEL_FILE: &str = "bge-reranker-v2-m3-Q8_0.gguf";

/// Maximum tokens for the combined `[CLS] query [SEP] doc [SEP]` input.
/// bge-reranker-v2-m3 supports 8192-token context; 1024 is conservative
/// and sufficient for code symbol search (queries ~50 tokens, docs ~300).
const RERANK_N_CTX: u32 = 1024;

/// Score returned when the model produces an error (neutral).
const FALLBACK_SCORE: f32 = 0.5;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Apply sigmoid to map a raw logit to [0, 1].
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

// ---------------------------------------------------------------------------
// LlamaCppReranker
// ---------------------------------------------------------------------------

/// Cross-encoder reranker backed by bge-reranker-v2-m3 (BERT) via llama.cpp.
///
/// Loads the GGUF model once and creates a fresh `LlamaContext` per scoring
/// call (context is `!Send`). Uses `LlamaPoolingType::Rank` which applies
/// the model's classification head to the [CLS] token representation.
pub struct LlamaCppReranker {
    backend: &'static llama_cpp_2::llama_backend::LlamaBackend,
    model: Arc<LlamaModel>,
    top_k: usize,
}

impl LlamaCppReranker {
    /// Load a GGUF cross-encoder model for reranking with Metal GPU offload.
    pub fn new(model_path: &Utf8Path, top_k: usize) -> Result<Self> {
        tracing::info!(
            model_path = %model_path,
            top_k,
            "Loading cross-encoder reranker model"
        );

        let backend = crate::llm::get_or_init_backend()?;

        // Offload all transformer layers to Metal GPU.
        let model_params = LlamaModelParams::default().with_n_gpu_layers(99);

        let model =
            LlamaModel::load_from_file(backend, model_path.as_std_path(), &model_params)
                .map_err(|e| anyhow!("Failed to load reranker GGUF model: {:?}", e))?;

        tracing::info!(
            vocab = model.n_vocab(),
            params = model.n_params(),
            ctx_train = model.n_ctx_train(),
            "Cross-encoder reranker model loaded (bge-reranker-v2-m3)"
        );

        Ok(Self {
            backend,
            model: Arc::new(model),
            top_k,
        })
    }

    /// Score a single (query, document) pair using cross-encoder classification.
    ///
    /// Tokenizes as `[CLS] query [SEP] document [SEP]`, runs a single forward
    /// pass with rank pooling, and returns `sigmoid(logit)`.
    fn score_pair(
        backend: &'static llama_cpp_2::llama_backend::LlamaBackend,
        model: &LlamaModel,
        query: &str,
        document: &str,
    ) -> Result<f32> {
        // Tokenize query with [CLS] (BOS for BERT models = [CLS])
        let query_tokens = model
            .str_to_token(query, AddBos::Always)
            .map_err(|e| anyhow!("Query tokenization failed: {:?}", e))?;

        // [SEP] token for segment boundary
        let sep_token = model.token_sep();

        // Tokenize document without leading [CLS]
        let doc_tokens = model
            .str_to_token(document, AddBos::Never)
            .map_err(|e| anyhow!("Document tokenization failed: {:?}", e))?;

        // Build input: [CLS] query_tokens [SEP] doc_tokens [SEP]
        let mut tokens = query_tokens;
        tokens.push(sep_token);

        // Truncate document to fit within context window
        let remaining = (RERANK_N_CTX as usize).saturating_sub(tokens.len() + 1); // +1 for trailing [SEP]
        if doc_tokens.len() > remaining {
            tokens.extend_from_slice(&doc_tokens[..remaining]);
        } else {
            tokens.extend_from_slice(&doc_tokens);
        }
        tokens.push(sep_token);

        if tokens.is_empty() {
            return Ok(FALLBACK_SCORE);
        }

        // Create context with rank pooling (classification head on [CLS])
        let n_ctx = (tokens.len() as u32).max(64);
        // BERT encoder models process the entire sequence in one forward pass,
        // so n_ubatch must be >= total tokens (unlike decoder models).
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(n_ctx)
            .with_n_ubatch(n_ctx)
            .with_embeddings(true)
            .with_pooling_type(LlamaPoolingType::Rank);

        let mut ctx = model
            .new_context(backend, ctx_params)
            .map_err(|e| anyhow!("Failed to create reranker context: {:?}", e))?;

        // Fill batch — all tokens in sequence 0
        let mut batch = LlamaBatch::new(tokens.len(), 1);
        for (i, token) in tokens.iter().enumerate() {
            let is_last = i == tokens.len() - 1;
            batch
                .add(*token, i as i32, &[0], is_last)
                .map_err(|e| anyhow!("Failed to add token to reranker batch: {:?}", e))?;
        }

        // Single forward pass (no autoregressive generation)
        ctx.decode(&mut batch)
            .map_err(|e| anyhow!("Reranker decode failed: {:?}", e))?;

        // Extract the classification score from rank pooling.
        // For LLAMA_POOLING_TYPE_RANK, the classifier head outputs a single
        // scalar stored at index 0 of the embeddings buffer.
        let scores = ctx
            .embeddings_seq_ith(0)
            .map_err(|e| anyhow!("Failed to extract reranker score: {:?}", e))?;

        let raw_logit = scores[0];
        Ok(sigmoid(raw_logit))
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
                        tracing::debug!(doc_name = %doc.name, score = s, "Cross-encoder scored document");
                        s
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, doc_name = %doc.name, "Cross-encoder scoring failed, using fallback");
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

/// Download the bge-reranker-v2-m3 GGUF model for cross-encoder reranking.
///
/// Fetches from HuggingFace into the system cache and symlinks into `target_dir`.
pub fn download_reranker_model(target_dir: &Utf8Path) -> Result<()> {
    use anyhow::Context;

    tracing::info!("Downloading reranker model from huggingface.co/{}", HF_REPO);

    let api = hf_hub::api::sync::Api::new()
        .context("Failed to initialize HuggingFace Hub API")?;
    let repo = api.model(HF_REPO.to_string());

    tracing::info!(
        "Downloading {} (~636 MB)...",
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

    // ── sigmoid ─────────────────────────────────────────────────────────────

    #[test]
    fn sigmoid_maps_zero_to_half() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn sigmoid_maps_large_positive_to_near_one() {
        assert!(sigmoid(10.0) > 0.999);
    }

    #[test]
    fn sigmoid_maps_large_negative_to_near_zero() {
        assert!(sigmoid(-10.0) < 0.001);
    }

    #[test]
    fn sigmoid_is_monotonic() {
        let values = [-5.0, -2.0, 0.0, 2.0, 5.0];
        for window in values.windows(2) {
            assert!(sigmoid(window[0]) < sigmoid(window[1]));
        }
    }

    // ── mock reranker (no model) ─────────────────────────────────────────────

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
            scores: vec![0.8],
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

    // ── integration test (requires model download) ──────────────────────────

    /// Verify that the cross-encoder produces sensible scores for obviously
    /// relevant vs. irrelevant documents.
    ///
    /// Requires the real model (~636 MB). Run with:
    ///   cargo test --lib reranker::llamacpp::tests::cross_encoder_relevance -- --ignored
    #[tokio::test]
    #[ignore]
    async fn cross_encoder_relevance() {
        let home = std::env::var("HOME").expect("HOME not set");
        let model_path = crate::path::Utf8PathBuf::from(format!(
            "{}/.code-intelligence/models/bge-reranker-v2-m3-gguf/{}",
            home, HF_MODEL_FILE
        ));
        if !model_path.exists() {
            eprintln!("Skipping: reranker model not found at {}", model_path);
            return;
        }

        let reranker = LlamaCppReranker::new(&model_path, 10).expect("failed to load reranker");

        let docs = vec![
            RerankDocument {
                id: "relevant".to_string(),
                text: "fn authenticate(user: &str, password: &str) -> Result<Token> { verify_credentials(user, password)?; generate_jwt(user) }".to_string(),
                name: "authenticate".to_string(),
            },
            RerankDocument {
                id: "irrelevant".to_string(),
                text: "fn calculate_tax(amount: f64, rate: f64) -> f64 { amount * rate }".to_string(),
                name: "calculate_tax".to_string(),
            },
        ];

        let scores = reranker
            .rerank("authentication and login", &docs)
            .await
            .unwrap();

        assert_eq!(scores.len(), 2);
        // The auth function should score higher than the tax function
        assert!(
            scores[0] > scores[1],
            "Expected relevant ({}) > irrelevant ({})",
            scores[0],
            scores[1]
        );
    }
}

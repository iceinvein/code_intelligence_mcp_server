//! ONNX-based cross-encoder reranker using ort runtime

use super::{Reranker, RerankDocument};
use anyhow::{Context, Result};
use ndarray::Array2;
use ort::{inputs, Session};
use std::path::{Path, PathBuf};
use tokenizers::Tokenizer;
use tokio::sync::Mutex;

/// Cross-encoder reranker using ONNX Runtime
pub struct CrossEncoderReranker {
    session: Session,
    tokenizer: Mutex<Tokenizer>,
    max_length: usize,
    top_k: usize,
    model_path: PathBuf,
}

impl CrossEncoderReranker {
    /// Create a new cross-encoder reranker from an ONNX model file
    pub fn new(
        model_path: &Path,
        _cache_dir: Option<&Path>,
        top_k: usize,
    ) -> Result<Self> {
        tracing::info!("Loading cross-encoder reranker from: {}", model_path.display());

        let session = Session::builder()?
            .with_execution_providers([
                ort::execution_providers::CPUExecutionProvider::default().build()
            ])?
            .commit_from_file(model_path)
            .context("Failed to load cross-encoder ONNX model")?;

        // Try to load tokenizer from same directory
        let tokenizer_path = model_path
            .parent()
            .map(|p| p.join("tokenizer.json"))
            .filter(|p| p.exists());

        let tokenizer = match tokenizer_path {
            Some(path) => {
                Tokenizer::from_file(&path)
                    .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?
            }
            None => {
                // Fallback: use default BERT tokenizer
                tracing::warn!("No tokenizer.json found, using default BERT tokenizer");
                Tokenizer::from_pretrained("bert-base-uncased", None)
                    .map_err(|e| anyhow::anyhow!("Failed to load default tokenizer: {}", e))?
            }
        };

        Ok(Self {
            session,
            tokenizer: Mutex::new(tokenizer),
            max_length: 512,
            top_k: top_k.min(50), // Cap at 50 for performance
            model_path: model_path.to_path_buf(),
        })
    }
}

#[async_trait::async_trait]
impl Reranker for CrossEncoderReranker {
    async fn rerank(&self, query: &str, documents: &[RerankDocument]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }

        // Limit to top_k documents
        let docs_to_rerank = documents.iter().take(self.top_k);
        let mut scores = Vec::new();

        for doc in docs_to_rerank {
            // Build document text: name + first part of text
            let doc_text = if doc.text.len() > 400 {
                format!("{}: {}", doc.name, &doc.text[..400])
            } else {
                format!("{}: {}", doc.name, doc.text)
            };

            let score = self.score_pair(query, &doc_text).await?;
            scores.push(score);
        }

        // Fill remaining with 0.0 if any documents were skipped
        while scores.len() < documents.len().min(self.top_k) {
            scores.push(0.0);
        }

        // Fill rest with 0.0 for documents beyond top_k
        while scores.len() < documents.len() {
            scores.push(0.0);
        }

        Ok(scores)
    }

    fn top_k(&self) -> usize {
        self.top_k
    }
}

impl CrossEncoderReranker {
    /// Score a single query-document pair
    async fn score_pair(&self, query: &str, document: &str) -> Result<f32> {
        let tokenizer = self.tokenizer.lock().await;

        // Encode query-document pair (cross-encoder input format)
        let encoding = tokenizer
            .encode_pair((query, document), true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        let ids: Vec<i64> = encoding
            .get_ids()
            .iter()
            .take(self.max_length)
            .map(|&x| x as i64)
            .collect();

        let attention_mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .take(self.max_length)
            .map(|&x| x as i64)
            .collect();

        let seq_len = ids.len();
        if seq_len == 0 {
            return Ok(0.0);
        }

        let ids_array = Array2::from_shape_vec((1, seq_len), ids)?;
        let mask_array = Array2::from_shape_vec((1, seq_len), attention_mask)?;

        // Run inference
        let outputs = self.session.run(vec![
            inputs!["input_ids" => ids_array]?,
            inputs!["attention_mask" => mask_array]?,
        ])?;

        // Extract logits (cross-encoders output relevance score)
        let logits = outputs[0].try_extract_tensor::<f32>()?;

        // Get score from first output (usually logit for "relevant" class)
        let score = logits.first().copied().unwrap_or(0.0);

        // Apply sigmoid if output is logits (not probability)
        // Using 1 / (1 + exp(-score)) for sigmoid
        let sigmoid_score = 1.0 / (1.0 + (-score).exp());

        Ok(sigmoid_score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigmoid_conversion() {
        // Test sigmoid calculation
        let score = 2.0f32;
        let sigmoid = 1.0 / (1.0 + (-score).exp());
        assert!(sigmoid > 0.8 && sigmoid < 1.0);

        let score_neg = -2.0f32;
        let sigmoid_neg = 1.0 / (1.0 + (-score_neg).exp());
        assert!(sigmoid_neg > 0.0 && sigmoid_neg < 0.2);

        let score_zero = 0.0f32;
        let sigmoid_zero = 1.0 / (1.0 + (-score_zero).exp());
        assert!((sigmoid_zero - 0.5).abs() < 0.01);
    }
}

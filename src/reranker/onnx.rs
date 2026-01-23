//! ONNX-based cross-encoder reranker using ort runtime

use super::{Reranker, RerankDocument};
use anyhow::{Context, Result};
use ndarray::Array;
use ort::session::Session;
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
                // Fallback: use a basic BERT tokenizer if available
                tracing::warn!("No tokenizer.json found, reranking may not work correctly");
                // Return a minimal tokenizer that will fail gracefully
                Tokenizer::new(tokenizers::ModelWrapper::BPE(
                    tokenizers::models::bpe::BPE::default()
                ))
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
        // Tokenizer::encode accepts tuples for pairs
        let encoding_result = tokenizer.encode((query, document), true);

        let encoding = match encoding_result {
            Ok(enc) => enc,
            Err(_) => {
                // Tokenization failed - return neutral score
                tracing::warn!("Tokenization failed for pair, returning neutral score");
                return Ok(0.5);
            }
        };

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

        let token_type_ids: Vec<i64> = encoding
            .get_type_ids()
            .iter()
            .take(self.max_length)
            .map(|&x| x as i64)
            .collect();

        drop(tokenizer);

        let seq_len = ids.len();
        if seq_len == 0 {
            return Ok(0.0);
        }

        // Create input arrays for ort 2.0
        let ids_array = Array::from_shape_vec((1, seq_len), ids)?;
        let mask_array = Array::from_shape_vec((1, seq_len), attention_mask)?;
        let type_ids_array = Array::from_shape_vec((1, seq_len), token_type_ids)?;

        // Run inference with ort 2.0 API
        // Create input values using the session's allocator
        let input_ids_value = ort::value::Value::from_array(ids_array)?;
        let attention_mask_value = ort::value::Value::from_array(mask_array)?;
        let token_type_ids_value = ort::value::Value::from_array(type_ids_array)?;

        // Convert to SessionInputValue
        let inputs: Vec<ort::session::SessionInputValue> = vec![
            input_ids_value.into(),
            attention_mask_value.into(),
            token_type_ids_value.into(),
        ];

        match self.session.run(inputs.as_slice()) {
            Ok(outputs) => {
                // Extract logits - cross-encoders output relevance score
                // SessionOutputs implements Index<usize> to get outputs by position
                if outputs.len() > 0 {
                    let output = &outputs[0]; // Get first output
                    if let Ok(tensor) = output.try_extract_tensor::<f32>() {
                        let score = tensor.first().copied().unwrap_or(0.0);

                        // Apply sigmoid if output is logits (not probability)
                        // Using 1 / (1 + exp(-score)) for sigmoid
                        let sigmoid_score = 1.0 / (1.0 + (-score).exp());
                        return Ok(sigmoid_score);
                    }
                }
                Ok(0.0)
            }
            Err(e) => {
                tracing::warn!("Reranker inference failed: {}, returning neutral score", e);
                Ok(0.5)
            }
        }
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

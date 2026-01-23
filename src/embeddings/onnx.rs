//! ONNX-based Jina Code embeddings using the ort crate
//!
//! This module provides an embedder implementation for the Jina Code embeddings model
//! (jina-embeddings-v2-base-code), which produces 768-dimensional vectors specifically
//! trained for code understanding across 30+ programming languages.

use crate::embeddings::Embedder;
use anyhow::{anyhow, Context, Result};
use ndarray::{Array, Array1, Array2, Axis};
use ort::{Environment, ExecutionProvider, Session, Value};
use std::path::Path;
use tokenizers::{Tokenizer, TruncationStrategy};

const JINA_CODE_DIM: usize = 768;
const MAX_SEQ_LENGTH: usize = 512;

/// Jina Code ONNX embedder using ort for inference.
///
/// This embedder loads the Jina Code v2 Base Code model and performs
/// mean pooling with L2 normalization to produce 768-dimensional embeddings.
pub struct JinaCodeEmbedder {
    _session: Session,
    tokenizer: Tokenizer,
    _env: Environment,
}

impl JinaCodeEmbedder {
    /// Create a new JinaCode embedder from a model directory.
    ///
    /// The directory should contain:
    /// - `model.onnx` - The ONNX model file (can be model_fp16.onnx or modelquant.onnx)
    /// - `tokenizer.json` - HuggingFace tokenizer configuration
    /// - `config.json` - Model configuration (optional, for metadata)
    ///
    /// # Arguments
    /// * `model_dir` - Path to directory containing model files
    ///
    /// # Errors
    /// Returns error if model files are missing or fail to load
    pub fn new(model_dir: &Path) -> Result<Self> {
        let model_dir = model_dir
            .canonicalize()
            .with_context(|| format!("Invalid model directory: {}", model_dir.display()))?;

        // Find the ONNX model file (try common names)
        let model_path = find_model_file(&model_dir)?;

        // Load tokenizer
        let tokenizer_path = model_dir.join("tokenizer.json");
        if !tokenizer_path.exists() {
            return Err(anyhow!(
                "Tokenizer not found at {}. Expected tokenizer.json",
                tokenizer_path.display()
            ));
        }

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path).with_context(|| {
            format!(
                "Failed to load tokenizer from {}",
                tokenizer_path.display()
            )
        })?;

        // Configure tokenizer with truncation
        tokenizer.with_truncation(Some(TruncationStrategy::LongestFirst, MAX_SEQ_LENGTH, 0));

        // Create ONNX Runtime environment
        let env = Environment::builder()
            .with_name("jina-code-embeddings")
            .with_execution_providers([ExecutionProvider::cpu()])
            .build()
            .context("Failed to create ONNX environment")?;

        // Load ONNX session
        let session = env
            .new_session(&model_path)
            .with_context(|| format!("Failed to load ONNX model from {}", model_path.display()))?;

        // Validate model inputs/outputs
        let inputs = session.inputs;
        let outputs = session.outputs;

        if !inputs.iter().any(|i| i.name == "input_ids") {
            return Err(anyhow!(
                "Model missing expected input 'input_ids'. Found inputs: {:?}",
                inputs.iter().map(|i| &i.name).collect::<Vec<_>>()
            ));
        }

        if !outputs.iter().any(|o| o.name == "last_hidden_state") {
            return Err(anyhow!(
                "Model missing expected output 'last_hidden_state'. Found outputs: {:?}",
                outputs.iter().map(|o| &o.name).collect::<Vec<_>>()
            ));
        }

        Ok(Self {
            _session: session,
            tokenizer,
            _env: env,
        })
    }

    /// Encode a single text into embeddings.
    fn encode_single(&self, text: &str) -> Result<Vec<f32>> {
        // Tokenize input
        let encoding = self
            .tokenizer
            .encode(text, true)
            .context("Failed to tokenize input")?;

        let input_ids: Vec<i64> = encoding.get_ids().to_vec();
        let attention_mask: Vec<i64> = encoding.get_attention_mask().to_vec();
        let token_type_ids: Vec<i64> = encoding.get_type_ids().to_vec();

        // Get sequence length
        let seq_len = input_ids.len();

        if seq_len == 0 {
            return Ok(vec![0.0f32; JINA_CODE_DIM]);
        }

        // Reshape inputs for ONNX: [batch_size, seq_len]
        let batch_size = 1;
        let input_ids_array =
            Array2::from_shape_vec((batch_size, seq_len), input_ids).context("Failed to shape input_ids")?;
        let attention_mask_array = Array2::from_shape_vec((batch_size, seq_len), attention_mask)
            .context("Failed to shape attention_mask")?;
        let token_type_ids_array = Array2::from_shape_vec((batch_size, seq_len), token_type_ids)
            .context("Failed to shape token_type_ids")?;

        // Run inference
        let outputs = self
            ._session
            .run(vec![
                Value::from_array(self._env.allocator(), input_ids_array)
                    .context("Failed to create input_ids tensor")?,
                Value::from_array(self._env.allocator(), attention_mask_array)
                    .context("Failed to create attention_mask tensor")?,
                Value::from_array(self._env.allocator(), token_type_ids_array)
                    .context("Failed to create token_type_ids tensor")?,
            ])
            .context("ONNX inference failed")?;

        // Extract last_hidden_state: [batch_size, seq_len, hidden_dim]
        let last_hidden_state = outputs
            .first()
            .ok_or_else(|| anyhow!("Model produced no outputs"))?;

        let hidden_array = last_hidden_state
            .try_extract::<f32>()
            .context("Failed to extract output tensor")?;

        // Apply mean pooling with attention mask
        let pooled = mean_pooling(&hidden_array, &attention_mask_array)?;

        // L2 normalize
        let normalized = l2_normalize(pooled);

        Ok(normalized.to_vec())
    }
}

impl Embedder for JinaCodeEmbedder {
    fn dim(&self) -> usize {
        JINA_CODE_DIM
    }

    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            let embedding = self.encode_single(text)?;
            results.push(embedding);
        }
        Ok(results)
    }
}

/// Find the ONNX model file in the given directory.
///
/// Tries common model file names in order of preference.
fn find_model_file(dir: &Path) -> Result<std::path::PathBuf> {
    let candidates = [
        "model_fp16.onnx",   // FP16 quantized (smaller, faster)
        "model_quant.onnx",  // INT8 quantized (even smaller)
        "model.onnx",        // Standard FP32 model
    ];

    for name in &candidates {
        let path = dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }

    Err(anyhow!(
        "No ONNX model file found in {}. Looked for: {:?}",
        dir.display(),
        candidates
    ))
}

/// Apply mean pooling to hidden states using attention mask.
///
/// This takes the weighted average of token embeddings based on the attention mask,
/// which is the standard pooling strategy for sentence embeddings.
///
/// # Arguments
/// * `hidden` - Hidden states [batch_size, seq_len, hidden_dim]
/// * `attention_mask` - Attention mask [batch_size, seq_len]
///
/// # Returns
/// Pooled vector [hidden_dim]
fn mean_pooling(hidden: &Array<f32, ndarray::IxDyn>, attention_mask: &Array2<i64>) -> Result<Array1<f32>> {
    let shape = hidden.shape();
    let (_batch_size, seq_len, hidden_dim) = (shape[0], shape[1], shape[2]);

    if seq_len == 0 {
        return Ok(Array::zeros(hidden_dim));
    }

    // hidden: [batch, seq_len, hidden] -> view as [seq_len, hidden]
    let hidden_view = hidden.index_axis_move(Axis(0), 0); // [seq_len, hidden_dim]

    // attention_mask: [batch, seq_len] -> [seq_len]
    let mask_view = attention_mask.index_axis_move(Axis(0), 0); // [seq_len]

    // Convert mask to f32 for computation
    let mask_f32: Vec<f32> = mask_view.iter().map(|&x| x as f32).collect();

    // Compute masked sum: sum(hidden[i] * mask[i])
    let mut sum = Array1::zeros(hidden_dim);
    let mut mask_sum = 0.0f32;

    for (i, &mask_val) in mask_f32.iter().enumerate() {
        if mask_val > 0.0 {
            let token_vec = hidden_view.index_axis(Axis(0), i);
            sum += &(token_vec * mask_val);
            mask_sum += mask_val;
        }
    }

    // Avoid division by zero
    if mask_sum > 0.0 {
        sum /= mask_sum;
    }

    Ok(sum)
}

/// L2 normalize a vector in-place.
///
/// This ensures all embeddings have unit length, which is important
/// for cosine similarity calculations.
fn l2_normalize(mut vec: Array1<f32>) -> Array1<f32> {
    let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        vec /= norm;
    }
    vec
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // Note: These tests require actual model files. In a real scenario,
    // you'd either download them or use a mock for testing.

    #[test]
    fn test_find_model_file_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let result = find_model_file(temp_dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_find_model_file_fp16() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join("model_fp16.onnx"), b"fake").unwrap();
        let result = find_model_file(temp_dir.path());
        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("model_fp16.onnx"));
    }

    #[test]
    fn test_find_model_file_fallback() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join("model.onnx"), b"fake").unwrap();
        let result = find_model_file(temp_dir.path());
        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("model.onnx"));
    }

    #[test]
    fn test_jina_code_dim_constant() {
        assert_eq!(JINA_CODE_DIM, 768);
    }

    #[test]
    fn test_mean_pooling_empty() {
        let hidden = Array::zeros((1, 0, 768).f());
        let attention_mask = Array2::zeros((1, 0));
        let result = mean_pooling(&hidden, &attention_mask).unwrap();
        assert_eq!(result.len(), 768);
        // All zeros for empty sequence
        assert!(result.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn test_l2_normalize() {
        let vec = Array1::from_vec(vec![3.0, 4.0]);
        let normalized = l2_normalize(vec);
        let norm = (normalized[0].powi(2) + normalized[1].powi(2)).sqrt();
        assert!((norm - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_l2_normalize_zero() {
        let vec = Array1::from_vec(vec![0.0, 0.0]);
        let normalized = l2_normalize(vec);
        assert_eq!(normalized[0], 0.0);
        assert_eq!(normalized[1], 0.0);
    }
}

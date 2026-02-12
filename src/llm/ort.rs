//! ONNX Runtime LLM generation backend (Qwen2.5-Coder-1.5B-Instruct)
//!
//! Implements autoregressive text generation with KV cache for GQA.
//! Supports CPU and Metal (CoreML) execution providers.

use anyhow::{anyhow, Context, Result};
use ndarray::{Array2, Array4, IxDyn};
use ort::session::Session;
use tokenizers::Tokenizer;

use crate::config::EmbeddingsDevice;
use crate::path::{Utf8Path, Utf8PathBuf};
use super::LlmGenerator;

/// Model architecture config for Qwen2.5-Coder-1.5B
struct ModelConfig {
    num_layers: usize,        // 28
    num_kv_heads: usize,      // 2 (GQA: 6:1 ratio with 12 attention heads)
    head_dim: usize,          // 128
    vocab_size: usize,        // 151936
    eos_token_ids: Vec<u32>,  // [151645, 151643]
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            num_layers: 28,
            num_kv_heads: 2,
            head_dim: 128,
            vocab_size: 151936,
            eos_token_ids: vec![151645, 151643],
        }
    }
}

/// ONNX Runtime-based LLM generator for Qwen2.5-Coder-1.5B-Instruct.
///
/// Uses autoregressive generation with KV cache for efficient token-by-token output.
/// The model uses Grouped Query Attention (GQA) with 2 KV heads (vs 12 attention heads).
pub struct OrtLlmGenerator {
    session: Session,
    tokenizer: Tokenizer,
    config: ModelConfig,
}

impl OrtLlmGenerator {
    /// Load the ONNX model and tokenizer from a directory.
    ///
    /// Expected files in `model_dir`:
    /// - `model.onnx` (or quantized variants: `model_q4.onnx`, `model_q8.onnx`, etc.)
    /// - `tokenizer.json`
    pub fn new(model_dir: &Utf8Path, device: EmbeddingsDevice) -> Result<Self> {
        tracing::info!("Loading LLM from: {}", model_dir);

        let model_path = find_model_file(model_dir)?;
        tracing::info!("Using model file: {}", model_path);

        // Build ONNX session with appropriate execution provider
        let builder = Session::builder()?;

        let session = match device {
            EmbeddingsDevice::Metal => {
                #[cfg(target_os = "macos")]
                {
                    tracing::info!("Using CoreML (Metal) execution provider for LLM");
                    let coreml = ort::execution_providers::CoreMLExecutionProvider::default();
                    builder
                        .with_execution_providers([coreml.into()])?
                        .commit_from_file(model_path.as_std_path())
                        .context("Failed to load LLM ONNX model with CoreML")?
                }
                #[cfg(not(target_os = "macos"))]
                {
                    tracing::warn!("Metal requested but not on macOS, falling back to CPU");
                    builder
                        .with_execution_providers([
                            ort::execution_providers::CPUExecutionProvider::default().build()
                        ])?
                        .commit_from_file(model_path.as_std_path())
                        .context("Failed to load LLM ONNX model with CPU")?
                }
            }
            EmbeddingsDevice::Cpu => {
                builder
                    .with_execution_providers([
                        ort::execution_providers::CPUExecutionProvider::default().build()
                    ])?
                    .commit_from_file(model_path.as_std_path())
                    .context("Failed to load LLM ONNX model with CPU")?
            }
        };

        // Load tokenizer
        let tokenizer_path = model_dir.join("tokenizer.json");
        let tokenizer = Tokenizer::from_file(tokenizer_path.as_str())
            .map_err(|e| anyhow!("Failed to load LLM tokenizer: {}", e))?;

        tracing::info!("LLM loaded successfully ({} layers, GQA with {} KV heads)",
            ModelConfig::default().num_layers,
            ModelConfig::default().num_kv_heads,
        );

        Ok(Self {
            session,
            tokenizer,
            config: ModelConfig::default(),
        })
    }
}

impl LlmGenerator for OrtLlmGenerator {
    fn generate(&self, prompt: &str, max_tokens: u32) -> Result<String> {
        // 1. Tokenize
        let encoding = self.tokenizer.encode(prompt, false)
            .map_err(|e| anyhow!("Tokenization failed: {}", e))?;
        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let seq_len = input_ids.len();

        if seq_len == 0 {
            return Ok(String::new());
        }

        // 2. First forward pass (prompt processing)
        let mut generated_tokens: Vec<u32> = Vec::new();
        let (mut logits, mut kv_cache) = self.forward_prompt(&input_ids)?;

        // 3. Autoregressive loop
        let mut total_len = seq_len;
        for _ in 0..max_tokens {
            let next_token = argmax_last(&logits, self.config.vocab_size)?;

            if self.config.eos_token_ids.contains(&next_token) {
                break;
            }

            generated_tokens.push(next_token);
            total_len += 1;

            let (new_logits, new_kv_cache) = self.forward_token(
                next_token as i64,
                total_len,
                kv_cache,
            )?;
            logits = new_logits;
            kv_cache = new_kv_cache;
        }

        // 4. Detokenize
        let output = self.tokenizer.decode(&generated_tokens, true)
            .map_err(|e| anyhow!("Detokenization failed: {}", e))?;

        Ok(output.trim().to_string())
    }
}

impl OrtLlmGenerator {
    /// First forward pass: process entire prompt, return logits and initial KV cache.
    fn forward_prompt(&self, input_ids: &[i64]) -> Result<(Vec<f32>, Vec<ort::value::Value>)> {
        let seq_len = input_ids.len();

        let ids_array = Array2::from_shape_vec((1, seq_len), input_ids.to_vec())?;
        let attention_mask = Array2::from_elem((1, seq_len), 1i64);
        let position_ids = Array2::from_shape_fn((1, seq_len), |(_, j)| j as i64);

        let empty_kv = self.empty_kv_cache()?;

        let mut inputs: Vec<ort::session::SessionInputValue> = vec![
            ort::value::Value::from_array(ids_array)?.into(),
            ort::value::Value::from_array(attention_mask)?.into(),
            ort::value::Value::from_array(position_ids)?.into(),
        ];
        inputs.extend(empty_kv.into_iter().map(|v| v.into()));

        let outputs = self.session.run(inputs.as_slice())
            .context("LLM forward pass (prompt) failed")?;

        let logits = extract_logits(&outputs[0])?;
        let kv_cache = extract_kv_cache(&outputs, self.config.num_layers)?;

        Ok((logits, kv_cache))
    }

    /// Subsequent forward pass: single token with KV cache.
    fn forward_token(
        &self,
        token_id: i64,
        total_len: usize,
        kv_cache: Vec<ort::value::Value>,
    ) -> Result<(Vec<f32>, Vec<ort::value::Value>)> {
        let ids_array = Array2::from_elem((1, 1), token_id);
        let attention_mask = Array2::from_elem((1, total_len), 1i64);
        let position_ids = Array2::from_elem((1, 1), (total_len - 1) as i64);

        let mut inputs: Vec<ort::session::SessionInputValue> = vec![
            ort::value::Value::from_array(ids_array)?.into(),
            ort::value::Value::from_array(attention_mask)?.into(),
            ort::value::Value::from_array(position_ids)?.into(),
        ];
        inputs.extend(kv_cache.into_iter().map(|v| v.into()));

        let outputs = self.session.run(inputs.as_slice())
            .context("LLM forward pass (token) failed")?;

        let logits = extract_logits(&outputs[0])?;
        let new_kv_cache = extract_kv_cache(&outputs, self.config.num_layers)?;

        Ok((logits, new_kv_cache))
    }

    /// Create empty KV cache tensors for the first forward pass.
    fn empty_kv_cache(&self) -> Result<Vec<ort::value::Value>> {
        let mut kv = Vec::with_capacity(self.config.num_layers * 2);
        for _ in 0..self.config.num_layers {
            let empty_k = Array4::<f32>::zeros((1, self.config.num_kv_heads, 0, self.config.head_dim));
            let empty_v = Array4::<f32>::zeros((1, self.config.num_kv_heads, 0, self.config.head_dim));
            kv.push(ort::value::Value::from_array(empty_k)?.into());
            kv.push(ort::value::Value::from_array(empty_v)?.into());
        }
        Ok(kv)
    }
}

/// Extract logits from the first output tensor.
fn extract_logits(output: &ort::value::Value) -> Result<Vec<f32>> {
    let tensor = output.try_extract_tensor::<f32>()
        .context("Failed to extract logits tensor")?;
    Ok(tensor.as_slice().unwrap_or(&[]).to_vec())
}

/// Extract KV cache tensors from model outputs (indices 1..=num_layers*2).
fn extract_kv_cache(outputs: &ort::session::SessionOutputs, num_layers: usize) -> Result<Vec<ort::value::Value>> {
    let mut kv = Vec::with_capacity(num_layers * 2);
    for i in 1..=(num_layers * 2) {
        let tensor = outputs[i].try_extract_tensor::<f32>()
            .context(format!("Failed to extract KV cache tensor {}", i))?;
        let shape: Vec<usize> = tensor.shape().to_vec();
        let data = tensor.as_slice().unwrap_or(&[]).to_vec();
        let arr = ndarray::Array::from_shape_vec(IxDyn(&shape), data)
            .context(format!("Failed to reshape KV cache tensor {}", i))?;
        kv.push(ort::value::Value::from_array(arr)?.into());
    }
    Ok(kv)
}

/// Greedy decoding: argmax of last token's logits.
fn argmax_last(logits: &[f32], vocab_size: usize) -> Result<u32> {
    if logits.is_empty() {
        return Err(anyhow!("Empty logits"));
    }

    // The last vocab_size elements correspond to the last token position
    let last_logits = if logits.len() >= vocab_size {
        &logits[logits.len() - vocab_size..]
    } else {
        logits
    };

    let (max_idx, _) = last_logits
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .ok_or_else(|| anyhow!("Failed to find argmax in logits"))?;

    Ok(max_idx as u32)
}

/// Find the best ONNX model file in the directory.
/// Prefers quantized models for speed.
fn find_model_file(model_dir: &Utf8Path) -> Result<Utf8PathBuf> {
    for name in &["model_q4.onnx", "model_q8.onnx", "model_fp16.onnx", "model.onnx"] {
        let path = model_dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(anyhow!(
        "No ONNX model file found in {}. Expected model.onnx or model_q4.onnx",
        model_dir
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argmax_last_finds_max() {
        let logits = vec![0.1, 0.5, 0.3, 0.9, 0.2];
        let result = argmax_last(&logits, 5).unwrap();
        assert_eq!(result, 3);
    }

    #[test]
    fn argmax_last_with_large_logits() {
        // Simulate [1, 2, vocab_size] flattened — take last vocab_size
        let mut logits = vec![0.0f32; 10]; // first token's logits
        let mut last = vec![0.0f32; 10];
        last[7] = 1.0; // max at index 7
        logits.extend(last);
        let result = argmax_last(&logits, 10).unwrap();
        assert_eq!(result, 7);
    }

    #[test]
    fn argmax_last_empty_fails() {
        let result = argmax_last(&[], 100);
        assert!(result.is_err());
    }

    #[test]
    fn model_config_defaults() {
        let config = ModelConfig::default();
        assert_eq!(config.num_layers, 28);
        assert_eq!(config.num_kv_heads, 2);
        assert_eq!(config.head_dim, 128);
        assert_eq!(config.vocab_size, 151936);
        assert_eq!(config.eos_token_ids, vec![151645, 151643]);
    }

    #[test]
    fn find_model_file_returns_error_for_empty_dir() {
        let dir = Utf8PathBuf::from("/nonexistent/path");
        let result = find_model_file(&dir);
        assert!(result.is_err());
    }
}

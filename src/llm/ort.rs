//! ONNX Runtime LLM generation backend (Qwen2.5-Coder-1.5B-Instruct)
//!
//! Implements autoregressive text generation with KV cache for efficient inference.
//! Uses the model_q4.onnx variant (pure int4 quantization with f32 KV cache) to
//! avoid ORT buffer reuse bugs that occur with mixed-precision models (q4f16).
//! Uses Metal (CoreML) execution provider on macOS for GPU acceleration.

use anyhow::{anyhow, Context, Result};
use ort::session::Session;
use std::sync::Mutex;
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
/// Uses KV-cached autoregressive generation: the prompt pass processes the full
/// input and populates the KV cache, then each subsequent token pass processes
/// only the new token with the accumulated cache. This is O(n) in output length.
pub struct OrtLlmGenerator {
    session: Mutex<Session>,
    tokenizer: Mutex<Tokenizer>,
    config: ModelConfig,
}

impl OrtLlmGenerator {
    /// Load the ONNX model and tokenizer from a directory.
    ///
    /// Expected files in `model_dir`:
    /// - `model_q4.onnx` (or other quantized variants)
    /// - `tokenizer.json`
    pub fn new(model_dir: &Utf8Path, device: EmbeddingsDevice) -> Result<Self> {
        tracing::info!("Loading LLM from: {}", model_dir);

        let model_path = find_model_file(model_dir)?;
        tracing::info!("Using model file: {}", model_path);

        // Disable memory pattern optimization since input sequence lengths vary between calls.
        let builder = Session::builder()?
            .with_memory_pattern(false)?;

        let session = match device {
            EmbeddingsDevice::Metal => {
                tracing::info!("Using CoreML (Metal) execution provider for LLM");
                let coreml = ort::execution_providers::CoreMLExecutionProvider::default();
                builder
                    .with_execution_providers([coreml.into()])?
                    .commit_from_file(model_path.as_std_path())
                    .context("Failed to load LLM ONNX model with CoreML")?
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
            session: Mutex::new(session),
            tokenizer: Mutex::new(tokenizer),
            config: ModelConfig::default(),
        })
    }
}

impl LlmGenerator for OrtLlmGenerator {
    fn generate(&self, prompt: &str, max_tokens: u32) -> Result<String> {
        let tokenizer = self.tokenizer.lock().unwrap();
        let encoding = tokenizer.encode(prompt, false)
            .map_err(|e| anyhow!("Tokenization failed: {}", e))?;
        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();

        if input_ids.is_empty() {
            return Ok(String::new());
        }

        let prompt_len = input_ids.len();

        // Prompt pass: process full sequence, get initial KV cache
        let (logits, mut kv_cache) = self.forward_prompt(&input_ids)?;
        let mut next_token = argmax_last(&logits, self.config.vocab_size)?;

        if self.config.eos_token_ids.contains(&next_token) {
            return Ok(String::new());
        }

        let mut generated_tokens: Vec<u32> = vec![next_token];
        let mut total_seq_len = prompt_len;

        // Token passes: generate one token at a time using KV cache
        for _ in 1..max_tokens {
            total_seq_len += 1;
            let (logits, new_kv) = self.forward_token(next_token as i64, total_seq_len, &kv_cache)?;
            kv_cache = new_kv;

            next_token = argmax_last(&logits, self.config.vocab_size)?;
            if self.config.eos_token_ids.contains(&next_token) {
                break;
            }
            generated_tokens.push(next_token);
        }

        let output = tokenizer.decode(&generated_tokens, true)
            .map_err(|e| anyhow!("Detokenization failed: {}", e))?;

        Ok(output.trim().to_string())
    }
}

/// KV cache: Vec of (key, value) tensors per layer.
type KvCache = Vec<(ort::value::Value, ort::value::Value)>;

impl OrtLlmGenerator {
    /// Prompt pass: process the full input sequence with empty KV cache.
    /// Returns logits for the last position and the populated KV cache.
    fn forward_prompt(&self, input_ids: &[i64]) -> Result<(Vec<f32>, KvCache)> {
        let seq_len = input_ids.len();

        let ids_array = ndarray::Array::from_shape_vec((1, seq_len), input_ids.to_vec())?;
        let attention_mask = ndarray::Array::from_elem((1, seq_len), 1i64);

        let empty_kv = self.empty_kv_cache()?;

        // model_q4.onnx inputs: input_ids, attention_mask, past_key_values.*.{key,value}
        let mut inputs: Vec<ort::session::SessionInputValue> = vec![
            ort::value::Tensor::from_array(ids_array)?.into(),
            ort::value::Tensor::from_array(attention_mask)?.into(),
        ];
        inputs.extend(empty_kv.into_iter().map(|v| v.into()));

        let mut session = self.session.lock().unwrap();
        let outputs = session.run(inputs.as_slice())
            .context("LLM prompt pass failed")?;

        let logits = extract_logits(&outputs[0])?;
        let kv = self.extract_kv_cache(&outputs)?;
        Ok((logits, kv))
    }

    /// Token pass: process a single new token with the accumulated KV cache.
    /// `total_seq_len` is the total sequence length including the new token (for attention mask).
    fn forward_token(&self, token_id: i64, total_seq_len: usize, kv_cache: &KvCache) -> Result<(Vec<f32>, KvCache)> {
        let ids_array = ndarray::Array::from_shape_vec((1, 1), vec![token_id])?;
        let attention_mask = ndarray::Array::from_elem((1, total_seq_len), 1i64);

        let mut inputs: Vec<ort::session::SessionInputValue> = vec![
            ort::value::Tensor::from_array(ids_array)?.into(),
            ort::value::Tensor::from_array(attention_mask)?.into(),
        ];

        // Pass existing KV cache as past_key_values
        for (k, v) in kv_cache {
            inputs.push(extract_tensor_copy(k)?.into());
            inputs.push(extract_tensor_copy(v)?.into());
        }

        let mut session = self.session.lock().unwrap();
        let outputs = session.run(inputs.as_slice())
            .context("LLM token pass failed")?;

        let logits = extract_logits(&outputs[0])?;
        let kv = self.extract_kv_cache(&outputs)?;
        Ok((logits, kv))
    }

    /// Create empty KV cache tensors (past_sequence_length=0).
    /// Uses ndarray because ort rc.11 rejects 0-size dims in tuple-form tensors.
    fn empty_kv_cache(&self) -> Result<Vec<ort::value::Value>> {
        let mut kv = Vec::with_capacity(self.config.num_layers * 2);
        for _ in 0..self.config.num_layers {
            let k = ndarray::Array4::<f32>::zeros((1, self.config.num_kv_heads, 0, self.config.head_dim));
            kv.push(ort::value::Tensor::from_array(k)?.into_dyn());
            let v = ndarray::Array4::<f32>::zeros((1, self.config.num_kv_heads, 0, self.config.head_dim));
            kv.push(ort::value::Tensor::from_array(v)?.into_dyn());
        }
        Ok(kv)
    }

    /// Extract KV cache from model outputs.
    /// Outputs: [logits, present_kv.0.key, present_kv.0.value, ..., present_kv.27.key, present_kv.27.value]
    fn extract_kv_cache(&self, outputs: &ort::session::SessionOutputs) -> Result<KvCache> {
        let mut kv = Vec::with_capacity(self.config.num_layers);
        for i in 0..self.config.num_layers {
            let k_idx = 1 + i * 2;
            let v_idx = 2 + i * 2;

            let k_tensor = extract_tensor_copy(&outputs[k_idx])?;
            let v_tensor = extract_tensor_copy(&outputs[v_idx])?;
            kv.push((k_tensor, v_tensor));
        }
        Ok(kv)
    }
}

/// Deep-copy a tensor Value, preserving its shape.
/// ORT output tensors borrow from session memory; we need owned copies for reuse.
fn extract_tensor_copy(tensor: &ort::value::Value) -> Result<ort::value::Value> {
    let (shape, data) = tensor.try_extract_tensor::<f32>()
        .context("Failed to extract tensor for copy")?;
    let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
    let array = ndarray::ArrayD::from_shape_vec(dims, data.to_vec())
        .context("Shape/data mismatch in tensor copy")?;
    Ok(ort::value::Tensor::from_array(array)?.into_dyn())
}

/// Extract logits from the first output tensor.
fn extract_logits(output: &ort::value::Value) -> Result<Vec<f32>> {
    let (_shape, data) = output.try_extract_tensor::<f32>()
        .context("Failed to extract logits tensor")?;
    Ok(data.to_vec())
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
/// Prefers model_q4.onnx (pure int4, f32 KV cache) over model_q4f16.onnx
/// (mixed precision, f16 KV cache) to avoid ORT buffer reuse bugs.
fn find_model_file(model_dir: &Utf8Path) -> Result<Utf8PathBuf> {
    for name in &["model_q4.onnx", "model_q4f16.onnx", "model_q8.onnx", "model_fp16.onnx", "model.onnx"] {
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

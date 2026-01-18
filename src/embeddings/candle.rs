use crate::{config::EmbeddingsDevice, embeddings::Embedder};
use anyhow::{anyhow, Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use tokenizers::{PaddingParams, Tokenizer};

pub struct CandleEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    dim: usize,
    batch_size: usize,
}

impl CandleEmbedder {
    pub fn load(model_dir: &Path, device: EmbeddingsDevice, batch_size: usize) -> Result<Self> {
        let config_path = model_dir.join("config.json");
        let tokenizer_path = model_dir.join("tokenizer.json");
        if !config_path.exists() {
            return Err(anyhow!("Missing config.json in {}", model_dir.display()));
        }
        if !tokenizer_path.exists() {
            return Err(anyhow!("Missing tokenizer.json in {}", model_dir.display()));
        }

        let config_text = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;
        let config: Config = serde_json::from_str(&config_text)
            .with_context(|| format!("Failed to parse {}", config_path.display()))?;

        let candle_device = match device {
            EmbeddingsDevice::Cpu => Device::Cpu,
            EmbeddingsDevice::Metal => metal_device()?,
        };

        let mut tokenizer =
            Tokenizer::from_file(&tokenizer_path).map_err(|e| anyhow!(e.to_string()))?;
        configure_tokenizer(&mut tokenizer, config.max_position_embeddings as usize);

        let weights = weight_files(model_dir)?;
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&weights, DTYPE, &candle_device)? };
        let model = BertModel::load(vb, &config)?;

        Ok(Self {
            model,
            tokenizer,
            device: candle_device,
            dim: config.hidden_size as usize,
            batch_size: batch_size.max(1),
        })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn batch_size(&self) -> usize {
        self.batch_size
    }

    pub fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.batch_size) {
            let mut vectors = self.embed_batch(chunk)?;
            out.append(&mut vectors);
        }
        Ok(out)
    }

    fn embed_batch(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let batch: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        let tokens = self
            .tokenizer
            .encode_batch(batch, true)
            .map_err(|e| anyhow!(e.to_string()))?;

        let token_ids = tokens
            .iter()
            .map(|t| {
                let ids = t.get_ids();
                let ids: Vec<u32> = ids.to_vec();
                Tensor::new(ids.as_slice(), &self.device).map_err(|e| anyhow!(e.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
        let token_ids = Tensor::stack(&token_ids, 0)?;
        let token_type_ids = token_ids.zeros_like()?;

        let embeddings = self.model.forward(&token_ids, &token_type_ids, None)?;
        let (_n_sent, n_tokens, _hidden) = embeddings.dims3()?;
        let pooled = (embeddings.sum(1)? / (n_tokens as f64))?;
        let pooled = normalize_l2(&pooled)?;
        let pooled = pooled.to_dtype(DType::F32)?;

        let vectors = pooled.to_vec2::<f32>()?;
        Ok(vectors)
    }
}

impl Embedder for CandleEmbedder {
    fn dim(&self) -> usize {
        CandleEmbedder::dim(self)
    }

    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        CandleEmbedder::embed(self, texts)
    }
}

fn configure_tokenizer(tokenizer: &mut Tokenizer, max_len: usize) {
    if let Some(pp) = tokenizer.get_padding_mut() {
        pp.strategy = tokenizers::PaddingStrategy::BatchLongest;
    } else {
        let pp = PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            ..Default::default()
        };
        tokenizer.with_padding(Some(pp));
    }

    let tp = tokenizers::TruncationParams {
        max_length: max_len,
        ..Default::default()
    };
    let _ = tokenizer.with_truncation(Some(tp));
}

fn normalize_l2(v: &Tensor) -> Result<Tensor> {
    Ok(v.broadcast_div(&v.sqr()?.sum_keepdim(1)?.sqrt()?)?)
}

#[derive(Deserialize)]
struct SafetensorsIndex {
    weight_map: std::collections::HashMap<String, String>,
}

fn weight_files(model_dir: &Path) -> Result<Vec<PathBuf>> {
    let single = model_dir.join("model.safetensors");
    if single.exists() {
        return Ok(vec![single]);
    }

    let index_path = model_dir.join("model.safetensors.index.json");
    if !index_path.exists() {
        return Err(anyhow!(
            "Missing model.safetensors (or model.safetensors.index.json) in {}",
            model_dir.display()
        ));
    }

    let text = fs::read_to_string(&index_path)
        .with_context(|| format!("Failed to read {}", index_path.display()))?;
    let idx: SafetensorsIndex = serde_json::from_str(&text)
        .with_context(|| format!("Failed to parse {}", index_path.display()))?;

    let mut uniq = HashSet::new();
    let mut out = Vec::new();
    for v in idx.weight_map.values() {
        if uniq.insert(v.as_str()) {
            out.push(model_dir.join(v));
        }
    }

    if out.is_empty() {
        return Err(anyhow!("Empty weight_map in {}", index_path.display()));
    }

    for p in &out {
        if !p.exists() {
            return Err(anyhow!("Missing weight file: {}", p.display()));
        }
    }

    Ok(out)
}

fn metal_device() -> Result<Device> {
    #[cfg(feature = "metal")]
    {
        Device::new_metal(0).context("Failed to create Metal device")
    }

    #[cfg(not(feature = "metal"))]
    {
        Err(anyhow!(
            "Binary not built with metal support. Rebuild with --features metal"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "code-intel-candle-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_errors_when_config_missing() {
        let dir = tmp_dir();
        let err = match CandleEmbedder::load(&dir, EmbeddingsDevice::Cpu, 8) {
            Ok(_) => panic!("expected error"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("Missing config.json"));
    }

    #[test]
    fn weight_files_prefers_single_safetensors() {
        let dir = tmp_dir();
        let p = dir.join("model.safetensors");
        std::fs::write(&p, "").unwrap();
        let out = weight_files(&dir).unwrap();
        assert_eq!(out, vec![p]);
    }

    #[test]
    fn weight_files_uses_index_when_sharded() {
        let dir = tmp_dir();
        let shard1 = dir.join("shard1.safetensors");
        let shard2 = dir.join("shard2.safetensors");
        std::fs::write(&shard1, "").unwrap();
        std::fs::write(&shard2, "").unwrap();
        std::fs::write(
            dir.join("model.safetensors.index.json"),
            r#"{ "weight_map": { "a": "shard1.safetensors", "b": "shard2.safetensors", "c": "shard1.safetensors" } }"#,
        )
        .unwrap();

        let out = weight_files(&dir).unwrap();
        assert_eq!(out.len(), 2);
        assert!(out.contains(&shard1));
        assert!(out.contains(&shard2));
    }

    #[test]
    fn weight_files_errors_on_empty_weight_map() {
        let dir = tmp_dir();
        std::fs::write(
            dir.join("model.safetensors.index.json"),
            r#"{ "weight_map": {} }"#,
        )
        .unwrap();
        let err = weight_files(&dir).unwrap_err().to_string();
        assert!(err.contains("Empty weight_map"));
    }

    #[test]
    fn metal_device_errors_without_feature() {
        let err = match metal_device() {
            Ok(_) => panic!("expected error"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("metal"));
    }
}

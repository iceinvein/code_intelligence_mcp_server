use crate::embeddings::Embedder;
use anyhow::Result;

pub struct HashEmbedder {
    dim: usize,
}

impl HashEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.max(8) }
    }
}

impl Embedder for HashEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            let mut v = vec![0.0f32; self.dim];
            for token in tokenize(text) {
                let h = fnv1a_64(token.as_bytes());
                let idx = (h as usize) % self.dim;
                let sign = if (h >> 63) == 0 { 1.0 } else { -1.0 };
                v[idx] += sign;
            }
            normalize_l2(&mut v);
            out.push(v);
        }
        Ok(out)
    }
}

fn tokenize(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
        .filter(|s| !s.is_empty())
}

use super::l2_normalize as normalize_l2;

fn fnv1a_64(data: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;
    let mut hash = OFFSET;
    for b in data {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

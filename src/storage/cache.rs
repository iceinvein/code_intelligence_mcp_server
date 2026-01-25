use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::storage::sqlite::SqliteStore;

/// Cache key generator using SHA-256
pub fn cache_key(model_name: &str, text_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model_name.as_bytes());
    hasher.update(b"|");
    hasher.update(text_hash);
    format!("{:x}", hasher.finalize())
}

/// Content hash for text (SHA-256)
pub fn content_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub struct EmbeddingCache {
    db: Arc<SqliteStore>,
    model_name: String,
    hits: AtomicU64,
    misses: AtomicU64,
    max_size_bytes: i64,
    enabled: bool,
}

impl EmbeddingCache {
    pub fn new(db: Arc<SqliteStore>, model_name: &str, enabled: bool, max_size_bytes: i64) -> Self {
        Self {
            db,
            model_name: model_name.to_string(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            max_size_bytes,
            enabled,
        }
    }

    /// Try to get cached embedding
    pub fn get(&self, text: &str) -> Option<Vec<f32>> {
        if !self.enabled {
            return None;
        }

        let text_hash = content_hash(text);
        let key = cache_key(&self.model_name, &text_hash);

        match self.db.get_cached_embedding(&key) {
            Ok(Some(blob)) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                match postcard::from_bytes::<Vec<f32>>(&blob) {
                    Ok(vec) => Some(vec),
                    Err(e) => {
                        tracing::warn!("Failed to deserialize cached embedding: {}", e);
                        None
                    }
                }
            }
            _ => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Store embedding in cache
    pub fn put(&self, text: &str, embedding: &[f32]) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let text_hash = content_hash(text);
        let key = cache_key(&self.model_name, &text_hash);
        let encoded = postcard::to_vec(embedding)
            .context("Failed to serialize embedding")?;

        self.db.put_cached_embedding(
            &key,
            &self.model_name,
            &text_hash,
            &encoded,
            embedding.len(),
        )?;

        // Lazy cleanup on put (every 1000 puts)
        let misses = self.misses.load(Ordering::Relaxed);
        if misses % 1000 == 0 {
            let _ = self.db.cleanup_cache(self.max_size_bytes);
        }

        Ok(())
    }

    /// Cache statistics
    pub fn stats(&self) -> CacheStats {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        };

        CacheStats {
            hits,
            misses,
            hit_rate,
        }
    }
}

pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
}

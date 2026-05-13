//! Per-process LRU cache for `ask_code` responses.
//!
//! Keyed by (question_hash, repo_index_version, quality). When the index
//! changes (new `index_runs` row), `repo_index_version` shifts and stale
//! entries naturally miss. Responses that should not be cached (e.g.
//! transient `llm_unavailable`) are filtered by the caller via
//! [`AskCodeCache::is_cacheable`].

use std::num::NonZeroUsize;
use std::sync::Mutex;

use lru::LruCache;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::ask_code::AnswerQuality;

/// Default LRU capacity. ~64 KB per response cap (set in handler) means
/// ~16 MB worst-case footprint for the full cache.
const CACHE_CAPACITY: usize = 256;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct AskCodeCacheKey {
    pub question_hash: u64,
    pub repo_index_version: i64,
    pub quality: AnswerQuality,
}

impl AskCodeCacheKey {
    pub fn new(question: &str, repo_index_version: i64, quality: AnswerQuality) -> Self {
        Self {
            question_hash: hash_question(question),
            repo_index_version,
            quality,
        }
    }
}

pub struct AskCodeCache {
    inner: Mutex<LruCache<AskCodeCacheKey, Value>>,
}

impl Default for AskCodeCache {
    fn default() -> Self {
        Self::with_capacity(CACHE_CAPACITY)
    }
}

impl AskCodeCache {
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).expect("capacity > 0");
        Self {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }

    /// Look up a cached response. Returns a fresh clone of the cached value
    /// so callers can mutate freely (e.g. to set `cached: true`).
    pub fn get(&self, key: &AskCodeCacheKey) -> Option<Value> {
        let mut guard = self.inner.lock().ok()?;
        guard.get(key).cloned()
    }

    /// Insert a response. No-op when the response is not cacheable (see
    /// [`Self::is_cacheable`]) — caller should still call this to keep the
    /// caching decision in one place.
    pub fn put(&self, key: AskCodeCacheKey, value: Value) {
        if !Self::is_cacheable(&value) {
            return;
        }
        if let Ok(mut guard) = self.inner.lock() {
            guard.put(key, value);
        }
    }

    /// Return the number of entries currently held. Used for diagnostics and
    /// tests; cheap because LRU's `len()` is O(1).
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Some responses are transient and must not be cached:
    /// - `stop_reason: "llm_unavailable"` — the LLM may load on a later call.
    /// - missing `stop_reason` — defensive; treat unknown shapes as
    ///   non-cacheable so we never poison the cache on a bug.
    ///
    /// Everything else (including `low_confidence`, `no_evidence`) is
    /// deterministic given (question, index_version, quality) and safe to
    /// cache. An `no_evidence` response can only change when the index
    /// version changes, which by design rotates the cache key.
    pub fn is_cacheable(value: &Value) -> bool {
        match value.get("stop_reason").and_then(|v| v.as_str()) {
            Some("llm_unavailable") => false,
            Some(_) => true,
            None => false,
        }
    }
}

fn hash_question(question: &str) -> u64 {
    let mut h = Sha256::new();
    h.update(question.trim().as_bytes());
    let digest = h.finalize();
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn answered() -> Value {
        json!({
            "question": "q",
            "answer": "an answer",
            "stop_reason": "answered",
            "confidence": "high",
        })
    }

    fn unavailable() -> Value {
        json!({
            "stop_reason": "llm_unavailable",
        })
    }

    #[test]
    fn key_deterministic_for_same_question() {
        let a = AskCodeCacheKey::new("Where is X?", 42, AnswerQuality::Balanced);
        let b = AskCodeCacheKey::new("Where is X?", 42, AnswerQuality::Balanced);
        assert_eq!(a, b);
    }

    #[test]
    fn key_ignores_question_leading_trailing_whitespace() {
        let a = AskCodeCacheKey::new("Where is X?", 1, AnswerQuality::Balanced);
        let b = AskCodeCacheKey::new("  Where is X?\n", 1, AnswerQuality::Balanced);
        assert_eq!(a, b);
    }

    #[test]
    fn key_distinct_per_index_version() {
        let a = AskCodeCacheKey::new("q", 1, AnswerQuality::Balanced);
        let b = AskCodeCacheKey::new("q", 2, AnswerQuality::Balanced);
        assert_ne!(a, b);
    }

    #[test]
    fn key_distinct_per_quality() {
        let a = AskCodeCacheKey::new("q", 1, AnswerQuality::Balanced);
        let b = AskCodeCacheKey::new("q", 1, AnswerQuality::Fast);
        assert_ne!(a, b);
    }

    #[test]
    fn get_returns_none_on_miss() {
        let cache = AskCodeCache::default();
        let key = AskCodeCacheKey::new("anything", 0, AnswerQuality::Balanced);
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn put_then_get_round_trips_cacheable_response() {
        let cache = AskCodeCache::default();
        let key = AskCodeCacheKey::new("q", 1, AnswerQuality::Balanced);
        cache.put(key.clone(), answered());
        let v = cache.get(&key).expect("hit");
        assert_eq!(v["answer"], "an answer");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn put_drops_uncacheable_response() {
        let cache = AskCodeCache::default();
        let key = AskCodeCacheKey::new("q", 1, AnswerQuality::Balanced);
        cache.put(key.clone(), unavailable());
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn is_cacheable_recognises_known_stop_reasons() {
        assert!(AskCodeCache::is_cacheable(
            &json!({"stop_reason": "answered"})
        ));
        assert!(AskCodeCache::is_cacheable(
            &json!({"stop_reason": "low_confidence"})
        ));
        assert!(AskCodeCache::is_cacheable(
            &json!({"stop_reason": "no_evidence"})
        ));
        assert!(!AskCodeCache::is_cacheable(
            &json!({"stop_reason": "llm_unavailable"})
        ));
        assert!(!AskCodeCache::is_cacheable(&json!({})));
    }

    #[test]
    fn eviction_at_capacity_drops_oldest() {
        let cache = AskCodeCache::with_capacity(2);
        let k1 = AskCodeCacheKey::new("q1", 1, AnswerQuality::Balanced);
        let k2 = AskCodeCacheKey::new("q2", 1, AnswerQuality::Balanced);
        let k3 = AskCodeCacheKey::new("q3", 1, AnswerQuality::Balanced);
        cache.put(k1.clone(), answered());
        cache.put(k2.clone(), answered());
        cache.put(k3.clone(), answered());
        assert!(
            cache.get(&k1).is_none(),
            "oldest entry should have been evicted"
        );
        assert!(cache.get(&k2).is_some());
        assert!(cache.get(&k3).is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn get_promotes_entry_in_lru_order() {
        let cache = AskCodeCache::with_capacity(2);
        let k1 = AskCodeCacheKey::new("q1", 1, AnswerQuality::Balanced);
        let k2 = AskCodeCacheKey::new("q2", 1, AnswerQuality::Balanced);
        let k3 = AskCodeCacheKey::new("q3", 1, AnswerQuality::Balanced);
        cache.put(k1.clone(), answered());
        cache.put(k2.clone(), answered());
        // Touch k1 to make it most-recent.
        let _ = cache.get(&k1);
        // Inserting k3 should now evict k2 (least-recently-used), not k1.
        cache.put(k3.clone(), answered());
        assert!(cache.get(&k1).is_some(), "promoted entry must survive");
        assert!(cache.get(&k2).is_none(), "non-promoted entry must evict");
        assert!(cache.get(&k3).is_some());
    }
}

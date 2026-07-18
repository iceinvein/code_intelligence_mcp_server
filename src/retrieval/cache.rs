//! Caching logic for retrieval operations

use crate::retrieval::assembler::ContextItem;
use crate::retrieval::SearchResponseWithSignals;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct LruCache<V> {
    max_entries: usize,
    max_bytes: Option<usize>,
    used_bytes: usize,
    order: VecDeque<String>,
    entries: HashMap<String, (V, usize)>,
}

impl<V: Clone> LruCache<V> {
    pub fn new(max_entries: usize, max_bytes: Option<usize>) -> Self {
        Self {
            max_entries,
            max_bytes,
            used_bytes: 0,
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    pub fn get(&mut self, key: &str) -> Option<V> {
        let (v, _) = self.entries.get(key).cloned()?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_string());
        Some(v)
    }

    pub fn insert(&mut self, key: String, value: V, size_bytes: usize) {
        if self.entries.contains_key(&key) {
            let old = self.entries.insert(key.clone(), (value, size_bytes));
            if let Some((_, old_size)) = old {
                self.used_bytes = self.used_bytes.saturating_sub(old_size);
            }
            self.used_bytes = self.used_bytes.saturating_add(size_bytes);
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
            self.order.push_back(key);
        } else {
            self.entries.insert(key.clone(), (value, size_bytes));
            self.used_bytes = self.used_bytes.saturating_add(size_bytes);
            self.order.push_back(key);
        }

        while self.order.len() > self.max_entries {
            if let Some(oldest) = self.order.pop_front() {
                if let Some((_, sz)) = self.entries.remove(&oldest) {
                    self.used_bytes = self.used_bytes.saturating_sub(sz);
                }
            }
        }

        if let Some(max) = self.max_bytes {
            while self.used_bytes > max {
                if let Some(oldest) = self.order.pop_front() {
                    if let Some((_, sz)) = self.entries.remove(&oldest) {
                        self.used_bytes = self.used_bytes.saturating_sub(sz);
                    }
                } else {
                    break;
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.order.clear();
        self.entries.clear();
        self.used_bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }
}

#[derive(Debug, Clone)]
pub struct AsyncSingleFlight<V> {
    pending: Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::OnceCell<V>>>>>,
}

impl<V> Default for AsyncSingleFlight<V> {
    fn default() -> Self {
        Self {
            pending: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }
}

impl<V: Clone> AsyncSingleFlight<V> {
    pub async fn run<E, F, Fut>(&self, key: String, init: F) -> Result<(V, bool), E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, E>>,
    {
        let (cell, is_leader) = {
            let mut pending = self.pending.lock().await;
            if let Some(existing) = pending.get(&key) {
                (existing.clone(), false)
            } else {
                let cell = Arc::new(tokio::sync::OnceCell::new());
                pending.insert(key.clone(), cell.clone());
                (cell, true)
            }
        };

        let value = cell.get_or_try_init(init).await.cloned();
        let mut pending = self.pending.lock().await;
        if pending
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, &cell))
        {
            pending.remove(&key);
        }
        value.map(|value| (value, is_leader))
    }
}

#[derive(Debug, Clone)]
pub struct RetrieverCaches {
    pub last_symbol_update_unix_s: Option<i64>,
    pub last_index_run_version: Option<String>,
    pub responses: LruCache<SearchResponseWithSignals>,
    pub embeddings: LruCache<Vec<f32>>,
    pub contexts: LruCache<(String, Vec<ContextItem>)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheInvalidation {
    pub responses: bool,
    pub embeddings: bool,
    pub contexts: bool,
}

impl RetrieverCaches {
    pub fn new() -> Self {
        Self {
            last_symbol_update_unix_s: None,
            last_index_run_version: None,
            responses: LruCache::new(128, None),
            embeddings: LruCache::new(2048, Some(32 * 1024 * 1024)),
            contexts: LruCache::new(128, Some(16 * 1024 * 1024)),
        }
    }

    pub fn invalidate_if_stale(
        &mut self,
        symbol_update_unix_s: Option<i64>,
        index_run_version: Option<String>,
    ) -> CacheInvalidation {
        if self.last_symbol_update_unix_s == symbol_update_unix_s
            && self.last_index_run_version == index_run_version
        {
            return CacheInvalidation::default();
        }

        let invalidation = CacheInvalidation {
            responses: !self.responses.is_empty(),
            embeddings: !self.embeddings.is_empty(),
            contexts: !self.contexts.is_empty(),
        };
        self.responses.clear();
        self.embeddings.clear();
        self.contexts.clear();
        self.last_symbol_update_unix_s = symbol_update_unix_s;
        self.last_index_run_version = index_run_version;
        invalidation
    }
}

impl Default for RetrieverCaches {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{AsyncSingleFlight, LruCache, RetrieverCaches};
    use crate::retrieval::{HitSignals, SearchResponse, SearchResponseWithSignals};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn reports_entry_and_byte_usage_after_eviction_and_clear() {
        let mut cache = LruCache::new(2, Some(6));
        cache.insert("a".to_string(), 1, 3);
        cache.insert("b".to_string(), 2, 3);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.used_bytes(), 6);

        cache.insert("c".to_string(), 3, 4);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.used_bytes(), 4);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.used_bytes(), 0);
    }

    #[test]
    fn invalidates_responses_for_two_index_runs_in_the_same_second() {
        let mut caches = RetrieverCaches::new();
        caches.last_symbol_update_unix_s = Some(123);
        caches.last_index_run_version = Some("123:1".to_string());
        caches.responses.insert(
            "query".to_string(),
            SearchResponseWithSignals {
                response: SearchResponse {
                    query: "query".to_string(),
                    limit: 5,
                    hits: Vec::new(),
                    context: String::new(),
                },
                hit_signals: HashMap::new(),
            },
            1,
        );

        let invalidation = caches.invalidate_if_stale(Some(123), Some("123:2".to_string()));

        assert!(invalidation.responses);
        assert!(caches.responses.is_empty());
        assert_eq!(caches.last_index_run_version.as_deref(), Some("123:2"));
    }

    #[test]
    fn response_cache_preserves_hit_signals() {
        let mut caches = RetrieverCaches::new();
        let mut hit_signals = HashMap::new();
        hit_signals.insert(
            "symbol-1".to_string(),
            HitSignals {
                base_score: 7.5,
                ..HitSignals::default()
            },
        );
        caches.responses.insert(
            "query".to_string(),
            SearchResponseWithSignals {
                response: SearchResponse {
                    query: "query".to_string(),
                    limit: 5,
                    hits: Vec::new(),
                    context: String::new(),
                },
                hit_signals,
            },
            1,
        );

        let cached = caches.responses.get("query").unwrap();
        assert_eq!(cached.hit_signals["symbol-1"].base_score, 7.5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn single_flight_coalesces_simultaneous_cache_misses() {
        let flight = Arc::new(AsyncSingleFlight::<Vec<f32>>::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Barrier::new(16));
        let mut tasks = tokio::task::JoinSet::new();

        for _ in 0..16 {
            let flight = flight.clone();
            let calls = calls.clone();
            let gate = gate.clone();
            tasks.spawn(async move {
                gate.wait().await;
                flight
                    .run("same query".to_string(), || async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        Ok::<_, anyhow::Error>(vec![1.0, 2.0])
                    })
                    .await
                    .unwrap()
                    .0
            });
        }

        while let Some(result) = tasks.join_next().await {
            assert_eq!(result.unwrap(), vec![1.0, 2.0]);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

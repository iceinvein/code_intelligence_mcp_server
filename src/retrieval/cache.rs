//! Caching logic for retrieval operations

use crate::retrieval::assembler::ContextItem;
use crate::retrieval::SearchResponse;
use std::collections::{HashMap, VecDeque};

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
}

#[derive(Debug, Clone)]
pub struct RetrieverCaches {
    pub last_symbol_update_unix_s: Option<i64>,
    pub last_index_run_started_at_unix_s: Option<i64>,
    pub responses: LruCache<SearchResponse>,
    pub embeddings: LruCache<Vec<f32>>,
    pub contexts: LruCache<(String, Vec<ContextItem>)>,
}

impl RetrieverCaches {
    pub fn new() -> Self {
        Self {
            last_symbol_update_unix_s: None,
            last_index_run_started_at_unix_s: None,
            responses: LruCache::new(64, None),
            embeddings: LruCache::new(256, Some(4 * 1024 * 1024)),
            contexts: LruCache::new(64, Some(8 * 1024 * 1024)),
        }
    }
}

impl Default for RetrieverCaches {
    fn default() -> Self {
        Self::new()
    }
}

//! Caching layer for reranker results

use super::{RerankDocument, Reranker};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// Cached reranker that memoizes reranking results
pub struct CachedReranker {
    inner: Box<dyn Reranker>,
    cache: Arc<Mutex<Cache>>,
    semaphore: Arc<Semaphore>,
}

struct Cache {
    entries: HashMap<CacheKey, Vec<f32>>,
    max_size: usize,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    query_hash: u64,
    docs_hash: u64,
}

impl CachedReranker {
    /// Create a new cached reranker wrapping the inner reranker
    pub fn new(inner: Box<dyn Reranker>, max_size: usize) -> Self {
        Self {
            inner,
            cache: Arc::new(Mutex::new(Cache {
                entries: HashMap::new(),
                max_size,
            })),
            semaphore: Arc::new(Semaphore::new(4)), // Limit concurrent rerankings
        }
    }

    /// Generate a cache key from query and documents
    fn cache_key(query: &str, documents: &[RerankDocument]) -> CacheKey {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        query.hash(&mut hasher);

        // Hash document IDs (not full text to save memory)
        for doc in documents.iter().take(20) {
            doc.id.hash(&mut hasher);
        }

        let mut docs_hasher = DefaultHasher::new();
        for doc in documents.iter().take(20) {
            doc.id.hash(&mut docs_hasher);
        }

        CacheKey {
            query_hash: hasher.finish(),
            docs_hash: docs_hasher.finish(),
        }
    }
}

#[async_trait::async_trait]
impl Reranker for CachedReranker {
    async fn rerank(&self, query: &str, documents: &[RerankDocument]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }

        let key = Self::cache_key(query, documents);

        // Check cache
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(scores) = cache.entries.get(&key) {
                return Ok(scores.clone());
            }
        }

        // Acquire semaphore to limit concurrent rerankings
        let _permit = self.semaphore.acquire().await?;

        // Double-check cache after acquiring permit
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(scores) = cache.entries.get(&key) {
                return Ok(scores.clone());
            }
        }

        // Run reranking
        let scores = self.inner.rerank(query, documents).await?;

        // Cache result
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.entries.len() >= cache.max_size {
            // Evict oldest entry (simple FIFO)
            if let Some(key) = cache.entries.keys().next().cloned() {
                cache.entries.remove(&key);
            }
        }
        cache.entries.insert(key, scores.clone());

        Ok(scores)
    }

    fn top_k(&self) -> usize {
        self.inner.top_k()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable() {
        use super::super::RerankDocument;

        let docs = vec![
            RerankDocument {
                id: "doc1".to_string(),
                text: "function foo() {}".to_string(),
                name: "foo".to_string(),
            },
            RerankDocument {
                id: "doc2".to_string(),
                text: "function bar() {}".to_string(),
                name: "bar".to_string(),
            },
        ];

        let key1 = CachedReranker::cache_key("query", &docs);
        let key2 = CachedReranker::cache_key("query", &docs);

        assert_eq!(key1, key2);
    }

    #[test]
    fn cache_key_differs_by_query() {
        use super::super::RerankDocument;

        let docs = vec![RerankDocument {
            id: "doc1".to_string(),
            text: "function foo() {}".to_string(),
            name: "foo".to_string(),
        }];

        let key1 = CachedReranker::cache_key("query1", &docs);
        let key2 = CachedReranker::cache_key("query2", &docs);

        assert_ne!(key1, key2);
    }

    #[test]
    fn cache_key_differs_by_docs() {
        use super::super::RerankDocument;

        let docs1 = vec![RerankDocument {
            id: "doc1".to_string(),
            text: "function foo() {}".to_string(),
            name: "foo".to_string(),
        }];
        let docs2 = vec![RerankDocument {
            id: "doc2".to_string(),
            text: "function bar() {}".to_string(),
            name: "bar".to_string(),
        }];

        let key1 = CachedReranker::cache_key("query", &docs1);
        let key2 = CachedReranker::cache_key("query", &docs2);

        assert_ne!(key1, key2);
    }
}

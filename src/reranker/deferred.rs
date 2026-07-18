//! Deferred reranker: a [`Reranker`] whose backing cross-encoder model is
//! slotted in later from a background task.
//!
//! Loading the bge-reranker-v2-m3 GGUF model downloads ~600 MB on first launch
//! and takes seconds to bring up on Metal. To keep daemon startup fast (the
//! same reason the embedder is deferred), the HTTP server starts immediately
//! with a `DeferredReranker` in place; a background task loads the real
//! reranker and calls [`DeferredReranker::set_inner`].
//!
//! Until the inner reranker is set, [`rerank`](DeferredReranker::rerank)
//! returns an error. The query pipeline already treats a reranker error as
//! "skip reranking" (it falls back to the pre-rerank ordering), so searches
//! run normally — just without cross-encoder reordering — until the model is
//! ready.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;

use super::{RerankDocument, Reranker};

/// A [`Reranker`] that delegates to an inner reranker once it has been loaded.
pub struct DeferredReranker {
    top_k: usize,
    inner: Arc<Mutex<Option<Arc<dyn Reranker>>>>,
}

impl DeferredReranker {
    /// Create a deferred reranker with no backing model yet. `top_k` is the
    /// value reported by [`top_k`](Self::top_k) before and after loading.
    pub fn new(top_k: usize) -> Self {
        Self {
            top_k,
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// Slot in the real reranker once it has been downloaded and loaded.
    pub fn set_inner(&self, reranker: Arc<dyn Reranker>) {
        let mut guard = self.inner.lock().expect("DeferredReranker mutex poisoned");
        *guard = Some(reranker);
    }

    /// Whether the real reranker has been loaded.
    pub fn is_ready(&self) -> bool {
        let guard = self.inner.lock().expect("DeferredReranker mutex poisoned");
        guard.is_some()
    }

    /// Clone of the inner slot for handing to a background loader task.
    pub fn inner_slot(&self) -> Arc<Mutex<Option<Arc<dyn Reranker>>>> {
        Arc::clone(&self.inner)
    }
}

#[async_trait]
impl Reranker for DeferredReranker {
    async fn rerank(&self, query: &str, documents: &[RerankDocument]) -> Result<Vec<f32>> {
        // Clone the inner Arc out under the lock, then release the lock before
        // the await point (the std Mutex guard is not Send across .await).
        let inner = {
            let guard = self.inner.lock().expect("DeferredReranker mutex poisoned");
            guard.clone()
        };
        match inner {
            Some(reranker) => reranker.rerank(query, documents).await,
            None => anyhow::bail!("reranker model is still loading"),
        }
    }

    fn top_k(&self) -> usize {
        self.top_k
    }

    fn is_ready(&self) -> bool {
        self.is_ready()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reranker stub that returns a fixed score per document, so tests can
    /// assert delegation happened.
    struct StubReranker {
        score: f32,
        top_k: usize,
    }

    #[async_trait]
    impl Reranker for StubReranker {
        async fn rerank(&self, _query: &str, documents: &[RerankDocument]) -> Result<Vec<f32>> {
            Ok(documents.iter().map(|_| self.score).collect())
        }
        fn top_k(&self) -> usize {
            self.top_k
        }
    }

    fn docs(n: usize) -> Vec<RerankDocument> {
        (0..n)
            .map(|i| RerankDocument {
                id: format!("id{i}"),
                text: format!("text {i}"),
                name: format!("name{i}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn not_ready_reports_unready_and_errs_on_rerank() {
        let dr = DeferredReranker::new(20);
        assert!(!dr.is_ready());
        assert_eq!(dr.top_k(), 20);
        let result = dr.rerank("q", &docs(3)).await;
        assert!(
            result.is_err(),
            "rerank should error before the model loads"
        );
    }

    #[tokio::test]
    async fn ready_delegates_to_inner() {
        let dr = DeferredReranker::new(20);
        dr.set_inner(Arc::new(StubReranker {
            score: 0.42,
            top_k: 10,
        }));
        assert!(dr.is_ready());
        // top_k stays the deferred wrapper's own value (stable across loading).
        assert_eq!(dr.top_k(), 20);
        let scores = dr.rerank("q", &docs(3)).await.expect("should delegate");
        assert_eq!(scores, vec![0.42, 0.42, 0.42]);
    }

    #[tokio::test]
    async fn inner_slot_fill_makes_it_ready() {
        let dr = DeferredReranker::new(5);
        let slot = dr.inner_slot();
        assert!(!dr.is_ready());
        *slot.lock().unwrap() = Some(Arc::new(StubReranker {
            score: 1.0,
            top_k: 5,
        }));
        assert!(dr.is_ready());
        let scores = dr.rerank("q", &docs(2)).await.expect("should delegate");
        assert_eq!(scores, vec![1.0, 1.0]);
    }
}

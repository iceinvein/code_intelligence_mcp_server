//! Async, concurrency-capped front-end for the sync [`Embedder`] trait.
//!
//! The index pipeline and the query path both need to embed text against the
//! same loaded model. The underlying [`Embedder`] implementations are sync and
//! interior-immutable (forward passes create a fresh `LlamaContext` per call),
//! so calls are reentrant in principle. `SharedEmbedder` adds two pieces on
//! top so they remain reentrant in practice:
//!
//! 1. **`tokio::task::spawn_blocking`** — embed calls are CPU/GPU work and can
//!    take hundreds of milliseconds to multiple seconds. Running them on the
//!    blocking pool keeps the async runtime healthy even under sustained
//!    indexer load.
//! 2. **`tokio::sync::Semaphore`** — bounds GPU concurrency. Multiple Metal
//!    forward passes can run simultaneously, but each one allocates KV cache
//!    in unified memory, so unbounded parallelism can OOM. The semaphore caps
//!    in-flight calls; permits are fair (FIFO), so an indexer batch already
//!    in flight does not starve a query waiting behind it.
//!
//! The cap is intentionally larger than 1 so a single indexer batch never
//! blocks a query at the Rust layer (which was the v3/v4 footgun behind the
//! 120-second `ask_code` timeout). With 4 permits, an indexer holding one
//! still leaves three lanes for query embeds.

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Semaphore;

use crate::embeddings::Embedder;

/// Default concurrency cap. Sized to allow several query embeds to overlap a
/// single indexer batch while staying well under Metal's KV-cache budget on
/// Apple Silicon with the 1.5B Q8_0 jina-code model.
const DEFAULT_MAX_CONCURRENCY: usize = 4;

/// Cloneable, async-friendly handle to a shared [`Embedder`].
#[derive(Clone)]
pub struct SharedEmbedder {
    inner: Arc<dyn Embedder>,
    semaphore: Arc<Semaphore>,
    dim: usize,
}

impl SharedEmbedder {
    /// Wrap an owned embedder with the default concurrency cap.
    pub fn new(inner: Box<dyn Embedder>) -> Self {
        Self::with_concurrency(inner, DEFAULT_MAX_CONCURRENCY)
    }

    /// Wrap an owned embedder with a custom concurrency cap (clamped to >= 1).
    pub fn with_concurrency(inner: Box<dyn Embedder>, max_concurrency: usize) -> Self {
        Self::from_arc(Arc::from(inner), max_concurrency)
    }

    /// Wrap an existing `Arc<dyn Embedder>` so a single sync backend can back
    /// multiple `SharedEmbedder` handles (e.g. if a caller needs to keep its
    /// own reference for `DeferredEmbedder::set_inner`).
    pub fn from_arc(inner: Arc<dyn Embedder>, max_concurrency: usize) -> Self {
        let dim = inner.dim();
        Self {
            inner,
            semaphore: Arc::new(Semaphore::new(max_concurrency.max(1))),
            dim,
        }
    }

    /// Embedding dimension. Cached at construction so callers do not need to
    /// take any locks to read it.
    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn is_ready(&self) -> bool {
        self.inner.is_ready()
    }

    /// Embed a batch of texts. The call is gated by the concurrency semaphore
    /// and runs on the blocking thread pool.
    pub async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .context("embedder semaphore closed")?;
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.embed(&texts))
            .await
            .context("embedder task panicked")?
    }

    /// Embed query texts. Identical semantics to [`embed`](Self::embed) but
    /// dispatches to the backend's `query_embed` (which may apply a retrieval
    /// instruction prefix on asymmetric models).
    pub async fn query_embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .context("embedder semaphore closed")?;
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.query_embed(&texts))
            .await
            .context("embedder task panicked")?
    }

    /// Acquire a permit and call into the backend with a closure. Exposed so
    /// rare callers that need to thread mutable state (e.g. tests verifying
    /// the semaphore behaviour) can run code on the blocking pool with the
    /// same backpressure as `embed`.
    pub async fn with_backend<R, F>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&dyn Embedder) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .context("embedder semaphore closed")?;
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || f(inner.as_ref()))
            .await
            .context("embedder task panicked")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::hash::HashEmbedder;
    use crate::embeddings::Embedder;
    use anyhow::Result;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::time::Instant;

    /// Test backend that sleeps for a configurable duration on every `embed`
    /// call, so we can assert that a long-running indexer batch does not
    /// block a concurrent query embed at the Rust layer.
    struct SlowEmbedder {
        dim: usize,
        delay: Duration,
        in_flight: AtomicUsize,
        peak_in_flight: AtomicUsize,
    }

    impl SlowEmbedder {
        fn new(dim: usize, delay: Duration) -> Self {
            Self {
                dim,
                delay,
                in_flight: AtomicUsize::new(0),
                peak_in_flight: AtomicUsize::new(0),
            }
        }
    }

    impl Embedder for SlowEmbedder {
        fn dim(&self) -> usize {
            self.dim
        }

        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_in_flight.fetch_max(current, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(texts.iter().map(|_| vec![0.0; self.dim]).collect())
        }
    }

    #[tokio::test]
    async fn dim_matches_inner() {
        let shared = SharedEmbedder::new(Box::new(HashEmbedder::new(32)));
        assert_eq!(shared.dim(), 32);
    }

    #[tokio::test]
    async fn embed_returns_vectors_of_expected_dim() {
        let shared = SharedEmbedder::new(Box::new(HashEmbedder::new(64)));
        let out = shared
            .embed(vec!["hello".into(), "world".into()])
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 64);
        assert_eq!(out[1].len(), 64);
    }

    #[tokio::test]
    async fn empty_input_short_circuits() {
        let shared = SharedEmbedder::new(Box::new(HashEmbedder::new(8)));
        let out = shared.embed(Vec::new()).await.unwrap();
        assert!(out.is_empty());
    }

    /// Regression test for the v4 contention bug: a slow indexer batch
    /// must not serialize a concurrent query embed.
    ///
    /// Drives two `embed` calls in parallel. With the old single-mutex design
    /// the second call would queue behind the first and the total wall-clock
    /// time would be roughly `2 * delay`. With `SharedEmbedder`'s spawn_blocking
    /// + multi-permit semaphore the two calls overlap on separate blocking
    /// threads and complete in roughly `delay`. The `peak_in_flight` counter
    /// confirms both calls were actually inside the backend simultaneously.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn query_embed_does_not_serialize_behind_indexer_batch() {
        let delay = Duration::from_millis(250);
        let backend = Arc::new(SlowEmbedder::new(8, delay));
        let shared: Arc<SharedEmbedder> = Arc::new(SharedEmbedder::from_arc(backend.clone(), 4));

        let indexer = shared.clone();
        let query = shared.clone();

        let started = Instant::now();
        let indexer_handle = tokio::spawn(async move {
            // Simulate a 32-text indexer batch as a single call.
            let texts: Vec<String> = (0..32).map(|i| format!("doc {i}")).collect();
            indexer.embed(texts).await.unwrap()
        });

        // Give the indexer task a head start so its blocking work is already
        // underway when the query embed acquires its own permit.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let query_handle =
            tokio::spawn(async move { query.embed(vec!["q".into()]).await.unwrap() });

        let (indexer_out, query_out) = tokio::join!(indexer_handle, query_handle);
        let elapsed = started.elapsed();

        assert_eq!(indexer_out.unwrap().len(), 32);
        assert_eq!(query_out.unwrap().len(), 1);

        let peak = backend.peak_in_flight.load(Ordering::SeqCst);
        assert!(
            peak >= 2,
            "expected indexer and query embeds to overlap, peak_in_flight={peak}",
        );

        // Generous upper bound: well under serialised execution (2 * delay)
        // but loose enough to absorb scheduler jitter on a busy CI host.
        assert!(
            elapsed < delay * 2 - Duration::from_millis(50),
            "embeds appear to be serialised — elapsed={elapsed:?}, delay={delay:?}",
        );
    }
}

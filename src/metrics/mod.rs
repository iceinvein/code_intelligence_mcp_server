use prometheus::{
    Counter, Gauge, Histogram, Registry,
};
use std::sync::Arc;

pub mod server;

pub use server::{spawn_metrics_server, MetricsState};

pub struct MetricsRegistry {
    pub registry: Registry,

    // Indexing metrics
    pub index_duration: Histogram,
    pub index_files_total: Counter,
    pub index_symbols_total: Counter,
    pub index_files_skipped: Counter,
    pub index_files_unchanged: Counter,
    pub index_cache_hits: Counter,
    pub index_cache_misses: Counter,

    // Search metrics
    pub search_duration: Histogram,
    pub search_results_total: Counter,
    pub search_errors_total: Counter,

    // Resource metrics
    pub index_size_bytes: Gauge,
    pub symbol_count: Gauge,
    pub cache_size_bytes: Gauge,
    pub cache_entries: Gauge,
}

impl MetricsRegistry {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        // Indexing duration histogram (1ms to 10 minutes)
        let index_duration = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "index_duration_seconds",
                "Indexing operation duration in seconds"
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 60.0, 300.0, 600.0])
        )?;

        let index_files_total = Counter::new(
            "index_files_total",
            "Total number of files indexed"
        )?;

        let index_symbols_total = Counter::new(
            "index_symbols_total",
            "Total number of symbols indexed"
        )?;

        let index_files_skipped = Counter::new(
            "index_files_skipped_total",
            "Total number of files skipped during indexing"
        )?;

        let index_files_unchanged = Counter::new(
            "index_files_unchanged_total",
            "Total number of unchanged files skipped"
        )?;

        let index_cache_hits = Counter::new(
            "index_cache_hits_total",
            "Total number of embedding cache hits"
        )?;

        let index_cache_misses = Counter::new(
            "index_cache_misses_total",
            "Total number of embedding cache misses"
        )?;

        // Search duration histogram (1ms to 5 seconds)
        let search_duration = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "search_duration_seconds",
                "Search query duration in seconds"
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0])
        )?;

        let search_results_total = Counter::new(
            "search_results_total",
            "Total number of search results returned"
        )?;

        let search_errors_total = Counter::new(
            "search_errors_total",
            "Total number of search errors"
        )?;

        // Resource gauges
        let index_size_bytes = Gauge::new(
            "index_size_bytes",
            "Current size of the index in bytes"
        )?;

        let symbol_count = Gauge::new(
            "symbol_count",
            "Current number of indexed symbols"
        )?;

        let cache_size_bytes = Gauge::new(
            "cache_size_bytes",
            "Current size of the embedding cache in bytes"
        )?;

        let cache_entries = Gauge::new(
            "cache_entries",
            "Current number of entries in the embedding cache"
        )?;

        // Register all metrics
        registry.register(Box::new(index_duration.clone()))?;
        registry.register(Box::new(index_files_total.clone()))?;
        registry.register(Box::new(index_symbols_total.clone()))?;
        registry.register(Box::new(index_files_skipped.clone()))?;
        registry.register(Box::new(index_files_unchanged.clone()))?;
        registry.register(Box::new(index_cache_hits.clone()))?;
        registry.register(Box::new(index_cache_misses.clone()))?;
        registry.register(Box::new(search_duration.clone()))?;
        registry.register(Box::new(search_results_total.clone()))?;
        registry.register(Box::new(search_errors_total.clone()))?;
        registry.register(Box::new(index_size_bytes.clone()))?;
        registry.register(Box::new(symbol_count.clone()))?;
        registry.register(Box::new(cache_size_bytes.clone()))?;
        registry.register(Box::new(cache_entries.clone()))?;

        Ok(Self {
            registry,
            index_duration,
            index_files_total,
            index_symbols_total,
            index_files_skipped,
            index_files_unchanged,
            index_cache_hits,
            index_cache_misses,
            search_duration,
            search_results_total,
            search_errors_total,
            index_size_bytes,
            symbol_count,
            cache_size_bytes,
            cache_entries,
        })
    }

    pub fn init(&self) -> Result<(), prometheus::Error> {
        // Initialize any default values
        Ok(())
    }
}

use prometheus::{
    Counter, Gauge, GaugeVec, Histogram, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry,
};
use std::time::Duration;

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
    pub query_operation_duration: HistogramVec,
    pub query_stage_duration: HistogramVec,
    pub query_candidates: HistogramVec,
    pub query_cache_events: IntCounterVec,
    pub query_cache_entries: GaugeVec,
    pub query_cache_bytes: GaugeVec,

    // Index stage and volume metrics
    pub index_stage_duration: HistogramVec,
    pub index_stage_items: HistogramVec,

    // Resource metrics
    pub index_size_bytes: Gauge,
    pub symbol_count: Gauge,
    pub cache_size_bytes: Gauge,
    pub cache_entries: Gauge,
    pub storage_bytes: GaugeVec,
    pub index_entities: GaugeVec,
    pub index_ratios: GaugeVec,
    pub process_peak_rss_bytes: Gauge,
    pub gpu_model_resident: GaugeVec,
}

impl MetricsRegistry {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        // Indexing duration histogram (1ms to 10 minutes)
        let index_duration = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "index_duration_seconds",
                "Indexing operation duration in seconds",
            )
            .buckets(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 60.0, 300.0,
                600.0,
            ]),
        )?;

        let index_files_total = Counter::new("index_files_total", "Total number of files indexed")?;

        let index_symbols_total =
            Counter::new("index_symbols_total", "Total number of symbols indexed")?;

        let index_files_skipped = Counter::new(
            "index_files_skipped_total",
            "Total number of files skipped during indexing",
        )?;

        let index_files_unchanged = Counter::new(
            "index_files_unchanged_total",
            "Total number of unchanged files skipped",
        )?;

        let index_cache_hits = Counter::new(
            "index_cache_hits_total",
            "Total number of embedding cache hits",
        )?;

        let index_cache_misses = Counter::new(
            "index_cache_misses_total",
            "Total number of embedding cache misses",
        )?;

        // Search duration histogram (1ms to 5 seconds)
        let search_duration = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "search_duration_seconds",
                "Search query duration in seconds",
            )
            .buckets(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
            ]),
        )?;

        let search_results_total = Counter::new(
            "search_results_total",
            "Total number of search results returned",
        )?;

        let search_errors_total =
            Counter::new("search_errors_total", "Total number of search errors")?;

        let query_operation_duration = HistogramVec::new(
            HistogramOpts::new(
                "code_intelligence_query_operation_duration_seconds",
                "End-to-end MCP query operation latency by stable operation and outcome",
            )
            .buckets(query_duration_buckets()),
            &["operation", "outcome"],
        )?;
        let query_stage_duration = HistogramVec::new(
            HistogramOpts::new(
                "code_intelligence_query_stage_duration_seconds",
                "Query latency by stable operation and internal stage",
            )
            .buckets(query_duration_buckets()),
            &["operation", "stage"],
        )?;
        let query_candidates = HistogramVec::new(
            HistogramOpts::new(
                "code_intelligence_query_candidates",
                "Candidate/result volume by stable operation and stage",
            )
            .buckets(vec![
                0.0, 1.0, 2.0, 5.0, 10.0, 20.0, 40.0, 80.0, 160.0, 320.0, 640.0, 1_280.0, 5_000.0,
            ]),
            &["operation", "stage"],
        )?;
        let query_cache_events = IntCounterVec::new(
            Opts::new(
                "code_intelligence_query_cache_events_total",
                "Query-cache events by cache and event (hit, miss, invalidation, wait)",
            ),
            &["cache", "event"],
        )?;
        let query_cache_entries = GaugeVec::new(
            Opts::new(
                "code_intelligence_query_cache_entries",
                "Current query-cache entry count by cache",
            ),
            &["cache"],
        )?;
        let query_cache_bytes = GaugeVec::new(
            Opts::new(
                "code_intelligence_query_cache_bytes",
                "Estimated query-cache memory use by cache",
            ),
            &["cache"],
        )?;

        let index_stage_duration = HistogramVec::new(
            HistogramOpts::new(
                "code_intelligence_index_stage_duration_seconds",
                "Indexing latency by stable pipeline stage",
            )
            .buckets(index_duration_buckets()),
            &["stage"],
        )?;
        let index_stage_items = HistogramVec::new(
            HistogramOpts::new(
                "code_intelligence_index_stage_items",
                "Per-run indexing volume by stable pipeline stage/entity",
            )
            .buckets(vec![
                0.0, 1.0, 10.0, 50.0, 100.0, 500.0, 1_000.0, 5_000.0, 10_000.0, 50_000.0, 250_000.0,
            ]),
            &["stage"],
        )?;

        // Resource gauges
        let index_size_bytes =
            Gauge::new("index_size_bytes", "Current size of the index in bytes")?;

        let symbol_count = Gauge::new("symbol_count", "Current number of indexed symbols")?;

        let cache_size_bytes = Gauge::new(
            "cache_size_bytes",
            "Current size of the embedding cache in bytes",
        )?;

        let cache_entries = Gauge::new(
            "cache_entries",
            "Current number of entries in the embedding cache",
        )?;
        let storage_bytes = GaugeVec::new(
            Opts::new(
                "code_intelligence_storage_bytes",
                "Current on-disk bytes by storage component",
            ),
            &["component"],
        )?;
        let index_entities = GaugeVec::new(
            Opts::new(
                "code_intelligence_index_entities",
                "Current indexed entity count by entity type",
            ),
            &["entity"],
        )?;
        let index_ratios = GaugeVec::new(
            Opts::new(
                "code_intelligence_index_ratio",
                "Current derived storage/index ratios",
            ),
            &["ratio"],
        )?;
        let process_peak_rss_bytes = Gauge::new(
            "code_intelligence_process_peak_rss_bytes",
            "Peak resident set size for this server process in bytes",
        )?;
        let gpu_model_resident = GaugeVec::new(
            Opts::new(
                "code_intelligence_gpu_model_resident",
                "Whether an optional Metal-backed model is currently resident",
            ),
            &["model"],
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
        registry.register(Box::new(query_operation_duration.clone()))?;
        registry.register(Box::new(query_stage_duration.clone()))?;
        registry.register(Box::new(query_candidates.clone()))?;
        registry.register(Box::new(query_cache_events.clone()))?;
        registry.register(Box::new(query_cache_entries.clone()))?;
        registry.register(Box::new(query_cache_bytes.clone()))?;
        registry.register(Box::new(index_stage_duration.clone()))?;
        registry.register(Box::new(index_stage_items.clone()))?;
        registry.register(Box::new(index_size_bytes.clone()))?;
        registry.register(Box::new(symbol_count.clone()))?;
        registry.register(Box::new(cache_size_bytes.clone()))?;
        registry.register(Box::new(cache_entries.clone()))?;
        registry.register(Box::new(storage_bytes.clone()))?;
        registry.register(Box::new(index_entities.clone()))?;
        registry.register(Box::new(index_ratios.clone()))?;
        registry.register(Box::new(process_peak_rss_bytes.clone()))?;
        registry.register(Box::new(gpu_model_resident.clone()))?;

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
            query_operation_duration,
            query_stage_duration,
            query_candidates,
            query_cache_events,
            query_cache_entries,
            query_cache_bytes,
            index_stage_duration,
            index_stage_items,
            index_size_bytes,
            symbol_count,
            cache_size_bytes,
            cache_entries,
            storage_bytes,
            index_entities,
            index_ratios,
            process_peak_rss_bytes,
            gpu_model_resident,
        })
    }

    pub fn init(&self) -> Result<(), prometheus::Error> {
        // Initialize any default values
        Ok(())
    }

    pub fn observe_query_operation(&self, operation: &str, outcome: &str, elapsed: Duration) {
        self.query_operation_duration
            .with_label_values(&[operation, outcome])
            .observe(elapsed.as_secs_f64());
        self.refresh_process_peak_rss();
    }

    pub fn observe_query_stage(&self, operation: &str, stage: &str, elapsed: Duration) {
        self.query_stage_duration
            .with_label_values(&[operation, stage])
            .observe(elapsed.as_secs_f64());
    }

    pub fn observe_query_candidates(&self, operation: &str, stage: &str, count: usize) {
        self.query_candidates
            .with_label_values(&[operation, stage])
            .observe(count as f64);
    }

    pub fn record_query_cache_event(&self, cache: &str, event: &str) {
        self.query_cache_events
            .with_label_values(&[cache, event])
            .inc();
    }

    pub fn set_query_cache_usage(&self, cache: &str, entries: usize, bytes: usize) {
        self.query_cache_entries
            .with_label_values(&[cache])
            .set(entries as f64);
        self.query_cache_bytes
            .with_label_values(&[cache])
            .set(bytes as f64);
    }

    pub fn observe_index_stage(&self, stage: &str, elapsed: Duration) {
        self.index_stage_duration
            .with_label_values(&[stage])
            .observe(elapsed.as_secs_f64());
    }

    pub fn observe_index_items(&self, stage: &str, count: usize) {
        self.index_stage_items
            .with_label_values(&[stage])
            .observe(count as f64);
    }

    pub fn refresh_process_peak_rss(&self) {
        if let Some(bytes) = process_peak_rss_bytes() {
            self.process_peak_rss_bytes.set(bytes as f64);
        }
    }
}

fn query_duration_buckets() -> Vec<f64> {
    vec![
        0.000_1, 0.000_5, 0.001, 0.002_5, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
        10.0, 30.0,
    ]
}

fn index_duration_buckets() -> Vec<f64> {
    vec![
        0.000_1, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
        600.0,
    ]
}

/// The project is macOS-only. On macOS `ru_maxrss` is reported in bytes
/// (unlike Linux, where it is KiB), so no platform conversion is needed.
fn process_peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the provided rusage on success and the
    // pointer is valid for the duration of the call.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status != 0 {
        return None;
    }
    // SAFETY: status == 0 means getrusage initialized the structure.
    let usage = unsafe { usage.assume_init() };
    u64::try_from(usage.ru_maxrss).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_cache_and_volume_metrics_have_stable_labels() {
        let metrics = MetricsRegistry::new().unwrap();
        metrics.observe_query_operation("search_code", "ok", Duration::from_millis(12));
        metrics.observe_query_stage("search", "keyword", Duration::from_millis(3));
        metrics.observe_query_candidates("search", "fused", 17);
        metrics.record_query_cache_event("response", "hit");
        metrics.observe_index_stage("parse", Duration::from_millis(7));
        metrics.observe_index_items("symbols", 42);

        let text = prometheus::TextEncoder::new()
            .encode_to_string(&metrics.registry.gather())
            .unwrap();
        assert!(text.contains("operation=\"search_code\",outcome=\"ok\""));
        assert!(text.contains("operation=\"search\",stage=\"keyword\""));
        assert!(text.contains("cache=\"response\",event=\"hit\""));
        assert!(text.contains("stage=\"parse\""));
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct IndexRunStats {
    pub files_scanned: usize,
    pub files_indexed: usize,
    pub symbols_indexed: usize,
    pub files_skipped: usize,
    pub files_unchanged: usize,
    pub files_deleted: usize,
    pub scan_ms: u64,
    pub cleanup_ms: u64,
    pub parse_ms: u64,
    pub sqlite_write_ms: u64,
    pub tantivy_ms: u64,
    pub binding_ms: u64,
    pub edge_ms: u64,
    pub embedding_ms: u64,
    pub vector_write_ms: u64,
    pub pagerank_ms: u64,
    pub optimize_ms: u64,
    pub embeddings_generated: usize,
    pub embeddings_skipped: usize,
    pub embedding_cache_hits: usize,
    pub embedding_cache_misses: usize,
}

#[derive(Debug, Clone, Default)]
pub struct EmbeddingRunStats {
    pub embedded: usize,
    pub skipped: usize,
    pub embedding_ms: u64,
    pub vector_write_ms: u64,
    pub cache_hits: usize,
    pub cache_misses: usize,
}

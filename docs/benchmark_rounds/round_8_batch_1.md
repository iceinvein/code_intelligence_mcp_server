# Round 8 - Batch 1

| # | Query | CI | Augment | Winner | Pattern |
|---|-------|-----|---------|--------|---------|
| 1 | Ranking and scoring system | 8 | 9 | Augment | -- |
| 2 | Embeddings generated and stored | 7 | 9 | Augment | Missing storage layer |
| 3 | Tree-sitter parsing | 5 | 9 | Augment | Keyword mismatch |
| 4 | Config from env vars | 7 | 9 | Augment | Missing Config struct |
| 5 | Indexing pipeline and symbol extraction | 5 | 9 | Augment | Definition bias |

## Per-Query Notes

### Q1: "How does the ranking and scoring system work?"
- **CI top-3:** reranker.rs:should_rerank, rrf.rs:reciprocal_rank_fusion, score.rs:apply_popularity_boost_with_signals
- **Augment top-3:** score.rs (rank_hits_with_signals + learning/popularity), ranking/mod.rs, rrf.rs
- **CI miss:** `rank_hits_with_signals` (the core scoring function) not in top results; `should_rerank` is a trivial helper ranked #1
- **CI hit:** Good file diversity across all 4 ranking files; rrf and score.rs both present

### Q2: "How are embeddings generated and stored?"
- **CI top-3:** embeddings/mod.rs:create_embedder, hash.rs:HashEmbedder, fastembed.rs:FastEmbedder
- **Augment top-3:** embeddings/mod.rs, fastembed.rs, hash.rs + vector.rs + cache.rs + pipeline/parallel.rs
- **CI miss:** `storage/vector.rs` (LanceDB storage) and `storage/cache.rs` (embedding cache) completely absent; query asks about "stored" but CI only returned generation side
- **CI hit:** Correct embedding generation files with relevant symbols (Embedder trait, create_embedder factory)

### Q3: "How does tree-sitter parsing work in this codebase?"
- **CI top-3:** package/parsers/go.rs:parse_go_mod, parser.rs:language_for_id, pipeline/parsing.rs:extract_usage_line
- **Augment top-3:** extract/typescript.rs, extract/go.rs, extract/c.rs + parser.rs + extract/rust.rs
- **CI miss:** No language extractors (typescript.rs, rust.rs, go.rs) in results; `parse_go_mod` (#1) is a Go module manifest parser, not tree-sitter parsing
- **CI hit:** parser.rs:language_for_id is relevant but ranked #2; pipeline/parsing.rs helpers are tangentially related

### Q4: "Configuration from environment variables"
- **CI top-3:** config.rs:optional_env, config.rs:to_utf8_pathbuf, config.rs:get_global_cimcp_dir
- **Augment top-3:** config.rs (Config struct, from_env, EmbeddingsBackend/Device enums, all env var reads), main.rs
- **CI miss:** `Config::from_env()` (the main entry point) and `Config` struct definition not returned; only helper functions surfaced
- **CI hit:** Correctly identified config.rs as the relevant file; all results from the right module

### Q5: "Indexing pipeline file scanning and symbol extraction"
- **CI top-3:** pipeline/edges.rs:symbol (test helper), extract/mod.rs:symbol (re-export), extract/symbol.rs:ExtractedSymbol
- **Augment top-3:** pipeline/mod.rs (index_all, scan_files usage), pipeline/scan.rs, pipeline/parallel.rs + extract/rust.rs + extract/symbol.rs
- **CI miss:** `pipeline/mod.rs:index_all` (main pipeline), `pipeline/scan.rs` (file scanning), `pipeline/parallel.rs` (parallel indexing) all absent
- **CI hit:** ExtractedSymbol and SymbolKind types are relevant but are data types, not the pipeline logic itself

//! PageRank algorithm for computing symbol importance scores

use crate::{
    config::Config,
    storage::sqlite::{SymbolMetricsRow, SqliteStore},
};
use anyhow::{Context, Result};
use std::collections::HashMap;

/// Compute and store PageRank scores for all symbols in the graph.
///
/// This function implements the iterative PageRank algorithm to compute
/// importance scores based on the graph structure of symbol references.
///
/// # Arguments
/// * `sqlite` - SQLite store for reading edges and writing metrics
/// * `config` - Configuration containing damping factor and iteration count
///
/// # Algorithm
/// 1. Load all symbol IDs and filter out kind="file" (FILE_ROOT exclusion)
/// 2. Load all edges to build adjacency list
/// 3. Initialize uniform scores (1.0 / num_symbols)
/// 4. Iterate for `config.pagerank_iterations`:
///    - base_score = (1 - damping) / num_symbols
///    - For each symbol: score = base_score + sum(damping * PR(from) / out_degree(from))
/// 5. Store results in symbol_metrics table
///
/// # Returns
/// Ok(()) on success, or error if database operations fail
pub fn compute_and_store_pagerank(sqlite: &SqliteStore, config: &Config) -> Result<()> {
    // Load all symbols with their kinds
    let all_symbols = sqlite
        .list_all_symbol_ids()
        .context("Failed to load symbol IDs")?;

    // Filter out FILE_ROOT symbols (kind="file") to avoid skew
    let symbols: Vec<(String, String)> = all_symbols
        .into_iter()
        .filter(|(_, kind)| kind != "file")
        .collect();

    let num_symbols = symbols.len();

    // Early return for empty graph
    if num_symbols == 0 {
        tracing::debug!("No symbols to rank (empty graph after filtering FILE_ROOT)");
        return Ok(());
    }

    // Create a set of valid symbol IDs for edge filtering
    let valid_symbol_ids: std::collections::HashSet<String> =
        symbols.iter().map(|(id, _)| id.clone()).collect();

    // Load all edges and build adjacency list
    let edges = sqlite
        .list_all_edges()
        .context("Failed to load edges")?;

    // Build adjacency list: symbol_id -> Vec<target_symbol_id>
    // Only include edges where both endpoints are valid (non-file) symbols
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();

    for (from_id, to_id) in edges {
        // Only include edges between valid symbols
        if valid_symbol_ids.contains(&from_id) && valid_symbol_ids.contains(&to_id) {
            adjacency
                .entry(from_id.clone())
                .or_default()
                .push(to_id);
        }
    }

    // Initialize PageRank scores uniformly
    let initial_score = 1.0 / num_symbols as f64;
    let mut scores: HashMap<String, f64> = symbols
        .iter()
        .map(|(id, _)| (id.clone(), initial_score))
        .collect();

    // Power iteration
    let damping = config.pagerank_damping as f64;
    let iterations = config.pagerank_iterations;

    let base_score = (1.0 - damping) / num_symbols as f64;

    for iteration in 0..iterations {
        let mut new_scores: HashMap<String, f64> = HashMap::new();

        // Initialize all scores with base_score
        for (id, _) in &symbols {
            new_scores.insert(id.clone(), base_score);
        }

        // Add contribution from incoming edges
        // We iterate over all symbols and add contributions from their outgoing edges
        for (from_id, _) in &symbols {
            let from_score = *scores.get(from_id).unwrap_or(&0.0);

            if let Some(outgoing) = adjacency.get(from_id) {
                let out_degree = outgoing.len() as f64;
                if out_degree > 0.0 {
                    let contribution = (damping * from_score) / out_degree;
                    for to_id in outgoing {
                        *new_scores.entry(to_id.clone()).or_insert(base_score) += contribution;
                    }
                }
            }
        }

        scores = new_scores;

        if iteration == 0 || iteration + 1 == iterations {
            let total_score: f64 = scores.values().sum();
            tracing::debug!(
                iteration = iteration + 1,
                total_iterations = iterations,
                num_symbols = num_symbols,
                total_score = total_score,
                avg_score = total_score / num_symbols as f64,
                "PageRank iteration complete"
            );
        }
    }

    // Store results
    for (symbol_id, pagerank) in &scores {
        let metrics = SymbolMetricsRow {
            symbol_id: symbol_id.clone(),
            pagerank: *pagerank,
            in_degree: 0, // Will be computed separately if needed
            out_degree: 0,
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        };

        sqlite
            .upsert_symbol_metrics(&metrics)
            .with_context(|| format!("Failed to store PageRank for {}", symbol_id))?;
    }

    tracing::info!(
        num_symbols = num_symbols,
        damping = config.pagerank_damping,
        iterations = iterations,
        "PageRank computation complete"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_store() -> SqliteStore {
        let conn = Connection::open_in_memory().unwrap();
        let sqlite = SqliteStore::from_connection(conn);
        sqlite.init().unwrap();
        sqlite
    }

    fn create_test_config() -> Config {
        Config {
            base_dir: "/tmp/test".into(),
            db_path: "/tmp/test.db".into(),
            vector_db_path: "/tmp/vectors".into(),
            tantivy_index_path: "/tmp/tantivy".into(),
            embeddings_backend: crate::config::EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_model_url: None,
            embeddings_model_sha256: None,
            embeddings_auto_download: false,
            embeddings_model_repo: None,
            embeddings_model_revision: None,
            embeddings_model_hf_token: None,
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 64,
            vector_search_limit: 20,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 0.1,
            rank_index_file_boost: 0.05,
            rank_test_penalty: 0.1,
            rank_popularity_weight: 0.05,
            rank_popularity_cap: 50,
            index_patterns: vec!["**/*.ts".to_string()],
            exclude_patterns: vec!["**/node_modules/**".to_string()],
            watch_mode: false,
            watch_debounce_ms: 250,
            max_context_bytes: 200_000,
            index_node_modules: false,
            repo_roots: vec!["/tmp/test".into()],
            reranker_model_path: None,
            reranker_top_k: 20,
            reranker_cache_dir: None,
            learning_enabled: false,
            learning_selection_boost: 0.1,
            learning_file_affinity_boost: 0.05,
            max_context_tokens: 8192,
            token_encoding: "o200k_base".to_string(),
            parallel_workers: 1,
            embedding_cache_enabled: true,
            pagerank_damping: 0.85,
            pagerank_iterations: 20,
            synonym_expansion_enabled: true,
            acronym_expansion_enabled: true,
            rrf_enabled: true,
            rrf_k: 60.0,
            rrf_keyword_weight: 1.0,
            rrf_vector_weight: 1.0,
            rrf_graph_weight: 0.5,
            hyde_enabled: false,
            hyde_llm_backend: "openai".to_string(),
            hyde_api_key: None,
            hyde_max_tokens: 512,
            metrics_enabled: true,
            metrics_port: 9090,
            package_detection_enabled: true,
        }
    }

    #[test]
    fn empty_graph_returns_ok() {
        let sqlite = setup_test_store();
        let config = create_test_config();

        let result = compute_and_store_pagerank(&sqlite, &config);
        assert!(result.is_ok());
    }

    #[test]
    fn three_node_graph_produces_expected_scores() {
        let sqlite = setup_test_store();
        let config = create_test_config();

        // Create a simple 3-node graph: A -> B -> C
        // Using low iterations for deterministic test
        let mut test_config = config;
        test_config.pagerank_iterations = 5;
        test_config.pagerank_damping = 0.5; // Lower damping for easier verification

        // Add symbols
        use crate::storage::sqlite::SymbolRow;
        let symbols = vec![
            SymbolRow {
                id: "sym_a".to_string(),
                file_path: "test.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "A".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 10,
                start_line: 1,
                end_line: 1,
                text: "function A() {}".to_string(),
            },
            SymbolRow {
                id: "sym_b".to_string(),
                file_path: "test.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "B".to_string(),
                exported: true,
                start_byte: 10,
                end_byte: 20,
                start_line: 2,
                end_line: 2,
                text: "function B() {}".to_string(),
            },
            SymbolRow {
                id: "sym_c".to_string(),
                file_path: "test.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "C".to_string(),
                exported: true,
                start_byte: 20,
                end_byte: 30,
                start_line: 3,
                end_line: 3,
                text: "function C() {}".to_string(),
            },
        ];

        for sym in symbols {
            sqlite.upsert_symbol(&sym).unwrap();
        }

        // Add edges: A -> B, B -> C
        use crate::storage::sqlite::EdgeRow;
        let edges = vec![
            EdgeRow {
                from_symbol_id: "sym_a".to_string(),
                to_symbol_id: "sym_b".to_string(),
                edge_type: "call".to_string(),
                at_file: Some("test.ts".to_string()),
                at_line: Some(1),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            },
            EdgeRow {
                from_symbol_id: "sym_b".to_string(),
                to_symbol_id: "sym_c".to_string(),
                edge_type: "call".to_string(),
                at_file: Some("test.ts".to_string()),
                at_line: Some(2),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            },
        ];

        for edge in edges {
            sqlite.upsert_edge(&edge).unwrap();
        }

        // Compute PageRank
        compute_and_store_pagerank(&sqlite, &test_config).unwrap();

        // Verify scores were stored
        let metrics_a = sqlite
            .get_symbol_metrics("sym_a")
            .unwrap()
            .expect("sym_a should have metrics");
        let metrics_b = sqlite
            .get_symbol_metrics("sym_b")
            .unwrap()
            .expect("sym_b should have metrics");
        let metrics_c = sqlite
            .get_symbol_metrics("sym_c")
            .unwrap()
            .expect("sym_c should have metrics");

        // All scores should be positive
        assert!(metrics_a.pagerank > 0.0);
        assert!(metrics_b.pagerank > 0.0);
        assert!(metrics_c.pagerank > 0.0);

        // C should have highest score (receives from B, who receives from A)
        // In a chain A->B->C, C accumulates the most flow
        assert!(metrics_c.pagerank > metrics_b.pagerank);
        assert!(metrics_c.pagerank > metrics_a.pagerank);
    }

    #[test]
    fn file_root_symbols_excluded_from_pagerank() {
        let sqlite = setup_test_store();
        let config = create_test_config();

        // Add both file and function symbols
        use crate::storage::sqlite::SymbolRow;
        let symbols = vec![
            SymbolRow {
                id: "file_root".to_string(),
                file_path: "test.ts".to_string(),
                language: "typescript".to_string(),
                kind: "file".to_string(), // This should be excluded
                name: "test.ts".to_string(),
                exported: false,
                start_byte: 0,
                end_byte: 100,
                start_line: 1,
                end_line: 10,
                text: "function f() {}".to_string(),
            },
            SymbolRow {
                id: "sym_f".to_string(),
                file_path: "test.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "f".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 10,
                start_line: 1,
                end_line: 1,
                text: "function f() {}".to_string(),
            },
        ];

        for sym in symbols {
            sqlite.upsert_symbol(&sym).unwrap();
        }

        // Add edge from file_root to function (should be ignored)
        use crate::storage::sqlite::EdgeRow;
        sqlite
            .upsert_edge(&EdgeRow {
                from_symbol_id: "file_root".to_string(),
                to_symbol_id: "sym_f".to_string(),
                edge_type: "contains".to_string(),
                at_file: Some("test.ts".to_string()),
                at_line: Some(1),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();

        // Compute PageRank
        compute_and_store_pagerank(&sqlite, &config).unwrap();

        // File root should not have metrics
        let file_metrics = sqlite.get_symbol_metrics("file_root").unwrap();
        assert!(file_metrics.is_none(), "FILE_ROOT symbols should not have PageRank");

        // Function should have metrics
        let func_metrics = sqlite
            .get_symbol_metrics("sym_f")
            .unwrap()
            .expect("function symbol should have metrics");
        assert!(func_metrics.pagerank > 0.0);
    }
}

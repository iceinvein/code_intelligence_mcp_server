//! Package-aware ranking for search results.
//!
//! This module provides functionality to boost search results based on package
//! context. Results from the same package as the query context receive higher
//! scores, with the boost magnitude varying by search intent.

use crate::config::Config;
use crate::retrieval::query::Intent;
use crate::retrieval::{HitSignals, RankedHit};
use crate::storage::sqlite::SqliteStore;
use anyhow::Result;
use std::collections::HashMap;

/// Apply package boost with signals tracking.
///
/// This function boosts search result scores for symbols that belong to the
/// same package as the query context. The boost is intent-aware:
///
/// - **Navigation intents** (Definition, Implementation, Reference): 1.2x boost
/// - **Generic intents** (Error): 1.1x boost
/// - **Other intents**: 1.15x boost
///
/// # Algorithm
///
/// 1. Determine query context package:
///    - If `query_package_id` is provided, use it
///    - Otherwise auto-detect from the first hit's file_path
/// 2. Collect all symbol IDs from hits
/// 3. Batch lookup package_id for all hits using `batch_get_symbol_packages`
/// 4. For each hit where hit_package_id == query_package_id:
///    - Apply intent-based multiplier: hit.score * boost
///    - Update hit_signals[id].package_boost = boost_amount
/// 5. Re-sort hits by updated score
/// 6. Return re-sorted hits
///
/// # Arguments
///
/// * `sqlite` - SQLite storage for package lookups
/// * `hits` - Search hits to rank
/// * `hit_signals` - Mutable signals tracking (HashMap of symbol_id to signals)
/// * `query_package_id` - Optional query context package ID
/// * `config` - Configuration (not currently used, for future extensibility)
/// * `intent` - Search intent for boost magnitude calculation
///
/// # Returns
///
/// Re-sorted hits with same-package boost applied.
pub fn apply_package_boost_with_signals(
    sqlite: &SqliteStore,
    mut hits: Vec<RankedHit>,
    hit_signals: &mut HashMap<String, HitSignals>,
    query_package_id: Option<&str>,
    _config: &Config,
    intent: Intent,
) -> Result<Vec<RankedHit>> {
    if hits.is_empty() {
        return Ok(hits);
    }

    // Step 1: Determine query context package
    let query_pkg = if let Some(pkg_id) = query_package_id {
        pkg_id.to_string()
    } else {
        // Auto-detect from first hit's file_path
        let first_path = &hits[0].file_path;
        match sqlite.get_package_id_for_file(first_path) {
            Ok(Some(pkg_id)) => pkg_id,
            Ok(None) => return Ok(hits), // No package found, no boost possible
            Err(_) => return Ok(hits), // Error looking up package, skip boost
        }
    };

    // Step 2: Collect all symbol IDs from hits
    let symbol_ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();

    // Step 3: Batch lookup package_id for all hits
    let package_map = sqlite
        .batch_get_symbol_packages(&symbol_ids)
        .unwrap_or_default();

    // If no packages found, return early
    if package_map.is_empty() {
        return Ok(hits);
    }

    // Step 4: Determine boost multiplier based on intent
    let boost_multiplier = intent_boost_multiplier(&intent);

    // Step 5: Apply boost to same-package hits
    for hit in hits.iter_mut() {
        if let Some(hit_package_id) = package_map.get(&hit.id) {
            if hit_package_id == &query_pkg {
                // Apply boost: score * multiplier
                let boost_amount = hit.score * (boost_multiplier - 1.0);
                hit.score *= boost_multiplier;

                // Update signals
                hit_signals
                    .entry(hit.id.clone())
                    .and_modify(|s| s.package_boost += boost_amount)
                    .or_insert(HitSignals {
                        keyword_score: 0.0,
                        vector_score: 0.0,
                        base_score: 0.0,
                        structural_adjust: 0.0,
                        intent_mult: 1.0,
                        definition_bias: 0.0,
                        popularity_boost: 0.0,
                        learning_boost: 0.0,
                        affinity_boost: 0.0,
                        docstring_boost: 0.0,
                        package_boost: boost_amount,
                    });
            }
        }
    }

    // Step 6: Re-sort by score after applying boosts
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.exported.cmp(&a.exported))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.file_path.cmp(&b.file_path))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(hits)
}

/// Calculate boost multiplier based on search intent.
///
/// Higher boost for navigation intents where staying in the same package
/// is more valuable (e.g., finding definitions in the same package).
///
/// # Arguments
///
/// * `intent` - The search intent
///
/// # Returns
///
/// Boost multiplier (1.0 = no boost, >1.0 = boost applied)
fn intent_boost_multiplier(intent: &Intent) -> f32 {
    match intent {
        // Navigation intents - higher boost for same-package results
        Intent::Definition | Intent::Implementation | Intent::Callers(_) => 1.2,

        // Generic intents - moderate boost
        Intent::Error => 1.1,

        // Other intents - default moderate boost
        _ => 1.15,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EmbeddingsBackend, EmbeddingsDevice};
    use crate::storage::sqlite::schema::{PackageRow, RepositoryRow, SymbolRow};
    use std::path::PathBuf;

    fn test_config() -> Config {
        Config {
            base_dir: PathBuf::from("/tmp/test"),
            db_path: PathBuf::from("/tmp/test.db"),
            vector_db_path: PathBuf::from("/tmp/vectors"),
            tantivy_index_path: PathBuf::from("/tmp/tantivy"),
            embeddings_backend: EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_model_url: None,
            embeddings_model_sha256: None,
            embeddings_auto_download: false,
            embeddings_model_repo: None,
            embeddings_model_revision: None,
            embeddings_model_hf_token: None,
            embeddings_device: EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 64,
            vector_search_limit: 20,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.5,
            rank_keyword_weight: 0.5,
            rank_exported_boost: 0.0,
            rank_index_file_boost: 0.0,
            rank_test_penalty: 0.1,
            rank_popularity_weight: 0.1,
            rank_popularity_cap: 0,
            index_patterns: vec!["**/*.ts".to_string()],
            exclude_patterns: vec!["**/node_modules/**".to_string()],
            watch_mode: false,
            watch_debounce_ms: 250,
            max_context_bytes: 200_000,
            index_node_modules: false,
            repo_roots: vec![],
            reranker_model_path: None,
            reranker_top_k: 5,
            reranker_cache_dir: None,
            learning_enabled: false,
            learning_selection_boost: 0.0,
            learning_file_affinity_boost: 0.0,
            max_context_tokens: 8000,
            token_encoding: "cl100k_base".to_string(),
            parallel_workers: 4,
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

    fn make_hit(id: &str, name: &str, file_path: &str, score: f32) -> RankedHit {
        RankedHit {
            id: id.to_string(),
            score,
            name: name.to_string(),
            kind: "function".to_string(),
            file_path: file_path.to_string(),
            exported: true,
            language: "typescript".to_string(),
        }
    }

    fn make_signals() -> HashMap<String, HitSignals> {
        HashMap::new()
    }

    #[test]
    fn test_same_package_boost_prioritizes_same_package() {
        let db_path = std::env::temp_dir().join("test_same_package_boost.db");
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            // Create repository
            let repo = RepositoryRow {
                id: "repo-123".to_string(),
                name: "test-repo".to_string(),
                root_path: "/path/to/repo".to_string(),
                vcs_type: Some("git".to_string()),
                remote_url: None,
                created_at: 1234567890,
            };
            sqlite.upsert_repository(&repo).unwrap();

            // Create two packages
            let pkg_a = PackageRow {
                id: "pkg-a".to_string(),
                repository_id: "repo-123".to_string(),
                name: "package-a".to_string(),
                version: Some("1.0.0".to_string()),
                manifest_path: "/path/to/repo/packages/a".to_string(),
                package_type: "npm".to_string(),
                created_at: 1234567891,
            };
            sqlite.upsert_package(&pkg_a).unwrap();

            let pkg_b = PackageRow {
                id: "pkg-b".to_string(),
                repository_id: "repo-123".to_string(),
                name: "package-b".to_string(),
                version: Some("1.0.0".to_string()),
                manifest_path: "/path/to/repo/packages/b".to_string(),
                package_type: "npm".to_string(),
                created_at: 1234567892,
            };
            sqlite.upsert_package(&pkg_b).unwrap();

            // Create symbols in each package
            let symbol_a = SymbolRow {
                id: "symbol-a".to_string(),
                file_path: "/path/to/repo/packages/a/src/util.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "helper".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 100,
                start_line: 1,
                end_line: 10,
                text: "export function helper() {}".to_string(),
            };
            sqlite.upsert_symbol(&symbol_a).unwrap();

            let symbol_b = SymbolRow {
                id: "symbol-b".to_string(),
                file_path: "/path/to/repo/packages/b/src/util.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "helper".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 100,
                start_line: 1,
                end_line: 10,
                text: "export function helper() {}".to_string(),
            };
            sqlite.upsert_symbol(&symbol_b).unwrap();

            // Create hits with equal initial scores
            let hits = vec![
                make_hit("symbol-a", "helper", "/path/to/repo/packages/a/src/util.ts", 10.0),
                make_hit("symbol-b", "helper", "/path/to/repo/packages/b/src/util.ts", 10.0),
            ];

            let mut hit_signals = make_signals();
            let config = test_config();

            // Apply package boost with query from pkg-a (using package ID, not name)
            let result = apply_package_boost_with_signals(
                &sqlite,
                hits,
                &mut hit_signals,
                Some("pkg-a"),
                &config,
                Intent::Definition,
            )
            .unwrap();

            // symbol-a (same package) should rank higher than symbol-b
            assert_eq!(result[0].id, "symbol-a");
            assert_eq!(result[1].id, "symbol-b");
            assert!(result[0].score > result[1].score);

            // Check that package_boost was tracked
            assert!(hit_signals.get("symbol-a").unwrap().package_boost > 0.0);
            assert_eq!(hit_signals.get("symbol-b").unwrap().package_boost, 0.0);
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_cross_package_no_boost_applied() {
        let db_path = std::env::temp_dir().join("test_cross_package.db");
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            // Create repository
            let repo = RepositoryRow {
                id: "repo-123".to_string(),
                name: "test-repo".to_string(),
                root_path: "/path/to/repo".to_string(),
                vcs_type: Some("git".to_string()),
                remote_url: None,
                created_at: 1234567890,
            };
            sqlite.upsert_repository(&repo).unwrap();

            // Create package
            let pkg_a = PackageRow {
                id: "pkg-a".to_string(),
                repository_id: "repo-123".to_string(),
                name: "package-a".to_string(),
                version: Some("1.0.0".to_string()),
                manifest_path: "/path/to/repo/packages/a".to_string(),
                package_type: "npm".to_string(),
                created_at: 1234567891,
            };
            sqlite.upsert_package(&pkg_a).unwrap();

            // Create symbol in package-a
            let symbol_a = SymbolRow {
                id: "symbol-a".to_string(),
                file_path: "/path/to/repo/packages/a/src/util.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "helper".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 100,
                start_line: 1,
                end_line: 10,
                text: "export function helper() {}".to_string(),
            };
            sqlite.upsert_symbol(&symbol_a).unwrap();

            let hits = vec![make_hit(
                "symbol-a",
                "helper",
                "/path/to/repo/packages/a/src/util.ts",
                10.0,
            )];

            let mut hit_signals = make_signals();
            let config = test_config();

            // Apply package boost with query from a DIFFERENT package
            let result = apply_package_boost_with_signals(
                &sqlite,
                hits,
                &mut hit_signals,
                Some("pkg-b"), // Different package
                &config,
                Intent::Definition,
            )
            .unwrap();

            // No boost should be applied (different package)
            assert_eq!(result[0].score, 10.0);
            assert_eq!(hit_signals.get("symbol-a").unwrap().package_boost, 0.0);
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_intent_affects_boost_multiplier() {
        let db_path = std::env::temp_dir().join("test_intent_boost_multiplier.db");
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            // Create repository
            let repo = RepositoryRow {
                id: "repo-123".to_string(),
                name: "test-repo".to_string(),
                root_path: "/path/to/repo".to_string(),
                vcs_type: Some("git".to_string()),
                remote_url: None,
                created_at: 1234567890,
            };
            sqlite.upsert_repository(&repo).unwrap();

            // Create package
            let pkg_a = PackageRow {
                id: "pkg-a".to_string(),
                repository_id: "repo-123".to_string(),
                name: "package-a".to_string(),
                version: Some("1.0.0".to_string()),
                manifest_path: "/path/to/repo/packages/a".to_string(),
                package_type: "npm".to_string(),
                created_at: 1234567891,
            };
            sqlite.upsert_package(&pkg_a).unwrap();

            // Create symbol
            let symbol_a = SymbolRow {
                id: "symbol-a".to_string(),
                file_path: "/path/to/repo/packages/a/src/util.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "helper".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 100,
                start_line: 1,
                end_line: 10,
                text: "export function helper() {}".to_string(),
            };
            sqlite.upsert_symbol(&symbol_a).unwrap();

            let config = test_config();

            // Test Definition intent (1.2x boost)
            let hits1 = vec![make_hit(
                "symbol-a",
                "helper",
                "/path/to/repo/packages/a/src/util.ts",
                10.0,
            )];
            let mut signals1 = make_signals();

            let result1 = apply_package_boost_with_signals(
                &sqlite,
                hits1,
                &mut signals1,
                Some("pkg-a"),
                &config,
                Intent::Definition,
            )
            .unwrap();

            // Definition intent: 1.2x boost, so score should be 12.0
            assert!((result1[0].score - 12.0).abs() < 0.01);
            assert!((signals1.get("symbol-a").unwrap().package_boost - 2.0).abs() < 0.01); // 12.0 - 10.0

            // Test Error intent (1.1x boost)
            let hits2 = vec![make_hit(
                "symbol-a",
                "helper",
                "/path/to/repo/packages/a/src/util.ts",
                10.0,
            )];
            let mut signals2 = make_signals();

            let result2 = apply_package_boost_with_signals(
                &sqlite,
                hits2,
                &mut signals2,
                Some("pkg-a"),
                &config,
                Intent::Error,
            )
            .unwrap();

            // Error intent: 1.1x boost, so score should be 11.0
            assert!((result2[0].score - 11.0).abs() < 0.01);
            assert!((signals2.get("symbol-a").unwrap().package_boost - 1.0).abs() < 0.01); // 11.0 - 10.0
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_auto_detect_package_from_first_hit() {
        let db_path = std::env::temp_dir().join("test_auto_detect_package.db");
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            // Create repository
            let repo = RepositoryRow {
                id: "repo-123".to_string(),
                name: "test-repo".to_string(),
                root_path: "/path/to/repo".to_string(),
                vcs_type: Some("git".to_string()),
                remote_url: None,
                created_at: 1234567890,
            };
            sqlite.upsert_repository(&repo).unwrap();

            // Create package
            let pkg_a = PackageRow {
                id: "pkg-a".to_string(),
                repository_id: "repo-123".to_string(),
                name: "package-a".to_string(),
                version: Some("1.0.0".to_string()),
                manifest_path: "/path/to/repo/packages/a".to_string(),
                package_type: "npm".to_string(),
                created_at: 1234567891,
            };
            sqlite.upsert_package(&pkg_a).unwrap();

            // Create symbols
            let symbol_a = SymbolRow {
                id: "symbol-a".to_string(),
                file_path: "/path/to/repo/packages/a/src/util.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "helper".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 100,
                start_line: 1,
                end_line: 10,
                text: "export function helper() {}".to_string(),
            };
            sqlite.upsert_symbol(&symbol_a).unwrap();

            let symbol_a2 = SymbolRow {
                id: "symbol-a2".to_string(),
                file_path: "/path/to/repo/packages/a/src/other.ts".to_string(),
                language: "typescript".to_string(),
                kind: "function".to_string(),
                name: "helper2".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 100,
                start_line: 1,
                end_line: 10,
                text: "export function helper2() {}".to_string(),
            };
            sqlite.upsert_symbol(&symbol_a2).unwrap();

            // Hits from same package but no query_package_id provided
            let hits = vec![
                make_hit(
                    "symbol-a",
                    "helper",
                    "/path/to/repo/packages/a/src/util.ts",
                    10.0,
                ),
                make_hit(
                    "symbol-a2",
                    "helper2",
                    "/path/to/repo/packages/a/src/other.ts",
                    9.0,
                ),
            ];

            let mut hit_signals = make_signals();
            let config = test_config();

            // Apply package boost WITHOUT explicit query_package_id (auto-detect)
            let result = apply_package_boost_with_signals(
                &sqlite,
                hits,
                &mut hit_signals,
                None, // No query_package_id - auto-detect from first hit
                &config,
                Intent::Definition,
            )
            .unwrap();

            // Both hits are in the same package (auto-detected), so both get boost
            // First hit was already higher, stays first after boost
            assert_eq!(result[0].id, "symbol-a");
            assert_eq!(result[1].id, "symbol-a2");

            // Both should have package_boost applied
            assert!(hit_signals.get("symbol-a").unwrap().package_boost > 0.0);
            assert!(hit_signals.get("symbol-a2").unwrap().package_boost > 0.0);
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_empty_hits_returns_early() {
        let db_path = std::env::temp_dir().join("test_empty_hits_package.db");
        let _ = std::fs::remove_file(&db_path);

        {
            let sqlite = SqliteStore::open(&db_path).unwrap();
            sqlite.init().unwrap();

            let hits: Vec<RankedHit> = vec![];
            let mut hit_signals: HashMap<String, HitSignals> = HashMap::new();

            let config = test_config();
            let result = apply_package_boost_with_signals(
                &sqlite,
                hits,
                &mut hit_signals,
                Some("pkg-a"),
                &config,
                Intent::Definition,
            )
            .unwrap();

            assert!(result.is_empty());
            assert!(hit_signals.is_empty());
        }

        let _ = std::fs::remove_file(&db_path);
    }
}

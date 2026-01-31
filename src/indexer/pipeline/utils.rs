use crate::config::Config;
use crate::indexer::extract::symbol::Import;
use crate::indexer::parser::LanguageId;
use crate::storage::sqlite::SqliteStore;
use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy)]
pub struct FileFingerprint {
    pub mtime_ns: i64,
    pub size_bytes: u64,
}

pub fn file_fingerprint(path: &Path) -> Result<FileFingerprint> {
    let meta =
        fs::metadata(path).with_context(|| format!("Failed to stat file: {}", path.display()))?;

    let size_bytes = meta.len();
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);

    Ok(FileFingerprint {
        mtime_ns,
        size_bytes,
    })
}

pub fn unix_now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

pub fn file_key(config: &Config, path: &crate::path::Utf8Path) -> String {
    // PathNormalizer already normalizes separators, so we just use the relative path directly
    config.path_relative_to_base(path).unwrap_or_else(|_| path.to_string())
}

/// Legacy version of file_key that accepts &Path for compatibility
pub fn file_key_path(config: &Config, path: &Path) -> String {
    let utf8_path = crate::path::Utf8PathBuf::from_path_buf(path.to_path_buf())
        .unwrap_or_else(|_| crate::path::Utf8PathBuf::from(path.to_string_lossy().as_ref()));
    file_key(config, &utf8_path)
}

pub fn fnv1a_64(data: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;
    let mut hash = OFFSET;
    for b in data {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

pub fn stable_symbol_id(file_path: &str, name: &str, start_byte: u32) -> String {
    let mut data = Vec::with_capacity(file_path.len() + name.len() + 16);
    data.extend_from_slice(file_path.as_bytes());
    data.push(b':');
    data.extend_from_slice(name.as_bytes());
    data.push(b':');
    data.extend_from_slice(start_byte.to_string().as_bytes());
    format!("{:016x}", fnv1a_64(&data))
}

pub fn language_string(language_id: LanguageId) -> &'static str {
    match language_id {
        LanguageId::Typescript => "typescript",
        LanguageId::Tsx => "tsx",
        LanguageId::Rust => "rust",
        LanguageId::Python => "python",
        LanguageId::Go => "go",
        LanguageId::Java => "java",
        LanguageId::Javascript => "javascript",
        LanguageId::C => "c",
        LanguageId::Cpp => "cpp",
    }
}

pub fn cluster_key_from_vector(vector: &[f32]) -> String {
    let mut bits = 0u64;
    for (i, v) in vector.iter().take(64).enumerate() {
        if *v >= 0.0 {
            bits |= 1u64 << i;
        }
    }
    format!("{:016x}", bits)
}

pub fn resolve_imported_symbol_id(current_file_path: &str, imp: &Import) -> Option<String> {
    // Enhanced resolution: try to find the actual exported symbol in target file
    // Falls back to file-level ID if symbol-level lookup fails

    let target_path = resolve_path(current_file_path, &imp.source)?;

    // Try to find an exported symbol with matching name in the target file
    // For now, we use the file-level ID as fallback since we don't have SqliteStore access here
    // TODO: Pass SqliteStore when available for symbol-level lookup

    // The ID is stable_symbol_id(target_path, imp.name, 0) for exported symbols
    Some(stable_symbol_id(&target_path, &imp.name, 0))
}

/// Enhanced import resolution that queries the database for actual exported symbols
/// This should be used when SqliteStore is available for more accurate resolution
pub fn resolve_imported_symbol_id_with_db(
    current_file_path: &str,
    imp: &Import,
    sqlite: &SqliteStore,
) -> Option<String> {
    // First, try to resolve the path and find the symbol in the expected file
    if let Some(target_path) = resolve_path(current_file_path, &imp.source) {
        // Try to find an exported symbol with matching name in the target file
        if let Ok(results) = sqlite.search_symbols_by_exact_name(&imp.name, Some(&target_path), 1) {
            if let Some(symbol) = results.iter().find(|s| s.exported) {
                return Some(symbol.id.clone());
            }
        }

        // Try alternative file extensions (.tsx, .jsx, .js, index.ts, index.tsx)
        for alt_path in alternative_import_paths(&target_path) {
            if let Ok(results) = sqlite.search_symbols_by_exact_name(&imp.name, Some(&alt_path), 1)
            {
                if let Some(symbol) = results.iter().find(|s| s.exported) {
                    return Some(symbol.id.clone());
                }
            }
        }
    }

    // Fallback: search by name only across all files (prefer exported symbols)
    // This handles cases where path resolution is wrong (e.g., monorepo packages,
    // re-exports, or unusual directory structures)
    if let Ok(results) = sqlite.search_symbols_by_exact_name(&imp.name, None, 10) {
        // Prefer exported symbols - the query already orders by exported DESC
        if let Some(symbol) = results.iter().find(|s| s.exported) {
            return Some(symbol.id.clone());
        }
    }

    // Final fallback to generated ID (for symbols not yet indexed)
    let target_path = resolve_path(current_file_path, &imp.source)?;
    Some(stable_symbol_id(&target_path, &imp.name, 0))
}

/// Generate alternative import paths to try when the default resolution fails
fn alternative_import_paths(base_path: &str) -> Vec<String> {
    let mut alternatives = Vec::new();

    // If path ends with .ts, try .tsx
    if base_path.ends_with(".ts") {
        let tsx_path = format!("{}x", base_path);
        alternatives.push(tsx_path);

        // Also try index files in directory
        let dir_path = &base_path[..base_path.len() - 3];
        alternatives.push(format!("{}/index.ts", dir_path));
        alternatives.push(format!("{}/index.tsx", dir_path));
    }
    // If path ends with .tsx, try .ts
    else if base_path.ends_with(".tsx") {
        let ts_path = base_path[..base_path.len() - 1].to_string();
        alternatives.push(ts_path);
    }
    // If no extension (shouldn't happen with resolve_path), try common extensions
    else if !base_path.contains('.') {
        alternatives.push(format!("{}.ts", base_path));
        alternatives.push(format!("{}.tsx", base_path));
        alternatives.push(format!("{}/index.ts", base_path));
        alternatives.push(format!("{}/index.tsx", base_path));
    }

    alternatives
}

pub fn resolve_path(current: &str, source: &str) -> Option<String> {
    if !source.starts_with('.') {
        return None;
    }

    // Normalize slashes first
    let current = current.replace('\\', "/");
    let source = source.replace('\\', "/");

    let current_path = PathBuf::from(&current);
    let parent = current_path.parent()?;

    // Manual join to avoid ./ weirdness if possible or clean it after
    let joined = parent.join(&source);
    let joined_str = joined.to_string_lossy().replace('\\', "/");

    // Clean path (lexical normalization)
    let parts: Vec<&str> = joined_str.split('/').collect();
    let mut stack = Vec::new();

    for part in parts {
        if part == "." || part.is_empty() {
            continue;
        }
        if part == ".." {
            stack.pop();
        } else {
            stack.push(part);
        }
    }

    let mut s = stack.join("/");

    // Quick hack: just append .ts if missing extension
    if !s.ends_with(".ts") && !s.ends_with(".tsx") && !s.ends_with(".rs") {
        s.push_str(".ts"); // Bias towards TS
    }

    Some(s)
}

pub fn build_import_map(imports: &[Import]) -> HashMap<&str, &Import> {
    let mut map = HashMap::new();
    for imp in imports {
        if let Some(alias) = &imp.alias {
            map.insert(alias.as_str(), imp);
        } else {
            map.insert(imp.name.as_str(), imp);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Utf8PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("code-intel-pipeline-test-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn file_key_is_relative_under_base_and_absolute_outside() {
        let base0 = tmp_dir();
        let base = base0.canonicalize().unwrap_or(base0);
        let base_utf8 = Utf8PathBuf::from_path_buf(base.clone()).unwrap();
        let inner = base.join("src/a.ts");
        std::fs::create_dir_all(inner.parent().unwrap()).unwrap();
        std::fs::write(&inner, "export function a() {}").unwrap();

        let other0 = tmp_dir();
        let other = other0.canonicalize().unwrap_or(other0);
        let outside = other.join("b.ts");
        std::fs::write(&outside, "export function b() {}").unwrap();

        let config = Config {
            base_dir: base_utf8.clone(),
            db_path: base_utf8.join("code-intelligence.db"),
            vector_db_path: base_utf8.join("vectors"),
            tantivy_index_path: base_utf8.join("tantivy-index"),
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
            hash_embedding_dim: 8,
            vector_search_limit: 10,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 0.0,
            rank_index_file_boost: 0.0,
            rank_test_penalty: 0.0,
            rank_popularity_weight: 0.0,
            rank_popularity_cap: 0,
            index_patterns: vec![],
            exclude_patterns: vec![],
            watch_mode: false,
            watch_debounce_ms: 100,
            watch_min_index_interval_ms: 50,
            max_context_bytes: 10_000,
            index_node_modules: false,
            repo_roots: vec![base_utf8.clone()],
            // Reranker config (FNDN-03)
            reranker_model_path: None,
            reranker_top_k: 20,
            reranker_cache_dir: None,
            // Learning config (FNDN-04)
            learning_enabled: false,
            learning_selection_boost: 0.1,
            learning_file_affinity_boost: 0.05,
            // Token config (FNDN-05)
            max_context_tokens: 8192,
            token_encoding: "o200k_base".to_string(),
            // Performance config (FNDN-06)
            parallel_workers: 4,
            embedding_cache_enabled: true,
            embedding_max_threads: 0,
            // PageRank config (FNDN-07)
            pagerank_damping: 0.85,
            pagerank_iterations: 20,
            // Query expansion config (FNDN-02)
            synonym_expansion_enabled: true,
            acronym_expansion_enabled: true,
            // RRF config (RETR-05)
            rrf_enabled: true,
            rrf_k: 60.0,
            rrf_keyword_weight: 1.0,
            rrf_vector_weight: 1.0,
            rrf_graph_weight: 0.5,
            // HyDE config (RETR-06, RETR-07)
            hyde_enabled: false,
            hyde_llm_backend: "openai".to_string(),
            hyde_api_key: None,
            hyde_max_tokens: 512,
            // Metrics config (PERF-04)
            metrics_enabled: true,
            metrics_port: 9090,
            package_detection_enabled: true,
        };

        let k1 = file_key_path(&config, &inner);
        assert_eq!(k1, "src/a.ts");

        let k2 = file_key_path(&config, &outside);
        assert!(k2.ends_with("/b.ts"));
        assert!(k2.contains(&*other.to_string_lossy()));
    }

    #[test]
    fn resolve_imported_symbol_id_finds_exported_symbol() {
        let base0 = tmp_dir();
        let base = base0.canonicalize().unwrap_or(base0);

        // Create a test database
        let db_path_buf = base.join("test.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let sqlite = SqliteStore::open(&db_path).unwrap();
        sqlite.init().unwrap();

        // Add a target symbol
        use crate::storage::sqlite::SymbolRow;
        let target_symbol = SymbolRow {
            id: "target_symbol_id".to_string(),
            file_path: "src/utils.ts".to_string(),
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
        sqlite.upsert_symbol(&target_symbol).unwrap();

        // Create an import that should resolve to the target symbol
        let imp = Import {
            name: "helper".to_string(),
            source: "./utils".to_string(),
            alias: None,
        };

        // Test the enhanced resolution with database
        let resolved = resolve_imported_symbol_id_with_db("src/index.ts", &imp, &sqlite);

        // Should resolve to the actual symbol ID from the database
        assert_eq!(resolved, Some("target_symbol_id".to_string()));
    }

    #[test]
    fn resolve_imported_symbol_id_fallback_to_file_level() {
        let base0 = tmp_dir();
        let base = base0.canonicalize().unwrap_or(base0);

        // Create a test database
        let db_path_buf = base.join("test.db");
        let db_path = Utf8PathBuf::from_path_buf(db_path_buf).unwrap();
        let sqlite = SqliteStore::open(&db_path).unwrap();
        sqlite.init().unwrap();

        // Create an import for a symbol that doesn't exist in the database
        let imp = Import {
            name: "nonExistent".to_string(),
            source: "./utils".to_string(),
            alias: None,
        };

        // Test the enhanced resolution - should fall back to file-level ID
        let resolved = resolve_imported_symbol_id_with_db("src/index.ts", &imp, &sqlite);

        // Should fall back to stable_symbol_id based on path and name
        assert!(resolved.is_some());
        let resolved_id = resolved.unwrap();
        assert!(resolved_id.starts_with("0x") || resolved_id.len() == 16);
    }
}

#[cfg(test)]
mod utils_proptest {
    use super::*;
    use proptest::prelude::*;

    // Strategy: realistic file paths (limited depth for performance testing)
    prop_compose! {
        fn file_path_strategy()(path in r"[a-z_]+(/[a-z_]+){0,3}\.(rs|ts|tsx|js|py|go|java|c|cpp|h)") -> String {
            path
        }
    }

    // Strategy: realistic symbol names (limited length for performance testing)
    prop_compose! {
        fn symbol_name_strategy()(name in r"[a-zA-Z_][a-zA-Z0-9_]{0,29}") -> String {
            name
        }
    }

    // Strategy: realistic byte offsets (limited range for typical source files)
    fn start_byte_strategy() -> impl Strategy<Value = u32> {
        0..50_000u32  // Covers files up to ~50KB
    }

    // Property 1: Determinism
    proptest! {
        #[test]
        fn prop_stable_symbol_id_deterministic(
            file_path in file_path_strategy(),
            name in symbol_name_strategy(),
            start_byte in start_byte_strategy(),
        ) {
            let id1 = stable_symbol_id(&file_path, &name, start_byte);
            let id2 = stable_symbol_id(&file_path, &name, start_byte);
            prop_assert_eq!(id1, id2);
        }
    }

    // Property 2: Output format (16-char lowercase hex)
    proptest! {
        #[test]
        fn prop_stable_symbol_id_format(
            file_path in file_path_strategy(),
            name in symbol_name_strategy(),
            start_byte in start_byte_strategy(),
        ) {
            let id = stable_symbol_id(&file_path, &name, start_byte);
            prop_assert_eq!(id.len(), 16);
            // All chars must be hex digits, and letters must be lowercase
            prop_assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
            prop_assert!(id.chars().all(|c| !c.is_ascii_alphabetic() || c.is_lowercase()));
        }
    }

    // Property 3: Name sensitivity
    proptest! {
        #[test]
        fn prop_stable_symbol_id_name_sensitivity(
            file_path in file_path_strategy(),
            name1 in symbol_name_strategy(),
            name2 in symbol_name_strategy(),
            start_byte in start_byte_strategy(),
        ) {
            prop_assume!(name1 != name2);
            let id1 = stable_symbol_id(&file_path, &name1, start_byte);
            let id2 = stable_symbol_id(&file_path, &name2, start_byte);
            prop_assert_ne!(id1, id2);
        }
    }

    // Property 4: Path sensitivity
    proptest! {
        #[test]
        fn prop_stable_symbol_id_path_sensitivity(
            path1 in file_path_strategy(),
            path2 in file_path_strategy(),
            name in symbol_name_strategy(),
            start_byte in start_byte_strategy(),
        ) {
            prop_assume!(path1 != path2);
            let id1 = stable_symbol_id(&path1, &name, start_byte);
            let id2 = stable_symbol_id(&path2, &name, start_byte);
            prop_assert_ne!(id1, id2);
        }
    }

    // Property 5: Byte position sensitivity
    proptest! {
        #[test]
        fn prop_stable_symbol_id_byte_sensitivity(
            file_path in file_path_strategy(),
            name in symbol_name_strategy(),
            byte1 in start_byte_strategy(),
            byte2 in start_byte_strategy(),
        ) {
            prop_assume!(byte1 != byte2);
            let id1 = stable_symbol_id(&file_path, &name, byte1);
            let id2 = stable_symbol_id(&file_path, &name, byte2);
            prop_assert_ne!(id1, id2);
        }
    }

    // Property 6: Collision resistance (sample-based)
    proptest! {
        #[test]
        fn prop_stable_symbol_id_no_trivial_collisions(
            file_path in file_path_strategy(),
            name in symbol_name_strategy(),
            start_byte in start_byte_strategy(),
        ) {
            // Single-bit change in name should produce different ID
            if name.len() > 0 {
                let mut modified_name = name.clone();
                modified_name.pop();
                if let Some(c) = modified_name.chars().last() {
                    let modified = format!("{}{}x", &name[..name.len()-1], c);
                    if modified != name {
                        let id1 = stable_symbol_id(&file_path, &name, start_byte);
                        let id2 = stable_symbol_id(&file_path, &modified, start_byte);
                        prop_assert_ne!(id1, id2);
                    }
                }
            }
        }
    }

    // Property 7: Avalanche effect (bit distribution)
    proptest! {
        #[test]
        fn prop_stable_symbol_id_avalanche(
            file_path in file_path_strategy(),
            name in symbol_name_strategy(),
            start_byte in start_byte_strategy(),
        ) {
            let id1 = stable_symbol_id(&file_path, &name, start_byte);
            let modified_path = format!("{}x", file_path);
            let id2 = stable_symbol_id(&modified_path, &name, start_byte);

            // Hamming distance should be significant (expected ~32 bits for 64-bit hash)
            let hamming = id1.chars().zip(id2.chars())
                .filter(|(c1, c2)| c1 != c2)
                .count();

            // At least 4 hex digits different (very weak bound, but filters bugs)
            prop_assert!(hamming >= 4, "Avalanche effect: only {} chars different", hamming);
        }
    }

    // Property 8: Performance
    proptest! {
        #[test]
        fn prop_stable_symbol_id_performance(
            file_path in file_path_strategy(),
            name in symbol_name_strategy(),
            start_byte in start_byte_strategy(),
        ) {
            let start = std::time::Instant::now();
            let _id = stable_symbol_id(&file_path, &name, start_byte);
            let elapsed = start.elapsed();

            // Should complete in 1ms for realistic inputs on typical hardware
            // NOTE: This is a regression test, not a strict benchmark. The threshold
            // is set generously to avoid false positives on slower CI hardware while
            // still catching significant performance regressions (e.g., accidental O(n^2)
            // algorithms introduced during refactoring).
            // If this test fails consistently on CI, it may indicate the hardware is slower
            // than expected. Consider increasing the threshold or making this a benchmark-only test.
            prop_assert!(elapsed.as_millis() < 1,
                "Symbol ID generation took {:?} for path={}, name={}, start_byte={}. \
                 If this fails consistently on CI, the threshold may need adjustment.",
                elapsed, file_path, name, start_byte);
        }
    }

    // Property 9: Large-scale consistency (verifies behavior stability across many inputs)
    proptest! {
        #[test]
        fn prop_stable_symbol_id_large_scale_consistency(
            // Generate a seed for reproducibility
            seed in any::<u64>(),
        ) {
            use std::collections::HashSet;

            // Track unique IDs and timing samples
            let mut unique_ids = HashSet::new();
            let mut timings = Vec::with_capacity(10_000);
            let mut collision_count = 0;

            // Test 10,000 randomly generated inputs
            for i in 0..10_000 {
                // Use the seed to generate deterministic "random" values
                let seeded_idx = (seed.wrapping_add(i as u64)) as usize;

                // Generate file path from seed
                let depth = (seeded_idx % 4) as usize;
                let dirs: Vec<_> = (0..depth).map(|d| format!("dir{}", (seeded_idx + d) % 100)).collect();
                let file_path = if dirs.is_empty() {
                    format!("file{}.rs", (seeded_idx % 50))
                } else {
                    format!("{}/file{}.rs", dirs.join("/"), (seeded_idx % 50))
                };

                // Generate symbol name from seed
                let name_len = 1 + (seeded_idx % 30);
                let name_chars: Vec<_> = (0..name_len)
                    .map(|c| match (seeded_idx + c) % 62 {
                        n @ 0..=9 => (b'0' + n as u8) as char,
                        n @ 10..=35 => (b'a' + (n - 10) as u8) as char,
                        n @ 36..=61 => (b'A' + (n - 36) as u8) as char,
                        _ => '_',
                    })
                    .collect();
                let name: String = name_chars.into_iter().collect();

                // Generate start_byte from seed
                let start_byte = (((seeded_idx % 50_000) * 17) % 50_000) as u32;

                let start = std::time::Instant::now();
                let id = stable_symbol_id(&file_path, &name, start_byte);
                let elapsed = start.elapsed();

                timings.push(elapsed.as_nanos());

                // Check for collisions (should be extremely rare)
                if !unique_ids.insert(id.clone()) {
                    collision_count += 1;
                }

                // Sanity check: output format still valid
                prop_assert_eq!(id.len(), 16,
                    "ID format invalid at iteration {}: {}", i, id);
                prop_assert!(id.chars().all(|c| c.is_ascii_hexdigit()),
                    "ID contains invalid characters at iteration {}: {}", i, id);
                prop_assert!(id.chars().all(|c| !c.is_ascii_alphabetic() || c.is_lowercase()),
                    "ID contains uppercase letters at iteration {}: {}", i, id);
            }

            // Collision check: with FNV-1a 64-bit, probability of collision in 10k is negligible
            // Birthday paradox: P(collision) ≈ 1 - e^(-n^2/2*2^64) ≈ 10^-8 for n=10,000
            prop_assert!(collision_count == 0,
                "Found {} collisions in 10,000 samples. This indicates a hash problem.", collision_count);

            // Performance consistency: median should remain reasonable
            timings.sort();
            let median = timings[timings.len() / 2];
            prop_assert!(median < 1_000_000, // 1 millisecond in nanoseconds
                "Median performance degraded to {}ns at scale. Expected <1ms.", median);

            // No single call should take more than 30 milliseconds (sanity check for CI)
            // Threshold increased for CI environments with variable performance (CPU throttling, etc.)
            let max = *timings.iter().max().unwrap();
            prop_assert!(max < 30_000_000, // 30 milliseconds in nanoseconds
                "Max performance outlier at {}ns. Possible performance regression.", max);
        }
    }
}

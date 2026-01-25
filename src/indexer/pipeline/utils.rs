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

pub fn file_key(config: &Config, path: &Path) -> String {
    if let Ok(rel) = config.path_relative_to_base(path) {
        return rel.replace('\\', "/");
    }
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
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
    let target_path = resolve_path(current_file_path, &imp.source)?;

    // Try to find an exported symbol with matching name in the target file
    if let Ok(results) = sqlite.search_symbols_by_exact_name(&imp.name, Some(&target_path), 1) {
        if let Some(symbol) = results.iter().find(|s| s.exported) {
            return Some(symbol.id.clone());
        }
    }

    // Fallback to file-level ID
    Some(stable_symbol_id(&target_path, &imp.name, 0))
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
        let inner = base.join("src/a.ts");
        std::fs::create_dir_all(inner.parent().unwrap()).unwrap();
        std::fs::write(&inner, "export function a() {}").unwrap();

        let other0 = tmp_dir();
        let other = other0.canonicalize().unwrap_or(other0);
        let outside = other.join("b.ts");
        std::fs::write(&outside, "export function b() {}").unwrap();

        let config = Config {
            base_dir: base.clone(),
            db_path: base.join("code-intelligence.db"),
            vector_db_path: base.join("vectors"),
            tantivy_index_path: base.join("tantivy-index"),
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
            max_context_bytes: 10_000,
            index_node_modules: false,
            repo_roots: vec![base.clone()],
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

        let k1 = file_key(&config, &inner);
        assert_eq!(k1, "src/a.ts");

        let k2 = file_key(&config, &outside);
        assert!(k2.ends_with("/b.ts"));
        assert!(k2.contains(&*other.to_string_lossy()));
    }

    #[test]
    fn resolve_imported_symbol_id_finds_exported_symbol() {
        let base0 = tmp_dir();
        let base = base0.canonicalize().unwrap_or(base0);

        // Create a test database
        let db_path = base.join("test.db");
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
        let db_path = base.join("test.db");
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

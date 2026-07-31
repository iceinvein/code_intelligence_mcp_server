use crate::config::Config;
use crate::indexer::extract::symbol::Import;
use crate::indexer::parser::LanguageId;
use crate::storage::sqlite::queries;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::{
    collections::{BTreeSet, HashMap},
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

/// Content identity for a source file, as 32 lowercase hex chars of SHA-256.
///
/// Truncated because this only needs to distinguish revisions of one file
/// within one repository, not resist collision attacks. 128 bits is far past
/// the birthday bound for any plausible file count.
pub fn content_hash_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let full = format!("{:x}", hasher.finalize());
    full[..32].to_string()
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
    config
        .path_relative_to_base(path)
        .unwrap_or_else(|_| path.to_string())
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

/// Stable logical identity for a same-name declaration that occupies a
/// distinct language namespace/kind. The common one-kind case deliberately
/// keeps using `stable_symbol_id(path, qualified_name, 0)` for compatibility.
pub fn stable_typed_logical_symbol_id(file_path: &str, qualified_name: &str, kind: &str) -> String {
    let key = format!("logical\u{1f}{file_path}\u{1f}{qualified_name}\u{1f}{kind}");
    format!("{:016x}", fnv1a_64(key.as_bytes()))
}

/// Deterministic id for a non-canonical declaration occurrence in a logical
/// overload/partial-declaration set. A unique signature remains stable across
/// harmless source movement; `duplicate_ordinal` distinguishes identical
/// partial declarations when syntax provides no stronger discriminator.
pub fn stable_symbol_occurrence_id(
    logical_id: &str,
    signature: &str,
    duplicate_ordinal: usize,
) -> String {
    let key = format!("occurrence\u{1f}{logical_id}\u{1f}{signature}\u{1f}{duplicate_ordinal}");
    format!("{:016x}", fnv1a_64(key.as_bytes()))
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
        LanguageId::Ruby => "ruby",
        LanguageId::Kotlin => "kotlin",
        LanguageId::CSharp => "csharp",
        LanguageId::Swift => "swift",
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
    conn: &Connection,
) -> Option<String> {
    let language = language_for_module_path(current_file_path)?;
    let ModuleFileResolution::Exact(target_path) =
        resolve_indexed_module_file(conn, current_file_path, &imp.source, language).ok()?
    else {
        return None;
    };
    let results =
        queries::symbols::search_symbols_by_exact_name(conn, &imp.name, Some(&target_path), 64)
            .ok()?;
    let exported = results
        .into_iter()
        .filter(|symbol| symbol.exported)
        .collect::<Vec<_>>();
    if !exported.is_empty() {
        let ids = exported
            .iter()
            .map(|symbol| symbol.id.clone())
            .collect::<Vec<_>>();
        let identities = queries::symbol_identities::get_by_symbol_ids(conn, &ids).ok()?;
        let logical_ids = exported
            .iter()
            .map(|symbol| {
                identities
                    .get(&symbol.id)
                    .map(|identity| identity.logical_id.clone())
                    .unwrap_or_else(|| symbol.id.clone())
            })
            .collect::<BTreeSet<_>>();
        if let [logical_id] = logical_ids.into_iter().collect::<Vec<_>>().as_slice() {
            return Some(logical_id.clone());
        }
        return None;
    }

    let public_targets =
        queries::module_bindings::list_public_target_ids(conn, &target_path, &imp.name).ok()?;
    match public_targets.as_slice() {
        [target] => Some(target.clone()),
        _ => None,
    }
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

/// Return repository-relative file candidates for a module specifier.
///
/// This is deliberately language-aware: the legacy import resolver above is
/// TypeScript-biased and cannot represent Python package initializers. The
/// caller must still verify candidates against indexed files before treating a
/// binding as exact.
pub fn module_source_candidates(current: &str, source: &str, language: &str) -> Vec<String> {
    match language {
        "python" => python_module_candidates(current, source),
        "typescript" | "tsx" | "javascript" => ecmascript_module_candidates(current, source),
        "rust" => rust_module_candidates(current, source),
        "java" => dotted_language_candidates(source, "java"),
        "kotlin" => dotted_language_candidates(source, "kt"),
        "csharp" => dotted_language_candidates(source, "cs"),
        "c" | "cpp" => include_module_candidates(current, source),
        "ruby" => ruby_module_candidates(current, source),
        _ => Vec::new(),
    }
}

pub fn module_source_is_local(language: &str, source: &str) -> bool {
    match language {
        "typescript" | "tsx" | "javascript" | "python" | "ruby" => source.starts_with('.'),
        "rust" => {
            source == "crate"
                || source == "self"
                || source == "super"
                || source.starts_with("crate::")
                || source.starts_with("self::")
                || source.starts_with("super::")
        }
        "c" | "cpp" => source.starts_with('.'),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleFileResolution {
    Exact(String),
    Ambiguous,
    Missing,
}

/// Resolve a module specifier only when it identifies one indexed file.
///
/// Python absolute imports are often rooted below the repository root (for
/// example a `src/` layout), so each language-derived candidate may also match
/// as a path suffix. More than one match is reported as ambiguous; callers must
/// never fall back to a global same-name symbol.
pub fn resolve_indexed_module_file(
    conn: &Connection,
    current_file: &str,
    source: &str,
    language: &str,
) -> Result<ModuleFileResolution> {
    let mut matches = BTreeSet::new();
    let mut stmt = conn.prepare_cached(
        r#"
SELECT DISTINCT file_path
FROM symbols
WHERE file_path = ?1 OR file_path LIKE ?2
LIMIT 3
"#,
    )?;
    for candidate in module_source_candidates(current_file, source, language) {
        let suffix_pattern = format!("%/{candidate}");
        let rows = stmt.query_map(rusqlite::params![candidate, suffix_pattern], |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            matches.insert(row?);
            if matches.len() > 1 {
                return Ok(ModuleFileResolution::Ambiguous);
            }
        }
    }
    Ok(match matches.into_iter().next() {
        Some(file) => ModuleFileResolution::Exact(file),
        None => ModuleFileResolution::Missing,
    })
}

fn language_for_module_path(file_path: &str) -> Option<&'static str> {
    if file_path.ends_with(".py") {
        Some("python")
    } else if file_path.ends_with(".tsx") {
        Some("tsx")
    } else if file_path.ends_with(".ts") {
        Some("typescript")
    } else if file_path.ends_with(".js") || file_path.ends_with(".jsx") {
        Some("javascript")
    } else {
        None
    }
}

fn normalize_relative_parts<'a>(
    base_parts: impl IntoIterator<Item = &'a str>,
    appended_parts: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut parts = base_parts
        .into_iter()
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_string)
        .collect::<Vec<_>>();
    for part in appended_parts {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part.to_string()),
        }
    }
    parts
}

fn ecmascript_module_candidates(current: &str, source: &str) -> Vec<String> {
    if !source.starts_with('.') {
        return Vec::new();
    }
    let parent = current
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    let parts = normalize_relative_parts(parent.split('/'), source.split('/'));
    let stem = parts.join("/");
    if stem.is_empty() {
        return Vec::new();
    }

    if [".ts", ".tsx", ".js", ".jsx"]
        .iter()
        .any(|extension| stem.ends_with(extension))
    {
        return vec![stem];
    }

    [
        format!("{stem}.ts"),
        format!("{stem}.tsx"),
        format!("{stem}.js"),
        format!("{stem}.jsx"),
        format!("{stem}/index.ts"),
        format!("{stem}/index.tsx"),
        format!("{stem}/index.js"),
        format!("{stem}/index.jsx"),
    ]
    .into_iter()
    .collect()
}

fn python_module_candidates(current: &str, source: &str) -> Vec<String> {
    if source.is_empty() {
        return Vec::new();
    }

    let leading_dots = source.chars().take_while(|ch| *ch == '.').count();
    let remainder = &source[leading_dots..];
    let mut base = if leading_dots > 0 {
        current
            .rsplit_once('/')
            .map(|(parent, _)| parent.split('/').collect::<Vec<_>>())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // One dot means the current package; each additional dot ascends once.
    for _ in 1..leading_dots {
        base.pop();
    }
    let parts = normalize_relative_parts(base, remainder.split('.'));
    let stem = parts.join("/");
    if stem.is_empty() {
        return Vec::new();
    }
    vec![format!("{stem}.py"), format!("{stem}/__init__.py")]
}

fn rust_module_candidates(current: &str, source: &str) -> Vec<String> {
    if source.is_empty() || source == "*" {
        return Vec::new();
    }
    let source_parts = source
        .split("::")
        .filter(|part| !part.is_empty() && *part != "*")
        .collect::<Vec<_>>();
    if source_parts.is_empty() {
        return Vec::new();
    }

    let current_parent = current
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    let (base, remaining) = match source_parts[0] {
        "crate" => (vec!["src".to_string()], &source_parts[1..]),
        "self" => (
            current_parent
                .split('/')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect(),
            &source_parts[1..],
        ),
        "super" => {
            let mut base = current_parent
                .split('/')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            let mut first_non_super = 0usize;
            while source_parts.get(first_non_super) == Some(&"super") {
                base.pop();
                first_non_super += 1;
            }
            (base, &source_parts[first_non_super..])
        }
        _ => (Vec::new(), source_parts.as_slice()),
    };
    let full_parts =
        normalize_relative_parts(base.iter().map(String::as_str), remaining.iter().copied());
    let mut stems = BTreeSet::new();
    if !full_parts.is_empty() {
        stems.insert(full_parts.join("/"));
    }
    if full_parts.len() > 1 {
        stems.insert(full_parts[..full_parts.len() - 1].join("/"));
    }

    stems
        .into_iter()
        .flat_map(|stem| [format!("{stem}.rs"), format!("{stem}/mod.rs")])
        .collect()
}

fn dotted_language_candidates(source: &str, extension: &str) -> Vec<String> {
    let stem = source.trim_end_matches(".*").replace('.', "/");
    if stem.is_empty() {
        Vec::new()
    } else {
        vec![format!("{stem}.{extension}")]
    }
}

fn include_module_candidates(current: &str, source: &str) -> Vec<String> {
    if source.is_empty() {
        return Vec::new();
    }
    let parent = current
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    let relative = normalize_relative_parts(parent.split('/'), source.split('/')).join("/");
    let mut candidates = BTreeSet::new();
    candidates.insert(source.trim_start_matches("./").to_string());
    if !relative.is_empty() {
        candidates.insert(relative);
    }
    candidates
        .into_iter()
        .filter(|path| !path.is_empty())
        .collect()
}

fn ruby_module_candidates(current: &str, source: &str) -> Vec<String> {
    if source.is_empty() {
        return Vec::new();
    }
    let parent = current
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    let relative = normalize_relative_parts(parent.split('/'), source.split('/')).join("/");
    let raw = source.trim_start_matches("./");
    let mut candidates = BTreeSet::new();
    for stem in [raw, relative.as_str()] {
        if stem.is_empty() {
            continue;
        }
        if stem.ends_with(".rb") {
            candidates.insert(stem.to_string());
        } else {
            candidates.insert(format!("{stem}.rb"));
        }
    }
    candidates.into_iter().collect()
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
    use crate::storage::sqlite::{SqliteStore, SymbolIdentityRow, SymbolRow};
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
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 8,
            vector_search_limit: 10,
            vector_guaranteed_results: 3,
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
            reranker_enabled: false,
            descriptions_enabled: false,
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
            external_index_auto: false,
            external_index_producer: None,
            external_index_on_refresh: "disabled".to_string(),
            external_index_min_interval_ms: 60_000,
            // LLM config
            llm_enabled: true,
            llm_device: crate::config::EmbeddingsDevice::Cpu,
            llm_model_dir: None,
            llm_max_tokens: 30,
            llm_batch_commit: 10,
            answer_llm_n_ctx: 16384,
            sampling_descriptions_enabled: true,
            leader_election_enabled: false,
            leader_heartbeat_interval_ms: 10_000,
            leader_ttl_seconds: 30,
            embedding_truncate_dim: None,
            embedding_dim_override: None,
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
            at_line: 1,
        };

        // Test the enhanced resolution with database
        let conn_guard = sqlite.read().unwrap();
        let resolved = resolve_imported_symbol_id_with_db("src/index.ts", &imp, &conn_guard);

        // Should resolve to the actual symbol ID from the database
        assert_eq!(resolved, Some("target_symbol_id".to_string()));
    }

    #[test]
    fn db_import_resolution_collapses_overload_occurrences() {
        let sqlite = SqliteStore::open_in_memory().unwrap();
        sqlite.init().unwrap();
        let conn = sqlite.read().unwrap();
        let make_symbol = |id: &str, start_byte: u32| SymbolRow {
            id: id.into(),
            file_path: "src/utils.ts".into(),
            language: "typescript".into(),
            kind: "function".into(),
            name: "helper".into(),
            exported: true,
            start_byte,
            end_byte: start_byte + 20,
            start_line: 1,
            end_line: 1,
            text: "export function helper(value: unknown);".into(),
        };
        queries::symbols::batch_upsert_symbols(
            &conn,
            &[make_symbol("helper", 0), make_symbol("helper-number", 30)],
        )
        .unwrap();
        queries::symbol_identities::batch_upsert(
            &conn,
            &[
                SymbolIdentityRow {
                    symbol_id: "helper".into(),
                    logical_id: "helper".into(),
                    qualified_name: "helper".into(),
                    signature: "helper(string)".into(),
                    occurrence_discriminator: "string:0".into(),
                    is_canonical: true,
                },
                SymbolIdentityRow {
                    symbol_id: "helper-number".into(),
                    logical_id: "helper".into(),
                    qualified_name: "helper".into(),
                    signature: "helper(number)".into(),
                    occurrence_discriminator: "number:0".into(),
                    is_canonical: false,
                },
            ],
        )
        .unwrap();
        let import = Import {
            name: "helper".into(),
            source: "./utils".into(),
            alias: None,
            at_line: 1,
        };

        assert_eq!(
            resolve_imported_symbol_id_with_db("src/index.ts", &import, &conn),
            Some("helper".into())
        );
    }

    #[test]
    fn resolve_imported_symbol_id_with_db_drops_missing_target() {
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
            at_line: 1,
        };

        // Database-backed resolution must not manufacture an orphan target.
        let conn_guard = sqlite.read().unwrap();
        let resolved = resolve_imported_symbol_id_with_db("src/index.ts", &imp, &conn_guard);

        assert_eq!(resolved, None);
    }

    #[test]
    fn db_import_resolution_follows_persisted_default_export_binding() {
        let sqlite = SqliteStore::open_in_memory().unwrap();
        sqlite.init().unwrap();
        for row in [
            SymbolRow {
                id: "worker-root".into(),
                file_path: "src/worker.ts".into(),
                language: "typescript".into(),
                kind: "file".into(),
                name: "src/worker.ts".into(),
                exported: false,
                start_byte: 0,
                end_byte: 20,
                start_line: 1,
                end_line: 1,
                text: "export default class Worker {}".into(),
            },
            SymbolRow {
                id: "worker".into(),
                file_path: "src/worker.ts".into(),
                language: "typescript".into(),
                kind: "class".into(),
                name: "Worker".into(),
                exported: true,
                start_byte: 0,
                end_byte: 20,
                start_line: 1,
                end_line: 1,
                text: "export default class Worker {}".into(),
            },
        ] {
            sqlite.upsert_symbol(&row).unwrap();
        }
        let conn = sqlite.write().unwrap();
        queries::module_bindings::batch_upsert(
            &conn,
            &[crate::storage::sqlite::ModuleBindingRow {
                id: 0,
                file_path: "src/worker.ts".into(),
                binding_kind: "export".into(),
                source_module: String::new(),
                source_file: None,
                imported_name: String::new(),
                local_name: "Worker".into(),
                exported_name: "default".into(),
                target_symbol_id: Some("worker".into()),
                at_line: 1,
                resolution: "exact".into(),
                confidence: 1.0,
            }],
        )
        .unwrap();
        let import = Import {
            name: "default".into(),
            source: "./worker".into(),
            alias: Some("DefaultWorker".into()),
            at_line: 1,
        };

        assert_eq!(
            resolve_imported_symbol_id_with_db("src/consumer.ts", &import, &conn),
            Some("worker".into())
        );
    }

    #[test]
    fn module_candidates_cover_typescript_barrels_and_python_packages() {
        assert_eq!(
            module_source_candidates("src/api/index.ts", "../worker", "typescript"),
            vec![
                "src/worker.ts",
                "src/worker.tsx",
                "src/worker.js",
                "src/worker.jsx",
                "src/worker/index.ts",
                "src/worker/index.tsx",
                "src/worker/index.js",
                "src/worker/index.jsx",
            ]
        );
        assert_eq!(
            module_source_candidates("django/urls/__init__.py", ".resolvers", "python"),
            vec![
                "django/urls/resolvers.py",
                "django/urls/resolvers/__init__.py"
            ]
        );
        assert_eq!(
            module_source_candidates("django/urls/base.py", "..conf", "python"),
            vec!["django/conf.py", "django/conf/__init__.py"]
        );
        let rust = module_source_candidates("src/api/mod.rs", "crate::worker::Worker", "rust");
        assert!(rust.contains(&"src/worker.rs".to_string()));
        assert!(rust.contains(&"src/worker/mod.rs".to_string()));
        assert_eq!(
            module_source_candidates("src/App.java", "com.example.Worker", "java"),
            vec!["com/example/Worker.java"]
        );
        let ruby = module_source_candidates("lib/api.rb", "./worker", "ruby");
        assert!(ruby.contains(&"lib/worker.rb".to_string()));
        assert!(module_source_is_local("rust", "crate::worker::Worker"));
        assert!(!module_source_is_local("java", "java.util.List"));
    }

    #[test]
    fn db_import_resolution_uses_unique_module_path_not_global_same_name() {
        let sqlite = SqliteStore::open_in_memory().unwrap();
        sqlite.init().unwrap();
        for (id, file_path, language) in [
            (
                "python-user-service",
                "fixtures/python/pkg/services.py",
                "python",
            ),
            ("go-user-service", "fixtures/go/service.go", "go"),
        ] {
            sqlite
                .upsert_symbol(&SymbolRow {
                    id: id.into(),
                    file_path: file_path.into(),
                    language: language.into(),
                    kind: "class".into(),
                    name: "UserService".into(),
                    exported: true,
                    start_byte: 0,
                    end_byte: 10,
                    start_line: 1,
                    end_line: 1,
                    text: "class UserService: pass".into(),
                })
                .unwrap();
        }
        let import = Import {
            name: "UserService".into(),
            source: "pkg.services".into(),
            alias: None,
            at_line: 1,
        };
        let conn = sqlite.read().unwrap();
        assert_eq!(
            resolve_imported_symbol_id_with_db("fixtures/python/pkg/views.py", &import, &conn),
            Some("python-user-service".into())
        );

        queries::symbols::upsert_symbol(
            &conn,
            &SymbolRow {
                id: "second-python-user-service".into(),
                file_path: "other/pkg/services.py".into(),
                language: "python".into(),
                kind: "class".into(),
                name: "UserService".into(),
                exported: true,
                start_byte: 0,
                end_byte: 10,
                start_line: 1,
                end_line: 1,
                text: "class UserService: pass".into(),
            },
        )
        .unwrap();
        assert_eq!(
            resolve_imported_symbol_id_with_db("fixtures/python/pkg/views.py", &import, &conn),
            None,
            "ambiguous module suffixes must not select a global same-name symbol"
        );
    }

    #[test]
    fn content_hash_is_stable_and_32_hex_chars() {
        let a = content_hash_hex(b"pub fn probe() {}\n");
        assert_eq!(a.len(), 32);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(a, content_hash_hex(b"pub fn probe() {}\n"));
        assert_ne!(a, content_hash_hex(b"pub fn probe() { }\n"));
    }

    #[test]
    fn content_hash_handles_empty_input() {
        assert_eq!(content_hash_hex(b"").len(), 32);
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
        0..50_000u32 // Covers files up to ~50KB
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
            if !name.is_empty() {
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
            // A single timed call is jitter-prone (scheduler preemption, cold
            // caches, background load): take the minimum over several runs,
            // which reflects the true cost of the input rather than one-off
            // machine noise. Persisted seeds from the old single-shot version
            // were timing artifacts, not input-dependent regressions.
            let elapsed = (0..10)
                .map(|_| {
                    let start = std::time::Instant::now();
                    let _id = stable_symbol_id(&file_path, &name, start_byte);
                    start.elapsed()
                })
                .min()
                .unwrap();

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
                let depth = seeded_idx % 4;
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

            // No single call should take more than 100 milliseconds (sanity check for CI)
            // Threshold set generously for CI environments with variable performance
            // (CPU throttling, resource contention, virtualization overhead, etc.)
            // This is a regression guard, not a benchmark - we want to catch O(n^2) bugs,
            // not minor CI performance variations.
            let max = *timings.iter().max().unwrap();
            prop_assert!(max < 100_000_000, // 100 milliseconds in nanoseconds
                "Max performance outlier at {}ns. Possible performance regression.", max);
        }
    }
}

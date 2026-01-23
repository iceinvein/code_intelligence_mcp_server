use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    env,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingsDevice {
    Cpu,
    Metal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingsBackend {
    FastEmbed,
    Hash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub base_dir: PathBuf,
    pub db_path: PathBuf,
    pub vector_db_path: PathBuf,
    pub tantivy_index_path: PathBuf,
    pub embeddings_backend: EmbeddingsBackend,
    pub embeddings_model_dir: Option<PathBuf>,
    pub embeddings_model_url: Option<String>,
    pub embeddings_model_sha256: Option<String>,
    pub embeddings_auto_download: bool,
    pub embeddings_model_repo: Option<String>,
    pub embeddings_model_revision: Option<String>,
    pub embeddings_model_hf_token: Option<String>,
    pub embeddings_device: EmbeddingsDevice,
    pub embedding_batch_size: usize,
    pub hash_embedding_dim: usize,
    pub vector_search_limit: usize,
    pub hybrid_alpha: f32,
    pub rank_vector_weight: f32,
    pub rank_keyword_weight: f32,
    pub rank_exported_boost: f32,
    pub rank_index_file_boost: f32,
    pub rank_test_penalty: f32,
    pub rank_popularity_weight: f32,
    pub rank_popularity_cap: u64,
    pub index_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub watch_mode: bool,
    pub watch_debounce_ms: u64,
    pub max_context_bytes: usize,
    pub index_node_modules: bool,
    pub repo_roots: Vec<PathBuf>,

    // Reranker config (FNDN-03)
    pub reranker_model_path: Option<PathBuf>,
    pub reranker_top_k: usize,
    pub reranker_cache_dir: Option<PathBuf>,

    // Learning config (FNDN-04)
    pub learning_enabled: bool,
    pub learning_selection_boost: f32,
    pub learning_file_affinity_boost: f32,

    // Token config (FNDN-05)
    pub max_context_tokens: usize,
    pub token_encoding: String,

    // Performance config (FNDN-06)
    pub parallel_workers: usize,
    pub embedding_cache_enabled: bool,

    // PageRank config (FNDN-07)
    pub pagerank_damping: f32,
    pub pagerank_iterations: usize,

    // Query expansion config (FNDN-02)
    pub synonym_expansion_enabled: bool,
    pub acronym_expansion_enabled: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let base_dir_raw = required_env("BASE_DIR")?;
        let base_dir = canonicalize_dir(Path::new(&base_dir_raw))
            .with_context(|| format!("Invalid BASE_DIR: {base_dir_raw}"))?;

        let embeddings_model_url = optional_env("EMBEDDINGS_MODEL_URL");
        let embeddings_model_sha256 = optional_env("EMBEDDINGS_MODEL_SHA256");

        let embeddings_auto_download = optional_env("EMBEDDINGS_AUTO_DOWNLOAD")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(false);

        let embeddings_backend_env = optional_env("EMBEDDINGS_BACKEND")
            .as_deref()
            .map(parse_embeddings_backend)
            .transpose()?;

        let (embeddings_backend, embeddings_model_dir) = match embeddings_backend_env {
            Some(EmbeddingsBackend::FastEmbed) => {
                let embeddings_model_dir = match optional_env("EMBEDDINGS_MODEL_DIR").as_deref() {
                    Some(raw) => Some(Path::new(raw).to_path_buf()),
                    None => {
                        // Default to .cimcp/embeddings-cache
                        Some(base_dir.join("./.cimcp/embeddings-cache"))
                    }
                };
                (EmbeddingsBackend::FastEmbed, embeddings_model_dir)
            }
            Some(EmbeddingsBackend::Hash) => (EmbeddingsBackend::Hash, None),
            None => {
                // Default to FastEmbed
                (
                    EmbeddingsBackend::FastEmbed,
                    Some(base_dir.join("./.cimcp/embeddings-cache")),
                )
            }
        };

        // We don't strictly need model_url/sha256/revision for FastEmbed as it manages it,
        // but we DO need the repo name (model name).
        let embeddings_model_repo = optional_env("EMBEDDINGS_MODEL_REPO")
            .unwrap_or_else(|| "BAAI/bge-base-en-v1.5".to_string()); // Default to BGE-Base-v1.5

        let db_path = default_path(&base_dir, "DB_PATH", "./.cimcp/code-intelligence.db")?;
        let vector_db_path = default_path(&base_dir, "VECTOR_DB_PATH", "./.cimcp/vectors")?;
        let tantivy_index_path =
            default_path(&base_dir, "TANTIVY_INDEX_PATH", "./.cimcp/tantivy-index")?;

        let embeddings_device = optional_env("EMBEDDINGS_DEVICE")
            .as_deref()
            .map(parse_embeddings_device)
            .transpose()?
            .unwrap_or(EmbeddingsDevice::Cpu);

        let embedding_batch_size = optional_env("EMBEDDING_BATCH_SIZE")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(32);

        let hash_embedding_dim = optional_env("HASH_EMBEDDING_DIM")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(64);

        let vector_search_limit = optional_env("VECTOR_SEARCH_LIMIT")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(20);

        let hybrid_alpha = optional_env("HYBRID_ALPHA")
            .as_deref()
            .map(parse_f32)
            .transpose()?
            .unwrap_or(0.7);

        let rank_vector_weight = optional_env("RANK_VECTOR_WEIGHT")
            .as_deref()
            .map(parse_any_f32)
            .transpose()?
            .unwrap_or(hybrid_alpha);
        let rank_keyword_weight = optional_env("RANK_KEYWORD_WEIGHT")
            .as_deref()
            .map(parse_any_f32)
            .transpose()?
            .unwrap_or(1.0 - hybrid_alpha);
        let rank_exported_boost = optional_env("RANK_EXPORTED_BOOST")
            .as_deref()
            .map(parse_any_f32)
            .transpose()?
            .unwrap_or(0.1);
        let rank_index_file_boost = optional_env("RANK_INDEX_FILE_BOOST")
            .as_deref()
            .map(parse_any_f32)
            .transpose()?
            .unwrap_or(0.05);
        let rank_test_penalty = optional_env("RANK_TEST_PENALTY")
            .as_deref()
            .map(parse_any_f32)
            .transpose()?
            .unwrap_or(0.1);
        let rank_popularity_weight = optional_env("RANK_POPULARITY_WEIGHT")
            .as_deref()
            .map(parse_any_f32)
            .transpose()?
            .unwrap_or(0.05);
        let rank_popularity_cap = optional_env("RANK_POPULARITY_CAP")
            .as_deref()
            .map(parse_u64)
            .transpose()?
            .unwrap_or(50);

        let index_patterns = parse_csv_or_default(
            optional_env("INDEX_PATTERNS").as_deref(),
            &["**/*.ts", "**/*.tsx", "**/*.rs"],
        );

        let exclude_patterns = parse_csv_or_default(
            optional_env("EXCLUDE_PATTERNS").as_deref(),
            &[
                "**/node_modules/**",
                "**/dist/**",
                "**/build/**",
                "**/.git/**",
                "**/*.test.*",
            ],
        );

        let watch_mode = optional_env("WATCH_MODE")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(true);

        let watch_debounce_ms = optional_env("WATCH_DEBOUNCE_MS")
            .as_deref()
            .map(parse_u64)
            .transpose()?
            .unwrap_or(250);

        let max_context_bytes = optional_env("MAX_CONTEXT_BYTES")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(200_000);

        let index_node_modules = optional_env("INDEX_NODE_MODULES")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(false);

        let mut repo_roots = vec![base_dir.clone()];
        if let Some(roots_raw) = optional_env("REPO_ROOTS") {
            for raw in parse_csv(&roots_raw) {
                let dir = canonicalize_dir(Path::new(&raw))
                    .with_context(|| format!("Invalid REPO_ROOTS entry: {raw}"))?;
                if !repo_roots.contains(&dir) {
                    repo_roots.push(dir);
                }
            }
        }

        // Reranker config (FNDN-03)
        let reranker_model_path = optional_env("RERANKER_MODEL_PATH")
            .map(|s| PathBuf::from(s));
        let reranker_top_k = optional_env("RERANKER_TOP_K")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(20);
        let reranker_cache_dir = optional_env("RERANKER_CACHE_DIR")
            .map(|s| PathBuf::from(s))
            .or_else(|| Some(base_dir.join(".cimcp/reranker-cache")));

        // Learning config (FNDN-04)
        let learning_enabled = optional_env("LEARNING_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(false);
        let learning_selection_boost = optional_env("LEARNING_SELECTION_BOOST")
            .as_deref()
            .map(parse_any_f32)
            .transpose()?
            .unwrap_or(0.1);
        let learning_file_affinity_boost = optional_env("LEARNING_FILE_AFFINITY_BOOST")
            .as_deref()
            .map(parse_any_f32)
            .transpose()?
            .unwrap_or(0.05);

        // Token config (FNDN-05)
        let max_context_tokens = optional_env("MAX_CONTEXT_TOKENS")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(8192);
        let token_encoding = optional_env("TOKEN_ENCODING")
            .unwrap_or_else(|| "o200k_base".to_string());

        // Performance config (FNDN-06)
        let parallel_workers = optional_env("PARALLEL_WORKERS")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or_else(|| num_cpus::get());
        let embedding_cache_enabled = optional_env("EMBEDDING_CACHE_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(true);

        // PageRank config (FNDN-07)
        let pagerank_damping = optional_env("PAGERANK_DAMPING")
            .as_deref()
            .map(parse_any_f32)
            .transpose()?
            .unwrap_or(0.85);
        let pagerank_iterations = optional_env("PAGERANK_ITERATIONS")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(20);

        // Query expansion config (FNDN-02)
        let synonym_expansion_enabled = optional_env("SYNONYM_EXPANSION_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(true);
        let acronym_expansion_enabled = optional_env("ACRONYM_EXPANSION_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(true);

        Ok(Self {
            base_dir,
            db_path,
            vector_db_path,
            tantivy_index_path,
            embeddings_backend,
            embeddings_model_dir,
            embeddings_model_url,
            embeddings_model_sha256,
            embeddings_auto_download,
            embeddings_model_repo: Some(embeddings_model_repo), // Always present now as a string
            embeddings_model_revision: None, // FastEmbed manages versions internally mostly, or we assume main
            embeddings_model_hf_token: None, // Not used by FastEmbed currently

            embeddings_device,
            embedding_batch_size,
            hash_embedding_dim,
            vector_search_limit,
            hybrid_alpha,
            rank_vector_weight,
            rank_keyword_weight,
            rank_exported_boost,
            rank_index_file_boost,
            rank_test_penalty,
            rank_popularity_weight,
            rank_popularity_cap,
            index_patterns,
            exclude_patterns,
            watch_mode,
            watch_debounce_ms,
            max_context_bytes,
            index_node_modules,
            repo_roots,

            // Reranker config (FNDN-03)
            reranker_model_path,
            reranker_top_k,
            reranker_cache_dir,

            // Learning config (FNDN-04)
            learning_enabled,
            learning_selection_boost,
            learning_file_affinity_boost,

            // Token config (FNDN-05)
            max_context_tokens,
            token_encoding,

            // Performance config (FNDN-06)
            parallel_workers,
            embedding_cache_enabled,

            // PageRank config (FNDN-07)
            pagerank_damping,
            pagerank_iterations,

            // Query expansion config (FNDN-02)
            synonym_expansion_enabled,
            acronym_expansion_enabled,
        })
    }

    pub fn normalize_path_to_base(&self, path: &Path) -> Result<PathBuf> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.base_dir.join(path)
        };
        Ok(abs)
    }

    pub fn path_relative_to_base(&self, path: &Path) -> Result<String> {
        let abs = self.normalize_path_to_base(path)?;
        let abs = abs.canonicalize().unwrap_or(abs);

        let rel = abs
            .strip_prefix(&self.base_dir)
            .map_err(|_| anyhow!("Path is not under BASE_DIR: {}", abs.display()))?;

        Ok(rel.to_string_lossy().into_owned())
    }
}

fn required_env(key: &str) -> Result<String> {
    env::var(key).map_err(|_| anyhow!("Missing required env var: {key}"))
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key).ok().and_then(|v| {
        let v = v.trim().to_string();
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    })
}

fn canonicalize_dir(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .context("Failed to get current_dir")?
            .join(path)
    };
    let meta = std::fs::metadata(&path)
        .with_context(|| format!("Path does not exist: {}", path.display()))?;
    if !meta.is_dir() {
        return Err(anyhow!("Expected directory, got file: {}", path.display()));
    }
    path.canonicalize()
        .with_context(|| format!("Failed to canonicalize: {}", path.display()))
}

fn default_path(base_dir: &Path, key: &str, default_rel: &str) -> Result<PathBuf> {
    let raw = optional_env(key).unwrap_or_else(|| default_rel.to_string());
    let path = Path::new(&raw);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    })
}

fn parse_csv_or_default(value: Option<&str>, default: &[&str]) -> Vec<String> {
    match value {
        Some(v) => parse_csv(v),
        None => default.iter().map(|s| (*s).to_string()).collect(),
    }
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn parse_embeddings_device(value: &str) -> Result<EmbeddingsDevice> {
    match value.trim().to_lowercase().as_str() {
        "cpu" => Ok(EmbeddingsDevice::Cpu),
        "metal" => Ok(EmbeddingsDevice::Metal),
        other => Err(anyhow!("Invalid EMBEDDINGS_DEVICE: {other}")),
    }
}

fn parse_embeddings_backend(value: &str) -> Result<EmbeddingsBackend> {
    match value.trim().to_lowercase().as_str() {
        "fastembed" => Ok(EmbeddingsBackend::FastEmbed),
        "hash" => Ok(EmbeddingsBackend::Hash),
        other => Err(anyhow!("Invalid EMBEDDINGS_BACKEND: {other}")),
    }
}

fn parse_usize(value: &str) -> Result<usize> {
    value
        .trim()
        .parse::<usize>()
        .map_err(|err| anyhow!("Invalid integer '{value}': {err}"))
}

fn parse_u64(value: &str) -> Result<u64> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|err| anyhow!("Invalid integer '{value}': {err}"))
}

fn parse_f32(value: &str) -> Result<f32> {
    let v = value
        .trim()
        .parse::<f32>()
        .map_err(|err| anyhow!("Invalid float '{value}': {err}"))?;

    if !(0.0..=1.0).contains(&v) {
        return Err(anyhow!("HYBRID_ALPHA must be in 0..=1"));
    }

    Ok(v)
}

fn parse_any_f32(value: &str) -> Result<f32> {
    value
        .trim()
        .parse::<f32>()
        .map_err(|err| anyhow!("Invalid float '{value}': {err}"))
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.trim().to_lowercase().as_str() {
        "true" | "1" | "yes" | "y" => Ok(true),
        "false" | "0" | "no" | "n" => Ok(false),
        other => Err(anyhow!("Invalid boolean '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "code-intel-config-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn clear_env() {
        for k in [
            "BASE_DIR",
            "DB_PATH",
            "VECTOR_DB_PATH",
            "TANTIVY_INDEX_PATH",
            "EMBEDDINGS_BACKEND",
            "EMBEDDINGS_MODEL_DIR",
            "EMBEDDINGS_MODEL_URL",
            "EMBEDDINGS_MODEL_SHA256",
            "EMBEDDINGS_AUTO_DOWNLOAD",
            "EMBEDDINGS_MODEL_REPO",
            "EMBEDDINGS_MODEL_REVISION",
            "EMBEDDINGS_MODEL_HF_TOKEN",
            "EMBEDDINGS_DEVICE",
            "EMBEDDING_BATCH_SIZE",
            "HASH_EMBEDDING_DIM",
            "VECTOR_SEARCH_LIMIT",
            "HYBRID_ALPHA",
            "RANK_VECTOR_WEIGHT",
            "RANK_KEYWORD_WEIGHT",
            "RANK_EXPORTED_BOOST",
            "RANK_INDEX_FILE_BOOST",
            "RANK_TEST_PENALTY",
            "RANK_POPULARITY_WEIGHT",
            "RANK_POPULARITY_CAP",
            "INDEX_PATTERNS",
            "EXCLUDE_PATTERNS",
            "WATCH_MODE",
            "WATCH_DEBOUNCE_MS",
            "MAX_CONTEXT_BYTES",
            "INDEX_NODE_MODULES",
            "REPO_ROOTS",
            // Reranker config (FNDN-03)
            "RERANKER_MODEL_PATH",
            "RERANKER_TOP_K",
            "RERANKER_CACHE_DIR",
            // Learning config (FNDN-04)
            "LEARNING_ENABLED",
            "LEARNING_SELECTION_BOOST",
            "LEARNING_FILE_AFFINITY_BOOST",
            // Token config (FNDN-05)
            "MAX_CONTEXT_TOKENS",
            "TOKEN_ENCODING",
            // Performance config (FNDN-06)
            "PARALLEL_WORKERS",
            "EMBEDDING_CACHE_ENABLED",
            // PageRank config (FNDN-07)
            "PAGERANK_DAMPING",
            "PAGERANK_ITERATIONS",
            // Query expansion config (FNDN-02)
            "SYNONYM_EXPANSION_ENABLED",
            "ACRONYM_EXPANSION_ENABLED",
        ] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn from_env_requires_base_dir() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let err = Config::from_env().unwrap_err().to_string();
        assert!(err.contains("BASE_DIR"));
    }

    #[test]
    fn from_env_defaults_to_fastembed_backend_without_model_dir() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string_lossy().to_string());

        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.embeddings_backend, EmbeddingsBackend::FastEmbed);
        assert!(cfg.embeddings_model_dir.is_some());
        assert_eq!(
            cfg.db_path,
            cfg.base_dir.join("./.cimcp/code-intelligence.db")
        );
        assert_eq!(cfg.vector_db_path, cfg.base_dir.join("./.cimcp/vectors"));
        assert_eq!(
            cfg.tantivy_index_path,
            cfg.base_dir.join("./.cimcp/tantivy-index")
        );
        assert_eq!(cfg.repo_roots, vec![cfg.base_dir.clone()]);
    }

    #[test]
    fn fastembed_backend_configured_by_default() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string_lossy().to_string());

        // Default should be FastEmbed
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.embeddings_backend, EmbeddingsBackend::FastEmbed);
        assert_eq!(
            cfg.embeddings_model_dir,
            Some(cfg.base_dir.join("./.cimcp/embeddings-cache"))
        );
        assert_eq!(
            cfg.embeddings_model_repo.as_deref(),
            Some("BAAI/bge-base-en-v1.5")
        );
    }

    #[test]
    fn fastembed_backend_allows_custom_model_dir() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        let custom = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string_lossy().to_string());
        std::env::set_var("EMBEDDINGS_BACKEND", "fastembed");
        std::env::set_var("EMBEDDINGS_MODEL_DIR", custom.to_string_lossy().to_string());

        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.embeddings_backend, EmbeddingsBackend::FastEmbed);
        assert_eq!(cfg.embeddings_model_dir, Some(custom));
    }

    #[test]
    fn fastembed_backend_defaults_if_backend_not_set() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string_lossy().to_string());
        // No backend set, should default to FastEmbed

        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.embeddings_backend, EmbeddingsBackend::FastEmbed);
        assert!(cfg.embeddings_model_dir.is_some());
    }

    #[test]
    fn repo_roots_parses_and_dedupes() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        let extra = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string_lossy().to_string());
        std::env::set_var(
            "REPO_ROOTS",
            format!(
                "  {} , {} , {} ",
                extra.to_string_lossy(),
                extra.to_string_lossy(),
                base.to_string_lossy()
            ),
        );

        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.repo_roots.len(), 2);
        let extra_c = extra.canonicalize().unwrap_or(extra);
        assert!(cfg.repo_roots.contains(&cfg.base_dir));
        assert!(cfg.repo_roots.contains(&extra_c));
    }

    #[test]
    fn hybrid_alpha_validation_and_weight_defaults() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string_lossy().to_string());
        std::env::set_var("HYBRID_ALPHA", "2");
        assert!(Config::from_env().is_err());

        std::env::set_var("HYBRID_ALPHA", "0.2");
        let cfg = Config::from_env().unwrap();
        assert!((cfg.hybrid_alpha - 0.2).abs() < f32::EPSILON);
        assert!((cfg.rank_vector_weight - 0.2).abs() < f32::EPSILON);
        assert!((cfg.rank_keyword_weight - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn watch_mode_defaults_to_true() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string_lossy().to_string());
        let cfg = Config::from_env().unwrap();
        assert!(cfg.watch_mode);
    }

    #[test]
    fn bool_parsing_accepts_multiple_spellings() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string_lossy().to_string());
        std::env::set_var("WATCH_MODE", "yes");
        std::env::set_var("INDEX_NODE_MODULES", "1");
        let cfg = Config::from_env().unwrap();
        assert!(cfg.watch_mode);
        assert!(cfg.index_node_modules);
    }
}

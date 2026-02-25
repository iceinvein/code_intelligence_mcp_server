use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs,
    path::{Path, PathBuf},
};

use crate::path::{PathError, PathNormalizer, Utf8PathBuf};
use crate::registry::RepoRegistry;

/// Returns the global data directory (~/.code-intelligence)
///
/// This function uses a layered fallback strategy to avoid panicking:
/// 1. Try $HOME/.code-intelligence
/// 2. Try $XDG_DATA_HOME/code-intelligence
/// 3. Fall back to /tmp/.code-intelligence (logs a warning since this is non-standard)
///
/// The /tmp fallback ensures the application can always start, even in
/// degraded environments (e.g., chroot, missing HOME).
pub fn get_data_dir() -> Utf8PathBuf {
    env::var("HOME")
        .ok()
        .and_then(|home| {
            Utf8PathBuf::from_path_buf(PathBuf::from(home).join(".code-intelligence")).ok()
        })
        .or_else(|| {
            env::var("XDG_DATA_HOME")
                .ok()
                .and_then(|p| {
                    Utf8PathBuf::from_path_buf(PathBuf::from(p).join("code-intelligence")).ok()
                })
        })
        .unwrap_or_else(|| {
            let fallback = Utf8PathBuf::from("/tmp/.code-intelligence");
            tracing::warn!(
                path = %fallback,
                "HOME and XDG_DATA_HOME not set, using temporary fallback (non-standard location)"
            );
            fallback
        })
}

/// Returns the per-repo data directory (~/.code-intelligence/repos/<hash>/)
///
/// Computes a deterministic 16-character SHA256 hash of the base_dir path
/// to isolate each repo's indexes (SQLite, Tantivy, LanceDB).
pub fn get_repo_data_dir(base_dir: &str) -> Utf8PathBuf {
    let data_dir = get_data_dir();
    let hash = RepoRegistry::path_hash(base_dir);
    data_dir.join("repos").join(hash)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingsDevice {
    Cpu,
    Metal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingsBackend {
    LlamaCpp,
    Hash,
}

// TOML parsing structs (private, for deserialization)
#[derive(Debug, Deserialize)]
struct ServerToml {
    server: Option<ServerTomlServer>,
    embeddings: Option<ServerTomlEmbeddings>,
    repos: Option<ServerTomlRepos>,
    lifecycle: Option<ServerTomlLifecycle>,
}

#[derive(Debug, Deserialize)]
struct ServerTomlServer {
    host: Option<String>,
    port: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct ServerTomlEmbeddings {
    backend: Option<EmbeddingsBackend>,
    device: Option<EmbeddingsDevice>,
}

#[derive(Debug, Deserialize)]
struct ServerTomlRepos {
    defaults: Option<ServerTomlRepoDefaults>,
}

#[derive(Debug, Deserialize)]
struct ServerTomlRepoDefaults {
    index_patterns: Option<String>,
    exclude_patterns: Option<String>,
    watch_mode: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ServerTomlLifecycle {
    warm_ttl_seconds: Option<u64>,
}

/// Configuration for standalone HTTP server mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandaloneConfig {
    pub host: String,
    pub port: u16,
    pub data_dir: Utf8PathBuf,
    pub warm_ttl_seconds: u64,
    pub embeddings_backend: EmbeddingsBackend,
    pub embeddings_device: EmbeddingsDevice,
    pub embeddings_model_dir: Option<Utf8PathBuf>,
    pub hash_embedding_dim: usize,
    pub default_index_patterns: Vec<String>,
    pub default_exclude_patterns: Vec<String>,
    pub default_watch_mode: bool,
    /// Matryoshka truncation dimension passed through to per-repo configs.
    pub embedding_truncate_dim: Option<usize>,
}

impl Default for StandaloneConfig {
    fn default() -> Self {
        let data_dir = get_data_dir();
        Self {
            host: "127.0.0.1".to_string(),
            port: 3333,
            data_dir: data_dir.clone(),
            warm_ttl_seconds: 300,
            embeddings_backend: EmbeddingsBackend::LlamaCpp,
            embeddings_device: EmbeddingsDevice::Metal,
            embeddings_model_dir: Some(data_dir.join("models/jina-code-embeddings-1.5b-gguf")),
            hash_embedding_dim: 64,
            default_index_patterns: vec![
                "**/*.ts".to_string(),
                "**/*.tsx".to_string(),
                "**/*.js".to_string(),
                "**/*.jsx".to_string(),
                "**/*.rs".to_string(),
                "**/*.py".to_string(),
                "**/*.go".to_string(),
                "**/*.java".to_string(),
                "**/*.c".to_string(),
                "**/*.h".to_string(),
                "**/*.cpp".to_string(),
                "**/*.cc".to_string(),
                "**/*.cxx".to_string(),
                "**/*.hpp".to_string(),
            ],
            default_exclude_patterns: vec![
                "**/node_modules/**".to_string(),
                "**/dist/**".to_string(),
                "**/build/**".to_string(),
                "**/.git/**".to_string(),
                "**/*.test.*".to_string(),
            ],
            default_watch_mode: true,
            embedding_truncate_dim: None,
        }
    }
}

impl StandaloneConfig {
    /// Parse standalone config from TOML string
    pub fn from_toml_str(toml_str: &str) -> Result<Self> {
        let parsed: ServerToml = toml::from_str(toml_str)
            .context("Failed to parse server.toml")?;

        let mut config = Self::default();

        if let Some(server) = parsed.server {
            if let Some(host) = server.host {
                config.host = host;
            }
            if let Some(port) = server.port {
                config.port = port;
            }
        }

        if let Some(embeddings) = parsed.embeddings {
            if let Some(backend) = embeddings.backend {
                config.embeddings_backend = backend;
            }
            if let Some(device) = embeddings.device {
                config.embeddings_device = device;
            }
        }

        if let Some(repos) = parsed.repos {
            if let Some(defaults) = repos.defaults {
                if let Some(patterns) = defaults.index_patterns {
                    config.default_index_patterns = parse_csv(&patterns);
                }
                if let Some(patterns) = defaults.exclude_patterns {
                    config.default_exclude_patterns = parse_csv(&patterns);
                }
                if let Some(watch_mode) = defaults.watch_mode {
                    config.default_watch_mode = watch_mode;
                }
            }
        }

        if let Some(lifecycle) = parsed.lifecycle {
            if let Some(warm_ttl) = lifecycle.warm_ttl_seconds {
                config.warm_ttl_seconds = warm_ttl;
            }
        }

        Ok(config)
    }

    /// Load standalone config from ~/.code-intelligence/server.toml with env var and CLI overrides
    ///
    /// Priority: CLI args > env vars > server.toml > defaults
    pub fn load(cli_host: Option<&str>, cli_port: Option<u16>) -> Result<Self> {
        let mut config = Self::default();

        // Try to load from server.toml
        let config_path = config.data_dir.join("server.toml");
        if config_path.exists() {
            let toml_str = fs::read_to_string(config_path.as_std_path())
                .context("Failed to read server.toml")?;
            config = Self::from_toml_str(&toml_str)?;
        }

        // Apply env var overrides
        if let Ok(backend) = std::env::var("EMBEDDINGS_BACKEND") {
            match parse_embeddings_backend(&backend) {
                Ok(b) => config.embeddings_backend = b,
                Err(_) => tracing::warn!("Unknown EMBEDDINGS_BACKEND: {}", backend),
            }
        }
        if let Ok(device) = std::env::var("EMBEDDINGS_DEVICE") {
            match device.to_lowercase().as_str() {
                "cpu" => config.embeddings_device = EmbeddingsDevice::Cpu,
                "metal" => config.embeddings_device = EmbeddingsDevice::Metal,
                other => tracing::warn!("Unknown EMBEDDINGS_DEVICE: {}", other),
            }
        }
        if let Ok(model_dir) = std::env::var("EMBEDDINGS_MODEL_DIR") {
            config.embeddings_model_dir = Some(Utf8PathBuf::from(model_dir));
        }
        if let Ok(val) = std::env::var("HASH_EMBEDDING_DIM") {
            if let Ok(dim) = val.parse::<usize>() {
                config.hash_embedding_dim = dim;
            }
        }
        if let Ok(val) = std::env::var("EMBEDDING_TRUNCATE_DIM") {
            match val.parse::<usize>() {
                Ok(dim) => config.embedding_truncate_dim = Some(dim),
                Err(_) => tracing::warn!(
                    value = %val,
                    "EMBEDDING_TRUNCATE_DIM is not a valid usize, ignoring"
                ),
            }
        }

        // Apply CLI overrides (highest priority)
        if let Some(host) = cli_host {
            config.host = host.to_string();
        }
        if let Some(port) = cli_port {
            config.port = port;
        }

        Ok(config)
    }

    /// Build a per-repo Config from standalone settings
    pub fn repo_config(&self, repo_path: Utf8PathBuf, repo_data_dir: &Utf8PathBuf) -> Config {
        let db_path = repo_data_dir.join("code-intelligence.db");
        let vector_db_path = repo_data_dir.join("vectors");
        let tantivy_index_path = repo_data_dir.join("tantivy-index");

        // Get global data dir for shared resources (models, caches)
        let global_dir = get_data_dir();

        Config {
            base_dir: repo_path.clone(),
            db_path,
            vector_db_path,
            tantivy_index_path,
            embeddings_backend: self.embeddings_backend,
            embeddings_model_dir: self.embeddings_model_dir.clone(),
            embeddings_device: self.embeddings_device,
            embedding_batch_size: 32,
            hash_embedding_dim: self.hash_embedding_dim,
            vector_search_limit: 20,
            vector_guaranteed_results: 3,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 1.0,
            rank_index_file_boost: 0.05,
            rank_test_penalty: 0.1,
            rank_popularity_weight: 0.05,
            rank_popularity_cap: 50,
            index_patterns: self.default_index_patterns.clone(),
            exclude_patterns: self.default_exclude_patterns.clone(),
            watch_mode: self.default_watch_mode,
            watch_debounce_ms: 2000,
            watch_min_index_interval_ms: 5000,
            max_context_bytes: 200_000,
            index_node_modules: false,
            repo_roots: vec![repo_path],
            reranker_enabled: false,
            reranker_model_path: None,
            reranker_top_k: 20,
            reranker_cache_dir: Some(global_dir.join("reranker-cache")),
            learning_enabled: true,
            learning_selection_boost: 0.1,
            learning_file_affinity_boost: 0.05,
            max_context_tokens: 8192,
            token_encoding: "o200k_base".to_string(),
            parallel_workers: std::thread::available_parallelism()
                .map(|n| n.get().div_ceil(2))
                .unwrap_or(2)
                .max(2),
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
            llm_enabled: true,
            llm_device: EmbeddingsDevice::Cpu,
            llm_model_dir: Some(global_dir.join("models/qwen2.5-coder-1.5b-gguf")),
            llm_max_tokens: 30,
            llm_batch_commit: 10,
            // Standalone mode has SessionManager for coordination — no need for flock
            leader_election_enabled: false,
            leader_heartbeat_interval_ms: 10_000,
            leader_ttl_seconds: 30,
            embedding_truncate_dim: self.embedding_truncate_dim,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub base_dir: Utf8PathBuf,
    pub db_path: Utf8PathBuf,
    pub vector_db_path: Utf8PathBuf,
    pub tantivy_index_path: Utf8PathBuf,
    pub embeddings_backend: EmbeddingsBackend,
    pub embeddings_model_dir: Option<Utf8PathBuf>,
    pub embeddings_device: EmbeddingsDevice,
    pub embedding_batch_size: usize,
    pub hash_embedding_dim: usize,
    pub vector_search_limit: usize,
    pub vector_guaranteed_results: usize,
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
    pub watch_min_index_interval_ms: u64, // Minimum time between index runs in watch mode
    pub max_context_bytes: usize,
    pub index_node_modules: bool,
    pub repo_roots: Vec<Utf8PathBuf>,

    // Reranker config (FNDN-03)
    pub reranker_enabled: bool,
    pub reranker_model_path: Option<Utf8PathBuf>,
    pub reranker_top_k: usize,
    pub reranker_cache_dir: Option<Utf8PathBuf>,

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

    // RRF config (RETR-05)
    pub rrf_enabled: bool,
    pub rrf_k: f32,
    pub rrf_keyword_weight: f32,
    pub rrf_vector_weight: f32,
    pub rrf_graph_weight: f32,

    // HyDE config (RETR-06, RETR-07)
    pub hyde_enabled: bool,
    pub hyde_llm_backend: String,
    pub hyde_api_key: Option<String>,
    pub hyde_max_tokens: usize,

    // Metrics config (PERF-04)
    pub metrics_enabled: bool,
    pub metrics_port: u16,

    // Package detection config (09-04)
    pub package_detection_enabled: bool,

    // LLM description generation config
    pub llm_enabled: bool,
    pub llm_device: EmbeddingsDevice,  // Reuse existing enum (Cpu/Metal)
    pub llm_model_dir: Option<Utf8PathBuf>,
    pub llm_max_tokens: u32,
    pub llm_batch_commit: usize,

    // Leader election config
    pub leader_election_enabled: bool,
    pub leader_heartbeat_interval_ms: u64,
    pub leader_ttl_seconds: u64,

    // Matryoshka embedding truncation
    /// Truncate full-dimension embeddings to this size after L2 re-normalization.
    /// `None` means use the model's native dimension (default).
    pub embedding_truncate_dim: Option<usize>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let base_dir_raw = required_env("BASE_DIR")?;
        let base_dir = canonicalize_dir(Path::new(&base_dir_raw))
            .with_context(|| format!("Invalid BASE_DIR: {base_dir_raw}"))?;

        let embeddings_backend_env = optional_env("EMBEDDINGS_BACKEND")
            .as_deref()
            .map(parse_embeddings_backend)
            .transpose()?;

        let (embeddings_backend, embeddings_model_dir) = match embeddings_backend_env {
            Some(EmbeddingsBackend::LlamaCpp) => {
                let embeddings_model_dir = match optional_env("EMBEDDINGS_MODEL_DIR").as_deref() {
                    Some(raw) => Some(to_utf8_pathbuf(Path::new(raw))?),
                    None => {
                        let data_dir = get_data_dir();
                        Some(data_dir.join("models/jina-code-embeddings-1.5b-gguf"))
                    }
                };
                (EmbeddingsBackend::LlamaCpp, embeddings_model_dir)
            }
            Some(EmbeddingsBackend::Hash) => (EmbeddingsBackend::Hash, None),
            None => {
                // Default to LlamaCpp with jina-code-1.5b GGUF
                let data_dir = get_data_dir();
                (
                    EmbeddingsBackend::LlamaCpp,
                    Some(data_dir.join("models/jina-code-embeddings-1.5b-gguf")),
                )
            }
        };

        // Per-repo data directory under ~/.code-intelligence/repos/<hash>/
        let repo_data_dir = get_repo_data_dir(base_dir.as_str());
        let data_dir = get_data_dir();
        let db_path = optional_env("DB_PATH")
            .map(|p| to_utf8_pathbuf(Path::new(&p)))
            .transpose()?
            .unwrap_or_else(|| repo_data_dir.join("code-intelligence.db"));

        let vector_db_path = optional_env("VECTOR_DB_PATH")
            .map(|p| to_utf8_pathbuf(Path::new(&p)))
            .transpose()?
            .unwrap_or_else(|| repo_data_dir.join("vectors"));

        let tantivy_index_path = optional_env("TANTIVY_INDEX_PATH")
            .map(|p| to_utf8_pathbuf(Path::new(&p)))
            .transpose()?
            .unwrap_or_else(|| repo_data_dir.join("tantivy-index"));

        let embeddings_device = optional_env("EMBEDDINGS_DEVICE")
            .as_deref()
            .map(parse_embeddings_device)
            .transpose()?
            .unwrap_or(EmbeddingsDevice::Metal);

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

        let vector_guaranteed_results = optional_env("VECTOR_GUARANTEED_RESULTS")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(3);

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
            .unwrap_or(1.0);
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
            &[
                "**/*.ts",
                "**/*.tsx",
                "**/*.js",
                "**/*.jsx",
                "**/*.rs",
                "**/*.py",
                "**/*.go",
                "**/*.java",
                "**/*.c",
                "**/*.h",
                "**/*.cpp",
                "**/*.cc",
                "**/*.cxx",
                "**/*.hpp",
            ],
        );

        let exclude_patterns = parse_csv_or_default(
            optional_env("EXCLUDE_PATTERNS").as_deref(),
            &[
                "**/node_modules/**",
                "**/dist/**",
                "**/build/**",
                "**/.git/**",
                "**/*.test.*",
                // Generated files
                "**/*.gen.*",
                "**/*.generated.*",
                // Minified files
                "**/*.min.*",
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
            .unwrap_or(2000);

        let watch_min_index_interval_ms = optional_env("WATCH_MIN_INDEX_INTERVAL_MS")
            .as_deref()
            .map(parse_u64)
            .transpose()?
            .unwrap_or(5000); // Default 5 seconds between index runs

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
        let reranker_enabled = optional_env("RERANKER_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(false);
        let reranker_model_path = optional_env("RERANKER_MODEL_PATH")
            .map(|p| to_utf8_pathbuf(Path::new(&p)))
            .transpose()?;
        let reranker_top_k = optional_env("RERANKER_TOP_K")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(20);
        let reranker_cache_dir = optional_env("RERANKER_CACHE_DIR")
            .map(|p| to_utf8_pathbuf(Path::new(&p)))
            .transpose()?
            .or_else(|| Some(data_dir.join("reranker-cache")));

        // Learning config (FNDN-04)
        let learning_enabled = optional_env("LEARNING_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(true);
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
        let token_encoding =
            optional_env("TOKEN_ENCODING").unwrap_or_else(|| "o200k_base".to_string());

        // Performance config (FNDN-06)
        // Default to half of available CPUs (minimum 2) to balance speed and contention
        // Parallel indexing can be enabled/tuned via PARALLEL_WORKERS env var
        let parallel_workers = optional_env("PARALLEL_WORKERS")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get().div_ceil(2))
                    .unwrap_or(2)
                    .max(2)
            });
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

        // RRF config (RETR-05)
        let rrf_enabled = optional_env("RRF_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(true); // Default enabled
        let rrf_k = optional_env("RRF_K")
            .as_deref()
            .map(parse_f32)
            .transpose()?
            .unwrap_or(60.0); // Standard RRF constant
        let rrf_keyword_weight = optional_env("RRF_KEYWORD_WEIGHT")
            .as_deref()
            .map(parse_f32)
            .transpose()?
            .unwrap_or(1.0);
        let rrf_vector_weight = optional_env("RRF_VECTOR_WEIGHT")
            .as_deref()
            .map(parse_f32)
            .transpose()?
            .unwrap_or(1.0);
        let rrf_graph_weight = optional_env("RRF_GRAPH_WEIGHT")
            .as_deref()
            .map(parse_f32)
            .transpose()?
            .unwrap_or(0.5); // Lower weight for graph

        // HyDE config (RETR-06, RETR-07)
        let hyde_enabled = optional_env("HYDE_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(false); // Default disabled (requires LLM)
        let hyde_llm_backend =
            optional_env("HYDE_LLM_BACKEND").unwrap_or_else(|| "openai".to_string());
        let hyde_api_key = optional_env("HYDE_API_KEY");
        let hyde_max_tokens = optional_env("HYDE_MAX_TOKENS")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(512);

        // Metrics config (PERF-04)
        let metrics_enabled = optional_env("METRICS_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(true);
        let metrics_port = optional_env("METRICS_PORT")
            .as_deref()
            .map(parse_u16)
            .transpose()?
            .unwrap_or(9090);

        // Package detection config (09-04)
        let package_detection_enabled = optional_env("PACKAGE_DETECTION_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(true); // Default enabled

        // LLM description generation config
        let llm_enabled = optional_env("LLM_ENABLED")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(true);
        let llm_device = optional_env("LLM_DEVICE")
            .as_deref()
            .map(parse_embeddings_device)
            .transpose()?
            .unwrap_or(EmbeddingsDevice::Cpu);
        let llm_model_dir = optional_env("LLM_MODEL_DIR")
            .map(|p| to_utf8_pathbuf(Path::new(&p)))
            .transpose()?
            .or_else(|| Some(data_dir.join("models/qwen2.5-coder-1.5b-gguf")));
        let llm_max_tokens: u32 = optional_env("LLM_MAX_TOKENS")
            .as_deref()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);
        let llm_batch_commit = optional_env("LLM_BATCH_COMMIT")
            .as_deref()
            .map(parse_usize)
            .transpose()?
            .unwrap_or(10);

        // Leader election config
        let leader_election_enabled = optional_env("LEADER_ELECTION")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(true);
        let leader_heartbeat_interval_ms = optional_env("LEADER_HEARTBEAT_MS")
            .as_deref()
            .map(parse_u64)
            .transpose()?
            .unwrap_or(10_000);
        let leader_ttl_seconds = optional_env("LEADER_TTL_SECONDS")
            .as_deref()
            .map(parse_u64)
            .transpose()?
            .unwrap_or(30);

        // Matryoshka embedding truncation
        let embedding_truncate_dim = match std::env::var("EMBEDDING_TRUNCATE_DIM") {
            Ok(val) => match val.parse::<usize>() {
                Ok(dim) => Some(dim),
                Err(_) => {
                    tracing::warn!(
                        value = %val,
                        "EMBEDDING_TRUNCATE_DIM is not a valid usize, ignoring"
                    );
                    None
                }
            },
            Err(_) => None,
        };

        Ok(Self {
            base_dir,
            db_path,
            vector_db_path,
            tantivy_index_path,
            embeddings_backend,
            embeddings_model_dir,
            embeddings_device,
            embedding_batch_size,
            hash_embedding_dim,
            vector_search_limit,
            vector_guaranteed_results,
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
            watch_min_index_interval_ms,
            max_context_bytes,
            index_node_modules,
            repo_roots,

            // Reranker config (FNDN-03)
            reranker_enabled,
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

            // RRF config (RETR-05)
            rrf_enabled,
            rrf_k,
            rrf_keyword_weight,
            rrf_vector_weight,
            rrf_graph_weight,

            // HyDE config (RETR-06, RETR-07)
            hyde_enabled,
            hyde_llm_backend,
            hyde_api_key,
            hyde_max_tokens,

            // Metrics config (PERF-04)
            metrics_enabled,
            metrics_port,

            // Package detection config (09-04)
            package_detection_enabled,

            // LLM description generation
            llm_enabled,
            llm_device,
            llm_model_dir,
            llm_max_tokens,
            llm_batch_commit,

            // Leader election
            leader_election_enabled,
            leader_heartbeat_interval_ms,
            leader_ttl_seconds,

            // Matryoshka embedding truncation
            embedding_truncate_dim,
        })
    }

    /// Normalize a path to be absolute relative to base directory.
    pub fn normalize_path_to_base(&self, path: &Path) -> Result<PathBuf> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.base_dir.as_std_path().join(path)
        };
        Ok(abs)
    }

    /// Get the relative path from base to the given path.
    pub fn path_relative_to_base(&self, path: &crate::path::Utf8Path) -> Result<String> {
        let normalizer = PathNormalizer::new(self.base_dir.clone());
        let relative = normalizer.relative_to_base(path)?;
        Ok(relative.as_str().to_string())
    }

    /// Get the relative path from base to the given path (PathBuf version for compatibility).
    pub fn path_relative_to_base_path(&self, path: &Path) -> Result<String> {
        let utf8_path = to_utf8_pathbuf(path)?;
        self.path_relative_to_base(&utf8_path)
    }
}

fn required_env(key: &str) -> Result<String> {
    env::var(key).map_err(|_| anyhow!("Missing required env var: {key}"))
}

/// Convert a std::path::Path to Utf8PathBuf, returning PathError on non-UTF-8.
fn to_utf8_pathbuf(path: &Path) -> Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(path.to_path_buf()).map_err(|_| PathError::NonUtf8 {
        path: path.to_path_buf(),
    }).map_err(|e| anyhow::anyhow!("{}", e))
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

fn canonicalize_dir(path: &Path) -> Result<Utf8PathBuf> {
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
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize: {}", path.display()))?;
    to_utf8_pathbuf(&canonical)
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
        "llamacpp" | "llama-cpp" | "llama" => Ok(EmbeddingsBackend::LlamaCpp),
        // Migration aliases: old names map to new LlamaCpp backend
        "jinacode" | "jina-code" | "jina" | "fastembed" => Ok(EmbeddingsBackend::LlamaCpp),
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

fn parse_u16(value: &str) -> Result<u16> {
    value
        .trim()
        .parse::<u16>()
        .map_err(|err| anyhow!("Invalid u16 '{value}': {err}"))
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

    fn tmp_dir() -> Utf8PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "code-intel-config-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        to_utf8_pathbuf(&dir).unwrap()
    }

    fn tmp_home_dir() -> Utf8PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "code-intel-home-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        to_utf8_pathbuf(&dir).unwrap()
    }

    fn clear_env() {
        for k in [
            "HOME", // Clear HOME for tests
            "BASE_DIR",
            "DB_PATH",
            "VECTOR_DB_PATH",
            "TANTIVY_INDEX_PATH",
            "EMBEDDINGS_BACKEND",
            "EMBEDDINGS_MODEL_DIR",
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
            "RERANKER_ENABLED",
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
            // RRF config (RETR-05)
            "RRF_ENABLED",
            "RRF_K",
            "RRF_KEYWORD_WEIGHT",
            "RRF_VECTOR_WEIGHT",
            "RRF_GRAPH_WEIGHT",
            // HyDE config (RETR-06, RETR-07)
            "HYDE_ENABLED",
            "HYDE_LLM_BACKEND",
            "HYDE_API_KEY",
            "HYDE_MAX_TOKENS",
            // Metrics config (PERF-04)
            "METRICS_ENABLED",
            "METRICS_PORT",
            // Package detection config (09-04)
            "PACKAGE_DETECTION_ENABLED",
            // LLM config
            "LLM_ENABLED",
            "LLM_DEVICE",
            "LLM_MODEL_DIR",
            "LLM_MAX_TOKENS",
            "LLM_BATCH_COMMIT",
            // Leader election config
            "LEADER_ELECTION",
            "LEADER_HEARTBEAT_MS",
            "LEADER_TTL_SECONDS",
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
    fn from_env_defaults_to_llamacpp_backend_without_model_dir() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        let home = tmp_home_dir();
        std::env::set_var("BASE_DIR", base.to_string());
        std::env::set_var("HOME", home.to_string());

        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.embeddings_backend, EmbeddingsBackend::LlamaCpp);
        assert!(cfg.embeddings_model_dir.is_some());

        // Paths should now use per-repo directory under ~/.code-intelligence/repos/<hash>/
        let repo_hash = crate::registry::RepoRegistry::path_hash(cfg.base_dir.as_str());
        let expected_repo_dir = home.join(".code-intelligence").join("repos").join(&repo_hash);
        assert_eq!(cfg.db_path, expected_repo_dir.join("code-intelligence.db"));
        assert_eq!(cfg.vector_db_path, expected_repo_dir.join("vectors"));
        assert_eq!(cfg.tantivy_index_path, expected_repo_dir.join("tantivy-index"));
        assert_eq!(cfg.repo_roots, vec![cfg.base_dir.clone()]);
    }

    #[test]
    fn llamacpp_backend_configured_by_default() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        let home = tmp_home_dir();
        std::env::set_var("BASE_DIR", base.to_string());
        std::env::set_var("HOME", home.to_string());

        // Default should be LlamaCpp
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.embeddings_backend, EmbeddingsBackend::LlamaCpp);
        assert_eq!(
            cfg.embeddings_model_dir,
            Some(home.join(".code-intelligence/models/jina-code-embeddings-1.5b-gguf"))
        );
    }

    #[test]
    fn legacy_backend_names_map_to_llamacpp() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        let custom = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string());
        // Old "fastembed" name should now map to LlamaCpp
        std::env::set_var("EMBEDDINGS_BACKEND", "fastembed");
        std::env::set_var("EMBEDDINGS_MODEL_DIR", custom.to_string());

        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.embeddings_backend, EmbeddingsBackend::LlamaCpp);
        assert_eq!(cfg.embeddings_model_dir, Some(custom));
    }

    #[test]
    fn llamacpp_backend_defaults_if_backend_not_set() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string());
        // No backend set, should default to LlamaCpp

        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.embeddings_backend, EmbeddingsBackend::LlamaCpp);
        assert!(cfg.embeddings_model_dir.is_some());
    }

    #[test]
    fn repo_roots_parses_and_dedupes() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        let extra = tmp_dir();
        // Canonicalize the extra path for comparison since repo_roots contains canonicalized paths
        let extra_canonical = to_utf8_pathbuf(&std::fs::canonicalize(&extra).unwrap()).unwrap();
        std::env::set_var("BASE_DIR", base.to_string());
        std::env::set_var(
            "REPO_ROOTS",
            format!(
                "  {} , {} , {} ",
                extra.as_str(),
                extra.as_str(),
                base.as_str()
            ),
        );

        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.repo_roots.len(), 2);
        assert!(cfg.repo_roots.contains(&cfg.base_dir));
        assert!(cfg.repo_roots.contains(&extra_canonical));
    }

    #[test]
    fn hybrid_alpha_validation_and_weight_defaults() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string());
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
        std::env::set_var("BASE_DIR", base.to_string());
        let cfg = Config::from_env().unwrap();
        assert!(cfg.watch_mode);
    }

    #[test]
    fn bool_parsing_accepts_multiple_spellings() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string());
        std::env::set_var("WATCH_MODE", "yes");
        std::env::set_var("INDEX_NODE_MODULES", "1");
        let cfg = Config::from_env().unwrap();
        assert!(cfg.watch_mode);
        assert!(cfg.index_node_modules);
    }

    #[test]
    fn new_config_fields_have_defaults() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string());

        let cfg = Config::from_env().unwrap();

        // Reranker defaults
        assert!(!cfg.reranker_enabled);
        assert!(cfg.reranker_model_path.is_none());
        assert_eq!(cfg.reranker_top_k, 20);
        assert!(cfg.reranker_cache_dir.is_some());

        // Learning defaults
        assert!(cfg.learning_enabled);
        assert!((cfg.learning_selection_boost - 0.1).abs() < f32::EPSILON);
        assert!((cfg.learning_file_affinity_boost - 0.05).abs() < f32::EPSILON);

        // Token defaults
        assert_eq!(cfg.max_context_tokens, 8192);
        assert_eq!(cfg.token_encoding, "o200k_base");

        // Performance defaults
        assert!(cfg.parallel_workers >= 2);
        assert!(cfg.embedding_cache_enabled);

        // PageRank defaults
        assert!((cfg.pagerank_damping - 0.85).abs() < f32::EPSILON);
        assert_eq!(cfg.pagerank_iterations, 20);

        // Query expansion defaults (FNDN-02)
        assert!(cfg.synonym_expansion_enabled);
        assert!(cfg.acronym_expansion_enabled);

        // LLM defaults
        assert!(cfg.llm_enabled);
        assert_eq!(cfg.llm_device, EmbeddingsDevice::Cpu);
        assert!(cfg.llm_model_dir.is_some());
        assert_eq!(cfg.llm_max_tokens, 30);
        assert_eq!(cfg.llm_batch_commit, 10);
    }

    #[test]
    fn new_config_fields_parsed_from_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let base = tmp_dir();
        std::env::set_var("BASE_DIR", base.to_string());
        std::env::set_var("LEARNING_ENABLED", "true");
        std::env::set_var("MAX_CONTEXT_TOKENS", "16384");
        std::env::set_var("PAGERANK_DAMPING", "0.9");
        std::env::set_var("PARALLEL_WORKERS", "4");
        std::env::set_var("SYNONYM_EXPANSION_ENABLED", "false");
        std::env::set_var("ACRONYM_EXPANSION_ENABLED", "false");

        let cfg = Config::from_env().unwrap();

        assert!(cfg.learning_enabled);
        assert_eq!(cfg.max_context_tokens, 16384);
        assert!((cfg.pagerank_damping - 0.9).abs() < f32::EPSILON);
        assert_eq!(cfg.parallel_workers, 4);
        assert!(!cfg.synonym_expansion_enabled);
        assert!(!cfg.acronym_expansion_enabled);
    }

    #[test]
    fn standalone_config_defaults() {
        let cfg = StandaloneConfig::default();
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 3333);
        assert_eq!(cfg.warm_ttl_seconds, 300);
        assert!(cfg.data_dir.to_string().contains("code-intelligence"));
    }

    #[test]
    fn standalone_config_from_toml() {
        let toml_str = r#"
[server]
host = "0.0.0.0"
port = 4444

[lifecycle]
warm_ttl_seconds = 600
"#;
        let cfg = StandaloneConfig::from_toml_str(toml_str).unwrap();
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 4444);
        assert_eq!(cfg.warm_ttl_seconds, 600);
    }

    #[test]
    fn standalone_config_repo_config_has_all_fields() {
        let standalone = StandaloneConfig::default();
        let repo_path = tmp_dir();
        let repo_data_dir = tmp_dir();

        let cfg = standalone.repo_config(repo_path.clone(), &repo_data_dir);

        // Verify all Config fields are properly set
        assert_eq!(cfg.base_dir, repo_path);
        assert_eq!(cfg.db_path, repo_data_dir.join("code-intelligence.db"));
        assert_eq!(cfg.vector_db_path, repo_data_dir.join("vectors"));
        assert_eq!(cfg.tantivy_index_path, repo_data_dir.join("tantivy-index"));
        assert_eq!(cfg.embeddings_backend, standalone.embeddings_backend);
        assert_eq!(cfg.embeddings_device, standalone.embeddings_device);
        assert_eq!(cfg.index_patterns, standalone.default_index_patterns);
        assert_eq!(cfg.exclude_patterns, standalone.default_exclude_patterns);
        assert_eq!(cfg.watch_mode, standalone.default_watch_mode);
        assert_eq!(cfg.hash_embedding_dim, standalone.hash_embedding_dim);
    }

    /// Verify that the default INDEX_PATTERNS cover every extension that
    /// `language_id_for_path` (in `src/indexer/parser.rs`) recognises.
    ///
    /// Supported extensions and the glob pattern that should cover them:
    ///   .ts   → **/*.ts          .tsx  → **/*.tsx
    ///   .js   → **/*.js          .jsx  → **/*.jsx
    ///   .rs   → **/*.rs
    ///   .py   → **/*.py
    ///   .go   → **/*.go
    ///   .java → **/*.java
    ///   .c    → **/*.c           .h    → **/*.h
    ///   .cpp  → **/*.cpp         .cc   → **/*.cc
    ///   .cxx  → **/*.cxx         .hpp  → **/*.hpp
    #[test]
    fn default_index_patterns_cover_all_supported_languages() {
        // All extensions that `language_id_for_path` handles
        let required_extensions = [
            "ts", "tsx", "js", "jsx", "rs", "py", "go", "java", "c", "h",
            "cpp", "cc", "cxx", "hpp",
        ];

        // ── Config::from_env defaults ────────────────────────────────────────
        let env_defaults =
            parse_csv_or_default(None, &["**/*.ts", "**/*.tsx", "**/*.rs"]);
        // (We read the actual default by calling parse_csv_or_default with None
        //  so this test stays in sync with the real code path.)
        let from_env_defaults: Vec<String> = {
            let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            clear_env();
            // Minimal required env — Config::from_env needs BASE_DIR
            std::env::set_var("BASE_DIR", "/tmp");
            let cfg = Config::from_env().expect("from_env must succeed");
            cfg.index_patterns
        };
        for ext in &required_extensions {
            let pattern = format!("**/*.{ext}");
            assert!(
                from_env_defaults.contains(&pattern),
                "Config::from_env default index_patterns is missing pattern '{pattern}' \
                 (extension .{ext} is supported by language_id_for_path but not indexed by default)"
            );
        }
        drop(env_defaults); // silence unused-variable warning

        // ── StandaloneConfig::default patterns ──────────────────────────────
        let standalone_defaults = StandaloneConfig::default().default_index_patterns;
        for ext in &required_extensions {
            let pattern = format!("**/*.{ext}");
            assert!(
                standalone_defaults.contains(&pattern),
                "StandaloneConfig::default().default_index_patterns is missing pattern '{pattern}' \
                 (extension .{ext} is supported by language_id_for_path but not indexed by default)"
            );
        }
    }
}

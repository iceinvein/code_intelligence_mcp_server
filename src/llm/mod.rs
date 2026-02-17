//! LLM-based symbol description generation.
//!
//! This module provides infrastructure for generating natural language descriptions
//! of code symbols using local LLM inference. Descriptions are appended to the
//! Tantivy text field to improve search relevance for semantic queries.

use anyhow::Result;
use once_cell::sync::OnceCell;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use crate::path::Utf8Path;
use llama_cpp_2::llama_backend::LlamaBackend;

pub mod llamacpp;

/// Process-wide singleton for the llama.cpp backend.
///
/// `LlamaBackend::init()` has an internal `AtomicBool` guard that panics on
/// double-init. We `Box::leak` the backend to get a `&'static` reference
/// that both the LLM generator and embedding model can share. The backend
/// lives for the entire process — its `Drop` never runs, which is correct
/// since freeing it would invalidate all loaded models.
static LLAMA_BACKEND: OnceCell<&'static LlamaBackend> = OnceCell::new();

pub fn get_or_init_backend() -> anyhow::Result<&'static LlamaBackend> {
    LLAMA_BACKEND
        .get_or_try_init(|| {
            let backend = LlamaBackend::init()
                .map_err(|e| anyhow::anyhow!("Failed to init llama.cpp backend: {:?}", e))?;
            Ok::<&'static LlamaBackend, anyhow::Error>(Box::leak(Box::new(backend)))
        })
        .copied()
}

/// Generate text descriptions for code symbols using an LLM.
pub trait LlmGenerator: Send + Sync {
    /// Generate a description given a prompt.
    ///
    /// # Arguments
    /// * `prompt` - The formatted prompt (should use Qwen2.5 chat template)
    /// * `max_tokens` - Maximum tokens to generate
    ///
    /// # Returns
    /// Generated text description
    fn generate(&self, prompt: &str, max_tokens: u32) -> Result<String>;
}

/// Mock LLM generator for testing.
///
/// Returns deterministic descriptions derived from the prompt without
/// requiring model inference.
pub struct MockLlmGenerator;

impl LlmGenerator for MockLlmGenerator {
    fn generate(&self, prompt: &str, _max_tokens: u32) -> Result<String> {
        // Extract the first line from the user section
        // Format: <|im_start|>user\n{kind} {name} in {module_path}:\n{body}<|im_end|>
        let user_start = prompt.find("<|im_start|>user\n");
        let user_end = prompt.find("<|im_end|>\n<|im_start|>assistant\n");

        if let (Some(start), Some(end)) = (user_start, user_end) {
            let user_content = &prompt[start + "<|im_start|>user\n".len()..end];
            let first_line = user_content.lines().next().unwrap_or("unknown");

            // Parse "{kind} {name} in {filename}:"
            if let Some(colon_pos) = first_line.rfind(':') {
                let header = &first_line[..colon_pos];
                Ok(format!("Mock description for {}", header))
            } else {
                Ok(format!("Mock description for {}", first_line))
            }
        } else {
            Ok("Mock description".to_string())
        }
    }
}

/// Build a Qwen2.5-formatted prompt for symbol description generation.
///
/// # Arguments
/// * `name` - Symbol name (e.g., "PathNormalizer")
/// * `kind` - Symbol kind (e.g., "struct", "function")
/// * `file_path` - Full path to the file (e.g., "src/indexer/pipeline/mod.rs")
/// * `body` - Symbol body text (will be truncated to first 10 lines)
///
/// # Returns
/// Formatted prompt using Qwen2.5 chat template
pub fn build_description_prompt(name: &str, kind: &str, file_path: &str, body: &str) -> String {
    // Use the full module path (e.g., "src/retrieval/ranking/score.rs") instead of
    // just the filename. This gives the LLM critical context about the module's
    // purpose — "score.rs" is ambiguous, but "retrieval/ranking/score.rs" tells
    // the model this is about search ranking and scoring.
    let module_path = file_path
        .strip_prefix("src/")
        .unwrap_or(file_path);

    // Truncate body to first 10 lines
    let truncated_body: String = body
        .lines()
        .take(10)
        .collect::<Vec<_>>()
        .join("\n");

    // Build Qwen2.5 chat template with an enhanced system prompt that requests
    // domain-specific vocabulary and technology names. The generic "Describe what
    // this code does" prompt produced descriptions like "This function ranks hits
    // based on signals" — missing "scoring", "BM25", "search ranking" etc.
    format!(
        "<|im_start|>system\nDescribe this code in one sentence. \
         Name specific libraries, technologies, and domain concepts. \
         Use terms a developer would search for.<|im_end|>\n\
         <|im_start|>user\n{} {} in {}:\n{}<|im_end|>\n\
         <|im_start|>assistant\n",
        kind, name, module_path, truncated_body
    )
}

/// Compute a content hash for caching LLM descriptions.
///
/// Includes a version prefix so that prompt changes invalidate all cached
/// descriptions and trigger regeneration. Bump PROMPT_VERSION when the
/// prompt format in `build_description_prompt` changes materially.
///
/// # Arguments
/// * `name` - Symbol name
/// * `kind` - Symbol kind
/// * `body` - Symbol body text (will be truncated to first 10 lines)
///
/// # Returns
/// SHA-256 hex string of format "v{N}:name:kind:first_10_lines"
pub fn compute_content_hash(name: &str, kind: &str, body: &str) -> String {
    // Bump when prompt format changes to invalidate cached descriptions.
    // v2: Enhanced system prompt + full module path (was filename-only).
    // v3: Switched to 3B model (was 1.5B) for more discriminating descriptions.
    const PROMPT_VERSION: &str = "v2";

    let truncated_body: String = body
        .lines()
        .take(10)
        .collect::<Vec<_>>()
        .join("\n");

    let input = format!("{}:{}:{}:{}", PROMPT_VERSION, name, kind, truncated_body);
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();

    hex::encode(result)
}

/// HuggingFace repository for the GGUF-format Qwen2.5-Coder model.
const HF_REPO: &str = "Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF";
/// Q4_K_M quantized GGUF model (~2.1 GB). GGUF embeds the tokenizer,
/// so no separate tokenizer.json download is needed.
const HF_MODEL_FILE: &str = "qwen2.5-coder-1.5b-instruct-q4_k_m.gguf";

/// Create an LLM generator based on config.
///
/// Returns `None` if LLM is disabled or model directory is not configured.
/// Auto-downloads the model from HuggingFace on first launch if not present.
pub fn create_llm_generator(
    config: &crate::config::Config,
) -> anyhow::Result<Option<Arc<dyn LlmGenerator>>> {
    if !config.llm_enabled {
        tracing::info!("LLM descriptions disabled (LLM_ENABLED=false)");
        return Ok(None);
    }

    let model_dir = match &config.llm_model_dir {
        Some(dir) => dir.clone(),
        None => {
            tracing::info!("No LLM model directory configured, descriptions disabled");
            return Ok(None);
        }
    };

    // GGUF model file path
    let model_file = model_dir.join(HF_MODEL_FILE);

    // Auto-download if model not found
    if !model_file.exists() {
        tracing::info!("LLM model not found at {}, attempting auto-download...", model_file);
        match download_model(&model_dir) {
            Ok(()) => {
                tracing::info!("LLM model downloaded successfully");
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to download LLM model: {}. Set LLM_ENABLED=false to suppress.",
                    e
                );
                return Ok(None);
            }
        }
    }

    // LLM_DEVICE is ignored — llama.cpp auto-detects Metal on macOS.
    let generator = llamacpp::LlamaCppGenerator::new(&model_file)?;
    Ok(Some(Arc::new(generator)))
}

/// Download the Qwen2.5-Coder-1.5B-Instruct GGUF model from HuggingFace.
///
/// Downloads a single GGUF file (~1.1 GB) into the HuggingFace cache
/// (`~/.cache/huggingface/hub/`), then creates a symlink in `target_dir`.
/// GGUF embeds the tokenizer, so only one file is needed.
pub fn download_model(target_dir: &Utf8Path) -> anyhow::Result<()> {
    use anyhow::Context;

    tracing::info!("Downloading LLM model from huggingface.co/{}", HF_REPO);

    let api = hf_hub::api::sync::Api::new()
        .context("Failed to initialize HuggingFace Hub API")?;
    let repo = api.model(HF_REPO.to_string());

    // Download GGUF model (~1.1 GB) — includes weights + tokenizer
    tracing::info!("Downloading {} (~1.1 GB, this may take a few minutes)...", HF_MODEL_FILE);
    let model_cached = repo.get(HF_MODEL_FILE)
        .context("Failed to download GGUF model file")?;

    // Create target directory
    std::fs::create_dir_all(target_dir.as_std_path())
        .context("Failed to create model directory")?;

    // Symlink from HF cache into our model directory
    let target_model = target_dir.join(HF_MODEL_FILE);
    symlink_or_copy(&model_cached, target_model.as_std_path())
        .context("Failed to link GGUF model file")?;

    tracing::info!("LLM model ready at {}", target_dir);
    Ok(())
}

/// Create a symlink from `source` to `target`.
fn symlink_or_copy(source: &std::path::Path, target: &std::path::Path) -> anyhow::Result<()> {
    // Remove existing target (stale symlink or old file)
    if target.exists() || target.symlink_metadata().is_ok() {
        std::fs::remove_file(target).ok();
    }

    std::os::unix::fs::symlink(source, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_description_prompt() {
        let prompt = build_description_prompt(
            "PathNormalizer",
            "struct",
            "src/path/mod.rs",
            "pub struct PathNormalizer {\n    base: Utf8PathBuf,\n}\n\nimpl PathNormalizer {\n    pub fn new(base: Utf8PathBuf) -> Self {\n        Self { base }\n    }\n    \n    pub fn normalize(&self, path: &Utf8Path) -> Result<Utf8PathBuf> {\n        // normalize logic\n        Ok(path.to_path_buf())\n    }\n}\n\n// More lines that should be truncated\n// Line 12\n// Line 13",
        );

        // Check structure
        assert!(prompt.contains("<|im_start|>system\n"));
        assert!(prompt.contains("Describe this code in one sentence."));
        assert!(prompt.contains("Name specific libraries"));
        assert!(prompt.contains("<|im_end|>\n<|im_start|>user\n"));
        // Uses full module path (strip "src/" prefix)
        assert!(prompt.contains("struct PathNormalizer in path/mod.rs:"));
        assert!(prompt.contains("pub struct PathNormalizer"));
        assert!(prompt.contains("<|im_end|>\n<|im_start|>assistant\n"));

        // Verify truncation (should have first 10 lines)
        let body_section = prompt.split("in path/mod.rs:\n").nth(1).unwrap();
        let body_section = body_section.split("<|im_end|>").next().unwrap();
        let line_count = body_section.lines().count();
        assert_eq!(line_count, 10, "Body should be truncated to 10 lines");

        // Verify lines 12-13 are NOT present
        assert!(!prompt.contains("Line 12"));
        assert!(!prompt.contains("Line 13"));
    }

    #[test]
    fn test_build_description_prompt_short_body() {
        let prompt = build_description_prompt(
            "foo",
            "function",
            "src/lib.rs",
            "fn foo() {\n    println!(\"hello\");\n}",
        );

        // "src/lib.rs" → strip "src/" → "lib.rs"
        assert!(prompt.contains("function foo in lib.rs:"));
        assert!(prompt.contains("fn foo()"));
        assert!(prompt.contains("println!"));
    }

    #[test]
    fn test_build_description_prompt_uses_module_path() {
        let prompt = build_description_prompt(
            "test",
            "function",
            "src/retrieval/ranking/score.rs",
            "fn test() {}",
        );

        // Full module path with "src/" stripped — gives LLM context about the module
        assert!(prompt.contains("in retrieval/ranking/score.rs:"));
    }

    #[test]
    fn test_build_description_prompt_non_src_path() {
        let prompt = build_description_prompt(
            "test",
            "function",
            "very/long/path/to/file.rs",
            "fn test() {}",
        );

        // No "src/" prefix to strip — uses full path as-is
        assert!(prompt.contains("in very/long/path/to/file.rs:"));
    }

    #[test]
    fn test_compute_content_hash() {
        let hash1 = compute_content_hash(
            "PathNormalizer",
            "struct",
            "pub struct PathNormalizer {\n    base: Utf8PathBuf,\n}",
        );

        // SHA-256 produces 64 hex characters
        assert_eq!(hash1.len(), 64);
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()));

        // Same input produces same hash
        let hash2 = compute_content_hash(
            "PathNormalizer",
            "struct",
            "pub struct PathNormalizer {\n    base: Utf8PathBuf,\n}",
        );
        assert_eq!(hash1, hash2);

        // Different input produces different hash
        let hash3 = compute_content_hash(
            "PathNormalizer",
            "struct",
            "pub struct PathNormalizer {\n    base: PathBuf,\n}",  // Changed type
        );
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_compute_content_hash_truncates_body() {
        let long_body = (0..20)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let hash1 = compute_content_hash("foo", "function", &long_body);

        // Hash should only include first 10 lines
        let short_body = (0..10)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let hash2 = compute_content_hash("foo", "function", &short_body);
        assert_eq!(hash1, hash2, "Hash should only use first 10 lines");
    }

    #[test]
    fn test_mock_llm_generator() {
        let generator = MockLlmGenerator;

        let prompt = build_description_prompt(
            "handle_request",
            "function",
            "src/server/handler.rs",
            "pub fn handle_request() -> Result<Response> {\n    Ok(Response::new())\n}",
        );

        let result = generator.generate(&prompt, 50).unwrap();
        // Module path is "server/handler.rs" (src/ stripped)
        assert!(result.contains("Mock description for function handle_request in server/handler.rs"));
    }

    #[test]
    fn test_mock_llm_generator_malformed_prompt() {
        let generator = MockLlmGenerator;
        let result = generator.generate("invalid prompt", 50).unwrap();
        assert_eq!(result, "Mock description");
    }

    #[test]
    fn test_mock_llm_generator_empty_prompt() {
        let generator = MockLlmGenerator;
        let result = generator.generate("", 50).unwrap();
        assert_eq!(result, "Mock description");
    }
}

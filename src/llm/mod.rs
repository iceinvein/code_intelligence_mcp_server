//! LLM-based symbol description generation.
//!
//! This module provides infrastructure for generating natural language descriptions
//! of code symbols using local LLM inference. Descriptions are appended to the
//! Tantivy text field to improve search relevance for semantic queries.

use anyhow::Result;
use sha2::{Digest, Sha256};

pub mod ort;

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
        // Format: <|im_start|>user\n{kind} {name} in {filename}:\n{body}<|im_end|>
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
    // Extract filename from path
    let filename = std::path::Path::new(file_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(file_path);

    // Truncate body to first 10 lines
    let truncated_body: String = body
        .lines()
        .take(10)
        .collect::<Vec<_>>()
        .join("\n");

    // Build Qwen2.5 chat template
    format!(
        "<|im_start|>system\nDescribe what this code does in one sentence.<|im_end|>\n\
         <|im_start|>user\n{} {} in {}:\n{}<|im_end|>\n\
         <|im_start|>assistant\n",
        kind, name, filename, truncated_body
    )
}

/// Compute a content hash for caching LLM descriptions.
///
/// # Arguments
/// * `name` - Symbol name
/// * `kind` - Symbol kind
/// * `body` - Symbol body text (will be truncated to first 10 lines)
///
/// # Returns
/// SHA-256 hex string of format "name:kind:first_10_lines"
pub fn compute_content_hash(name: &str, kind: &str, body: &str) -> String {
    let truncated_body: String = body
        .lines()
        .take(10)
        .collect::<Vec<_>>()
        .join("\n");

    let input = format!("{}:{}:{}", name, kind, truncated_body);
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();

    hex::encode(result)
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
        assert!(prompt.contains("Describe what this code does in one sentence."));
        assert!(prompt.contains("<|im_end|>\n<|im_start|>user\n"));
        assert!(prompt.contains("struct PathNormalizer in mod.rs:"));
        assert!(prompt.contains("pub struct PathNormalizer"));
        assert!(prompt.contains("<|im_end|>\n<|im_start|>assistant\n"));

        // Verify truncation (should have first 10 lines)
        let body_section = prompt.split("in mod.rs:\n").nth(1).unwrap();
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

        assert!(prompt.contains("function foo in lib.rs:"));
        assert!(prompt.contains("fn foo()"));
        assert!(prompt.contains("println!"));
    }

    #[test]
    fn test_build_description_prompt_extracts_filename() {
        let prompt = build_description_prompt(
            "test",
            "function",
            "very/long/path/to/file.rs",
            "fn test() {}",
        );

        assert!(prompt.contains("in file.rs:"));
        assert!(!prompt.contains("very/long/path"));
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
        assert!(result.contains("Mock description for function handle_request in handler.rs"));
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

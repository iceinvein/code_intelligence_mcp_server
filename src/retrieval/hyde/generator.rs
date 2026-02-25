//! Hypothetical code generation for HyDE
//!
//! This module provides LLM-based generation of hypothetical code snippets
//! that can be embedded for better semantic retrieval.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::Arc;

use crate::llm::LlmGenerator;

/// HyDE (Hypothetical Document Embeddings) query result
#[derive(Debug, Clone)]
pub struct HyDEQuery {
    /// Original user query
    pub original_query: String,
    /// Generated hypothetical code snippet
    pub hypothetical_code: String,
    /// Language hint for generation
    pub language: String,
}

/// Generator for hypothetical code snippets using HyDE
///
/// Supports multiple backends:
/// - `"openai"` — OpenAI Chat Completions API (gpt-4o-mini)
/// - `"anthropic"` — Anthropic Messages API (claude-3-haiku)
/// - `"local"` — On-device Qwen2.5-Coder-1.5B via llama.cpp (no network)
/// - `"mock"` — Deterministic stub for unit tests
#[derive(Clone)]
pub struct HypotheticalCodeGenerator {
    /// LLM backend ("openai", "anthropic", "local", or "mock" for testing)
    backend: String,
    /// API key for the LLM service (not required for "local" or "mock")
    api_key: Option<String>,
    /// Maximum tokens for generated code
    max_tokens: usize,
    /// Local LLM generator (used when backend == "local")
    ///
    /// Wrapped in `Arc` so that `HypotheticalCodeGenerator` remains cheaply
    /// `Clone`-able — cloning increments the reference count rather than
    /// duplicating model weights.
    pub local_llm: Option<Arc<dyn LlmGenerator>>,
}

impl HypotheticalCodeGenerator {
    /// Create a new HyDE generator.
    ///
    /// The `local_llm` field is initialised to `None`; call
    /// [`with_local_llm`](Self::with_local_llm) to attach an on-device model
    /// when using the `"local"` backend.
    pub fn new(backend: String, api_key: Option<String>, max_tokens: usize) -> Self {
        Self {
            backend,
            api_key,
            max_tokens,
            local_llm: None,
        }
    }

    /// Attach a local LLM generator (builder-style).
    ///
    /// Required when `backend == "local"`. The generator is shared via `Arc`,
    /// so multiple `HypotheticalCodeGenerator` clones share the same loaded
    /// model weights.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let gen = HypotheticalCodeGenerator::new("local".to_string(), None, 256)
    ///     .with_local_llm(Arc::new(my_llm_generator));
    /// ```
    #[must_use]
    pub fn with_local_llm(mut self, llm: Arc<dyn LlmGenerator>) -> Self {
        self.local_llm = Some(llm);
        self
    }

    /// Generate hypothetical code for a query.
    ///
    /// Dispatches to the appropriate backend. Failures from the `"local"`
    /// backend propagate as errors; callers in the hot search path should treat
    /// HyDE results as best-effort and handle errors gracefully.
    pub async fn generate(&self, query: &str, language: &str) -> Result<HyDEQuery> {
        let prompt = self.build_prompt(query, language);

        match self.backend.as_str() {
            "openai" => self.generate_openai(&prompt, language).await,
            "anthropic" => self.generate_anthropic(&prompt, language).await,
            "local" => self.generate_local(query, language).await,
            "mock" => self.generate_mock(query, language),
            _ => Err(anyhow::anyhow!("Unknown LLM backend: {}", self.backend)),
        }
    }

    /// Build the generic multi-backend prompt (used by openai / anthropic).
    fn build_prompt(&self, query: &str, language: &str) -> String {
        format!(
            r#"Given this question about {} code: "{}"

Please write a detailed, hypothetical code snippet that would answer this question.
The code should be:
- Well-commented and idiomatic {}
- Include type definitions and function signatures
- Demonstrate the pattern or concept being asked about

Respond ONLY with the code snippet, no explanation."#,
            language, query, language
        )
    }

    /// Build the Qwen2.5 chat-template prompt for on-device HyDE generation.
    ///
    /// Asks the model for a SHORT, focused code snippet rather than a complete
    /// implementation. Shorter outputs (≤ ~100 tokens) reduce per-query latency
    /// (~300 ms on the 1.5B model) while still producing a useful embedding
    /// anchor for vector search.
    fn build_local_prompt(&self, query: &str, language: &str) -> String {
        format!(
            "<|im_start|>system\n\
             You are a code search assistant. Write a short {language} code snippet \
             (function signature, struct, or a few lines) that would be found when \
             searching for: \"{query}\". \
             Respond ONLY with code, no explanation, no markdown fences.<|im_end|>\n\
             <|im_start|>user\n\
             Search query: {query}\n\
             Language: {language}<|im_end|>\n\
             <|im_start|>assistant\n",
            language = language,
            query = query,
        )
    }

    /// Generate a hypothetical code snippet using the on-device llama.cpp LLM.
    ///
    /// The `LlmGenerator::generate` implementation is synchronous (it drives the
    /// llama.cpp inference loop on the calling thread). We hand it off to a
    /// `spawn_blocking` thread so the async executor is not stalled during the
    /// ~300 ms inference window.
    async fn generate_local(&self, query: &str, language: &str) -> Result<HyDEQuery> {
        let llm = self
            .local_llm
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "HyDE backend is 'local' but no local LLM was configured. \
                     Call with_local_llm() before using the 'local' backend."
                )
            })?
            .clone(); // Arc clone — cheap

        let prompt = self.build_local_prompt(query, language);
        let max_tokens = u32::try_from(self.max_tokens).unwrap_or(256);

        // Offload synchronous inference to a blocking thread so we do not stall
        // the async executor. `LlamaModel` is Send but `LlamaContext` (created
        // inside `generate`) is !Send — that is fine since spawn_blocking runs
        // the entire closure on a dedicated OS thread.
        let hypothetical_code = tokio::task::spawn_blocking(move || llm.generate(&prompt, max_tokens))
            .await
            .context("Local HyDE inference thread panicked")??;

        let hypothetical_code = extract_code_from_markdown(&hypothetical_code);

        Ok(HyDEQuery {
            original_query: query.to_string(),
            hypothetical_code,
            language: language.to_string(),
        })
    }

    async fn generate_openai(&self, prompt: &str, language: &str) -> Result<HyDEQuery> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("OpenAI API key not set"))?;

        let client = reqwest::Client::new();
        let response = client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&serde_json::json!({
                "model": "gpt-4o-mini",
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": self.max_tokens,
                "temperature": 0.7,
            }))
            .send()
            .await
            .context("OpenAI API request failed")?;

        let response_text: OpenAIResponse = response
            .json()
            .await
            .context("Failed to parse OpenAI response")?;

        let hypothetical = response_text
            .choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_else(|| prompt.to_string());

        // Extract code from markdown code blocks if present
        let hypothetical = extract_code_from_markdown(&hypothetical);

        Ok(HyDEQuery {
            original_query: prompt.to_string(),
            hypothetical_code: hypothetical,
            language: language.to_string(),
        })
    }

    async fn generate_anthropic(&self, prompt: &str, language: &str) -> Result<HyDEQuery> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Anthropic API key not set"))?;

        let client = reqwest::Client::new();
        let response = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&serde_json::json!({
                "model": "claude-3-haiku-20240307",
                "max_tokens": self.max_tokens,
                "messages": [{"role": "user", "content": prompt}],
            }))
            .send()
            .await
            .context("Anthropic API request failed")?;

        let response_text: AnthropicResponse = response
            .json()
            .await
            .context("Failed to parse Anthropic response")?;

        let hypothetical = response_text
            .content
            .first()
            .map(|c| c.text.trim().to_string())
            .unwrap_or_else(|| prompt.to_string());

        let hypothetical = extract_code_from_markdown(&hypothetical);

        Ok(HyDEQuery {
            original_query: prompt.to_string(),
            hypothetical_code: hypothetical,
            language: language.to_string(),
        })
    }

    fn generate_mock(&self, query: &str, language: &str) -> Result<HyDEQuery> {
        // Mock implementation for testing without API calls
        let mock_code = format!(
            "// Hypothetical {} code for: {}\n// Implementation would go here",
            language, query
        );

        Ok(HyDEQuery {
            original_query: query.to_string(),
            hypothetical_code: mock_code,
            language: language.to_string(),
        })
    }
}

fn extract_code_from_markdown(text: &str) -> String {
    // Extract code from markdown code blocks
    if let Some(start) = text.find("```") {
        let after_start = &text[start + 3..];
        if let Some(lang_end) = after_start.find('\n') {
            let potential_code = &after_start[lang_end + 1..];
            if let Some(end) = potential_code.find("```") {
                return potential_code[..end].trim().to_string();
            }
        }
    }
    text.to_string()
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAIMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContent {
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmGenerator;

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    /// Build a generator using the `"local"` backend wired to a `MockLlmGenerator`.
    fn local_generator_with_mock(max_tokens: usize) -> HypotheticalCodeGenerator {
        HypotheticalCodeGenerator::new("local".to_string(), None, max_tokens)
            .with_local_llm(Arc::new(MockLlmGenerator))
    }

    // ---------------------------------------------------------------------------
    // Existing tests (unchanged behaviour)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_mock_generation() {
        let generator = HypotheticalCodeGenerator::new("mock".to_string(), None, 512);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async { generator.generate("how to parse JSON", "rust").await });

        assert!(result.is_ok());
        let hyde = result.unwrap();
        assert!(hyde.hypothetical_code.contains("rust"));
        assert!(hyde.hypothetical_code.contains("JSON"));
    }

    #[test]
    fn test_extract_code_from_markdown() {
        let input = "```rust\nfn hello() {}\n```";
        let output = extract_code_from_markdown(input);
        assert_eq!(output.trim(), "fn hello() {}");
    }

    #[test]
    fn test_extract_code_with_language() {
        let input = "```typescript\ninterface User {\n  name: string;\n}\n```";
        let output = extract_code_from_markdown(input);
        assert!(output.contains("interface User"));
        assert!(output.contains("name: string"));
    }

    #[test]
    fn test_extract_code_no_markdown() {
        let input = "fn hello() {}";
        let output = extract_code_from_markdown(input);
        assert_eq!(output, "fn hello() {}");
    }

    #[test]
    fn test_hyde_query_structure() {
        let hyde = HyDEQuery {
            original_query: "test query".to_string(),
            hypothetical_code: "fn test() {}".to_string(),
            language: "rust".to_string(),
        };

        assert_eq!(hyde.original_query, "test query");
        assert_eq!(hyde.hypothetical_code, "fn test() {}");
        assert_eq!(hyde.language, "rust");
    }

    #[test]
    fn test_generator_creation() {
        let gen = HypotheticalCodeGenerator::new("mock".to_string(), Some("key".to_string()), 256);

        assert_eq!(gen.backend, "mock");
        assert_eq!(gen.api_key, Some("key".to_string()));
        assert_eq!(gen.max_tokens, 256);
        assert!(gen.local_llm.is_none());
    }

    // ---------------------------------------------------------------------------
    // New tests: local backend
    // ---------------------------------------------------------------------------

    /// `build_local_prompt` must produce a well-formed Qwen2.5 chat-template
    /// string containing the query, language, and all required delimiters.
    #[test]
    fn test_build_local_prompt_format() {
        let gen = HypotheticalCodeGenerator::new("local".to_string(), None, 128);
        let prompt = gen.build_local_prompt("parse JSON", "rust");

        // System turn
        assert!(
            prompt.contains("<|im_start|>system\n"),
            "missing system turn open"
        );
        assert!(
            prompt.contains("<|im_end|>\n<|im_start|>user\n"),
            "missing system→user boundary"
        );

        // Query and language are embedded
        assert!(prompt.contains("parse JSON"), "query missing from prompt");
        assert!(prompt.contains("rust"), "language missing from prompt");

        // Assistant turn open (no closing tag — model continues from here)
        assert!(
            prompt.ends_with("<|im_start|>assistant\n"),
            "prompt must end with open assistant turn so the model generates next"
        );
    }

    /// The local backend should successfully generate a `HyDEQuery` when a
    /// `MockLlmGenerator` is attached. The hypothetical code must be non-empty.
    #[tokio::test]
    async fn test_local_backend_generate_success() {
        let gen = local_generator_with_mock(128);
        let result = gen.generate("error handling patterns", "rust").await;

        assert!(result.is_ok(), "expected Ok, got {:?}", result.err());
        let hyde = result.unwrap();

        assert_eq!(hyde.original_query, "error handling patterns");
        assert_eq!(hyde.language, "rust");
        // MockLlmGenerator returns a non-empty string based on the prompt
        assert!(!hyde.hypothetical_code.is_empty(), "hypothetical_code must not be empty");
    }

    /// When the `"local"` backend is selected but no `local_llm` is attached,
    /// `generate()` must return an error rather than panic.
    #[tokio::test]
    async fn test_local_backend_missing_llm_returns_error() {
        let gen = HypotheticalCodeGenerator::new("local".to_string(), None, 128);
        let result = gen.generate("some query", "rust").await;

        assert!(result.is_err(), "expected Err when local_llm is None");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("local") || msg.contains("local_llm") || msg.contains("with_local_llm"),
            "error message should mention the missing local LLM: {msg}"
        );
    }

    /// `with_local_llm` is a builder that sets the field and returns `Self`.
    #[test]
    fn test_with_local_llm_builder() {
        let llm: Arc<dyn LlmGenerator> = Arc::new(MockLlmGenerator);
        let gen = HypotheticalCodeGenerator::new("local".to_string(), None, 64)
            .with_local_llm(Arc::clone(&llm));

        assert!(gen.local_llm.is_some(), "local_llm should be Some after with_local_llm");
    }

    /// Cloning a generator with a local LLM shares the same Arc (does not
    /// duplicate the underlying generator).
    #[test]
    fn test_clone_shares_local_llm_arc() {
        let llm: Arc<dyn LlmGenerator> = Arc::new(MockLlmGenerator);
        let gen = HypotheticalCodeGenerator::new("local".to_string(), None, 64)
            .with_local_llm(Arc::clone(&llm));

        let gen2 = gen.clone();

        // Both clones reference the same Arc allocation
        let ptr1 = Arc::as_ptr(gen.local_llm.as_ref().unwrap());
        let ptr2 = Arc::as_ptr(gen2.local_llm.as_ref().unwrap());
        assert_eq!(ptr1, ptr2, "clone should share the same Arc<dyn LlmGenerator>");
    }

    /// The response from the local LLM is passed through `extract_code_from_markdown`.
    /// Verify that markdown-fenced output is unwrapped correctly.
    #[tokio::test]
    async fn test_local_backend_unwraps_markdown_fences() {
        struct FencedLlm;
        impl LlmGenerator for FencedLlm {
            fn generate(&self, _prompt: &str, _max_tokens: u32) -> Result<String> {
                Ok("```rust\nfn example() -> u32 { 42 }\n```".to_string())
            }
        }

        let gen = HypotheticalCodeGenerator::new("local".to_string(), None, 64)
            .with_local_llm(Arc::new(FencedLlm));

        let result = gen.generate("example function", "rust").await.unwrap();
        assert_eq!(
            result.hypothetical_code,
            "fn example() -> u32 { 42 }",
            "markdown fences should be stripped"
        );
    }

    /// When the local LLM returns an error, `generate_local` propagates it.
    #[tokio::test]
    async fn test_local_backend_propagates_llm_error() {
        struct FailingLlm;
        impl LlmGenerator for FailingLlm {
            fn generate(&self, _prompt: &str, _max_tokens: u32) -> Result<String> {
                Err(anyhow::anyhow!("inference engine on fire"))
            }
        }

        let gen = HypotheticalCodeGenerator::new("local".to_string(), None, 64)
            .with_local_llm(Arc::new(FailingLlm));

        let result = gen.generate("some query", "rust").await;
        assert!(result.is_err(), "LLM error must propagate");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("inference engine on fire"),
            "root cause must be preserved: {msg}"
        );
    }

    /// `max_tokens` is passed through to the underlying LLM as a `u32`.
    /// Verify that the field is correctly forwarded (via a capturing closure).
    #[tokio::test]
    async fn test_local_backend_passes_max_tokens() {
        use std::sync::atomic::{AtomicU32, Ordering};

        struct TokenCaptureLlm(Arc<AtomicU32>);
        impl LlmGenerator for TokenCaptureLlm {
            fn generate(&self, _prompt: &str, max_tokens: u32) -> Result<String> {
                self.0.store(max_tokens, Ordering::SeqCst);
                Ok("fn placeholder() {}".to_string())
            }
        }

        let captured = Arc::new(AtomicU32::new(0));
        let llm = Arc::new(TokenCaptureLlm(Arc::clone(&captured)));

        let gen = HypotheticalCodeGenerator::new("local".to_string(), None, 200)
            .with_local_llm(llm);

        gen.generate("query", "rust").await.unwrap();
        assert_eq!(captured.load(Ordering::SeqCst), 200, "max_tokens must be forwarded");
    }
}

//! Hypothetical code generation for HyDE
//!
//! This module provides LLM-based generation of hypothetical code snippets
//! that can be embedded for better semantic retrieval.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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
pub struct HypotheticalCodeGenerator {
    /// LLM backend ("openai", "anthropic", or "mock" for testing)
    backend: String,
    /// API key for the LLM service
    api_key: Option<String>,
    /// Maximum tokens for generated code
    max_tokens: usize,
}

impl HypotheticalCodeGenerator {
    /// Create a new HyDE generator
    pub fn new(backend: String, api_key: Option<String>, max_tokens: usize) -> Self {
        Self {
            backend,
            api_key,
            max_tokens,
        }
    }

    /// Generate hypothetical code for a query
    pub async fn generate(&self, query: &str, language: &str) -> Result<HyDEQuery> {
        let prompt = self.build_prompt(query, language);

        match self.backend.as_str() {
            "openai" => self.generate_openai(&prompt, language).await,
            "anthropic" => self.generate_anthropic(&prompt, language).await,
            "mock" => self.generate_mock(query, language),
            _ => Err(anyhow::anyhow!("Unknown LLM backend: {}", self.backend)),
        }
    }

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

    async fn generate_openai(&self, prompt: &str, language: &str) -> Result<HyDEQuery> {
        let api_key = self.api_key.as_ref()
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
            .and_then(|c| Some(c.message.content.trim().to_string()))
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
        let api_key = self.api_key.as_ref()
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
            .and_then(|c| Some(c.text.trim().to_string()))
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

    #[test]
    fn test_mock_generation() {
        let generator = HypotheticalCodeGenerator::new(
            "mock".to_string(),
            None,
            512,
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            generator.generate("how to parse JSON", "rust").await
        });

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
        let gen = HypotheticalCodeGenerator::new(
            "mock".to_string(),
            Some("key".to_string()),
            256,
        );

        assert_eq!(gen.backend, "mock");
        assert_eq!(gen.api_key, Some("key".to_string()));
        assert_eq!(gen.max_tokens, 256);
    }
}

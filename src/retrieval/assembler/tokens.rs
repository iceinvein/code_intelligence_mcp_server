//! Token counting using tiktoken-rs
//!
//! Provides a singleton TokenCounter for efficient token counting
//! using OpenAI-compatible tokenization (o200k_base, cl100k_base, etc.).

use anyhow::{anyhow, Result};
use once_cell::sync::OnceCell;
use tiktoken_rs::tokenizer::Tokenizer;
use tiktoken_rs::CoreBPE;

/// Convert an encoding name string to a Tokenizer enum
///
/// Supported encodings: "o200k_base", "cl100k_base", "p50k_base", "p50k_edit", "r50k_base", "gpt2"
fn parse_tokenizer(encoding: &str) -> Result<Tokenizer> {
    match encoding.to_lowercase().as_str() {
        "o200k_base" => Ok(Tokenizer::O200kBase),
        "cl100k_base" => Ok(Tokenizer::Cl100kBase),
        "p50k_base" => Ok(Tokenizer::P50kBase),
        "p50k_edit" => Ok(Tokenizer::P50kEdit),
        "r50k_base" => Ok(Tokenizer::R50kBase),
        "gpt2" => Ok(Tokenizer::Gpt2),
        _ => Err(anyhow!(
            "Unknown encoding '{}'. Supported: o200k_base, cl100k_base, p50k_base, p50k_edit, r50k_base, gpt2",
            encoding
        )),
    }
}

/// Token counter using tiktoken BPE encoding
pub struct TokenCounter {
    bpe: CoreBPE,
    encoding_name: String,
}

impl TokenCounter {
    /// Create a new TokenCounter with the specified encoding
    ///
    /// Supported encodings: "o200k_base", "cl100k_base", "p50k_base", "p50k_edit", "r50k_base", "gpt2"
    pub fn new(encoding: &str) -> Result<Self> {
        let tokenizer = parse_tokenizer(encoding)?;
        let bpe = tiktoken_rs::get_bpe_from_tokenizer(tokenizer)
            .map_err(|e| anyhow!("Failed to get BPE for encoding '{}': {}", encoding, e))?;
        Ok(Self {
            bpe,
            encoding_name: encoding.to_string(),
        })
    }

    /// Count tokens in a string using the configured encoding
    pub fn count(&self, text: &str) -> usize {
        self.bpe.encode_with_special_tokens(text).len()
    }

    /// Count tokens for multiple strings efficiently
    pub fn count_batch(&self, texts: &[&str]) -> usize {
        texts.iter().map(|t| self.count(t)).sum()
    }

    /// Get the encoding name for this counter
    pub fn encoding_name(&self) -> &str {
        &self.encoding_name
    }
}

/// Singleton token counter instance
static TOKEN_COUNTER: OnceCell<TokenCounter> = OnceCell::new();

/// Get or initialize the singleton TokenCounter
///
/// Uses "o200k_base" (GPT-4o, o1, o3, o4) as default encoding.
/// This encoding is suitable for modern code understanding tasks.
pub fn get_token_counter() -> &'static TokenCounter {
    TOKEN_COUNTER.get_or_init(|| {
        // Default to o200k_base for GPT-4o/o1/o3/o4 models
        // This is the most common encoding for modern OpenAI models
        TokenCounter::new("o200k_base")
            .expect("Failed to initialize tokenizer with default encoding 'o200k_base'")
    })
}

/// Get or initialize the singleton TokenCounter with a specific encoding
///
/// This function is only called once per encoding; subsequent calls return
/// the cached instance. If a different encoding is requested, this will
/// return the first-initialized encoding (for now, to keep singleton simple).
pub fn get_token_counter_with_encoding(encoding: &str) -> &'static TokenCounter {
    TOKEN_COUNTER.get_or_init(|| {
        TokenCounter::new(encoding).unwrap_or_else(|_| {
            TokenCounter::new("o200k_base")
                .expect("Failed to initialize tokenizer with fallback encoding 'o200k_base'")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_counts_tokens() {
        let counter = TokenCounter::new("o200k_base").unwrap();
        // Simple test: "hello" is typically 1 token in o200k_base
        assert_eq!(counter.count("hello"), 1);
        // Code often has different tokenization
        let code = "function hello() { return 42; }";
        let tokens = counter.count(code);
        assert!(tokens > 0);
        assert!(tokens < code.len()); // Tokens should be fewer than characters
    }

    #[test]
    fn test_counter_count_batch() {
        let counter = TokenCounter::new("o200k_base").unwrap();
        let texts = vec!["hello", "world", "function test() {}"];
        let batch_count = counter.count_batch(&texts);
        let individual_count =
            counter.count("hello") + counter.count("world") + counter.count("function test() {}");
        assert_eq!(batch_count, individual_count);
    }

    #[test]
    fn test_counter_encoding_name() {
        let counter = TokenCounter::new("cl100k_base").unwrap();
        assert_eq!(counter.encoding_name(), "cl100k_base");
    }

    #[test]
    fn test_get_token_counter_singleton() {
        let c1 = get_token_counter();
        let c2 = get_token_counter();
        // Should return the same instance
        assert_eq!(c1.encoding_name(), c2.encoding_name());
        assert_eq!(c1.encoding_name(), "o200k_base");
    }

    #[test]
    fn test_empty_string() {
        let counter = TokenCounter::new("o200k_base").unwrap();
        assert_eq!(counter.count(""), 0);
    }

    #[test]
    fn test_unicode_handling() {
        let counter = TokenCounter::new("o200k_base").unwrap();
        // Unicode characters may be multiple tokens
        let text = "hello 世界 🌍";
        let tokens = counter.count(text);
        assert!(tokens > 0);
    }
}

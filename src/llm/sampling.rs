//! MCP Sampling-based LLM description generation.
//!
//! Uses the MCP client's LLM (e.g., Claude in Claude Code) to generate symbol
//! descriptions via the `sampling/createMessage` protocol method. Falls back to
//! local llama.cpp inference when sampling is unavailable.

use anyhow::{anyhow, Result};
use once_cell::sync::OnceCell;
use rust_mcp_sdk::{
    schema::{
        CreateMessageContent, CreateMessageRequestParams, Role, SamplingMessage,
        SamplingMessageContent, TextContent,
    },
    McpServer,
};
use std::sync::Arc;

use super::LlmGenerator;

/// Generates symbol descriptions by sending sampling requests to the MCP client.
///
/// The MCP `sampling/createMessage` method asks the client's LLM (e.g., Claude)
/// to describe a code symbol. This produces higher-quality descriptions than the
/// local 1.5B model, since the client typically has access to a much larger model.
///
/// # Sync/Async Bridge
///
/// `LlmGenerator::generate()` is synchronous (called from `spawn_blocking` in
/// the description worker). We bridge to the async `request_message_creation()`
/// using `tokio::task::block_in_place` + `Handle::block_on`.
pub struct SamplingLlmGenerator {
    runtime: Arc<dyn McpServer>,
}

impl SamplingLlmGenerator {
    pub fn new(runtime: Arc<dyn McpServer>) -> Self {
        Self { runtime }
    }
}

impl LlmGenerator for SamplingLlmGenerator {
    fn generate(&self, prompt: &str, max_tokens: u32) -> Result<String> {
        // Check if client supports sampling before making the request
        match self.runtime.client_supports_sampling() {
            Some(true) => {}
            Some(false) => {
                return Err(anyhow!("Client does not support MCP sampling"));
            }
            None => {
                return Err(anyhow!(
                    "Client info not yet available, cannot determine sampling support"
                ));
            }
        }

        // Extract the user content from the Qwen2.5 chat template prompt.
        // The prompt format is:
        //   <|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n
        // We extract the user section to send as the user message, and use
        // the system section as the system prompt for the sampling request.
        let (system_prompt, user_content) = parse_qwen_prompt(prompt);

        let params = CreateMessageRequestParams {
            messages: vec![SamplingMessage {
                content: SamplingMessageContent::TextContent(TextContent::new(
                    user_content,
                    None,
                    None,
                )),
                role: Role::User,
                meta: None,
            }],
            max_tokens: max_tokens as i64,
            system_prompt: Some(system_prompt),
            include_context: None,
            model_preferences: None,
            stop_sequences: vec![],
            metadata: None,
            meta: None,
            task: None,
            temperature: None,
            tool_choice: None,
            tools: vec![],
        };

        // Bridge sync -> async: block_in_place allows blocking in a tokio
        // context without starving the runtime (it moves the current task
        // off the worker thread).
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                self.runtime.request_message_creation(params).await
            })
        })
        .map_err(|e| anyhow!("MCP sampling request failed: {}", e))?;

        // Extract text from the response
        match result.content {
            CreateMessageContent::TextContent(tc) => Ok(tc.text),
            _ => Err(anyhow!(
                "MCP sampling returned non-text content (expected TextContent)"
            )),
        }
    }
}

/// Tries MCP sampling first, falls back to local LLM generator.
///
/// The MCP runtime may not be available at server startup (it's set on the
/// first tool call from the client). The `OnceCell` is checked on each
/// `generate()` call so that once the runtime becomes available, subsequent
/// descriptions use the client's LLM. Until then, all descriptions use the
/// local generator.
pub struct FallbackLlmGenerator {
    /// MCP runtime cell, populated lazily on first tool call from the client.
    /// Checked on every `generate()` call so new availability is picked up.
    mcp_runtime: Arc<OnceCell<Arc<dyn McpServer + 'static>>>,
    /// Local llama.cpp generator (always available).
    local: Arc<dyn LlmGenerator>,
}

impl FallbackLlmGenerator {
    pub fn new(
        mcp_runtime: Arc<OnceCell<Arc<dyn McpServer + 'static>>>,
        local: Arc<dyn LlmGenerator>,
    ) -> Self {
        Self {
            mcp_runtime,
            local,
        }
    }
}

impl LlmGenerator for FallbackLlmGenerator {
    fn generate(&self, prompt: &str, max_tokens: u32) -> Result<String> {
        // Check the OnceCell on each call — the runtime is set lazily on the
        // first tool call from the client, which typically arrives after the
        // description worker has already started.
        if let Some(runtime) = self.mcp_runtime.get() {
            let sampling = SamplingLlmGenerator::new(runtime.clone());
            match sampling.generate(prompt, max_tokens) {
                Ok(text) => {
                    tracing::debug!("MCP sampling succeeded for description generation");
                    return Ok(text);
                }
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        "MCP sampling failed, falling back to local LLM"
                    );
                }
            }
        }

        // Fall back to local generator
        self.local.generate(prompt, max_tokens)
    }
}

/// Parse a Qwen2.5 chat template prompt into (system_prompt, user_content).
///
/// Input format:
/// ```text
/// <|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n
/// ```
fn parse_qwen_prompt(prompt: &str) -> (String, String) {
    let system_default = "List 5-10 specific search terms for this code, comma-separated. \
        Include: domain concepts, algorithms, design patterns, libraries, and alternative names. \
        Do NOT include generic terms like function, code, error, return, data, handle."
        .to_string();

    // Extract system prompt
    let system = if let Some(start) = prompt.find("<|im_start|>system\n") {
        let content_start = start + "<|im_start|>system\n".len();
        if let Some(end) = prompt[content_start..].find("<|im_end|>") {
            prompt[content_start..content_start + end].to_string()
        } else {
            system_default
        }
    } else {
        system_default
    };

    // Extract user content
    let user = if let Some(start) = prompt.find("<|im_start|>user\n") {
        let content_start = start + "<|im_start|>user\n".len();
        if let Some(end) = prompt[content_start..].find("<|im_end|>") {
            prompt[content_start..content_start + end].to_string()
        } else {
            prompt.to_string()
        }
    } else {
        // If not in Qwen format, use the whole prompt as user content
        prompt.to_string()
    };

    (system, user)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmGenerator;
    use rust_mcp_sdk::{
        auth::AuthInfo,
        error::SdkResult,
        schema::{
            schema_utils::{ClientMessage, MessageFromServer, ServerMessage},
            ClientCapabilities, ClientSampling, Implementation, InitializeRequestParams,
            InitializeResult, RequestId, ServerCapabilities,
        },
        task_store::{ClientTaskStore, ServerTaskStore},
    };
    use std::time::Duration;
    use tokio::sync::RwLockReadGuard;

    // --- Minimal Mock McpServer ---
    //
    // Implements all required McpServer methods. Most are unreachable in our
    // tests. Only `client_info()` and `server_info()` return meaningful values.

    struct MockSamplingServer {
        supports_sampling: bool,
        server_details: InitializeResult,
    }

    impl MockSamplingServer {
        fn new(supports_sampling: bool) -> Self {
            Self {
                supports_sampling,
                server_details: InitializeResult {
                    server_info: Implementation {
                        name: "test-server".into(),
                        version: "1.0.0".into(),
                        title: None,
                        description: None,
                        icons: vec![],
                        website_url: None,
                    },
                    capabilities: ServerCapabilities::default(),
                    protocol_version: "2025-11-25".into(),
                    instructions: None,
                    meta: None,
                },
            }
        }
    }

    #[async_trait::async_trait]
    impl McpServer for MockSamplingServer {
        async fn start(self: Arc<Self>) -> SdkResult<()> {
            unimplemented!("not needed for sampling tests")
        }

        async fn set_client_details(
            &self,
            _client_details: InitializeRequestParams,
        ) -> SdkResult<()> {
            unimplemented!("not needed for sampling tests")
        }

        fn server_info(&self) -> &InitializeResult {
            &self.server_details
        }

        fn client_info(&self) -> Option<InitializeRequestParams> {
            let sampling = if self.supports_sampling {
                Some(ClientSampling::default())
            } else {
                None
            };
            Some(InitializeRequestParams {
                capabilities: ClientCapabilities {
                    sampling,
                    ..Default::default()
                },
                client_info: Implementation {
                    name: "test-client".into(),
                    version: "1.0.0".into(),
                    title: None,
                    description: None,
                    icons: vec![],
                    website_url: None,
                },
                protocol_version: "2025-11-25".into(),
                meta: None,
            })
        }

        async fn auth_info(&self) -> RwLockReadGuard<'_, Option<AuthInfo>> {
            unimplemented!("not needed for sampling tests")
        }

        async fn auth_info_cloned(&self) -> Option<AuthInfo> {
            None
        }

        async fn update_auth_info(&self, _auth_info: Option<AuthInfo>) {}

        async fn wait_for_initialization(&self) {}

        fn task_store(&self) -> Option<Arc<ServerTaskStore>> {
            None
        }

        fn client_task_store(&self) -> Option<Arc<ClientTaskStore>> {
            None
        }

        async fn stderr_message(&self, _message: String) -> SdkResult<()> {
            Ok(())
        }

        fn session_id(&self) -> Option<String> {
            None
        }

        async fn send(
            &self,
            _message: MessageFromServer,
            _request_id: Option<RequestId>,
            _request_timeout: Option<Duration>,
        ) -> SdkResult<Option<ClientMessage>> {
            // Return None to simulate an empty response, which will cause
            // request_message_creation to return an error.
            Ok(None)
        }

        async fn send_batch(
            &self,
            _messages: Vec<ServerMessage>,
            _request_timeout: Option<Duration>,
        ) -> SdkResult<Option<Vec<ClientMessage>>> {
            Ok(None)
        }
    }

    // --- Tests ---

    #[test]
    fn test_parse_qwen_prompt() {
        let prompt = "<|im_start|>system\nList 5-10 specific search terms for this code, comma-separated. \
            Include: domain concepts, algorithms, design patterns, libraries, and alternative names. \
            Do NOT include generic terms like function, code, error, return, data, handle.<|im_end|>\n\
            <|im_start|>user\nfunction foo in lib.rs:\nfn foo() {}<|im_end|>\n\
            <|im_start|>assistant\n";

        let (system, user) = parse_qwen_prompt(prompt);

        assert!(system.contains("List 5-10 specific search terms"));
        assert!(system.contains("domain concepts"));
        assert!(user.contains("function foo in lib.rs:"));
        assert!(user.contains("fn foo() {}"));
    }

    #[test]
    fn test_parse_qwen_prompt_plain_text() {
        let prompt = "Just a plain prompt with no template markers";
        let (system, user) = parse_qwen_prompt(prompt);

        // System should be the default (keyword prompt)
        assert!(system.contains("search terms"));
        // User should be the entire prompt
        assert_eq!(user, prompt);
    }

    #[test]
    fn test_fallback_generator_no_runtime_uses_local() {
        // OnceCell is empty — no runtime has been set yet (server just started)
        let cell = Arc::new(OnceCell::new());
        let local = Arc::new(MockLlmGenerator) as Arc<dyn LlmGenerator>;
        let fallback = FallbackLlmGenerator::new(cell, local);

        let prompt = crate::llm::build_description_prompt(
            "test_func",
            "function",
            "src/lib.rs",
            "fn test_func() {}",
        );

        let result = fallback.generate(&prompt, 50).unwrap();
        assert!(
            result.contains("Mock description"),
            "Should use local MockLlmGenerator when no runtime available"
        );
    }

    #[test]
    fn test_fallback_generator_sampling_not_supported_uses_local() {
        // Client that does NOT support sampling
        let mock_server = Arc::new(MockSamplingServer::new(false));
        let cell: Arc<OnceCell<Arc<dyn McpServer + 'static>>> = Arc::new(OnceCell::new());
        cell.set(mock_server as Arc<dyn McpServer>).ok();
        let local = Arc::new(MockLlmGenerator) as Arc<dyn LlmGenerator>;
        let fallback = FallbackLlmGenerator::new(cell, local);

        let prompt = crate::llm::build_description_prompt(
            "test_func",
            "function",
            "src/lib.rs",
            "fn test_func() {}",
        );

        let result = fallback.generate(&prompt, 50).unwrap();
        assert!(
            result.contains("Mock description"),
            "Should fall back to local when sampling not supported"
        );
    }

    #[test]
    fn test_sampling_generator_rejects_when_not_supported() {
        // Client that does NOT support sampling
        let mock_server = Arc::new(MockSamplingServer::new(false));
        let sampling = SamplingLlmGenerator::new(mock_server as Arc<dyn McpServer>);

        let result = sampling.generate("test prompt", 50);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("does not support MCP sampling"),
            "Should error when client doesn't support sampling"
        );
    }

    #[tokio::test]
    async fn test_sampling_generator_passes_support_check_when_supported() {
        // Client that DOES support sampling.
        // The request will fail because our mock's send() returns None,
        // but it should get past the support check.
        let mock_server = Arc::new(MockSamplingServer::new(true));
        let sampling = SamplingLlmGenerator::new(mock_server as Arc<dyn McpServer>);

        // Run in a blocking context to match the real usage pattern
        let result = tokio::task::spawn_blocking(move || {
            sampling.generate("test prompt", 50)
        })
        .await
        .unwrap();

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        // Should NOT fail on "does not support" — it should fail later (empty response)
        assert!(
            !err_msg.contains("does not support"),
            "Should pass the sampling support check; got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("sampling request failed"),
            "Should fail at the request level; got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_fallback_generator_falls_back_on_sampling_failure() {
        // Client that supports sampling but whose send() returns None (causing request to fail)
        let mock_server = Arc::new(MockSamplingServer::new(true));
        let cell: Arc<OnceCell<Arc<dyn McpServer + 'static>>> = Arc::new(OnceCell::new());
        cell.set(mock_server as Arc<dyn McpServer>).ok();
        let local = Arc::new(MockLlmGenerator) as Arc<dyn LlmGenerator>;
        let fallback = FallbackLlmGenerator::new(cell, local);

        let prompt = crate::llm::build_description_prompt(
            "test_func",
            "function",
            "src/lib.rs",
            "fn test_func() {}",
        );

        // Run in a blocking context to match real usage
        let result = tokio::task::spawn_blocking(move || fallback.generate(&prompt, 50))
            .await
            .unwrap();

        // Should succeed via fallback to local MockLlmGenerator
        let text = result.unwrap();
        assert!(
            text.contains("Mock description"),
            "Should fall back to local after sampling failure; got: {}",
            text
        );
    }

    #[test]
    fn test_fallback_generator_picks_up_runtime_dynamically() {
        // Simulate the real startup sequence: FallbackLlmGenerator is created
        // before the first tool call, so the OnceCell is empty. Later, a tool
        // call populates it. The next generate() should attempt sampling.
        let cell: Arc<OnceCell<Arc<dyn McpServer + 'static>>> = Arc::new(OnceCell::new());
        let local = Arc::new(MockLlmGenerator) as Arc<dyn LlmGenerator>;
        let fallback = FallbackLlmGenerator::new(cell.clone(), local);

        let prompt = crate::llm::build_description_prompt(
            "test_func",
            "function",
            "src/lib.rs",
            "fn test_func() {}",
        );

        // First call: no runtime yet — uses local
        let result1 = fallback.generate(&prompt, 50).unwrap();
        assert!(result1.contains("Mock description"), "Should use local when OnceCell is empty");

        // Simulate first tool call populating the runtime
        let mock_server = Arc::new(MockSamplingServer::new(false));
        cell.set(mock_server as Arc<dyn McpServer>).ok();

        // Second call: runtime is now available (but doesn't support sampling),
        // so it tries sampling, fails, falls back to local
        let result2 = fallback.generate(&prompt, 50).unwrap();
        assert!(
            result2.contains("Mock description"),
            "Should attempt sampling then fall back to local"
        );
    }
}

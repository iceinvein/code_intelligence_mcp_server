//! Chat agent loop — builds prompts, parses tool calls, orchestrates multi-round execution.
//!
//! Implements the Qwen2.5 Hermes-style tool calling protocol:
//! - Tools defined in `<tools>...</tools>` XML in system prompt
//! - Model outputs `<tool_call>{"name": "...", "arguments": {...}}</tool_call>`
//! - Tool results fed back as `<tool_response>...</tool_response>` in user role

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::handlers::AppState;

use super::llm::ChatLlm;
use super::tools::{self, ToolCall};

/// Default maximum number of tool-call rounds before forcing a final streaming response.
pub const DEFAULT_MAX_TOOL_ROUNDS: usize = 3;

/// A chat message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role of the message sender: `"user"`, `"assistant"`, `"system"`, or `"tool"`.
    pub role: String,
    /// Text content of the message.
    pub content: String,
}

/// SSE event sent to the client during a chat turn.
///
/// Uses `#[serde(tag = "type")]` so each variant serialises with a `"type"` field
/// matching the variant's rename, enabling the client to dispatch on event kind.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ChatEvent {
    /// A single generated token from the final streaming response.
    #[serde(rename = "token")]
    Token { content: String },
    /// The LLM has decided to call a tool — sent before execution.
    #[serde(rename = "tool_call")]
    ToolCallStart { tool: String, args: Value },
    /// A tool has finished executing — sent after execution.
    #[serde(rename = "tool_result")]
    ToolResult { tool: String, summary: String },
    /// A non-recoverable error occurred during the agent loop.
    #[serde(rename = "error")]
    Error { message: String },
    /// The agent loop has finished; no more events will follow.
    #[serde(rename = "done")]
    Done,
}

/// Build a Qwen2.5 chat template prompt from a conversation history.
///
/// The system message includes repository context and all tool definitions in
/// `<tools>` XML.  If the first message in `messages` has `role == "system"`,
/// its content is used in place of the default instruction, but the tool block
/// is always appended.  Tool results (role `"tool"`) are wrapped in
/// `<tool_response>` tags inside a user-role turn, matching the Hermes protocol.
///
/// # Arguments
/// * `messages`  - Ordered conversation history (user / assistant / tool turns)
/// * `repo_name` - Human-readable repository name shown in the system prompt
/// * `repo_path` - Absolute path to the repository shown in the system prompt
///
/// # Returns
/// A fully-formatted prompt string ready for tokenisation, ending with the
/// `<|im_start|>assistant\n` marker so the model continues generation immediately.
pub fn build_prompt(messages: &[ChatMessage], repo_name: &str, repo_path: &str) -> String {
    // Serialize tool definitions — one compact JSON object per line.
    let tool_defs = tools::tool_definitions();
    let tools_json: String = tool_defs
        .iter()
        .map(|t| serde_json::to_string(t).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    // Determine whether the caller supplied a custom system message.
    let (custom_system, remaining) = if messages.first().map(|m| m.role.as_str()) == Some("system")
    {
        (Some(messages[0].content.as_str()), &messages[1..])
    } else {
        (None, messages)
    };

    let base_instruction = custom_system.unwrap_or("");

    // Build the system message content.
    let system_body = if let Some(custom) = custom_system {
        format!(
            "{custom}\n\n# Tools\n\nYou may call one or more functions to assist with the user query.\n\n\
             You are provided with function signatures within <tools></tools> XML tags:\n\
             <tools>\n{tools_json}\n</tools>\n\n\
             For each function call, return a json object with function name and arguments within \
             <tool_call></tool_call> XML tags:\n\
             <tool_call>\n\
             {{\"name\": \"<function-name>\", \"arguments\": <args-json-object>}}\n\
             </tool_call>"
        )
    } else {
        format!(
            "You are a code assistant for the repository \"{repo_name}\" at {repo_path}. \
             You have tools to search and navigate the codebase. Always use tools to look up code \
             before answering questions. Cite file paths and line numbers. Use markdown with \
             syntax-highlighted code blocks.\n\n\
             # Tools\n\n\
             You may call one or more functions to assist with the user query.\n\n\
             You are provided with function signatures within <tools></tools> XML tags:\n\
             <tools>\n{tools_json}\n</tools>\n\n\
             For each function call, return a json object with function name and arguments within \
             <tool_call></tool_call> XML tags:\n\
             <tool_call>\n\
             {{\"name\": \"<function-name>\", \"arguments\": <args-json-object>}}\n\
             </tool_call>"
        )
    };

    // Suppress the unused-variable warning from the `base_instruction` binding above.
    let _ = base_instruction;

    let mut prompt = format!("<|im_start|>system\n{system_body}<|im_end|>\n");

    // Render conversation turns.
    for msg in remaining {
        match msg.role.as_str() {
            "tool" => {
                // Tool results are injected as a user turn with <tool_response> tags.
                prompt.push_str(&format!(
                    "<|im_start|>user\n<tool_response>\n{}\n</tool_response><|im_end|>\n",
                    msg.content
                ));
            }
            role => {
                prompt.push_str(&format!(
                    "<|im_start|>{role}\n{}<|im_end|>\n",
                    msg.content
                ));
            }
        }
    }

    // Prompt ends with the assistant marker so the model generates a continuation.
    prompt.push_str("<|im_start|>assistant\n");
    prompt
}

/// Parse all `<tool_call>…</tool_call>` XML blocks from LLM output.
///
/// Each block must contain a JSON object with at minimum a `"name"` string field.
/// Blocks that are missing, empty, or contain malformed JSON are silently skipped
/// so that a partially-formed response never aborts the agent loop.
///
/// # Arguments
/// * `output` - Raw text produced by the LLM (may mix prose and tool calls)
///
/// # Returns
/// Zero or more [`ToolCall`] values in document order.
pub fn parse_tool_calls(output: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let open_tag = "<tool_call>";
    let close_tag = "</tool_call>";

    let mut search_from = 0usize;
    while let Some(start) = output[search_from..].find(open_tag) {
        let abs_start = search_from + start + open_tag.len();
        if let Some(end_offset) = output[abs_start..].find(close_tag) {
            let json_slice = output[abs_start..abs_start + end_offset].trim();
            search_from = abs_start + end_offset + close_tag.len();

            // Attempt to parse the JSON block.
            let Ok(val) = serde_json::from_str::<Value>(json_slice) else {
                tracing::debug!("Skipping malformed tool_call JSON: {json_slice}");
                continue;
            };

            let Some(name) = val.get("name").and_then(|n| n.as_str()) else {
                tracing::debug!("Skipping tool_call with missing name field");
                continue;
            };

            let arguments = val
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));

            calls.push(ToolCall {
                name: name.to_string(),
                arguments,
            });
        } else {
            // No matching close tag — stop scanning.
            break;
        }
    }

    calls
}

/// Returns `true` if `output` contains at least one `<tool_call>` opening tag.
///
/// This is a cheap O(n) scan used to decide whether full parsing is needed.
#[inline]
pub fn has_tool_calls(output: &str) -> bool {
    output.contains("<tool_call>")
}

/// Extract the text portions of `output` that lie outside any `<tool_call>` blocks.
///
/// Useful for surfacing explanatory prose the model wrote alongside its tool calls
/// (e.g. "I'll search for that now.") without including the raw JSON payload.
///
/// # Arguments
/// * `output` - Raw LLM output that may interleave prose and tool calls
///
/// # Returns
/// A new `String` with all `<tool_call>…</tool_call>` spans removed and the
/// surrounding text concatenated in order.
pub fn extract_text_outside_tool_calls(output: &str) -> String {
    let open_tag = "<tool_call>";
    let close_tag = "</tool_call>";

    let mut result = String::with_capacity(output.len());
    let mut cursor = 0usize;

    while let Some(rel_start) = output[cursor..].find(open_tag) {
        let abs_start = cursor + rel_start;
        // Append everything before the tool call block.
        result.push_str(&output[cursor..abs_start]);

        // Advance past the closing tag.
        let after_open = abs_start + open_tag.len();
        if let Some(rel_end) = output[after_open..].find(close_tag) {
            cursor = after_open + rel_end + close_tag.len();
        } else {
            // Unclosed tag — consume the rest and stop.
            cursor = output.len();
            break;
        }
    }

    // Append any trailing text after the last tool call block.
    result.push_str(&output[cursor..]);
    result
}

/// Run the multi-round chat agent loop.
///
/// Iterates up to `max_tool_rounds` times.  Each round:
/// 1. Builds the full Qwen2.5 chat-template prompt from `messages`.
/// 2. Runs [`ChatLlm::generate`] on a blocking thread to get the full output.
/// 3. If the output contains no tool calls, switches to streaming via
///    [`ChatLlm::generate_to_sender`] and forwards every token as a
///    [`ChatEvent::Token`], then breaks.
/// 4. If the output contains tool calls, executes each one in sequence, emitting
///    [`ChatEvent::ToolCallStart`] before and [`ChatEvent::ToolResult`] after each,
///    and appending the results to `messages` for the next round.
/// 5. On the final round (round `max_tool_rounds`), forces a streaming response
///    regardless of whether tool calls were present.
///
/// Always sends [`ChatEvent::Done`] as the last event before returning.
pub async fn run_agent(
    llm: Arc<ChatLlm>,
    state: &AppState,
    mut messages: Vec<ChatMessage>,
    repo_name: &str,
    repo_path: &str,
    event_tx: mpsc::Sender<ChatEvent>,
    max_tool_rounds: usize,
) {
    for round in 0..max_tool_rounds {
        let is_last_round = round == max_tool_rounds - 1;

        let prompt = build_prompt(&messages, repo_name, repo_path);

        // On the last round we always stream, so skip the non-streaming pass.
        if !is_last_round {
            // Non-streaming generation so we can inspect for tool calls.
            // Run on a blocking thread to avoid starving the tokio runtime.
            let llm_clone = llm.clone();
            let prompt_clone = prompt.clone();
            let output = match tokio::task::spawn_blocking(move || {
                llm_clone.generate(&prompt_clone, 2048)
            })
            .await
            {
                Ok(Ok(o)) => o,
                Ok(Err(e)) => {
                    let _ = event_tx
                        .send(ChatEvent::Error {
                            message: e.to_string(),
                        })
                        .await;
                    let _ = event_tx.send(ChatEvent::Done).await;
                    return;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(ChatEvent::Error {
                            message: format!("Generation task failed: {}", e),
                        })
                        .await;
                    let _ = event_tx.send(ChatEvent::Done).await;
                    return;
                }
            };

            if !has_tool_calls(&output) {
                // No tool calls — stream the final response from scratch.
                stream_response(llm.clone(), &prompt, &event_tx).await;
                let _ = event_tx.send(ChatEvent::Done).await;
                return;
            }

            // Execute all tool calls in the output.
            let tool_calls = parse_tool_calls(&output);
            let mut tool_results = Vec::with_capacity(tool_calls.len());

            for tc in &tool_calls {
                // Notify the client that a tool call is starting.
                let _ = event_tx
                    .send(ChatEvent::ToolCallStart {
                        tool: tc.name.clone(),
                        args: tc.arguments.clone(),
                    })
                    .await;

                let result = match tools::execute_tool(state, tc).await {
                    Ok(r) => r,
                    Err(e) => {
                        format!("{{\"error\": \"{}\"}}", e)
                    }
                };

                // Summarise the result for the SSE event (first 200 chars).
                let summary = if result.len() > 200 {
                    format!("{}…", &result[..200])
                } else {
                    result.clone()
                };

                let _ = event_tx
                    .send(ChatEvent::ToolResult {
                        tool: tc.name.clone(),
                        summary,
                    })
                    .await;

                tool_results.push(result);
            }

            // Append the assistant's raw output and the tool results to the conversation.
            messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: output,
            });
            // Combine all tool results into a single tool-role message.
            let combined_results = tool_results.join("\n\n");
            messages.push(ChatMessage {
                role: "tool".to_string(),
                content: combined_results,
            });

            // Continue to the next round.
        } else {
            // Final round: always stream.
            stream_response(llm.clone(), &prompt, &event_tx).await;
            let _ = event_tx.send(ChatEvent::Done).await;
            return;
        }
    }

    // Unreachable under normal control flow (the last round always returns),
    // but keeps the compiler happy.
    let _ = event_tx.send(ChatEvent::Done).await;
}

/// Generate a streaming response and forward each token as a [`ChatEvent::Token`].
///
/// Spawns the synchronous LLM generation on a blocking thread via
/// [`tokio::task::spawn_blocking`], then drains the token channel
/// asynchronously and forwards each token to the client.
async fn stream_response(
    llm: Arc<ChatLlm>,
    prompt: &str,
    event_tx: &mpsc::Sender<ChatEvent>,
) {
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let prompt_owned = prompt.to_owned();

    // Spawn generation on a dedicated blocking thread.
    // `blocking_send` is safe here because `spawn_blocking` runs on
    // a thread outside the async runtime.
    tokio::task::spawn_blocking(move || {
        if let Err(e) = llm.generate_to_sender(&prompt_owned, 2048, tx) {
            tracing::error!("Streaming generation failed: {}", e);
        }
        // `tx` is dropped here, closing the channel and ending the recv loop below.
    });

    // Forward tokens to the client as they arrive in real-time.
    while let Some(token) = rx.recv().await {
        if event_tx
            .send(ChatEvent::Token { content: token })
            .await
            .is_err()
        {
            // Client disconnected — stop streaming.
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_calls_single() {
        let output = r#"I'll search for that.
<tool_call>
{"name": "search_code", "arguments": {"query": "ranking system"}}
</tool_call>"#;
        let calls = parse_tool_calls(output);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search_code");
        assert_eq!(calls[0].arguments["query"], "ranking system");
    }

    #[test]
    fn test_parse_tool_calls_multiple() {
        let output = r#"<tool_call>
{"name": "search_code", "arguments": {"query": "auth"}}
</tool_call>
<tool_call>
{"name": "get_definition", "arguments": {"symbol_name": "login"}}
</tool_call>"#;
        let calls = parse_tool_calls(output);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "search_code");
        assert_eq!(calls[1].name, "get_definition");
    }

    #[test]
    fn test_parse_tool_calls_none() {
        let output = "The ranking system uses BM25 keyword search combined with vector embeddings.";
        let calls = parse_tool_calls(output);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_has_tool_calls() {
        assert!(has_tool_calls("text <tool_call>{}</tool_call> more"));
        assert!(!has_tool_calls("just a plain response"));
    }

    #[test]
    fn test_extract_text_outside_tool_calls() {
        let output = "Hello <tool_call>{\"name\":\"x\",\"arguments\":{}}</tool_call> world";
        assert_eq!(extract_text_outside_tool_calls(output), "Hello  world");
    }

    #[test]
    fn test_build_prompt_structure() {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: "How does search work?".to_string(),
        }];
        let prompt = build_prompt(&messages, "my-project", "/home/user/my-project");
        assert!(prompt.contains("<|im_start|>system"));
        assert!(prompt.contains("<tools>"));
        assert!(prompt.contains("search_code"));
        assert!(prompt.contains("</tools>"));
        assert!(prompt.contains("<|im_start|>user"));
        assert!(prompt.contains("How does search work?"));
        assert!(prompt.contains("<|im_start|>assistant"));
    }

    #[test]
    fn test_build_prompt_with_tool_result() {
        let messages = vec![
            ChatMessage {
                role: "user".to_string(),
                content: "find foo".to_string(),
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "<tool_call>{}</tool_call>".to_string(),
            },
            ChatMessage {
                role: "tool".to_string(),
                content: "{\"results\":[]}".to_string(),
            },
        ];
        let prompt = build_prompt(&messages, "proj", "/proj");
        assert!(prompt.contains("<tool_response>"));
        assert!(prompt.contains("{\"results\":[]}"));
        assert!(prompt.contains("</tool_response>"));
    }
}

//! Application state

use super::ask_code_cache::AskCodeCache;
use crate::config::Config;
use crate::indexer::pipeline::IndexPipeline;
use crate::llm::LlmGenerator;
use crate::retrieval::Retriever;
use crate::storage::sqlite::SqliteStore;
use once_cell::sync::OnceCell;
use rust_mcp_sdk::McpServer;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub indexer: IndexPipeline,
    pub retriever: Retriever,
    pub sqlite: Arc<SqliteStore>,
    /// MCP runtime reference, set lazily on first tool call from the client.
    ///
    /// Used by `FallbackLlmGenerator` to attempt MCP sampling for symbol
    /// descriptions instead of local LLM inference. `Arc<OnceCell<...>>`
    /// allows cloning `AppState` while sharing the same `OnceCell`.
    pub mcp_runtime: Arc<OnceCell<Arc<dyn McpServer + 'static>>>,
    /// LLM used for query-time answer synthesis by the `ask_code` handler.
    ///
    /// Inner value is `Option<Arc<dyn LlmGenerator>>`:
    /// - `OnceCell` empty: not yet initialised; handler will try to load.
    /// - `Some(Some(gen))`: resident LLM ready to generate answers.
    /// - `Some(None)`: tried to load and failed (or disabled); handler returns
    ///   `stop_reason="llm_unavailable"` with raw evidence so callers can fall back.
    ///
    /// Kept separate from the description-pipeline LLM, which is freed after
    /// indexing. `ask_code` needs a resident generator for low-latency
    /// query-time inference.
    pub answer_generator: Arc<OnceCell<Option<Arc<dyn LlmGenerator>>>>,
    /// Per-process LRU cache of `ask_code` responses keyed by question,
    /// index-run version, quality, and response-shaping inputs.
    pub ask_code_cache: Arc<AskCodeCache>,
}

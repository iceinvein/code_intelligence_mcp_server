//! Application state

use crate::config::Config;
use crate::indexer::pipeline::IndexPipeline;
use crate::leader::Role;
use crate::retrieval::Retriever;
use crate::storage::sqlite::SqliteStore;
use once_cell::sync::OnceCell;
use rust_mcp_sdk::McpServer;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub indexer: IndexPipeline,
    pub retriever: Retriever,
    pub sqlite: Arc<SqliteStore>,
    pub is_leader: Arc<AtomicBool>,
    pub role_rx: watch::Receiver<Role>,
    /// MCP runtime reference, set lazily on first tool call from the client.
    ///
    /// Used by `FallbackLlmGenerator` to attempt MCP sampling for symbol
    /// descriptions instead of local LLM inference. `Arc<OnceCell<...>>`
    /// allows cloning `AppState` while sharing the same `OnceCell`.
    pub mcp_runtime: Arc<OnceCell<Arc<dyn McpServer + 'static>>>,
}

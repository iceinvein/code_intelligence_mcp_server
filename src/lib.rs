#![allow(
    clippy::collapsible_match,
    clippy::items_after_test_module,
    clippy::should_implement_trait
)]

pub mod agent_install;
pub mod classify;
pub mod cli;
pub mod config;
pub mod embeddings;
pub mod external_index;
pub mod graph;
pub mod handlers;
pub mod indexer;
pub mod install;
pub mod llm;
pub mod log_broadcast;
pub mod logging;
pub mod metrics;
pub mod os;
pub mod path;
pub mod registry;
pub mod reranker;
pub mod retrieval;
pub mod server;
pub mod session;
pub mod storage;
pub mod text;
pub mod tools;

//! Application state

use crate::config::Config;
use crate::indexer::pipeline::IndexPipeline;
use crate::leader::Role;
use crate::retrieval::Retriever;
use crate::storage::sqlite::SqliteStore;
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
}

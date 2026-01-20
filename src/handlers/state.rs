//! Application state

use crate::config::Config;
use crate::indexer::pipeline::IndexPipeline;
use crate::retrieval::Retriever;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub indexer: IndexPipeline,
    pub retriever: Retriever,
}

//! HyDE (Hypothetical Document Embeddings) module
//!
//! HyDE generates hypothetical code snippets to improve embedding-based retrieval
//! when queries are abstract or lack specific code terms.

pub mod generator;

pub use generator::{HypotheticalCodeGenerator, HyDEQuery};

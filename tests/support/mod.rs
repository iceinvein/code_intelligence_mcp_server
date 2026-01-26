//! Test support module
//!
//! This module provides shared test utilities including fixtures and helper functions.

pub mod helpers;

// Re-export helper functions for convenient use in tests
pub use helpers::{
    create_test_symbol, create_test_symbol_with_language, create_test_symbol_with_text,
    tmp_db_path, tmp_dir,
};

// Re-export rstest fixtures for convenient use in tests
pub mod fixtures;
pub use fixtures::*;

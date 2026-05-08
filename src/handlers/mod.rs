//! MCP tool handlers
//!
//! Handlers are organized by domain:
//! - `index`: refresh_index, get_index_stats
//! - `search`: search_code, explain_search, find_similar_code
//! - `navigation`: get_definition, find_references, get_file_symbols, etc.
//! - `graph`: call_hierarchy, type_graph, dependency_graph, trace_data_flow
//! - `analysis`: find_affected_code, find_dead_code, predict_impact, etc.
//! - `cross_repo`: search_across_repos, explore_cross_repo_dependencies
//! - `learning`: report_selection, report_file_access

mod analysis;
mod budget;
mod cross_repo;
mod graph;
mod index;
mod learning;
mod navigation;
mod planning;
mod search;
mod state;

use crate::path::PathError;
use rust_mcp_sdk::schema::{CallToolError, CallToolRequestParams};
use serde::de::DeserializeOwned;

pub use state::AppState;

// Re-export all handlers for use by server dispatch
pub use analysis::{
    handle_find_affected_code, handle_find_dead_code, handle_find_duplicates,
    handle_find_stale_descriptions, handle_find_tests_for_symbol,
    handle_find_undocumented_symbols, handle_get_context_bundle, handle_predict_impact,
    handle_search_decorators, handle_search_framework_patterns, handle_search_todos,
};
pub use cross_repo::{handle_explore_cross_repo_dependencies, handle_search_across_repos};
pub use graph::{
    handle_explore_dependency_graph, handle_get_call_hierarchy, handle_get_similarity_cluster,
    handle_get_type_graph, handle_trace_data_flow,
};
pub use index::{handle_get_index_stats, handle_refresh_index};
pub use learning::{handle_report_file_access, handle_report_selection};
pub use navigation::{
    handle_find_references, handle_get_definition, handle_get_file_symbols,
    handle_get_module_summary, handle_get_usage_examples, handle_hydrate_symbols,
    handle_summarize_file,
};
pub use planning::handle_plan_code_investigation;
pub use search::{handle_explain_search, handle_find_similar_code, handle_search_code};

/// Parse tool arguments from MCP request
pub fn parse_tool_args<T: DeserializeOwned>(
    params: &CallToolRequestParams,
) -> std::result::Result<T, CallToolError> {
    let args = params.arguments.clone().unwrap_or_default();
    let args = serde_json::Value::Object(args);
    serde_json::from_value(args)
        .map_err(|err| CallToolError::invalid_arguments(&params.name, Some(err.to_string())))
}

/// Convert internal error to MCP tool error
///
/// Logs the error before converting to MCP error format for observability.
/// Preserves PathError context for helpful error messages.
pub fn tool_internal_error(err: anyhow::Error) -> CallToolError {
    let message = if let Some(path_err) = err.downcast_ref::<PathError>() {
        path_err.to_string()
    } else {
        err.to_string()
    };

    tracing::error!(
        error = %err,
        "Handler error: converting to MCP error"
    );
    CallToolError::from_message(message)
}

/// Extract a line containing the symbol name from text
pub fn extract_usage_line(text: &str, symbol_name: &str) -> Option<String> {
    for line in text.lines() {
        if line.contains(symbol_name) {
            let mut s = line.trim().to_string();
            if s.len() > 200 {
                s.truncate(200);
            }
            return Some(s);
        }
    }
    None
}

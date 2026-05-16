//! Indexing-related handlers: refresh_index, get_index_stats

use super::AppState;
use crate::path::{PathError, Utf8PathBuf};
use crate::tools::*;
use serde_json::json;

/// Handle refresh_index tool
pub async fn handle_refresh_index(
    state: &AppState,
    tool: RefreshIndexTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let normalizer = crate::path::PathNormalizer::new(state.config.base_dir.clone());

    let stats = if let Some(files) = tool.files {
        let paths = files
            .into_iter()
            .map(|p| {
                // Convert to Utf8Path and validate it's within base
                let path_buf = std::path::PathBuf::from(&p);
                let utf8_path = Utf8PathBuf::from_path_buf(path_buf.clone())
                    .map_err(|_| PathError::NonUtf8 { path: path_buf })?;

                // Validate path is within base directory
                normalizer.validate_within_base(&utf8_path)?;

                Ok(utf8_path)
            })
            .collect::<Result<Vec<_>, anyhow::Error>>()?;

        // Pass Utf8PathBuf slice directly to pipeline API
        state.indexer.index_paths(&paths).await
    } else {
        state.indexer.index_all().await
    }?;

    Ok(json!({
        "ok": true,
        "stats": stats,
    }))
}

/// Handle get_index_stats tool
pub fn handle_get_index_stats(state: &AppState) -> Result<serde_json::Value, anyhow::Error> {
    let sqlite = &state.sqlite;

    let symbols = sqlite.count_symbols()?;
    let edges = sqlite.count_edges()?;
    let descriptions = sqlite.count_descriptions()?;
    let undescribed = sqlite.count_undescribed_symbols()?;
    let last_updated = sqlite.most_recent_symbol_update()?;
    let latest_index_run = sqlite.latest_index_run()?;
    let latest_search_run = sqlite.latest_search_run()?;

    Ok(json!({
        "base_dir": state.config.base_dir,
        "symbols": symbols,
        "edges": edges,
        "descriptions": descriptions,
        "undescribed_symbols": undescribed,
        "last_updated_unix_s": last_updated,
        "latest_index_run": latest_index_run,
        "latest_search_run": latest_search_run,
    }))
}

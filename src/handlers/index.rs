//! Indexing-related handlers: refresh_index, get_index_stats

use super::AppState;
use crate::indexer::pipeline::ExternalIndexTrigger;
use crate::path::{PathError, PathNormalizer, Utf8PathBuf};
use crate::tools::*;
use serde_json::json;

/// Handle refresh_index tool
pub async fn handle_refresh_index(
    state: &AppState,
    tool: RefreshIndexTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let normalizer = PathNormalizer::new(state.config.base_dir.clone());

    let outcome = if let Some(files) = tool.files {
        let paths = files
            .into_iter()
            .map(|p| {
                // Convert to Utf8Path and validate it's within base
                let path_buf = std::path::PathBuf::from(&p);
                let utf8_path = Utf8PathBuf::from_path_buf(path_buf.clone())
                    .map_err(|_| PathError::NonUtf8 { path: path_buf })?;

                let candidate = if utf8_path.is_absolute() {
                    utf8_path
                } else {
                    normalizer.join_base(utf8_path.as_str())
                };

                // Validate the resolved target is within base directory.
                normalizer
                    .canonicalize_within_base(&candidate)
                    .map_err(anyhow::Error::from)
            })
            .collect::<Result<Vec<_>, anyhow::Error>>()?;

        // Pass Utf8PathBuf slice directly to pipeline API
        state
            .indexer
            .index_paths_with_external_index(&paths, ExternalIndexTrigger::ManualRefresh)
            .await
    } else {
        state
            .indexer
            .index_all_with_external_index(ExternalIndexTrigger::ManualRefresh)
            .await
    }?;

    Ok(json!({
        "ok": true,
        "stats": outcome.stats,
        "external_index": outcome.external_index,
    }))
}

/// Handle import_external_index tool
pub async fn handle_import_external_index(
    state: &AppState,
    tool: ImportExternalIndexTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let artifact = resolve_external_index_artifact_path(state, &tool.artifact_path)?;
    let sqlite = state.sqlite.clone();
    let base_dir = state.config.base_dir.clone();

    let report = tokio::task::spawn_blocking(move || {
        crate::external_index::importer::import_external_index(
            &sqlite,
            base_dir.as_str(),
            artifact.as_std_path(),
        )
    })
    .await
    .map_err(|err| anyhow::anyhow!("External index import task failed: {err}"))??;

    Ok(json!({
        "ok": true,
        "index_id": report.index_id,
        "symbols_imported": report.symbols_imported,
        "references_imported": report.references_imported,
        "symbols_mapped": report.symbols_mapped,
        "symbols_unmapped": report.symbols_unmapped,
    }))
}

/// Handle generate_external_index tool
pub async fn handle_generate_external_index(
    state: &AppState,
    tool: GenerateExternalIndexTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let sqlite = state.sqlite.clone();
    let base_dir = state.config.base_dir.clone();
    let repo_data_dir = state
        .config
        .db_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Configured SQLite path has no parent directory"))?
        .to_path_buf();
    let producer = tool.producer;
    let language = tool.language;

    tokio::task::spawn_blocking(move || {
        crate::external_index::producers::generate_and_import(
            &sqlite,
            base_dir.as_str(),
            &repo_data_dir,
            producer,
            language,
        )
    })
    .await
    .map_err(|err| anyhow::anyhow!("External index producer task failed: {err}"))?
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
    let external = sqlite.external_overlay_stats()?;

    Ok(json!({
        "base_dir": state.config.base_dir,
        "symbols": symbols,
        "edges": edges,
        "descriptions": descriptions,
        "undescribed_symbols": undescribed,
        "last_updated_unix_s": last_updated,
        "latest_index_run": latest_index_run,
        "latest_search_run": latest_search_run,
        "external_indexes": {
            "index_count": external.index_count,
            "symbol_count": external.symbol_count,
            "reference_count": external.reference_count,
            "mapped_symbol_count": external.mapped_symbol_count,
        },
    }))
}

fn resolve_external_index_artifact_path(
    state: &AppState,
    artifact_path: &str,
) -> Result<Utf8PathBuf, anyhow::Error> {
    let normalizer = PathNormalizer::new(state.config.base_dir.clone());
    let path_buf = std::path::PathBuf::from(artifact_path);
    let utf8_path = Utf8PathBuf::from_path_buf(path_buf.clone())
        .map_err(|_| PathError::NonUtf8 { path: path_buf })?;

    let candidate = if utf8_path.is_absolute() {
        utf8_path
    } else {
        normalizer.join_base(utf8_path.as_str())
    };

    normalizer
        .canonicalize_within_base(&candidate)
        .map_err(anyhow::Error::from)
}

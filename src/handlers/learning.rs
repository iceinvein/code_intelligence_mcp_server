//! User learning handlers: report_selection, report_file_access

use super::AppState;
use crate::tools::*;
use serde_json::json;

/// Handle report_selection tool
pub async fn handle_report_selection(
    state: &AppState,
    tool: ReportSelectionTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let sqlite = &state.sqlite;

    // Strip query controls (id:, file:, lang:, etc.) before normalizing,
    // so the key matches what retrieval uses for selection boost lookup.
    let (query_without_controls, _controls) =
        crate::retrieval::query::parse_query_controls(&tool.query);
    let normalized = query_without_controls.to_lowercase().trim().to_string();

    let row_id = sqlite.insert_query_selection(
        &tool.query,
        &normalized,
        &tool.selected_symbol_id,
        tool.position,
    )?;

    Ok(json!({
        "ok": true,
        "recorded": true,
        "selection_id": row_id,
        "query_normalized": normalized,
    }))
}

/// Handle report_file_access tool
pub async fn handle_report_file_access(
    state: &AppState,
    tool: ReportFileAccessTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let action = tool.action.as_deref().unwrap_or("view");
    let (view_inc, edit_inc) = match action {
        "edit" => (0, 1),
        _ => (1, 0), // default to "view"
    };

    state
        .sqlite
        .upsert_file_affinity(&tool.file_path, view_inc, edit_inc)?;

    Ok(json!({
        "ok": true,
        "recorded": true,
        "file_path": tool.file_path,
        "action": action,
    }))
}

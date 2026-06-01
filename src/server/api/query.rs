//! `/api/query/*` endpoints: the human-facing wrappers around the MCP query
//! tools (search, investigate, ask, hydrate, repo-map, definition, references).
//! Each resolves the target repo, calls the matching handler, and wraps the
//! result in the stable agent-contract envelope.

use axum::{extract::State, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use super::{ApiError, ApiState};

#[derive(Debug, Deserialize)]
pub(crate) struct QuerySearchRequest {
    repo: String,
    query: String,
    limit: Option<u32>,
    context: Option<String>,
    exported_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QueryInvestigateRequest {
    repo: String,
    question: String,
    target: Option<String>,
    file_path: Option<String>,
    mode: Option<String>,
    max_hops: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QueryAskRequest {
    repo: String,
    question: String,
    target: Option<String>,
    file_path: Option<String>,
    mode: Option<String>,
    max_evidence: Option<u32>,
    quality: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QueryHydrateRequest {
    repo: String,
    ids: Vec<String>,
    mode: Option<String>,
    verbose: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QueryRepoMapRequest {
    repo: String,
    budget_tokens: Option<u32>,
    max_files: Option<u32>,
    max_symbols_per_file: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QueryDefinitionRequest {
    repo: String,
    symbol_name: String,
    file: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QueryReferencesRequest {
    repo: String,
    symbol_name: String,
    file: Option<String>,
    reference_type: Option<String>,
    limit: Option<u32>,
}

pub(crate) async fn handle_query_search(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QuerySearchRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.query.trim().is_empty() {
        return Err(ApiError("query is required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::SearchCodeTool {
        query: req.query,
        limit: req.limit,
        exported_only: req.exported_only,
        context: req.context,
    };
    let result =
        crate::handlers::handle_search_code(&app_state.retriever, &app_state.config.db_path, tool)
            .await
            .map_err(|e| ApiError(format!("search failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "search",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

pub(crate) async fn handle_query_investigate(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryInvestigateRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.question.trim().is_empty() {
        return Err(ApiError("question is required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::InvestigateTool {
        question: req.question,
        target: req.target,
        file_path: req.file_path,
        mode: req.mode,
        max_hops: req.max_hops,
    };
    let result = crate::handlers::handle_investigate(&app_state, tool)
        .await
        .map_err(|e| ApiError(format!("investigate failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "investigate",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

pub(crate) async fn handle_query_ask(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryAskRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.question.trim().is_empty() {
        return Err(ApiError("question is required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::AskCodeTool {
        question: req.question,
        target: req.target,
        file_path: req.file_path,
        mode: req.mode,
        max_evidence: req.max_evidence,
        quality: req.quality,
    };
    let result = crate::handlers::handle_ask_code(&app_state, tool)
        .await
        .map_err(|e| ApiError(format!("ask failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "ask",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

pub(crate) async fn handle_query_hydrate(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryHydrateRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.ids.is_empty() {
        return Err(ApiError("ids are required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::HydrateSymbolsTool {
        ids: req.ids,
        mode: req.mode,
        verbose: req.verbose,
    };
    let result = crate::handlers::handle_hydrate_symbols(&app_state, tool)
        .map_err(|e| ApiError(format!("hydrate failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "hydrate",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

pub(crate) async fn handle_query_repo_map(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryRepoMapRequest>,
) -> Result<Json<Value>, ApiError> {
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let result = crate::handlers::handle_repo_map(
        &app_state,
        crate::handlers::RepoMapOptions {
            budget_tokens: req.budget_tokens,
            max_files: req.max_files,
            max_symbols_per_file: req.max_symbols_per_file,
        },
    )
    .map_err(|e| ApiError(format!("repo-map failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "repo-map",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

pub(crate) async fn handle_query_definition(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryDefinitionRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.symbol_name.trim().is_empty() {
        return Err(ApiError("symbol_name is required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::GetDefinitionTool {
        symbol_name: req.symbol_name,
        file: req.file,
        limit: req.limit,
    };
    let result = crate::handlers::handle_get_definition(&app_state, tool)
        .await
        .map_err(|e| ApiError(format!("definition failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "definition",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

pub(crate) async fn handle_query_references(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<QueryReferencesRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.symbol_name.trim().is_empty() {
        return Err(ApiError("symbol_name is required".to_string()));
    }
    let (repo_path, repo_id, app_state) = resolve_query_repo(&state, &req.repo).await?;
    let tool = crate::tools::FindReferencesTool {
        symbol_name: req.symbol_name,
        file: req.file,
        reference_type: req.reference_type,
        limit: req.limit,
    };
    // handle_find_references is synchronous (unlike handle_get_definition above), so no .await here.
    let result = crate::handlers::handle_find_references(&app_state, tool)
        .map_err(|e| ApiError(format!("references failed: {e}")))?;
    let index_version = app_state.sqlite.most_recent_symbol_update().ok().flatten();
    Ok(Json(query_envelope(
        "references",
        &repo_path,
        &repo_id,
        index_version,
        result,
    )))
}

async fn resolve_query_repo(
    state: &ApiState,
    repo: &str,
) -> Result<
    (
        crate::path::Utf8PathBuf,
        String,
        Arc<crate::handlers::AppState>,
    ),
    ApiError,
> {
    let raw = crate::path::Utf8PathBuf::from(repo);
    let canonical = dunce::canonicalize(raw.as_std_path()).map_err(|e| {
        ApiError(format!(
            "workspace not found or not accessible: {repo}: {e}"
        ))
    })?;
    if !canonical.is_dir() {
        return Err(ApiError(format!("workspace is not a directory: {repo}")));
    }
    let repo_path = crate::path::Utf8PathBuf::from_path_buf(canonical)
        .map_err(|_| ApiError(format!("workspace path is not valid UTF-8: {repo}")))?;
    let repo_id = crate::registry::RepoRegistry::path_hash(repo_path.as_str());
    let app_state = state
        .session_manager
        .get_or_create_repo(&repo_path)
        .await
        .map_err(|e| ApiError(format!("failed to load repo: {e}")))?;
    Ok((repo_path, repo_id, app_state))
}

fn query_envelope(
    command: &str,
    repo_path: &crate::path::Utf8Path,
    repo_id: &str,
    index_version_unix_s: Option<i64>,
    result: Value,
) -> Value {
    json!({
        "ok": true,
        "command": command,
        "repo": {
            "path": repo_path.as_str(),
            "id": repo_id,
        },
        "index": {
            "version_unix_s": index_version_unix_s,
            "fresh": true,
        },
        "warnings": [],
        "result": result,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_envelope_has_stable_agent_contract_fields() {
        for command in [
            "ask",
            "search",
            "investigate",
            "hydrate",
            "repo-map",
            "definition",
            "references",
        ] {
            let envelope = query_envelope(
                command,
                crate::path::Utf8Path::new("/tmp/workspace"),
                "repo123",
                Some(123),
                json!({ "value": true }),
            );

            assert_eq!(envelope["ok"], true);
            assert_eq!(envelope["command"], command);
            assert_eq!(envelope["repo"]["path"], "/tmp/workspace");
            assert_eq!(envelope["repo"]["id"], "repo123");
            assert_eq!(envelope["index"]["version_unix_s"], 123);
            assert_eq!(envelope["index"]["fresh"], true);
            assert_eq!(envelope["warnings"].as_array().unwrap().len(), 0);
            assert_eq!(envelope["result"]["value"], true);
        }
    }
}

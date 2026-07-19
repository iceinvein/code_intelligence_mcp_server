//! `/api/consent` endpoints: list repos awaiting an indexing decision (plus
//! previously declined repos), and approve/decline/re-approve them.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use super::{validate_repo_path, ApiError, ApiState};

pub(crate) fn build_consent_response(
    pending: Vec<crate::session::PendingConsent>,
    repos: Vec<crate::registry::RepoEntry>,
) -> Value {
    let declined: Vec<Value> = repos
        .into_iter()
        .filter(|e| e.consent == crate::registry::IndexConsent::Declined)
        .map(|e| {
            let detected =
                crate::server::project_check::classify_repo(crate::path::Utf8Path::new(&e.path))
                    .kind();
            json!({
                "repo_path": e.path,
                "repo_id": crate::registry::RepoRegistry::path_hash(&e.path),
                "detected": detected,
            })
        })
        .collect();
    json!({
        "pending": serde_json::to_value(&pending).unwrap_or(Value::Null),
        "declined": declined,
    })
}

pub(crate) async fn handle_consent_get(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Value>, ApiError> {
    let pending = state.session_manager.list_pending();
    let repos = state
        .session_manager
        .registry
        .list_all()
        .map_err(|e| ApiError(format!("failed to list repos: {e}")))?;
    Ok(Json(build_consent_response(pending, repos)))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ConsentDecisionRequest {
    repo: String,
    decision: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ConsentDecision {
    Approve,
    Decline,
}

fn parse_consent_decision(decision: &str) -> Result<ConsentDecision, String> {
    match decision {
        "approve" => Ok(ConsentDecision::Approve),
        "decline" => Ok(ConsentDecision::Decline),
        other => Err(format!(
            "decision must be \"approve\" or \"decline\", got: {other}"
        )),
    }
}

pub(crate) async fn handle_consent_post(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<ConsentDecisionRequest>,
) -> Result<Response, ApiError> {
    let decision = match parse_consent_decision(&req.decision) {
        Ok(d) => d,
        Err(msg) => {
            return Ok((StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response());
        }
    };
    let repo_path = match validate_repo_path(&req.repo) {
        Ok(p) => p,
        Err(msg) => {
            return Ok((StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response());
        }
    };
    let repo_id = crate::registry::RepoRegistry::path_hash(repo_path.as_str());

    // Only act on repos the gate already surfaced (pending) or the user already
    // declined. Indexing an arbitrary new path is the Repos -> Add flow, not this.
    let is_declined = matches!(
        state
            .session_manager
            .registry
            .consent_status(repo_path.as_str())
            .ok()
            .flatten(),
        Some(crate::registry::IndexConsent::Declined)
    );
    if !state.session_manager.is_pending(&repo_id) && !is_declined {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "repo is neither pending nor previously declined; use Add Repo to index a new path"
            })),
        )
            .into_response());
    }

    match decision {
        ConsentDecision::Approve => {
            state
                .session_manager
                .get_or_create_repo(&repo_path)
                .await
                .map_err(|e| ApiError(format!("failed to start indexing: {e}")))?;
            state.session_manager.clear_pending(&repo_id);
            Ok((
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "status": "indexing_started",
                    "repo": repo_path.as_str(),
                    "repo_id": repo_id,
                })),
            )
                .into_response())
        }
        ConsentDecision::Decline => {
            state
                .session_manager
                .registry
                .set_consent(repo_path.as_str(), crate::registry::IndexConsent::Declined)
                .map_err(|e| ApiError(format!("failed to record decline: {e}")))?;
            state.session_manager.clear_pending(&repo_id);
            Ok((
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "status": "declined",
                    "repo": repo_path.as_str(),
                    "repo_id": repo_id,
                })),
            )
                .into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Utf8PathBuf;
    use crate::registry::RepoEntry;

    #[test]
    fn build_consent_response_shapes_pending_and_filters_declined() {
        let pending = vec![crate::session::PendingConsent {
            repo_path: "/Users/me/wt".to_string(),
            repo_id: "id_pending".to_string(),
            detected: "git_worktree".to_string(),
            recommendation: "ask before indexing".to_string(),
            detail: Some("git worktree of /Users/me/main".to_string()),
            first_seen_unix_s: 10,
            last_seen_unix_s: 20,
            occurrences: 3,
        }];
        let repos = vec![
            RepoEntry {
                path: "/Users/me/declined".to_string(),
                name: "declined".to_string(),
                data_dir: Utf8PathBuf::from("/data/declined"),
                created_at: "x".to_string(),
                last_accessed: "x".to_string(),
                consent: crate::registry::IndexConsent::Declined,
                initial_index_approved_at: None,
                initial_index_completed_at: None,
            },
            RepoEntry {
                path: "/Users/me/approved".to_string(),
                name: "approved".to_string(),
                data_dir: Utf8PathBuf::from("/data/approved"),
                created_at: "x".to_string(),
                last_accessed: "x".to_string(),
                consent: crate::registry::IndexConsent::Approved,
                initial_index_approved_at: None,
                initial_index_completed_at: None,
            },
        ];

        let v = build_consent_response(pending, repos);

        // Pending item carries every field the frontend type expects.
        assert_eq!(v["pending"][0]["repo_path"], "/Users/me/wt");
        assert_eq!(v["pending"][0]["repo_id"], "id_pending");
        assert_eq!(v["pending"][0]["detected"], "git_worktree");
        assert_eq!(v["pending"][0]["recommendation"], "ask before indexing");
        assert_eq!(v["pending"][0]["detail"], "git worktree of /Users/me/main");
        assert_eq!(v["pending"][0]["occurrences"], 3);

        // Only the declined repo is surfaced; the approved one is filtered out.
        assert_eq!(v["declined"].as_array().unwrap().len(), 1);
        assert_eq!(v["declined"][0]["repo_path"], "/Users/me/declined");
        assert_eq!(v["declined"][0]["detected"], "standard");
        assert!(v["declined"][0]["repo_id"].is_string());
    }

    #[test]
    fn parse_consent_decision_accepts_approve_decline_and_rejects_other() {
        assert_eq!(
            parse_consent_decision("approve"),
            Ok(ConsentDecision::Approve)
        );
        assert_eq!(
            parse_consent_decision("decline"),
            Ok(ConsentDecision::Decline)
        );
        let err = parse_consent_decision("maybe").unwrap_err();
        assert!(err.contains("approve"));
        assert!(err.contains("maybe"));
    }
}

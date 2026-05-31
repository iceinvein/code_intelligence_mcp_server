//! Structured JSON payloads returned to the agent when indexing a repo needs
//! the user's consent (or was previously declined). These are returned as
//! normal successful tool results so the agent reliably parses the `status`
//! field and relays the question to the user.

use crate::path::Utf8Path;
use crate::server::project_check::classify_repo;
use serde_json::{json, Value};

/// Payload for a never-indexed repo bound implicitly: ask the user, then call
/// `approve_indexing`.
pub fn consent_required_payload(repo_path: &str, repo_id: &str) -> Value {
    let class = classify_repo(Utf8Path::new(repo_path));
    let mut obj = json!({
        "status": "consent_required",
        "repo": repo_path,
        "repo_id": repo_id,
        "detected": class.kind(),
        "recommendation": class.recommendation(),
        "action": format!(
            "Ask the user whether to index this repo. Then call approve_indexing with {{\"repo\": \"{repo_path}\", \"decision\": \"approve\"}} or {{\"decision\": \"decline\"}}."
        ),
        "message": format!(
            "Repo {repo_path} is not indexed yet. Indexing uses GPU and memory. Confirm with the user before proceeding."
        ),
    });
    if let Some(detail) = class.detail() {
        obj["detail"] = json!(detail);
    }
    obj
}

/// Payload for a repo the user previously declined.
pub fn declined_payload(repo_path: &str, repo_id: &str) -> Value {
    json!({
        "status": "declined",
        "repo": repo_path,
        "repo_id": repo_id,
        "message": format!(
            "Indexing was previously declined for {repo_path}."
        ),
        "action": format!(
            "If the user now wants this repo indexed, call approve_indexing with {{\"repo\": \"{repo_path}\", \"decision\": \"approve\"}}."
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_required_standard_repo_has_expected_fields() {
        let v = consent_required_payload("/Users/me/project", "deadbeefdeadbeef");
        assert_eq!(v["status"], "consent_required");
        assert_eq!(v["repo"], "/Users/me/project");
        assert_eq!(v["repo_id"], "deadbeefdeadbeef");
        assert_eq!(v["detected"], "standard");
        assert!(v["action"].as_str().unwrap().contains("approve_indexing"));
        // No worktree detail for a plain path.
        assert!(v.get("detail").is_none());
    }

    #[test]
    fn declined_payload_has_expected_fields() {
        let v = declined_payload("/Users/me/project", "deadbeefdeadbeef");
        assert_eq!(v["status"], "declined");
        assert!(v["action"].as_str().unwrap().contains("approve"));
        assert_eq!(v["repo"], "/Users/me/project");
        assert_eq!(v["repo_id"], "deadbeefdeadbeef");
        assert!(v["message"].as_str().unwrap().contains("declined"));
    }

    #[test]
    fn consent_required_worktree_includes_detail() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = crate::path::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        std::fs::write(
            dir.join(".git"),
            b"gitdir: /Users/me/main-repo/.git/worktrees/feature\n",
        )
        .unwrap();
        let v = consent_required_payload(dir.as_str(), "abc123abc123abc1");
        assert_eq!(v["detected"], "git_worktree");
        assert_eq!(v["detail"], "git worktree of /Users/me/main-repo");
    }
}

//! Structured JSON payloads returned to the agent when indexing a repo needs
//! the user's consent (or was previously declined). These are returned as
//! normal successful tool results so the agent reliably parses the `status`
//! field and relays the question to the user.

use crate::path::Utf8Path;
use crate::server::jobs::Job;
use crate::server::project_check::classify_repo;
use serde_json::{json, Value};

/// Payload for a never-indexed repository: ask the user, then call
/// `approve_indexing` only after explicit confirmation.
pub fn consent_required_payload(repo_path: &str, repo_id: &str) -> Value {
    let class = classify_repo(Utf8Path::new(repo_path));
    let mut obj = json!({
        "status": "consent_required",
        "repo": repo_path,
        "repo_id": repo_id,
        "detected": class.kind(),
        "recommendation": class.recommendation(),
        "action": "Tell the user in chat that this repository needs its first full index and that indexing uses local compute, memory, and disk. Ask for permission and wait for explicit approval. Only then call approve_indexing with decision \"approve\". If the user declines, call it with decision \"decline\".",
        "message": format!(
            "Repository {repo_path} needs its first full index before code tools can run."
        ),
    });
    if let Some(detail) = class.detail() {
        obj["detail"] = json!(detail);
    }
    obj
}

/// Payload returned while the repository's first full index is running.
pub fn indexing_payload(job: &Job, started: bool) -> Value {
    let status = if started {
        "indexing_started"
    } else {
        "indexing_in_progress"
    };
    let message = if started {
        format!("The first full index has started for {}.", job.repo_path)
    } else {
        format!(
            "The first full index is still running for {}.",
            job.repo_path
        )
    };

    json!({
        "ok": true,
        "status": status,
        "repo": job.repo_path,
        "repo_id": job.repo_id,
        "job_id": job.id,
        "message": message,
        "action": "Tell the user that indexing is running. Retry the original code tool after the job completes. Do not request approval again.",
    })
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
    fn consent_payload_requires_chat_confirmation_before_approval() {
        let value = consent_required_payload("/Users/me/project", "deadbeefdeadbeef");
        let action = value["action"].as_str().unwrap();
        assert!(action.contains("Tell the user in chat"));
        assert!(action.contains("wait for explicit approval"));
        assert!(action.contains("approve_indexing"));
        assert!(value["message"]
            .as_str()
            .unwrap()
            .contains("first full index"));
    }

    #[test]
    fn indexing_payload_distinguishes_started_from_in_progress() {
        let job = crate::server::jobs::Job {
            id: "initial-repo-1".to_string(),
            kind: crate::server::jobs::JobKind::InitialBind,
            repo_id: "repo".to_string(),
            repo_path: "/repo".to_string(),
            status: crate::server::jobs::JobStatus::Running,
            started_at_unix_s: 1,
            finished_at_unix_s: None,
            duration_ms: None,
            stats: None,
            error: None,
            coalesced_count: 0,
        };
        assert_eq!(indexing_payload(&job, true)["status"], "indexing_started");
        assert_eq!(
            indexing_payload(&job, false)["status"],
            "indexing_in_progress"
        );
    }

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

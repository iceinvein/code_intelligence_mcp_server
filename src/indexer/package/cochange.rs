//! Git co-change analysis for change impact prediction.
//!
//! Walks the git log to build a pairwise co-change matrix showing which files
//! tend to be modified together. This information is used by the `predict_impact`
//! tool to augment structural dependency analysis with historical change patterns.

use crate::path::Utf8Path;
use crate::storage::sqlite::SqliteStore;
use std::collections::HashMap;

/// Statistics returned after building the co-change matrix.
#[derive(Debug, Clone)]
pub struct CoChangeStats {
    pub commits_walked: usize,
    pub commits_skipped: usize,
    pub pairs_recorded: usize,
}

/// Build a co-change matrix from git history and store it in SQLite.
///
/// Algorithm:
/// 1. Open the git repository at `repo_path`
/// 2. Walk the revision log (HEAD, topological sort) up to `max_commits`
/// 3. For each commit, diff against parent to get changed files
/// 4. Skip commits with >50 changed files (merges or large refactors)
/// 5. Build pairwise combinations of changed files
/// 6. Track per-file total commit counts
/// 7. Clear existing co_changes and bulk-insert all pairs
///    with confidence = co_change_count / min(total_a, total_b)
pub fn build_co_change_matrix(
    repo_path: &Utf8Path,
    sqlite: &SqliteStore,
    max_commits: usize,
) -> anyhow::Result<CoChangeStats> {
    let repo = git2::Repository::open(repo_path.as_std_path())?;

    let mut revwalk = repo.revwalk()?;
    revwalk.push_head()?;
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL)?;

    // Per-file commit count
    let mut file_commit_counts: HashMap<String, u32> = HashMap::new();
    // Pairwise co-change count: (file_a, file_b) -> count  (always file_a < file_b)
    let mut pair_counts: HashMap<(String, String), u32> = HashMap::new();

    let mut commits_walked = 0usize;
    let mut commits_skipped = 0usize;

    for oid_result in revwalk {
        if commits_walked >= max_commits {
            break;
        }

        let oid = match oid_result {
            Ok(oid) => oid,
            Err(_) => continue,
        };

        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Get changed files by diffing against parent
        let changed_files = match get_changed_files(&repo, &commit) {
            Ok(files) => files,
            Err(_) => {
                commits_skipped += 1;
                continue;
            }
        };

        // Skip commits with >50 changed files (merges or large refactors)
        if changed_files.len() > 50 {
            commits_skipped += 1;
            commits_walked += 1;
            continue;
        }

        // Skip empty commits
        if changed_files.is_empty() {
            commits_walked += 1;
            continue;
        }

        // Update per-file commit counts
        for file in &changed_files {
            *file_commit_counts.entry(file.clone()).or_insert(0) += 1;
        }

        // Build pairwise combinations
        let files_vec: Vec<&String> = changed_files.iter().collect();
        for i in 0..files_vec.len() {
            for j in (i + 1)..files_vec.len() {
                let (a, b) = if files_vec[i] < files_vec[j] {
                    (files_vec[i].clone(), files_vec[j].clone())
                } else {
                    (files_vec[j].clone(), files_vec[i].clone())
                };
                *pair_counts.entry((a, b)).or_insert(0) += 1;
            }
        }

        commits_walked += 1;
    }

    // Clear existing co_changes and bulk-insert in a single transaction
    let pairs_recorded = pair_counts.len();
    {
        let conn = sqlite.write()?;
        conn.execute_batch("BEGIN")?;
        crate::storage::sqlite::queries::cochange::clear_co_changes(&conn)?;
        for ((file_a, file_b), co_count) in &pair_counts {
            let total_a = *file_commit_counts.get(file_a).unwrap_or(&0);
            let total_b = *file_commit_counts.get(file_b).unwrap_or(&0);
            crate::storage::sqlite::queries::cochange::upsert_co_change(
                &conn, file_a, file_b, *co_count, total_a, total_b,
            )?;
        }
        conn.execute_batch("COMMIT")?;
    }

    Ok(CoChangeStats {
        commits_walked,
        commits_skipped,
        pairs_recorded,
    })
}

/// Extract the set of changed file paths from a commit by diffing against its parent.
fn get_changed_files(
    repo: &git2::Repository,
    commit: &git2::Commit,
) -> anyhow::Result<std::collections::HashSet<String>> {
    let tree = commit.tree()?;
    let mut changed = std::collections::HashSet::new();

    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };

    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;

    diff.foreach(
        &mut |delta, _| {
            if let Some(path) = delta.new_file().path() {
                if let Some(path_str) = path.to_str() {
                    changed.insert(path_str.to_string());
                }
            }
            if let Some(path) = delta.old_file().path() {
                if let Some(path_str) = path.to_str() {
                    changed.insert(path_str.to_string());
                }
            }
            true
        },
        None,
        None,
        None,
    )?;

    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::operations::SqliteStore;
    use tempfile::TempDir;

    /// Create a test git repo with some commits for co-change analysis.
    fn create_test_repo() -> (TempDir, git2::Repository) {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        // Configure git user for commits
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();

        (dir, repo)
    }

    fn commit_files(repo: &git2::Repository, files: &[(&str, &str)], message: &str) {
        let mut index = repo.index().unwrap();

        for (path, content) in files {
            let full_path = repo.workdir().unwrap().join(path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full_path, content).unwrap();
            index.add_path(std::path::Path::new(path)).unwrap();
        }

        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = repo.signature().unwrap();

        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();

        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .unwrap();
    }

    #[test]
    fn test_build_co_change_matrix_basic() {
        let (dir, repo) = create_test_repo();
        let repo_path = Utf8Path::new(dir.path().to_str().unwrap());

        // Commit 1: a.rs and b.rs changed together
        commit_files(&repo, &[("src/a.rs", "fn a(){}"), ("src/b.rs", "fn b(){}")], "commit 1");

        // Commit 2: a.rs and c.rs changed together
        commit_files(&repo, &[("src/a.rs", "fn a(){ // v2 }"), ("src/c.rs", "fn c(){}")], "commit 2");

        // Commit 3: a.rs and b.rs changed together again
        commit_files(&repo, &[("src/a.rs", "fn a(){ // v3 }"), ("src/b.rs", "fn b(){ // v2 }")], "commit 3");

        // Create an in-memory SQLite store
        let sqlite = SqliteStore::open_in_memory().unwrap();
        sqlite.init().unwrap();

        let stats = build_co_change_matrix(repo_path, &sqlite, 500).unwrap();

        assert_eq!(stats.commits_walked, 3);
        assert_eq!(stats.commits_skipped, 0);
        assert!(stats.pairs_recorded > 0);

        // a.rs and b.rs should have co_change_count = 2 (commits 1 and 3)
        let co_changes = sqlite.get_co_changes_for_file("src/a.rs", 10).unwrap();
        assert!(!co_changes.is_empty());

        // Find the a.rs <-> b.rs pair
        let ab_pair = co_changes.iter().find(|c| {
            (c.file_a == "src/a.rs" && c.file_b == "src/b.rs")
                || (c.file_a == "src/b.rs" && c.file_b == "src/a.rs")
        });
        assert!(ab_pair.is_some(), "Should find a.rs <-> b.rs co-change pair");
        let ab = ab_pair.unwrap();
        assert_eq!(ab.co_change_count, 2);
    }

    #[test]
    fn test_build_co_change_matrix_empty_repo() {
        let (dir, _repo) = create_test_repo();
        let repo_path = Utf8Path::new(dir.path().to_str().unwrap());

        let sqlite = SqliteStore::open_in_memory().unwrap();
        sqlite.init().unwrap();
        // Empty repo has no HEAD, so push_head() will fail and we get an error
        let result = build_co_change_matrix(repo_path, &sqlite, 500);
        // Should return an error for empty repo (no HEAD)
        assert!(result.is_err());
    }

    #[test]
    fn test_build_co_change_matrix_respects_max_commits() {
        let (dir, repo) = create_test_repo();
        let repo_path = Utf8Path::new(dir.path().to_str().unwrap());

        // Create 5 commits
        for i in 0..5 {
            commit_files(
                &repo,
                &[("src/a.rs", &format!("fn a(){{ // v{} }}", i))],
                &format!("commit {}", i),
            );
        }

        let sqlite = SqliteStore::open_in_memory().unwrap();
        sqlite.init().unwrap();
        let stats = build_co_change_matrix(repo_path, &sqlite, 3).unwrap();

        assert_eq!(stats.commits_walked, 3);
    }

    #[test]
    fn test_confidence_ordering() {
        let (dir, repo) = create_test_repo();
        let repo_path = Utf8Path::new(dir.path().to_str().unwrap());

        // Create commits where a.rs+b.rs appear together 3 times, a.rs+c.rs once
        commit_files(&repo, &[("src/a.rs", "v1"), ("src/b.rs", "v1")], "c1");
        commit_files(&repo, &[("src/a.rs", "v2"), ("src/b.rs", "v2")], "c2");
        commit_files(&repo, &[("src/a.rs", "v3"), ("src/b.rs", "v3")], "c3");
        commit_files(&repo, &[("src/a.rs", "v4"), ("src/c.rs", "v1")], "c4");

        let sqlite = SqliteStore::open_in_memory().unwrap();
        sqlite.init().unwrap();
        build_co_change_matrix(repo_path, &sqlite, 500).unwrap();

        let co_changes = sqlite.get_co_changes_for_file("src/a.rs", 10).unwrap();
        assert!(co_changes.len() >= 2);
        // First result should have higher confidence (a+b: 3 co-changes vs a+c: 1)
        assert!(co_changes[0].confidence >= co_changes[1].confidence);
    }
}

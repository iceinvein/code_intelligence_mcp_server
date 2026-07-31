//! Resolving a git worktree to the repository it was created from.
//!
//! A linked worktree has `.git` as a FILE containing a `gitdir:` pointer, and
//! its "common directory" is the main repository's `.git`. That common
//! directory is the authoritative link; the older approach of splitting the
//! pointer text on `/.git/worktrees/` breaks for nested worktrees and for
//! non-default worktree layouts.

use crate::path::{Utf8Path, Utf8PathBuf};

/// Return the main repository root when `path` is a linked git worktree.
///
/// Returns `None` when `path` is not a git repository, is the main repository
/// itself, or when the resolved base is not a readable directory distinct from
/// `path`.
pub fn resolve_base_repo(path: &Utf8Path) -> Option<Utf8PathBuf> {
    let repo = git2::Repository::open(path.as_std_path()).ok()?;

    // Authoritative predicate: libgit2 knows whether this is a linked worktree.
    if !repo.is_worktree() {
        return None;
    }

    // For a linked worktree, `path()` is the worktree's own private git dir
    // (`<base>/.git/worktrees/<name>`), which contains a `commondir` file
    // holding the path to the shared git dir, normally the relative `../..`.
    // This is git's documented plumbing; `Repository::commondir()` would give
    // the same answer but only landed in git2 0.20, and this project pins 0.19.
    let git_dir = repo.path();
    let pointer = std::fs::read_to_string(git_dir.join("commondir")).ok()?;
    let pointer = pointer.trim();
    if pointer.is_empty() {
        return None;
    }
    let common = if std::path::Path::new(pointer).is_absolute() {
        std::path::PathBuf::from(pointer)
    } else {
        git_dir.join(pointer)
    };

    // Canonicalize to collapse the `../..` before taking the parent, otherwise
    // `parent()` would just strip one `..` component.
    let common = std::fs::canonicalize(common).ok()?;

    // `common` is `<base>/.git`, so the base root is its parent.
    let base_root = common.parent()?;
    if !base_root.is_dir() {
        return None;
    }

    // Guard against a degenerate layout resolving back to the worktree itself.
    let canonical_self = std::fs::canonicalize(path.as_std_path()).ok()?;
    if base_root == canonical_self {
        return None;
    }

    Utf8PathBuf::from_path_buf(base_root.to_path_buf()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a git repo with one commit at `root`.
    fn init_repo(root: &Utf8Path) -> git2::Repository {
        let repo = git2::Repository::init(root.as_std_path()).unwrap();
        std::fs::write(root.join("lib.rs").as_std_path(), "pub fn probe() {}\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("lib.rs")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        drop(tree);
        drop(index);
        repo
    }

    fn utf8(p: &std::path::Path) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(p.to_path_buf()).unwrap()
    }

    #[test]
    fn resolves_linked_worktree_to_its_main_repo() {
        let base_temp = tempfile::tempdir().unwrap();
        let base = utf8(base_temp.path());
        let repo = init_repo(&base);

        let wt_temp = tempfile::tempdir().unwrap();
        let wt = utf8(wt_temp.path()).join("feature");
        repo.worktree("feature", wt.as_std_path(), None).unwrap();

        let resolved = resolve_base_repo(&wt).expect("worktree must resolve to a base");
        assert_eq!(
            std::fs::canonicalize(resolved.as_std_path()).unwrap(),
            std::fs::canonicalize(base.as_std_path()).unwrap()
        );
    }

    #[test]
    fn main_repo_resolves_to_none() {
        let base_temp = tempfile::tempdir().unwrap();
        let base = utf8(base_temp.path());
        init_repo(&base);
        assert_eq!(resolve_base_repo(&base), None);
    }

    #[test]
    fn non_git_directory_resolves_to_none() {
        let temp = tempfile::tempdir().unwrap();
        let dir = utf8(temp.path());
        std::fs::write(dir.join("Cargo.toml").as_std_path(), b"[package]\n").unwrap();
        assert_eq!(resolve_base_repo(&dir), None);
    }

    #[test]
    fn missing_path_resolves_to_none() {
        assert_eq!(
            resolve_base_repo(Utf8Path::new("/nonexistent/definitely/not/here")),
            None
        );
    }
}

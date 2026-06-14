//! Heuristic check that guards auto-binding against random folders.
//!
//! When an agent spawns in a directory like `/private/tmp` or `$HOME`, MCP
//! clients often forward that directory via `roots/list`. Auto-binding such
//! a path triggers a full `initial_bind` index pass over thousands of
//! unrelated files. This module gates the implicit binding paths (roots/list
//! and the single-repo fallback) with two cheap filters:
//!
//! 1. A blocklist of system/temp roots that are never real projects.
//! 2. A check for at least one common project marker (`.git`, `Cargo.toml`,
//!    `package.json`, etc.).
//!
//! Explicit binding paths (the `?repo=` URL query and the `bind_workspace`
//! tool) bypass this check; the user has already declared intent.

use crate::path::{Utf8Path, Utf8PathBuf};
use std::fmt;

/// Reason an automatic bind was rejected. Surfaced via tracing so it lands
/// in the dashboard log stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Path matches a blocklisted system or temporary root.
    Blocklisted { matched: &'static str },
    /// No common project manifest or VCS directory was found.
    NoProjectMarkers,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::Blocklisted { matched } => {
                write!(f, "path is a known non-project root ({matched})")
            }
            SkipReason::NoProjectMarkers => write!(
                f,
                "no project markers found (looked for .git, Cargo.toml, package.json, pyproject.toml, go.mod, etc.)"
            ),
        }
    }
}

/// Project markers checked relative to the candidate directory. Hitting any
/// one of these passes the marker check.
const PROJECT_MARKERS: &[&str] = &[
    // VCS roots
    ".git",
    ".hg",
    ".svn",
    // Rust
    "Cargo.toml",
    // JS/TS/Node/Bun/Deno
    "package.json",
    "tsconfig.json",
    "deno.json",
    "deno.jsonc",
    "bun.lockb",
    "bun.lock",
    // Python
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    // Go
    "go.mod",
    // Java/Kotlin/Scala
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "build.sbt",
    // Ruby
    "Gemfile",
    // PHP
    "composer.json",
    // Elixir
    "mix.exs",
    // Swift
    "Package.swift",
    // C/C++
    "CMakeLists.txt",
    "Makefile",
    "meson.build",
    // Bazel/Buck
    "WORKSPACE",
    "WORKSPACE.bazel",
    "MODULE.bazel",
    "BUILD.bazel",
];

/// Literal path prefixes that are never real project roots. On macOS the
/// `/private/*` form covers cases where a tool has resolved the `/tmp`,
/// `/etc`, or `/var` symlink.
const BLOCKED_PREFIXES: &[&str] = &[
    "/tmp",
    "/private/tmp",
    "/var",
    "/private/var",
    "/etc",
    "/private/etc",
    "/usr",
    "/bin",
    "/sbin",
    "/dev",
    "/System",
    "/Library",
    "/Volumes",
    "/cores",
    "/Network",
];

/// Stateless gate that wraps the heuristic with environment lookups. Made a
/// struct (not free functions) so tests can supply explicit `$HOME` /
/// `$TMPDIR` values instead of reading the process environment.
#[derive(Debug, Clone)]
pub struct ProjectGate {
    home_dir: Option<Utf8PathBuf>,
    tmp_dir: Option<Utf8PathBuf>,
}

impl ProjectGate {
    /// Build a gate that reads `$HOME` and `$TMPDIR` from the process env.
    pub fn from_env() -> Self {
        Self {
            home_dir: std::env::var("HOME").ok().map(Utf8PathBuf::from),
            tmp_dir: std::env::var("TMPDIR").ok().map(Utf8PathBuf::from),
        }
    }

    /// Build a gate with explicit values (used in tests).
    pub fn with_env(home_dir: Option<Utf8PathBuf>, tmp_dir: Option<Utf8PathBuf>) -> Self {
        Self { home_dir, tmp_dir }
    }

    /// Return `Ok(())` if the path is safe to auto-bind, otherwise the
    /// reason it was rejected.
    pub fn check(&self, path: &Utf8Path) -> Result<(), SkipReason> {
        if let Some(reason) = self.path_block_reason(path) {
            return Err(SkipReason::Blocklisted { matched: reason });
        }
        if !looks_like_project(path) {
            return Err(SkipReason::NoProjectMarkers);
        }
        Ok(())
    }

    fn path_block_reason(&self, path: &Utf8Path) -> Option<&'static str> {
        let path_str = path.as_str();

        if path_str == "/" {
            return Some("/");
        }

        for prefix in BLOCKED_PREFIXES {
            if path_eq_or_inside(path_str, prefix) {
                return Some(*prefix);
            }
        }

        // Literal `$HOME` (subdirectories of $HOME are still indexable; only
        // the bare home directory is blocked).
        if let Some(home) = &self.home_dir {
            if path == home.as_path() {
                return Some("$HOME");
            }
        }

        // Anything under `$TMPDIR`. macOS gives every user a private TMPDIR
        // under `/var/folders/...` which falls outside the blocklist above.
        if let Some(tmp) = &self.tmp_dir {
            if path_eq_or_inside(path_str, tmp.as_str()) {
                return Some("$TMPDIR");
            }
        }

        None
    }
}

/// Return true if at least one well-known project marker exists in `dir`.
fn looks_like_project(dir: &Utf8Path) -> bool {
    PROJECT_MARKERS.iter().any(|m| dir.join(m).exists())
}

/// Match `candidate == prefix` or `candidate` starts with `prefix/`. Avoids
/// the classic `/tmpfoo` false-positive that plain `starts_with` produces.
fn path_eq_or_inside(candidate: &str, prefix: &str) -> bool {
    if candidate == prefix {
        return true;
    }
    // Strip a trailing `/` from the prefix so `/tmp/` and `/tmp` behave the
    // same, then require the next character in `candidate` to be `/`.
    let trimmed = prefix.trim_end_matches('/');
    if let Some(rest) = candidate.strip_prefix(trimmed) {
        return rest.starts_with('/');
    }
    false
}

/// Coarse classification of a candidate repo path, used to add context to the
/// indexing-consent prompt. Cheap, synchronous filesystem checks only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoClass {
    /// A git worktree. `main` is the main repo path when it could be parsed
    /// from the `.git` file's `gitdir:` pointer.
    GitWorktree { main: Option<String> },
    /// Path lives under a temporary directory.
    TempDir,
    /// Path name/segments look ephemeral (e.g. a `worktrees/` directory).
    Ephemeral,
    /// A normal new project.
    Standard,
}

impl RepoClass {
    /// Stable machine-readable tag for the `detected` field of the consent payload.
    pub fn kind(&self) -> &'static str {
        match self {
            RepoClass::GitWorktree { .. } => "git_worktree",
            RepoClass::TempDir => "temp_dir",
            RepoClass::Ephemeral => "ephemeral",
            RepoClass::Standard => "standard",
        }
    }

    /// Human-readable recommendation shown to the agent (relayed to the user).
    pub fn recommendation(&self) -> String {
        match self {
            RepoClass::GitWorktree { main } => {
                let of = main
                    .as_deref()
                    .map(|m| format!(" of {m}"))
                    .unwrap_or_default();
                format!(
                    "Looks like a git worktree{of} (usually ephemeral). Indexing runs a full GPU embedding pass and starts a file watcher. Most worktrees should be skipped."
                )
            }
            RepoClass::TempDir => {
                "Path is under a temporary directory; it is probably not a repo you want to index."
                    .to_string()
            }
            RepoClass::Ephemeral => {
                "Path looks ephemeral (e.g. a worktrees directory). Indexing runs a full GPU embedding pass; skip unless you will work here repeatedly."
                    .to_string()
            }
            RepoClass::Standard => {
                "Indexing runs a full GPU embedding pass and starts a file watcher. Approve if this is a project you will work in repeatedly."
                    .to_string()
            }
        }
    }

    /// Optional structured detail (currently only the worktree's main repo).
    pub fn detail(&self) -> Option<String> {
        match self {
            RepoClass::GitWorktree { main: Some(m) } => Some(format!("git worktree of {m}")),
            _ => None,
        }
    }
}

/// Classify a repo path using the process `$TMPDIR`.
pub fn classify_repo(path: &Utf8Path) -> RepoClass {
    classify_repo_with_env(path, std::env::var("TMPDIR").ok().as_deref())
}

/// Classify a repo path with an explicit `$TMPDIR` value (for tests).
pub fn classify_repo_with_env(path: &Utf8Path, tmpdir: Option<&str>) -> RepoClass {
    // A git worktree has `.git` as a FILE beginning with `gitdir:`.
    let dot_git = path.join(".git");
    if dot_git.as_std_path().is_file() {
        let main = std::fs::read_to_string(dot_git.as_std_path())
            .ok()
            .and_then(|contents| {
                contents
                    .trim()
                    .strip_prefix("gitdir:")
                    .map(|rest| rest.trim().to_string())
            })
            .and_then(|gitdir| {
                gitdir
                    .split_once("/.git/worktrees/")
                    .map(|(main, _)| main.to_string())
            });
        return RepoClass::GitWorktree { main };
    }
    if dot_git.as_std_path().is_dir() {
        return RepoClass::Standard;
    }

    let p = path.as_str();
    let under = |prefix: &str| {
        let prefix = prefix.trim_end_matches('/');
        p == prefix || p.starts_with(&format!("{prefix}/"))
    };
    if [
        "/tmp",
        "/private/tmp",
        "/var/tmp",
        "/private/var/tmp",
        "/var/folders",
        "/private/var/folders",
    ]
    .iter()
    .any(|pre| under(pre))
        || tmpdir.map(under).unwrap_or(false)
    {
        return RepoClass::TempDir;
    }

    if p.contains("/worktrees/")
        || p.contains("/.worktrees/")
        || path
            .file_name()
            .map(|n| n.ends_with("-worktree"))
            .unwrap_or(false)
    {
        return RepoClass::Ephemeral;
    }

    RepoClass::Standard
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn gate(home: &str, tmp: &str) -> ProjectGate {
        ProjectGate::with_env(Some(Utf8PathBuf::from(home)), Some(Utf8PathBuf::from(tmp)))
    }

    #[test]
    fn rejects_literal_root() {
        let g = gate("/Users/me", "/var/folders/xx");
        assert_eq!(
            g.check(Utf8Path::new("/")),
            Err(SkipReason::Blocklisted { matched: "/" })
        );
    }

    #[test]
    fn rejects_tmp_and_private_tmp() {
        let g = gate("/Users/me", "/var/folders/xx");
        assert!(matches!(
            g.check(Utf8Path::new("/tmp")),
            Err(SkipReason::Blocklisted { .. })
        ));
        assert!(matches!(
            g.check(Utf8Path::new("/private/tmp")),
            Err(SkipReason::Blocklisted { .. })
        ));
        assert!(matches!(
            g.check(Utf8Path::new("/private/tmp/scratch")),
            Err(SkipReason::Blocklisted { .. })
        ));
    }

    #[test]
    fn rejects_system_paths() {
        let g = gate("/Users/me", "/var/folders/xx");
        for p in ["/usr", "/etc", "/var", "/System", "/Library", "/Volumes/X"] {
            assert!(
                matches!(
                    g.check(Utf8Path::new(p)),
                    Err(SkipReason::Blocklisted { .. })
                ),
                "expected {p} to be blocked"
            );
        }
    }

    #[test]
    fn rejects_literal_home_but_not_subdirs() {
        let g = gate("/Users/me", "/var/folders/xx");
        // The bare home directory: blocked.
        assert!(matches!(
            g.check(Utf8Path::new("/Users/me")),
            Err(SkipReason::Blocklisted { matched: "$HOME" })
        ));
        // Subdirs of home are checked normally (marker required).
        assert_eq!(
            g.check(Utf8Path::new("/Users/me/scratch")),
            Err(SkipReason::NoProjectMarkers)
        );
    }

    #[test]
    fn rejects_inside_custom_tmpdir() {
        // Use a TMPDIR that does not collide with the macOS `/var`-rooted
        // default, otherwise the `/var` blocklist matches first.
        let g = gate("/Users/me", "/Users/me/scratch-tmp");
        assert!(matches!(
            g.check(Utf8Path::new("/Users/me/scratch-tmp/foo")),
            Err(SkipReason::Blocklisted { matched: "$TMPDIR" })
        ));
    }

    #[test]
    fn does_not_false_positive_prefix() {
        // `/tmpfoo` must not be blocked by the `/tmp` rule.
        let g = gate("/Users/me", "/var/folders/xx");
        assert_eq!(
            g.check(Utf8Path::new("/tmpfoo")),
            Err(SkipReason::NoProjectMarkers)
        );
    }

    /// Build a tempdir inside the project's `target/` directory so the
    /// blocklist (which catches `/var/folders/...`, the macOS default
    /// tempdir) does not match it first.
    fn project_local_tempdir() -> tempfile::TempDir {
        let target = std::env::current_dir().expect("cwd").join("target");
        fs::create_dir_all(&target).expect("create target dir");
        tempfile::Builder::new()
            .prefix("project_check_test_")
            .tempdir_in(&target)
            .expect("tempdir")
    }

    /// Gate whose env values cannot match a tempdir under `target/`.
    fn permissive_gate() -> ProjectGate {
        ProjectGate::with_env(
            Some("/nonexistent-home".into()),
            Some("/nonexistent-tmp".into()),
        )
    }

    #[test]
    fn accepts_directory_with_git() {
        let tmp = project_local_tempdir();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        fs::create_dir(dir.join(".git")).unwrap();
        assert_eq!(permissive_gate().check(&dir), Ok(()));
    }

    #[test]
    fn accepts_directory_with_cargo_toml() {
        let tmp = project_local_tempdir();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        fs::write(dir.join("Cargo.toml"), b"[package]\nname = \"x\"\n").unwrap();
        assert_eq!(permissive_gate().check(&dir), Ok(()));
    }

    #[test]
    fn rejects_directory_without_markers() {
        let tmp = project_local_tempdir();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        fs::write(dir.join("notes.txt"), b"hello").unwrap();
        assert_eq!(
            permissive_gate().check(&dir),
            Err(SkipReason::NoProjectMarkers)
        );
    }

    #[test]
    fn classify_detects_git_worktree_and_main_repo() {
        let tmp = project_local_tempdir();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        // A worktree has `.git` as a FILE pointing at the main repo.
        fs::write(
            dir.join(".git"),
            b"gitdir: /Users/me/main-repo/.git/worktrees/feature\n",
        )
        .unwrap();
        let class = classify_repo_with_env(&dir, Some("/nonexistent-tmp"));
        assert_eq!(
            class,
            RepoClass::GitWorktree {
                main: Some("/Users/me/main-repo".to_string())
            }
        );
        assert_eq!(class.kind(), "git_worktree");
        assert_eq!(
            class.detail().as_deref(),
            Some("git worktree of /Users/me/main-repo")
        );
    }

    #[test]
    fn classify_treats_git_directory_as_standard() {
        let tmp = project_local_tempdir();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        // A normal repo has `.git` as a DIRECTORY.
        fs::create_dir(dir.join(".git")).unwrap();
        assert_eq!(
            classify_repo_with_env(&dir, Some("/nonexistent-tmp")),
            RepoClass::Standard
        );
    }

    #[test]
    fn classify_detects_tmpdir() {
        assert_eq!(
            classify_repo_with_env(Utf8Path::new("/tmp/scratch"), Some("/nonexistent-tmp")),
            RepoClass::TempDir
        );
        assert_eq!(
            classify_repo_with_env(
                Utf8Path::new("/Users/me/scratch-tmp/foo"),
                Some("/Users/me/scratch-tmp")
            ),
            RepoClass::TempDir
        );
    }

    #[test]
    fn classify_detects_ephemeral_worktrees_dir() {
        assert_eq!(
            classify_repo_with_env(
                Utf8Path::new("/Users/me/project/.worktrees/feature"),
                Some("/nonexistent-tmp")
            ),
            RepoClass::Ephemeral
        );
    }

    #[test]
    fn classify_worktree_without_parseable_gitdir_yields_none_main() {
        let tmp = project_local_tempdir();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        // `.git` is a file but its content is not a `gitdir:` pointer.
        fs::write(dir.join(".git"), b"ref: refs/heads/main\n").unwrap();
        assert_eq!(
            classify_repo_with_env(&dir, Some("/nonexistent-tmp")),
            RepoClass::GitWorktree { main: None }
        );
    }

    #[test]
    fn classify_detects_worktree_suffixed_dir_as_ephemeral() {
        assert_eq!(
            classify_repo_with_env(
                Utf8Path::new("/Users/me/projects/feature-worktree"),
                Some("/nonexistent-tmp")
            ),
            RepoClass::Ephemeral
        );
    }
}

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
}

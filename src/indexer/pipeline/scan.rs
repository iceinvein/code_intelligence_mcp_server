use crate::config::Config;
use crate::indexer::parser::{language_id_for_path, LanguageId};
use anyhow::Result;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn scan_files(config: &Config, root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(dir = %dir.display(), error = %err, "Failed to read dir");
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(
                        dir = %dir.display(),
                        error = %err,
                        "Failed to read dir entry"
                    );
                    continue;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            if file_type.is_dir() {
                if should_skip_dir(config, root, &path) {
                    continue;
                }
                stack.push(path);
                continue;
            }

            if file_type.is_file() && should_index_file(config, root, &path) {
                out.push(path);
            }
        }
    }
    Ok(out)
}

/// Path used for glob matching: relative to the repo root, with a leading slash.
///
/// Matching the repo-root-relative path (rather than the absolute path) means a
/// pattern like `**/bench/state/repos/**` excludes that directory only when it is
/// a subdirectory WITHIN the indexed repo. It no longer fires when the repo root
/// itself lives under such a path (e.g. a bench fixture checked out at
/// `bench/state/repos/<repo>`), which previously zeroed the whole index.
fn rel_match_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let s = rel.to_string_lossy().replace('\\', "/");
    format!("/{}", s.trim_start_matches('/'))
}

pub fn should_skip_dir(config: &Config, root: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    match name {
        // VCS
        ".git" | ".worktrees" => return true,
        // Build output (Electron defaults to `out/`, see electron-builder)
        "dist" | "build" | "target" | "out" | ".output" | ".next" | ".nuxt" | ".svelte-kit" => {
            return true;
        }
        // Caches
        "__pycache__" | ".mypy_cache" | ".pytest_cache" | ".turbo" | ".cache" => return true,
        // Test coverage output
        "coverage" => return true,
        // AI tool config/skills (contains scripts that pollute search)
        ".claude" | ".planning" => return true,
        // Package manager
        "node_modules" if !config.index_node_modules => return true,
        _ => {}
    }
    // Check user-configured exclude patterns against the repo-root-relative path.
    let s = rel_match_path(root, path);
    for pat in &config.exclude_patterns {
        if pattern_matches_dir(&s, pat) {
            return true;
        }
    }
    false
}

/// Check if a directory path matches an exclude pattern like `**/dirname/**`.
fn pattern_matches_dir(path: &str, pattern: &str) -> bool {
    let pat = pattern.replace('\\', "/");
    // Extract the directory name from patterns like **/dirname/** or **/dirname
    let stripped = pat
        .trim_start_matches("**/")
        .trim_end_matches("/**")
        .trim_end_matches("/*");
    if !stripped.contains('*') && !stripped.is_empty() {
        let needle = format!("/{stripped}");
        if path.ends_with(&needle) || path.contains(&format!("{needle}/")) {
            return true;
        }
    }
    false
}

pub fn should_index_file(config: &Config, root: &Path, path: &Path) -> bool {
    if is_excluded(config, root, path) {
        return false;
    }
    if !matches!(
        language_id_for_path(path),
        Some(
            LanguageId::Typescript
                | LanguageId::Tsx
                | LanguageId::Rust
                | LanguageId::Python
                | LanguageId::Go
                | LanguageId::Java
                | LanguageId::Javascript
                | LanguageId::C
                | LanguageId::Cpp
                | LanguageId::Ruby
                | LanguageId::Kotlin
                | LanguageId::CSharp
                | LanguageId::Swift
                | LanguageId::Markdown
        )
    ) {
        return false;
    }

    let s = rel_match_path(root, path);
    config
        .index_patterns
        .iter()
        .any(|pat| pattern_matches_file(&s, pat))
}

fn is_excluded(config: &Config, root: &Path, path: &Path) -> bool {
    let s = rel_match_path(root, path);
    for pat in &config.exclude_patterns {
        if pattern_matches_file(&s, pat) {
            return true;
        }
    }
    false
}

/// Match a file path against an exclude pattern.
///
/// Supports common glob-like patterns:
/// - `**/dirname/**`  → matches if `/dirname/` appears in path
/// - `**/*.ext`       → matches if path ends with `.ext`
/// - `**/*.foo.*`     → matches if filename contains `.foo.`
/// - `**/exact.file`  → matches if filename equals `exact.file`
fn pattern_matches_file(path: &str, pattern: &str) -> bool {
    let pat = pattern.replace('\\', "/");

    // Strip leading **/ to get the meaningful part
    let core = pat.trim_start_matches("**/");

    // Pattern like `dirname/**` → check if /dirname/ appears in path
    if let Some(dir) = core.strip_suffix("/**") {
        let needle = format!("/{dir}/");
        return path.contains(&needle);
    }

    // Directory-scoped pattern like `docs/**/*.md`: the directory prefix
    // must appear as a path segment, and the trailing glob must match the
    // file's basename (the `**` spans any depth of subdirectories).
    if let Some((dir, tail)) = core.split_once("/**/") {
        let needle = format!("/{dir}/");
        let Some(start) = path.find(&needle) else {
            return false;
        };
        let rest = &path[start + needle.len()..];
        // Match the tail against the basename when it has no interior slash
        // wildcards; otherwise fall back to ordered substring matching over
        // the remaining relative path.
        let target = if tail.contains("**") {
            rest
        } else {
            rest.rsplit('/').next().unwrap_or(rest)
        };
        let parts: Vec<&str> = tail.split('*').collect();
        let mut pos = 0;
        for part in parts {
            if part.is_empty() {
                continue;
            }
            if let Some(idx) = target[pos..].find(part) {
                pos += idx + part.len();
            } else {
                return false;
            }
        }
        return true;
    }

    // Pattern with wildcards in filename (e.g. `*.test.*`, `*.gen.*`, `*.min.*`)
    if core.contains('*') {
        // Split on * and check all parts appear in order in the filename
        let filename = path.rsplit('/').next().unwrap_or(path);
        let parts: Vec<&str> = core.split('*').collect();
        let mut pos = 0;
        for part in &parts {
            if part.is_empty() {
                continue;
            }
            if let Some(idx) = filename[pos..].find(part) {
                pos += idx + part.len();
            } else {
                return false;
            }
        }
        return true;
    }

    // Exact filename match (e.g. `routeTree.gen.ts`)
    if let Some(filename) = path.rsplit('/').next() {
        if filename == core {
            return true;
        }
    }

    // Substring path match as fallback
    path.contains(core)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StandaloneConfig;
    use crate::path::Utf8PathBuf;

    fn test_config() -> Config {
        StandaloneConfig::default().repo_config(
            Utf8PathBuf::from("/tmp/repo"),
            &Utf8PathBuf::from("/tmp/data"),
        )
    }

    #[test]
    fn directory_scoped_doublestar_pattern_matches_nested_and_direct() {
        let mut config = test_config();
        config.index_patterns = vec!["docs/**/*.md".to_string()];
        let root = Path::new("/tmp/repo");

        // Direct child of docs/ and deeply nested both match.
        assert!(should_index_file(
            &config,
            root,
            Path::new("/tmp/repo/docs/adr-001.md")
        ));
        assert!(should_index_file(
            &config,
            root,
            Path::new("/tmp/repo/docs/architecture/deep/notes.md")
        ));
        // Outside docs/ does not match, even with the same basename.
        assert!(!should_index_file(
            &config,
            root,
            Path::new("/tmp/repo/src/adr-001.md")
        ));
        // Different extension under docs/ does not match.
        assert!(!should_index_file(
            &config,
            root,
            Path::new("/tmp/repo/docs/adr-001.txt")
        ));
    }

    #[test]
    fn should_index_file_respects_index_patterns() {
        let mut config = test_config();
        config.index_patterns = vec!["**/*.rs".to_string()];
        let root = Path::new("/tmp/repo");

        assert!(should_index_file(
            &config,
            root,
            Path::new("/tmp/repo/src/lib.rs")
        ));
        assert!(!should_index_file(
            &config,
            root,
            Path::new("/tmp/repo/src/app.py")
        ));
    }

    #[test]
    fn exclude_patterns_are_relative_to_repo_root() {
        // Regression: a repo checked out UNDER a path matching an exclude pattern
        // (e.g. a bench fixture at bench/state/repos/<repo>) must still index its
        // own files. Previously the absolute path matched **/bench/state/repos/**
        // and zeroed the whole index.
        let mut config = test_config();
        config.index_patterns = vec!["**/*.py".to_string()];
        config.exclude_patterns = vec!["**/bench/state/repos/**".to_string()];

        let fixture_root = Path::new("/home/u/proj/bench/state/repos/django");
        assert!(
            should_index_file(
                &config,
                fixture_root,
                Path::new("/home/u/proj/bench/state/repos/django/django/shortcuts.py"),
            ),
            "a file inside a repo rooted under bench/state/repos must still be indexed"
        );

        // The same pattern still excludes a bench/state/repos subdir WITHIN a repo.
        let code_intel_root = Path::new("/home/u/code-intel");
        assert!(
            !should_index_file(
                &config,
                code_intel_root,
                Path::new("/home/u/code-intel/bench/state/repos/django/x.py"),
            ),
            "a bench/state/repos subdir within a repo must still be excluded"
        );
        assert!(
            should_skip_dir(
                &config,
                code_intel_root,
                Path::new("/home/u/code-intel/bench/state/repos"),
            ),
            "the bench/state/repos subdir should be skipped during the walk"
        );
        // ...but the fixture repo's own same-named subdir is not skipped.
        assert!(!should_skip_dir(
            &config,
            fixture_root,
            Path::new("/home/u/proj/bench/state/repos/django/django"),
        ));
    }
}

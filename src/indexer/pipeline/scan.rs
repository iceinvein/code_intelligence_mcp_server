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
                if should_skip_dir(config, &path) {
                    continue;
                }
                stack.push(path);
                continue;
            }

            if file_type.is_file() && should_index_file(config, &path) {
                out.push(path);
            }
        }
    }
    Ok(out)
}

pub fn should_skip_dir(config: &Config, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    match name {
        // VCS
        ".git" | ".worktrees" => return true,
        // Build output
        "dist" | "build" | "target" | ".output" | ".next" | ".nuxt" | ".svelte-kit" => {
            return true
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
    // Check user-configured exclude patterns against the full path
    let s = path.to_string_lossy().replace('\\', "/");
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
    let stripped = pat.trim_start_matches("**/").trim_end_matches("/**").trim_end_matches("/*");
    if !stripped.contains('*') && !stripped.is_empty() {
        let needle = format!("/{stripped}");
        if path.ends_with(&needle) || path.contains(&format!("{needle}/")) {
            return true;
        }
    }
    false
}

pub fn should_index_file(config: &Config, path: &Path) -> bool {
    if is_excluded(config, path) {
        return false;
    }
    matches!(
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
        )
    )
}

fn is_excluded(config: &Config, path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
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

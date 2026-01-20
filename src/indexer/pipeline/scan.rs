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
    if name == ".git" || name == "dist" || name == "build" || name == "target" {
        return true;
    }
    if !config.index_node_modules && name == "node_modules" {
        return true;
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
    if !config.index_node_modules && s.contains("/node_modules/") {
        return true;
    }
    if s.contains("/.git/") || s.contains("/dist/") || s.contains("/build/") {
        return true;
    }
    if s.contains(".test.") {
        return true;
    }
    for pat in &config.exclude_patterns {
        if simple_exclude_match(&s, pat) {
            return true;
        }
    }
    false
}

fn simple_exclude_match(path: &str, pattern: &str) -> bool {
    let pat = pattern.replace('\\', "/");
    if pat.contains("node_modules") && path.contains("/node_modules/") {
        return true;
    }
    if pat.contains(".git") && path.contains("/.git/") {
        return true;
    }
    if pat.contains("/dist/") && path.contains("/dist/") {
        return true;
    }
    if pat.contains("/build/") && path.contains("/build/") {
        return true;
    }
    if pat.contains("*.test.") && path.contains(".test.") {
        return true;
    }
    false
}

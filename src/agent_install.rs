//! Agent installer helpers.
//!
//! This installs only managed guidance blocks and MCP snippets. It deliberately
//! avoids taking ownership of whole agent configuration files whose schemas vary
//! across tools.

use crate::cli::{AgentInstallOpts, AgentUninstallOpts};
use crate::install::{patch_claude_json, DEFAULT_PORT};
use anyhow::{Context, Result};
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;

const START_MARKER: &str = "<!-- code-intelligence-agent:start -->";
const END_MARKER: &str = "<!-- code-intelligence-agent:end -->";

pub fn handle_install_agent(opts: AgentInstallOpts) -> Result<()> {
    let port = opts.port.unwrap_or(DEFAULT_PORT);
    let repo = resolve_repo(opts.repo.as_deref())?;
    let targets = expand_targets(&repo, &opts.targets);
    let mut printed_config = false;

    if opts.print_config {
        print_agent_config(&targets, port, Some(&repo));
        return Ok(());
    }

    if !opts.no_instructions {
        if opts.scope == "project" {
            let block = render_instruction_block(port);
            for path in planned_instruction_files(&repo, &targets) {
                if opts.dry_run {
                    println!("Would update {}", path.display());
                } else {
                    write_managed_file(&path, &block)?;
                    println!("Updated {}", path.display());
                }
            }
        } else {
            println!(
                "User-scope instruction writes are not automated yet; printing snippets instead."
            );
            print_agent_config(&targets, port, Some(&repo));
            printed_config = true;
        }
    }

    if !opts.no_mcp {
        if opts.scope == "user" && targets.iter().any(|target| target == "claude") {
            if opts.dry_run {
                println!("Would patch ~/.claude.json with code-intelligence MCP endpoint");
            } else if let Some(path) = patch_claude_json(port)? {
                println!("Updated {}", path.display());
            }
        } else if !printed_config {
            print_agent_config(&targets, port, Some(&repo));
        }
    }

    Ok(())
}

pub fn handle_uninstall_agent(opts: AgentUninstallOpts) -> Result<()> {
    let repo = resolve_repo(opts.repo.as_deref())?;
    let targets = expand_targets(&repo, &opts.targets);

    if !opts.no_instructions {
        if opts.scope == "project" {
            for path in planned_instruction_files(&repo, &targets) {
                if opts.dry_run {
                    println!("Would remove managed block from {}", path.display());
                } else {
                    remove_managed_file_block(&path)?;
                    println!("Removed managed block from {}", path.display());
                }
            }
        } else {
            println!("User-scope instruction removal is not automated yet.");
        }
    }

    if !opts.no_mcp {
        println!("MCP config removal is not automated yet; remove code-intelligence from agent config if needed.");
    }

    Ok(())
}

fn resolve_repo(repo: Option<&str>) -> Result<PathBuf> {
    match repo {
        Some(path) => Ok(PathBuf::from(path)),
        None => env::current_dir().context("failed to resolve current directory"),
    }
}

fn render_instruction_block(port: u16) -> String {
    managed_block(&format!(
        r#"## Code Intelligence

Use the local code-intelligence tools before broad text search when answering repository questions.

Main session workflow:
- Start with `code-intelligence-mcp-server repo-map --repo <repo>` to get a compact project map.
- Use `code-intelligence-mcp-server search --repo <repo> "query"` for semantic code search.
- Use `code-intelligence-mcp-server investigate --repo <repo> --mode impact "what breaks if this changes?"` for blast-radius work.
- Use `code-intelligence-mcp-server hydrate --repo <repo> --ids <id1,id2>` before quoting or editing exact source bodies.

Exploration subagent workflow:
- Use `code-intelligence-mcp-server ask --repo <repo> "question"` when a subagent needs a broader evidence pass.
- Use `code-intelligence-mcp-server investigate --repo <repo> "question"` for open-ended trace, dependency, or module exploration.

- If an MCP client is available, connect it to `http://127.0.0.1:{port}/mcp` and add `?repo=/absolute/path` for clients that do not negotiate roots.
"#
    ))
}

fn render_mcp_config(port: u16, repo: Option<&Path>) -> String {
    let mut url =
        Url::parse(&format!("http://127.0.0.1:{port}/mcp")).expect("static MCP URL is valid");
    if let Some(repo) = repo {
        url.query_pairs_mut()
            .append_pair("repo", &repo.display().to_string());
    }

    serde_json::to_string_pretty(&json!({
        "mcpServers": {
            "code-intelligence": {
                "type": "streamable-http",
                "url": url.as_str(),
                "alwaysAllow": []
            }
        }
    }))
    .expect("static MCP config should serialize")
}

fn print_agent_config(targets: &[String], port: u16, repo: Option<&Path>) {
    println!("Targets: {}", targets.join(", "));
    println!();
    println!("MCP config:");
    println!("{}", render_mcp_config(port, repo));
    println!();
    println!("Instruction block:");
    println!("{}", render_instruction_block(port));
}

fn planned_instruction_files(root: &Path, targets: &[String]) -> Vec<PathBuf> {
    let targets = expand_targets(root, targets);
    let mut files = Vec::new();
    for target in targets {
        let path = match target.as_str() {
            "claude" => root.join("CLAUDE.md"),
            "cursor" => root.join(".cursor/rules/code-intelligence.mdc"),
            "codex" | "generic" | "opencode" => root.join("AGENTS.md"),
            _ => continue,
        };
        if !files.contains(&path) {
            files.push(path);
        }
    }
    files
}

fn expand_targets(root: &Path, targets: &[String]) -> Vec<String> {
    let normalized: Vec<String> = targets
        .iter()
        .flat_map(|target| target.split(','))
        .map(|target| target.trim().to_ascii_lowercase())
        .filter(|target| !target.is_empty())
        .collect();

    if normalized.iter().any(|target| target == "all") {
        return vec![
            "codex".to_string(),
            "claude".to_string(),
            "cursor".to_string(),
            "opencode".to_string(),
        ];
    }

    if normalized.is_empty() || normalized.iter().any(|target| target == "auto") {
        let mut detected = vec!["codex".to_string()];
        if root.join("CLAUDE.md").exists() {
            detected.push("claude".to_string());
        }
        if root.join(".cursor").exists() {
            detected.push("cursor".to_string());
        }
        return detected;
    }

    normalized
}

fn upsert_managed_block(existing: &str, body: &str) -> String {
    let block = if body.contains(START_MARKER) {
        body.trim().to_string()
    } else {
        managed_block(body)
    };

    if let Some((start, end)) = managed_block_range(existing) {
        let mut output = String::new();
        output.push_str(existing[..start].trim_end());
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(&block);
        let tail = existing[end..].trim_start();
        if !tail.is_empty() {
            output.push_str("\n\n");
            output.push_str(tail);
        } else {
            output.push('\n');
        }
        output
    } else {
        let mut output = existing.trim_end().to_string();
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(&block);
        output.push('\n');
        output
    }
}

fn remove_managed_block(existing: &str) -> String {
    if let Some((start, end)) = managed_block_range(existing) {
        let mut output = String::new();
        output.push_str(existing[..start].trim_end());
        let tail = existing[end..].trim_start();
        if !output.is_empty() && !tail.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(tail);
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output
    } else {
        existing.to_string()
    }
}

fn managed_block(body: &str) -> String {
    format!("{START_MARKER}\n{}\n{END_MARKER}", body.trim())
}

fn managed_block_range(existing: &str) -> Option<(usize, usize)> {
    let start = existing.find(START_MARKER)?;
    let end_marker_start = existing[start..].find(END_MARKER)? + start;
    let end = end_marker_start + END_MARKER.len();
    Some((start, end))
}

fn write_managed_file(path: &Path, block: &str) -> Result<()> {
    let existing = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let updated = upsert_managed_block(&existing, block);
    atomic_write(path, updated.as_bytes())
}

fn remove_managed_file_block(path: &Path) -> Result<()> {
    let existing = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let updated = remove_managed_block(&existing);
    atomic_write(path, updated.as_bytes())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!(
        "{}tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| format!("{ext}."))
            .unwrap_or_default()
    ));
    fs::write(&tmp, bytes).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn render_instruction_block_mentions_cli_first_workflow() {
        let block = render_instruction_block(20000);

        assert!(block.contains("<!-- code-intelligence-agent:start -->"));
        assert!(block.contains("code-intelligence-mcp-server repo-map --repo"));
        assert!(block.contains("code-intelligence-mcp-server ask --repo"));
        assert!(block.contains("code-intelligence-mcp-server search --repo"));
        assert!(block.contains("http://127.0.0.1:20000/mcp"));
        assert!(block.contains("<!-- code-intelligence-agent:end -->"));
    }

    #[test]
    fn render_instruction_block_describes_main_and_exploration_workflows() {
        let block = render_instruction_block(17800);

        assert!(block.contains("Main session workflow"));
        assert!(block.contains("Exploration subagent workflow"));
        assert!(block.contains("investigate --repo <repo> --mode impact"));
        assert!(block.contains("ask --repo <repo>"));
    }

    #[test]
    fn render_mcp_config_includes_encoded_repo_query() {
        let config = render_mcp_config(20000, Some(Path::new("/tmp/my repo")));

        assert!(config.contains("http://127.0.0.1:20000/mcp?repo=%2Ftmp%2Fmy+repo"));
    }

    #[test]
    fn upsert_managed_block_appends_then_replaces() {
        let original = "# Project\n\nHuman guidance.\n";
        let first = upsert_managed_block(original, "first block");
        let second = upsert_managed_block(&first, "second block");

        assert!(first.contains("Human guidance."));
        assert!(second.contains("Human guidance."));
        assert!(!second.contains("first block"));
        assert!(second.contains("second block"));
        assert_eq!(second.matches("code-intelligence-agent:start").count(), 1);
    }

    #[test]
    fn remove_managed_block_preserves_human_content() {
        let content = upsert_managed_block("# Project\n\nKeep this.\n", "generated");
        let removed = remove_managed_block(&content);

        assert!(removed.contains("Keep this."));
        assert!(!removed.contains("generated"));
        assert!(!removed.contains("code-intelligence-agent:start"));
    }

    #[test]
    fn planned_instruction_files_are_agent_specific() {
        let root = Path::new("/tmp/repo");
        let files =
            planned_instruction_files(root, &["codex".into(), "claude".into(), "cursor".into()]);

        assert_eq!(files.len(), 3);
        assert!(files.contains(&root.join("AGENTS.md")));
        assert!(files.contains(&root.join("CLAUDE.md")));
        assert!(files.contains(&root.join(".cursor/rules/code-intelligence.mdc")));
    }
}

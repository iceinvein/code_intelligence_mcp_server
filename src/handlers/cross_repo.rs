//! Cross-repo MCP tool handlers

use crate::handlers::AppState;
use crate::path::{Utf8Path, Utf8PathBuf};
use crate::tools::{ExploreCrossRepoDependenciesTool, SearchAcrossReposTool};
use anyhow::Result;
use serde_json::json;

/// A single search hit tagged with its source repository.
#[derive(serde::Serialize)]
struct CrossRepoHit {
    repo: String,
    name: String,
    kind: String,
    file_path: String,
    score: f32,
    snippet: String,
}

pub async fn handle_search_across_repos(
    session_manager: &crate::session::SessionManager,
    tool: SearchAcrossReposTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(10).clamp(1, 100) as usize;
    let include_display = tool.include_display.unwrap_or(false);

    // Gather all repos currently registered
    let entries = session_manager.registry.list_all()?;
    let total_repos = entries.len();

    if total_repos == 0 {
        let display = include_display.then(|| {
            format!(
                "## Cross-Repo Search: \"{}\"\n\nNo repositories are indexed yet.",
                tool.query
            )
        });
        let mut response = json!({
            "query": tool.query,
            "total_repos_searched": 0,
            "results": [],
        });
        if let Some(display) = display {
            response["display"] = json!(display);
        }
        return Ok(response);
    }

    // Initialise (or retrieve cached) AppState for every registered repo, then
    // search them all in parallel.  Each future returns (repo_path, Result) so
    // we always know which repo failed.
    let query = tool.query.clone();
    let mut search_futures = Vec::with_capacity(total_repos);

    for entry in &entries {
        let repo_path = Utf8PathBuf::from(entry.path.clone());
        let entry_path = entry.path.clone();
        let query_clone = query.clone();
        let sm = session_manager;
        search_futures.push(async move {
            let result = async {
                let state = sm.get_or_create_repo(&repo_path).await?;
                let result = state.retriever.search(&query_clone, limit, false).await?;
                Ok::<_, anyhow::Error>(result.response)
            }
            .await;
            (entry_path, result)
        });
    }

    let outcomes = futures::future::join_all(search_futures).await;

    let mut all_hits: Vec<CrossRepoHit> = Vec::new();
    let mut repos_searched: usize = 0;

    for (repo_path, outcome) in outcomes {
        match outcome {
            Ok(response) => {
                repos_searched += 1;
                for hit in response.hits {
                    let snippet =
                        extract_context_snippet(&response.context, &hit.name, &hit.file_path);
                    all_hits.push(CrossRepoHit {
                        repo: repo_path.clone(),
                        name: hit.name,
                        kind: hit.kind,
                        file_path: hit.file_path,
                        score: hit.score,
                        snippet,
                    });
                }
            }
            Err(err) => {
                tracing::warn!(
                    repo = %repo_path,
                    error = %err,
                    "search_across_repos: skipping repo due to search error"
                );
            }
        }
    }

    // Sort by score descending (total_cmp for deterministic NaN handling),
    // then take the top `limit` overall.
    all_hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    all_hits.truncate(limit);

    let display = include_display.then(|| format_cross_repo_results(&tool.query, repos_searched, &all_hits));

    let mut response = json!({
        "query": tool.query,
        "total_repos_searched": repos_searched,
        "results": all_hits,
    });
    if let Some(display) = display {
        response["display"] = json!(display);
    }
    Ok(response)
}

/// Extract a short snippet for a named symbol from the assembled context string.
///
/// The context string produced by `ContextAssembler` uses fenced code blocks;
/// we look for a line that mentions both the symbol name and file path, then
/// grab the surrounding few lines.  Falls back to an empty string when nothing
/// useful is found.
fn extract_context_snippet(context: &str, name: &str, file_path: &str) -> String {
    let lines: Vec<&str> = context.lines().collect();
    let file_stem = Utf8Path::new(file_path).file_name().unwrap_or(file_path);

    // Walk lines looking for the file marker, then capture up to 3 lines after
    // that mention the symbol name.
    let mut in_file_block = false;
    for (i, line) in lines.iter().enumerate() {
        // Detect file section headers (e.g., "--- src/foo.rs ---" or "// file: src/foo.rs")
        // Reset in_file_block when we enter a different file's section.
        if line.contains(file_stem) {
            in_file_block = true;
        } else if (line.starts_with("---")
            || line.starts_with("// file:")
            || line.starts_with("```"))
            && !line.trim().is_empty()
            && line.contains('/')
            && !line.contains(file_stem)
        {
            in_file_block = false;
        }
        if in_file_block && line.contains(name) {
            let end = (i + 3).min(lines.len());
            let snip: String = lines[i..end]
                .iter()
                .map(|l| l.trim())
                .collect::<Vec<_>>()
                .join(" ");
            if snip.len() > 200 {
                // Truncate at a char boundary to avoid panicking on multi-byte UTF-8
                let truncate_at = snip
                    .char_indices()
                    .take_while(|(i, _)| *i <= 200)
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                return format!("{}…", &snip[..truncate_at]);
            }
            return snip;
        }
    }
    String::new()
}

/// Format cross-repo search results as a Markdown string.
fn format_cross_repo_results(query: &str, repos_searched: usize, hits: &[CrossRepoHit]) -> String {
    let mut out = format!(
        "## Cross-Repo Search: \"{query}\"\n\nSearched {repos_searched} repos, found {} results\n\n",
        hits.len()
    );

    for (i, hit) in hits.iter().enumerate() {
        let repo_label = Utf8Path::new(&hit.repo).file_name().unwrap_or(&hit.repo);

        out.push_str(&format!(
            "{}. **{}** `{}` — `{}` [repo: {repo_label}] *(score: {:.2})*\n",
            i + 1,
            hit.name,
            hit.kind,
            hit.file_path,
            hit.score
        ));
        let snippet = hit.snippet.trim();
        if !snippet.is_empty() {
            out.push_str(&format!("   > {snippet}\n"));
        }
    }

    out
}

/// Handle explore_cross_repo_dependencies tool — query cross-repo edges for a symbol.
///
/// Standalone mode only. In embedded mode, `dispatch_tool_call` returns an error
/// before this function is reached.
pub fn handle_explore_cross_repo_dependencies(
    state: &AppState,
    resolver: &dyn crate::graph::CrossRepoResolver,
    tool: ExploreCrossRepoDependenciesTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let limit = tool.limit.unwrap_or(20).clamp(1, 200) as usize;
    let direction = tool.direction.as_deref().unwrap_or("both");
    let include_display = tool.include_display.unwrap_or(false);

    // Validate direction
    if !matches!(direction, "downstream" | "upstream" | "both") {
        return Ok(json!({
            "error": "invalid_direction",
            "message": format!("Invalid direction '{}'. Use 'downstream', 'upstream', or 'both'.", direction),
        }));
    }

    // Find the root symbol in this repo
    let candidates = state.sqlite.search_symbols_by_exact_name(
        &tool.symbol_name,
        tool.file_path.as_deref(),
        1,
    )?;

    let root = match candidates.into_iter().next() {
        Some(s) => s,
        None => {
            return Ok(json!({
                "error": "not_found",
                "message": format!("Symbol '{}' not found in this repo", tool.symbol_name),
            }));
        }
    };

    let mut downstream_edges: Vec<serde_json::Value> = Vec::new();
    let upstream_edges: Vec<serde_json::Value> = Vec::new();

    // Downstream: edges FROM this symbol TO other repos
    if direction == "downstream" || direction == "both" {
        let edges = resolver.list_cross_repo_edges_from(&state.sqlite, &root.id, limit)?;
        for edge in edges {
            let repo_name = resolver
                .repo_name_for_hash(&edge.to_repo_hash)?
                .unwrap_or_else(|| edge.to_repo_hash.clone());

            // Attempt lazy resolution if not yet resolved
            let resolved_info = if edge.to_symbol_id.is_some() {
                // Already resolved — try to get symbol details
                if let Ok(Some((_store, sym))) = resolver.resolve_cross_repo_symbol(
                    &edge.to_repo_hash,
                    &edge.to_symbol_name,
                    edge.to_symbol_file.as_deref(),
                ) {
                    Some(json!({
                        "id": sym.id,
                        "name": sym.name,
                        "kind": sym.kind,
                        "file_path": sym.file_path,
                        "line_range": [sym.start_line, sym.end_line],
                    }))
                } else {
                    None
                }
            } else {
                None
            };

            downstream_edges.push(json!({
                "from_symbol_id": edge.from_symbol_id,
                "to_repo_hash": edge.to_repo_hash,
                "to_repo_name": repo_name,
                "to_symbol_name": edge.to_symbol_name,
                "to_symbol_file": edge.to_symbol_file,
                "edge_type": edge.edge_type,
                "at_file": edge.at_file,
                "at_line": edge.at_line,
                "confidence": edge.confidence,
                "resolution": edge.resolution,
                "resolved_symbol": resolved_info,
            }));
        }
    }

    // Upstream: would require querying OTHER repos' cross_repo_edges tables
    // for edges pointing to this repo. For now, we only support downstream.
    if direction == "upstream" || direction == "both" {
        // Upstream cross-repo discovery is not yet implemented.
        // It would require iterating all loaded repos and checking their
        // cross_repo_edges tables for to_repo_hash matching this repo.
        // This is intentionally deferred — the schema and queries support it,
        // but the cross-repo detection pipeline must be wired first.
    }

    let mut response = json!({
        "symbol_name": root.name,
        "symbol_id": root.id,
        "direction": direction,
        "downstream": downstream_edges,
        "upstream": upstream_edges,
    });

    if include_display {
        let downstream_edges = response
            .get("downstream")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let upstream_edges = response
            .get("upstream")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut display = format!(
            "## Cross-Repo Dependencies: `{}`\n\nDirection: {direction}\n\n",
            root.name
        );

        if downstream_edges.is_empty() && upstream_edges.is_empty() {
            display.push_str("No cross-repo dependencies found for this symbol.\n");
            display.push_str("Cross-repo edges are populated during indexing when references to other indexed repos are detected.\n");
        } else {
            if !downstream_edges.is_empty() {
                display.push_str(&format!(
                    "### Downstream ({} edges)\n\n",
                    downstream_edges.len()
                ));
                for (i, edge) in downstream_edges.iter().enumerate() {
                    display.push_str(&format!(
                        "{}. `{}` -> `{}` in **{}** ({})\n",
                        i + 1,
                        edge["from_symbol_id"].as_str().unwrap_or("?"),
                        edge["to_symbol_name"].as_str().unwrap_or("?"),
                        edge["to_repo_name"].as_str().unwrap_or("?"),
                        edge["edge_type"].as_str().unwrap_or("?"),
                    ));
                }
            }
            if !upstream_edges.is_empty() {
                display.push_str(&format!(
                    "\n### Upstream ({} edges)\n\n",
                    upstream_edges.len()
                ));
                for (i, edge) in upstream_edges.iter().enumerate() {
                    display.push_str(&format!("{}. {}\n", i + 1, edge));
                }
            }
        }
        response["display"] = json!(display);
    }

    Ok(response)
}

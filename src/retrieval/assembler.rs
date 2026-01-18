use crate::config::Config;
use crate::storage::sqlite::SqliteStore;
use crate::storage::sqlite::SymbolRow;
use anyhow::{anyhow, Context, Result};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

pub struct ContextAssembler {
    config: Arc<Config>,
}

impl ContextAssembler {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }

    pub fn assemble_context(&self, store: &SqliteStore, roots: &[SymbolRow]) -> Result<String> {
        // 1. Expand context using graph with scoring
        // We fetch more candidates than we strictly need, then rerank.
        let expanded = self.expand_with_scoring(store, roots, 50)?;

        // 2. Format output
        self.format_context(roots, &expanded)
    }

    fn expand_with_scoring(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let mut candidates: HashMap<String, (SymbolRow, f32)> = HashMap::new();
        let mut visited: HashSet<String> = roots.iter().map(|r| r.id.clone()).collect();

        // Initialize frontier with roots.
        // We don't add roots to candidates because they are already in the context.
        // We just use them to explore.
        let mut frontier: Vec<String> = roots.iter().map(|r| r.id.clone()).collect();

        let max_depth = 2; // Depth 1 (direct neighbors) and maybe Depth 2
        let exploration_limit = 100; // Don't fetch too many symbols total

        for depth in 0..max_depth {
            if candidates.len() >= exploration_limit {
                break;
            }
            if frontier.is_empty() {
                break;
            }

            let mut next_frontier = Vec::new();

            for from_id in frontier {
                if candidates.len() >= exploration_limit {
                    break;
                }

                let edges = store.list_edges_from(&from_id, 20)?; // Limit fan-out per node
                for edge in edges {
                    if visited.contains(&edge.to_symbol_id) {
                        continue;
                    }

                    if let Some(row) = store.get_symbol_by_id(&edge.to_symbol_id)? {
                        // Calculate score
                        // Base score decays with depth
                        let depth_penalty = 1.0 / ((depth + 1) as f32);

                        // Edge type multiplier
                        let type_multiplier = match edge.edge_type.as_str() {
                            "type" => 1.5,
                            "call" => 1.0,
                            "reference" => 0.8,
                            _ => 1.0,
                        };

                        let score = depth_penalty * type_multiplier;

                        // If we already saw it (via another path in same depth?), keep max score?
                        // But we check `visited` so we only see it once.
                        // Wait, BFS level-by-level ensures shortest path.

                        candidates.insert(row.id.clone(), (row.clone(), score));
                        visited.insert(row.id.clone());
                        next_frontier.push(row.id);
                    }
                }
            }
            frontier = next_frontier;
        }

        // Convert to vec and sort by score
        let mut scored_rows: Vec<(SymbolRow, f32)> = candidates.into_values().collect();
        scored_rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Take top `limit`
        let result = scored_rows
            .into_iter()
            .take(limit)
            .map(|(row, _)| row)
            .collect();
        Ok(result)
    }

    pub fn format_context(&self, roots: &[SymbolRow], expanded: &[SymbolRow]) -> Result<String> {
        let mut out = String::new();
        let mut used = 0usize;
        let mut seen = HashSet::<&str>::new();
        let root_ids: HashSet<&String> = roots.iter().map(|r| &r.id).collect();

        // Prioritize roots, then expanded
        for sym in roots.iter().chain(expanded.iter()) {
            if !seen.insert(&sym.id) {
                continue;
            }

            let is_root = root_ids.contains(&sym.id);
            let mut text = self.read_or_get_text(sym)?;

            if !is_root {
                text = self.simplify_code(&text, &sym.kind);
            }

            let header = format!(
                "=== {}:{}-{} ({} {}) ===\n",
                sym.file_path, sym.start_line, sym.end_line, sym.kind, sym.name
            );

            // ... check limits ...
            let header_len = header.len();
            if used + header_len >= self.config.max_context_bytes {
                break;
            }

            let mut block = header;
            block.push_str(&text);
            if !block.ends_with('\n') {
                block.push('\n');
            }
            block.push('\n');

            if used + block.len() > self.config.max_context_bytes {
                // ... truncate logic ...
                let remaining = self.config.max_context_bytes.saturating_sub(used);
                let bytes = block.as_bytes();
                let mut cut = remaining.min(bytes.len());
                while cut > 0 && !block.is_char_boundary(cut) {
                    cut -= 1;
                }
                out.push_str(&block[..cut]);
                break;
            }

            out.push_str(&block);
            used += block.len();
        }

        Ok(out)
    }

    fn simplify_code(&self, text: &str, kind: &str) -> String {
        // Basic skeleton generation
        // If it's a function and > 5 lines, strip body
        if (kind == "function" || kind == "method") && text.lines().count() > 5 {
            if let Some(start) = text.find('{') {
                if let Some(end) = text.rfind('}') {
                    if start < end {
                        let signature = &text[..start + 1];
                        return format!("{} ... }}", signature);
                    }
                }
            }
        }
        text.to_string()
    }

    fn read_or_get_text(&self, sym: &SymbolRow) -> Result<String> {
        match read_symbol_snippet(&self.config.base_dir, sym) {
            Ok(s) => Ok(s),
            Err(_) => Ok(sym.text.clone()),
        }
    }
}

fn read_symbol_snippet(base_dir: &Path, sym: &SymbolRow) -> Result<String> {
    let abs = base_dir.join(&sym.file_path);
    let bytes = fs::read(&abs).with_context(|| format!("Failed to read {}", abs.display()))?;

    let start = sym.start_byte as usize;
    let end = sym.end_byte as usize;
    if start >= bytes.len() || end > bytes.len() || start >= end {
        return Err(anyhow!("Invalid byte span for {}", sym.id));
    }

    let slice = &bytes[start..end];
    let s = std::str::from_utf8(slice).unwrap_or("").to_string();
    Ok(s)
}

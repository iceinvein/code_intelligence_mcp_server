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

    pub fn assemble_context(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        extra: &[SymbolRow],
    ) -> Result<String> {
        // 1. Expand context using graph with scoring
        // We fetch more candidates than we strictly need, then rerank.
        // We use both roots and extra as starting points, but we prioritize roots.
        let mut seeds = Vec::new();
        seeds.extend_from_slice(roots);
        seeds.extend_from_slice(extra);

        let expanded = self.expand_with_scoring(store, &seeds, 50)?;

        // 2. Format output
        // Roots are full text. Extra and Expanded are simplified.
        self.format_context(roots, extra, &expanded)
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

    pub fn format_context(
        &self,
        roots: &[SymbolRow],
        explicit_extra: &[SymbolRow],
        expanded: &[SymbolRow],
    ) -> Result<String> {
        let mut out = String::new();
        let mut used = 0usize;
        let mut seen = HashSet::<&str>::new();
        let root_ids: HashSet<&String> = roots.iter().map(|r| &r.id).collect();

        // Prioritize roots, then explicit_extra, then expanded
        for sym in roots
            .iter()
            .chain(explicit_extra.iter())
            .chain(expanded.iter())
        {
            if !seen.insert(&sym.id) {
                continue;
            }

            let is_root = root_ids.contains(&sym.id);
            let mut text = self.read_or_get_text(sym)?;

            text = self.simplify_code(&text, &sym.kind, is_root);

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

    fn simplify_code(&self, text: &str, kind: &str, is_root: bool) -> String {
        let lines: Vec<&str> = text.lines().collect();
        // Spec: "If the body is >100 lines, provide the signature, the first 10 lines, ... and the last 5 lines."
        // We apply this to both roots and extra symbols to keep context manageable while "hydrating" structure.

        // Give roots more room. Files get generous room if they are roots.
        let limit = if is_root {
            if kind == "file" {
                1000
            } else {
                500
            }
        } else {
            100
        };

        if lines.len() <= limit {
            return text.to_string();
        }

        let head_count = if kind == "file" { 50 } else { 15 }; // Signature + start
        let tail_count = 5;

        if lines.len() <= head_count + tail_count {
            return text.to_string();
        }

        let head = &lines[..head_count];
        let tail = &lines[lines.len().saturating_sub(tail_count)..];

        let mut out = head.join("\n");
        out.push_str(&format!(
            "\n    // ... ({} lines hidden) ...\n",
            lines.len().saturating_sub(head_count + tail_count)
        ));
        out.push_str(&tail.join("\n"));
        out
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::SymbolRow;

    fn make_config(max_bytes: usize) -> Arc<Config> {
        Arc::new(Config {
            db_path: std::path::PathBuf::from("db"),
            vector_db_path: std::path::PathBuf::from("vec"),
            tantivy_index_path: std::path::PathBuf::from("tantivy"),
            base_dir: std::path::PathBuf::from("."),
            embeddings_backend: crate::config::EmbeddingsBackend::Hash,
            embeddings_model_dir: None,
            embeddings_model_url: None,
            embeddings_model_sha256: None,
            embeddings_auto_download: false,
            embeddings_model_repo: None,
            embeddings_model_revision: None,
            embeddings_model_hf_token: None,
            embeddings_device: crate::config::EmbeddingsDevice::Cpu,
            embedding_batch_size: 32,
            hash_embedding_dim: 8,
            vector_search_limit: 10,
            hybrid_alpha: 0.7,
            rank_vector_weight: 0.7,
            rank_keyword_weight: 0.3,
            rank_exported_boost: 0.0,
            rank_index_file_boost: 0.0,
            rank_test_penalty: 0.0,
            rank_popularity_weight: 0.0,
            rank_popularity_cap: 0,
            index_patterns: vec![],
            exclude_patterns: vec![],
            watch_mode: false,
            watch_debounce_ms: 100,
            max_context_bytes: max_bytes,
            index_node_modules: false,
            repo_roots: vec![],
        })
    }

    #[test]
    fn format_context_hydrates_small_and_truncates_huge() {
        let config = make_config(10000);
        let assembler = ContextAssembler::new(config);

        // 1. Small function (should be hydrated/full)
        let small_body = "{\n  line1;\n  line2;\n}";
        let small = SymbolRow {
            id: "small".to_string(),
            file_path: "small.ts".to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: "smallFunc".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            end_line: 4,
            text: format!("function smallFunc() {}", small_body),
        };

        // 2. Huge function (should be truncated)
        let mut lines = Vec::new();
        for i in 0..200 {
            lines.push(format!("  line{};", i));
        }
        let huge_body = lines.join("\n");
        let huge = SymbolRow {
            id: "huge".to_string(),
            file_path: "huge.ts".to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: "hugeFunc".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            end_line: 202,
            text: format!("function hugeFunc() {{\n{}\n}}", huge_body),
        };

        let output = assembler
            .format_context(&[], &[small.clone(), huge.clone()], &[])
            .unwrap();

        // Check Small
        assert!(output.contains("line1"), "Small symbol should contain body");
        assert!(
            output.contains("line2;"),
            "Small symbol should contain end of body"
        );

        // Check Huge (in a separate call to avoid confusion or if concatenated)
        // Actually output contains both.
        // Let's check specific truncation markers.
        assert!(
            output.contains("lines hidden"),
            "Huge symbol should be truncated"
        );
        assert!(output.contains("line0;"), "Huge symbol should show head");
        assert!(output.contains("line199;"), "Huge symbol should show tail");
        assert!(
            !output.contains("line100;"),
            "Huge symbol should hide middle"
        );
    }
}

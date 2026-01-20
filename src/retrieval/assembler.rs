use crate::config::Config;
use crate::storage::sqlite::SqliteStore;
use crate::storage::sqlite::SymbolRow;
use crate::storage::sqlite::UsageExampleRow;
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
pub struct ContextItem {
    pub id: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub kind: String,
    pub name: String,
    pub role: String,
    pub reasons: Vec<String>,
    pub truncated: bool,
    pub bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum FormatMode {
    Default,
    Full,
}

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
        Ok(self.assemble_context_with_items(store, roots, extra)?.0)
    }

    pub fn assemble_context_with_items(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        extra: &[SymbolRow],
    ) -> Result<(String, Vec<ContextItem>)> {
        self.assemble_context_with_items_mode(store, roots, extra, FormatMode::Default)
    }

    pub fn assemble_context_with_items_mode(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        extra: &[SymbolRow],
        mode: FormatMode,
    ) -> Result<(String, Vec<ContextItem>)> {
        // 1. Expand context using graph with scoring
        // We fetch more candidates than we strictly need, then rerank.
        // We use both roots and extra as starting points, but we prioritize roots.
        let mut seeds = Vec::new();
        seeds.extend_from_slice(roots);
        seeds.extend_from_slice(extra);

        let expanded = self.expand_with_scoring(store, &seeds, 50)?;

        let mut stitched = Vec::new();
        for root in roots {
            let examples = store.list_usage_examples_for_symbol(&root.id, 5)?;
            for ex in examples {
                stitched.push(symbol_row_from_usage_example(root, &ex));
            }
        }
        let mut combined_extra = extra.to_vec();
        combined_extra.extend(stitched);

        // 2. Format output
        // Roots are full text. Extra and Expanded are simplified.
        self.format_context_with_mode(store, roots, &combined_extra, &expanded, mode)
    }

    fn expand_with_scoring(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let mut candidates: HashMap<String, (SymbolRow, f32)> = HashMap::new();
        let mut expanded_frontier: HashSet<String> = roots.iter().map(|r| r.id.clone()).collect();

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
                    let depth_penalty = 1.0 / ((depth + 1) as f32);
                    let type_multiplier = match edge.edge_type.as_str() {
                        "extends" | "implements" | "alias" | "type" => 1.5,
                        "call" => 1.0,
                        "reference" => 0.8,
                        _ => 1.0,
                    };
                    let score = depth_penalty * type_multiplier * edge.confidence;

                    let entry = candidates.get_mut(&edge.to_symbol_id);
                    if let Some((_, s)) = entry {
                        if score > *s {
                            *s = score;
                        }
                        continue;
                    }

                    if let Some(row) = store.get_symbol_by_id(&edge.to_symbol_id)? {
                        candidates.insert(row.id.clone(), (row.clone(), score));
                        if expanded_frontier.insert(row.id.clone()) {
                            next_frontier.push(row.id);
                        }
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
        store: &SqliteStore,
        roots: &[SymbolRow],
        explicit_extra: &[SymbolRow],
        expanded: &[SymbolRow],
    ) -> Result<(String, Vec<ContextItem>)> {
        self.format_context_with_mode(store, roots, explicit_extra, expanded, FormatMode::Default)
    }

    pub fn format_context_with_mode(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        explicit_extra: &[SymbolRow],
        expanded: &[SymbolRow],
        mode: FormatMode,
    ) -> Result<(String, Vec<ContextItem>)> {
        let mut out = String::new();
        let mut used = 0usize;
        let mut seen = HashSet::<String>::new();
        let root_ids: HashSet<&String> = roots.iter().map(|r| &r.id).collect();
        let extra_ids: HashSet<&String> = explicit_extra.iter().map(|r| &r.id).collect();
        let mut items: Vec<ContextItem> = Vec::new();

        let max_bytes = self.config.max_context_bytes;
        let root_cap = ((max_bytes as f32) * 0.7) as usize;
        let extra_cap = ((max_bytes as f32) * 0.2) as usize;
        let expanded_cap = max_bytes.saturating_sub(root_cap + extra_cap);

        let mut used_by_role = HashMap::<String, usize>::new();
        let mut count_by_role = HashMap::<String, usize>::new();
        let mut count_by_cluster_key = HashMap::<String, usize>::new();
        let mut count_by_fingerprint = HashMap::<u64, usize>::new();

        // Prioritize roots, then explicit_extra, then expanded
        for sym in roots
            .iter()
            .chain(explicit_extra.iter())
            .chain(expanded.iter())
        {
            if !seen.insert(sym.id.clone()) {
                continue;
            }

            let is_root = root_ids.contains(&sym.id);
            let text = self.read_or_get_text(sym)?;

            let (text, simplified) = match mode {
                FormatMode::Full => (text, false),
                FormatMode::Default => self.simplify_code(&text, &sym.kind, is_root),
            };
            let role = role_for_symbol(is_root, extra_ids.contains(&sym.id));
            let cluster_key = store.get_similarity_cluster_key(&sym.id).ok().flatten();

            let header = format!(
                "=== {}:{}-{} ({} {}) id={} ===\n",
                sym.file_path, sym.start_line, sym.end_line, sym.kind, sym.name, sym.id
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

            let role_cluster_limit = match role.as_str() {
                "root" => 2usize,
                _ => 1usize,
            };

            if let Some(key) = &cluster_key {
                let n = count_by_cluster_key.get(key).copied().unwrap_or(0);
                if n >= role_cluster_limit {
                    continue;
                }
            } else {
                let fp = fingerprint_text(&text);
                let n = count_by_fingerprint.get(&fp).copied().unwrap_or(0);
                if n >= role_cluster_limit {
                    continue;
                }
            }

            let role_cap = match role.as_str() {
                "root" => root_cap,
                "extra" => extra_cap,
                _ => expanded_cap,
            };
            let role_used = used_by_role.get(&role).copied().unwrap_or(0);
            let role_count = count_by_role.get(&role).copied().unwrap_or(0);
            if role_count > 0 && role_used.saturating_add(block.len()) > role_cap {
                continue;
            }

            if used + block.len() > self.config.max_context_bytes {
                // ... truncate logic ...
                let remaining = self.config.max_context_bytes.saturating_sub(used);
                let bytes = block.as_bytes();
                let mut cut = remaining.min(bytes.len());
                while cut > 0 && !block.is_char_boundary(cut) {
                    cut -= 1;
                }
                out.push_str(&block[..cut]);
                let mut reasons = vec![format!("role:{role}")];
                if let Some(key) = &cluster_key {
                    reasons.push(format!("cluster:{key}"));
                } else {
                    reasons.push("dedupe:fingerprint".to_string());
                }
                if simplified {
                    reasons.push("simplified".to_string());
                }
                reasons.push("truncated".to_string());
                items.push(ContextItem {
                    id: sym.id.clone(),
                    file_path: sym.file_path.clone(),
                    start_line: sym.start_line,
                    end_line: sym.end_line,
                    kind: sym.kind.clone(),
                    name: sym.name.clone(),
                    role: role.clone(),
                    reasons,
                    truncated: true,
                    bytes: cut,
                });
                *used_by_role.entry(role.clone()).or_insert(0) += cut;
                *count_by_role.entry(role).or_insert(0) += 1;
                if let Some(key) = &cluster_key {
                    *count_by_cluster_key.entry(key.clone()).or_insert(0) += 1;
                } else {
                    let fp = fingerprint_text(&text);
                    *count_by_fingerprint.entry(fp).or_insert(0) += 1;
                }
                break;
            }

            out.push_str(&block);
            used += block.len();

            let mut reasons = vec![format!("role:{role}")];
            if let Some(key) = &cluster_key {
                reasons.push(format!("cluster:{key}"));
            } else {
                reasons.push("dedupe:fingerprint".to_string());
            }
            if simplified {
                reasons.push("simplified".to_string());
            }
            items.push(ContextItem {
                id: sym.id.clone(),
                file_path: sym.file_path.clone(),
                start_line: sym.start_line,
                end_line: sym.end_line,
                kind: sym.kind.clone(),
                name: sym.name.clone(),
                role: role.clone(),
                reasons,
                truncated: simplified,
                bytes: block.len(),
            });
            *used_by_role.entry(role.clone()).or_insert(0) += block.len();
            *count_by_role.entry(role).or_insert(0) += 1;
            if let Some(key) = &cluster_key {
                *count_by_cluster_key.entry(key.clone()).or_insert(0) += 1;
            } else {
                let fp = fingerprint_text(&text);
                *count_by_fingerprint.entry(fp).or_insert(0) += 1;
            }
        }

        Ok((out, items))
    }

    fn simplify_code(&self, text: &str, kind: &str, is_root: bool) -> (String, bool) {
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
            return (text.to_string(), false);
        }

        let head_count = if kind == "file" { 50 } else { 15 }; // Signature + start
        let tail_count = 5;

        if lines.len() <= head_count + tail_count {
            return (text.to_string(), false);
        }

        let head = &lines[..head_count];
        let tail = &lines[lines.len().saturating_sub(tail_count)..];

        let mut out = head.join("\n");
        out.push_str(&format!(
            "\n... ({} lines omitted) ...\n",
            lines.len().saturating_sub(head_count + tail_count)
        ));
        out.push_str(&tail.join("\n"));
        (out, true)
    }

    fn read_or_get_text(&self, sym: &SymbolRow) -> Result<String> {
        match read_symbol_snippet(&self.config.base_dir, sym) {
            Ok(s) => Ok(s),
            Err(_) => Ok(sym.text.clone()),
        }
    }
}

fn role_for_symbol(is_root: bool, is_extra: bool) -> String {
    if is_root {
        "root".to_string()
    } else if is_extra {
        "extra".to_string()
    } else {
        "expanded".to_string()
    }
}

fn fingerprint_text(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.to_lowercase().hash(&mut h);
    h.finish()
}

fn symbol_row_from_usage_example(root: &SymbolRow, ex: &UsageExampleRow) -> SymbolRow {
    let id = stable_usage_id(&root.id, ex);
    let line = ex.line.unwrap_or(1);
    SymbolRow {
        id,
        file_path: ex.file_path.clone(),
        language: root.language.clone(),
        kind: format!("usage_{}", ex.example_type),
        name: root.name.clone(),
        exported: false,
        start_byte: 0,
        end_byte: 0,
        start_line: line,
        end_line: line,
        text: ex.snippet.clone(),
    }
}

fn stable_usage_id(root_id: &str, ex: &UsageExampleRow) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root_id.hash(&mut h);
    ex.example_type.hash(&mut h);
    ex.file_path.hash(&mut h);
    ex.line.hash(&mut h);
    ex.snippet.hash(&mut h);
    format!("usage:{:016x}", h.finish())
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
        let store = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        store.init().unwrap();

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

        let (output, items) = assembler
            .format_context(&store, &[], &[small.clone(), huge.clone()], &[])
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
            output.contains("lines omitted"),
            "Huge symbol should be truncated"
        );
        assert!(output.contains("line0;"), "Huge symbol should show head");
        assert!(output.contains("line199;"), "Huge symbol should show tail");
        assert!(
            !output.contains("line100;"),
            "Huge symbol should hide middle"
        );

        assert_eq!(items.len(), 2);
        let huge_item = items.iter().find(|i| i.id == "huge").unwrap();
        assert!(huge_item.truncated);
    }

    #[test]
    fn format_context_caps_roots_and_leaves_room_for_extra_and_expanded() {
        let config = make_config(1200);
        let assembler = ContextAssembler::new(config);
        let store = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        store.init().unwrap();

        let mk = |id: &str, file: &str, name: &str, ch: char, text_len: usize| SymbolRow {
            id: id.to_string(),
            file_path: file.to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: name.to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            end_line: 1,
            text: ch.to_string().repeat(text_len),
        };

        let roots = vec![
            mk("r1", "r1.ts", "r1", 'r', 160),
            mk("r2", "r2.ts", "r2", 'r', 160),
        ];
        let extra = vec![mk("e1", "e1.ts", "e1", 'e', 120)];
        let expanded = vec![mk("x1", "x1.ts", "x1", 'x', 120)];

        let (_output, items) = assembler
            .format_context(&store, &roots, &extra, &expanded)
            .unwrap();
        let roots_n = items.iter().filter(|i| i.role == "root").count();
        let extra_n = items.iter().filter(|i| i.role == "extra").count();
        let expanded_n = items.iter().filter(|i| i.role == "expanded").count();

        assert!(roots_n >= 1);
        assert!(extra_n >= 1);
        assert!(expanded_n >= 1);
    }

    #[test]
    fn format_context_dedupes_by_fingerprint_when_no_cluster_key() {
        let config = make_config(10_000);
        let assembler = ContextAssembler::new(config);
        let store = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        store.init().unwrap();

        let mk = |id: &str| SymbolRow {
            id: id.to_string(),
            file_path: format!("{id}.ts"),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: id.to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            end_line: 1,
            text: "export function same() { return 1 }".to_string(),
        };

        let expanded = vec![mk("a"), mk("b")];
        let (_output, items) = assembler
            .format_context(&store, &[], &[], &expanded)
            .unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].reasons.iter().any(|r| r == "dedupe:fingerprint"));
    }
}

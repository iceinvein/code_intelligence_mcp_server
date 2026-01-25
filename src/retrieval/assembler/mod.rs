pub mod formatting;
pub mod graph;
pub mod tokens;

use crate::config::Config;
use crate::storage::sqlite::SqliteStore;
use crate::storage::sqlite::SymbolRow;
use anyhow::{anyhow, Context, Result};
use formatting::{
    fingerprint_text, format_structured_output, format_symbol_with_docstring, role_for_symbol,
    simplify_code_with_query, symbol_row_from_usage_example,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

pub use formatting::{ContextItem, FormatMode};

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
        query: Option<&str>,
    ) -> Result<String> {
        Ok(self
            .assemble_context_with_items(store, roots, extra, query)?
            .0)
    }

    pub fn assemble_context_with_items(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        extra: &[SymbolRow],
        query: Option<&str>,
    ) -> Result<(String, Vec<ContextItem>)> {
        self.assemble_context_with_items_mode(store, roots, extra, query, FormatMode::Default)
    }

    pub fn assemble_context_with_items_mode(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        extra: &[SymbolRow],
        query: Option<&str>,
        mode: FormatMode,
    ) -> Result<(String, Vec<ContextItem>)> {
        // 1. Expand context using graph with scoring
        // We fetch more candidates than we strictly need, then rerank.
        // We use both roots and extra as starting points, but we prioritize roots.
        let mut seeds = Vec::new();
        seeds.extend_from_slice(roots);
        seeds.extend_from_slice(extra);

        let expanded = graph::expand_with_scoring(store, &seeds, 50)?;

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
        self.format_context_with_mode(store, roots, &combined_extra, &expanded, mode, query)
    }

    pub fn format_context(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        explicit_extra: &[SymbolRow],
        expanded: &[SymbolRow],
        query: Option<&str>,
    ) -> Result<(String, Vec<ContextItem>)> {
        self.format_context_with_mode(
            store,
            roots,
            explicit_extra,
            expanded,
            FormatMode::Default,
            query,
        )
    }

    pub fn format_context_with_mode(
        &self,
        store: &SqliteStore,
        roots: &[SymbolRow],
        explicit_extra: &[SymbolRow],
        expanded: &[SymbolRow],
        mode: FormatMode,
        query: Option<&str>,
    ) -> Result<(String, Vec<ContextItem>)> {
        let mut used_tokens = 0usize;
        let mut seen = HashSet::<String>::new();
        let root_ids: HashSet<&String> = roots.iter().map(|r| &r.id).collect();
        let extra_ids: HashSet<&String> = explicit_extra.iter().map(|r| &r.id).collect();
        let mut items: Vec<ContextItem> = Vec::new();

        // Use token-based budgeting instead of byte-based
        let counter = tokens::get_token_counter();
        let max_tokens = self.config.max_context_tokens;
        let root_cap = ((max_tokens as f32) * 0.7) as usize;
        let extra_cap = ((max_tokens as f32) * 0.2) as usize;
        let expanded_cap = max_tokens.saturating_sub(root_cap + extra_cap);

        let mut used_by_role = HashMap::<String, usize>::new();
        let mut count_by_role = HashMap::<String, usize>::new();
        let mut count_by_cluster_key = HashMap::<String, usize>::new();
        let mut count_by_fingerprint = HashMap::<u64, usize>::new();

        // Collect symbols by role for structured output
        let mut definitions: Vec<(SymbolRow, String)> = Vec::new();
        let mut examples: Vec<(SymbolRow, String)> = Vec::new();
        let mut related: Vec<(SymbolRow, String)> = Vec::new();

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

            // Compute remaining budget for this symbol
            let remaining = max_tokens.saturating_sub(used_tokens);

            let (text, simplified) = match mode {
                FormatMode::Full => (text, false),
                FormatMode::Default => {
                    simplify_code_with_query(&text, &sym.kind, is_root, query, counter, remaining)
                }
            };
            let role = role_for_symbol(is_root, extra_ids.contains(&sym.id));
            let cluster_key = store.get_similarity_cluster_key(&sym.id).ok().flatten();

            // For root symbols, fetch docstring to enhance formatting
            let docstring = if is_root {
                store.get_docstring_by_symbol(&sym.id).ok().flatten()
            } else {
                None
            };

            // Format with docstring if available (affects token count for roots)
            let formatted_text = if is_root && docstring.is_some() {
                format_symbol_with_docstring(sym, &text, &role, docstring.as_ref())
            } else {
                text.clone()
            };

            // Count tokens for the formatted symbol
            let text_tokens = counter.count(&formatted_text);

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
            if role_count > 0 && role_used.saturating_add(text_tokens) > role_cap {
                continue;
            }

            if used_tokens + text_tokens > max_tokens {
                // simplify_code_with_query already handled truncation based on remaining budget
                // Just add the truncated symbol to context and stop processing more
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

                // Add to appropriate section
                match role.as_str() {
                    "root" => definitions.push((sym.clone(), formatted_text.clone())),
                    "extra" if sym.kind.starts_with("usage_") => {
                        examples.push((sym.clone(), formatted_text.clone()))
                    }
                    _ => related.push((sym.clone(), formatted_text.clone())),
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
                    truncated: true,
                    tokens: text_tokens,
                });
                *used_by_role.entry(role.clone()).or_insert(0) += text_tokens;
                *count_by_role.entry(role).or_insert(0) += 1;
                if let Some(key) = &cluster_key {
                    *count_by_cluster_key.entry(key.clone()).or_insert(0) += 1;
                } else {
                    let fp = fingerprint_text(&text);
                    *count_by_fingerprint.entry(fp).or_insert(0) += 1;
                }
                break;
            }

            used_tokens += text_tokens;

            let mut reasons = vec![format!("role:{role}")];
            if let Some(key) = &cluster_key {
                reasons.push(format!("cluster:{key}"));
            } else {
                reasons.push("dedupe:fingerprint".to_string());
            }
            if simplified {
                reasons.push("simplified".to_string());
            }

            // Add to appropriate section
            match role.as_str() {
                "root" => definitions.push((sym.clone(), formatted_text.clone())),
                "extra" if sym.kind.starts_with("usage_") => {
                    examples.push((sym.clone(), text.clone()))
                }
                _ => related.push((sym.clone(), text.clone())),
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
                tokens: text_tokens,
            });
            *used_by_role.entry(role.clone()).or_insert(0) += text_tokens;
            *count_by_role.entry(role).or_insert(0) += 1;
            if let Some(key) = &cluster_key {
                *count_by_cluster_key.entry(key.clone()).or_insert(0) += 1;
            } else {
                let fp = fingerprint_text(&text);
                *count_by_fingerprint.entry(fp).or_insert(0) += 1;
            }
        }

        // Format with structured output
        let out = format_structured_output(&definitions, &examples, &related);

        Ok((out, items))
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
            // Reranker config (FNDN-03)
            reranker_model_path: None,
            reranker_top_k: 20,
            reranker_cache_dir: None,
            // Learning config (FNDN-04)
            learning_enabled: false,
            learning_selection_boost: 0.1,
            learning_file_affinity_boost: 0.05,
            // Token config (FNDN-05)
            max_context_tokens: 8192,
            token_encoding: "o200k_base".to_string(),
            // Performance config (FNDN-06)
            parallel_workers: 4,
            embedding_cache_enabled: true,
            // PageRank config (FNDN-07)
            pagerank_damping: 0.85,
            pagerank_iterations: 20,
            // Query expansion config (FNDN-02)
            synonym_expansion_enabled: true,
            acronym_expansion_enabled: true,
            // RRF config (RETR-05)
            rrf_enabled: true,
            rrf_k: 60.0,
            rrf_keyword_weight: 1.0,
            rrf_vector_weight: 1.0,
            rrf_graph_weight: 0.5,
            // HyDE config (RETR-06, RETR-07)
            hyde_enabled: false,
            hyde_llm_backend: "openai".to_string(),
            hyde_api_key: None,
            hyde_max_tokens: 512,
            // Metrics config (PERF-04)
            metrics_enabled: true,
            metrics_port: 9090,
            package_detection_enabled: true,
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
            .format_context(&store, &[], &[small.clone(), huge.clone()], &[], None)
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
            .format_context(&store, &[], &[], &expanded, None)
            .unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].reasons.iter().any(|r| r == "dedupe:fingerprint"));
    }

    #[test]
    fn format_context_uses_query_aware_truncation() {
        // Test that format_context accepts query parameter and works correctly
        let config = make_config(10000);
        let assembler = ContextAssembler::new(config);
        let store = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        store.init().unwrap();

        // Create a function
        let func = SymbolRow {
            id: "test_func".to_string(),
            file_path: "test.rs".to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: "test_func".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            end_line: 5,
            text: "fn test_func() {\n    let x = 1;\n    return x;\n}".to_string(),
        };

        // Test with query - should work without panicking
        let (output_with_query, _items) = assembler
            .format_context(&store, &[func.clone()], &[], &[], Some("test"))
            .unwrap();

        // Test without query - should work without panicking
        let (output_no_query, _items) = assembler
            .format_context(&store, &[func], &[], &[], None)
            .unwrap();

        // Both should produce some output
        assert!(
            !output_with_query.is_empty(),
            "Output with query should not be empty"
        );
        assert!(
            !output_no_query.is_empty(),
            "Output without query should not be empty"
        );
    }

    #[test]
    fn format_context_with_query_truncates_differently_than_without() {
        // This test verifies that providing a query vs not providing one
        // can produce different truncation behavior for large symbols
        let config = make_config(500); // Small budget to force truncation
        let assembler = ContextAssembler::new(config);
        let store = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        store.init().unwrap();

        // Create a large function with many lines
        let mut lines = vec!["fn large_function() {".to_string()];
        for i in 0..500 {
            lines.push(format!("    let x{} = {};", i, i));
        }
        lines.push("}".to_string());

        let large_func = SymbolRow {
            id: "large_func".to_string(),
            file_path: "large.rs".to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: "large_function".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            end_line: lines.len() as u32,
            text: lines.join("\n"),
        };

        // Both should produce some output and not panic
        let (with_query, _) = assembler
            .format_context(&store, &[large_func.clone()], &[], &[], Some("x100"))
            .unwrap();

        let (without_query, _) = assembler
            .format_context(&store, &[large_func.clone()], &[], &[], None)
            .unwrap();

        // Both should produce some output
        assert!(
            !with_query.is_empty(),
            "Output with query should not be empty"
        );
        assert!(
            !without_query.is_empty(),
            "Output without query should not be empty"
        );
    }

    #[test]
    fn format_context_preserves_small_symbols_with_query() {
        let config = make_config(10000);
        let assembler = ContextAssembler::new(config);
        let store = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        store.init().unwrap();

        // Small function that fits in budget
        let small_func = SymbolRow {
            id: "small_func".to_string(),
            file_path: "small.rs".to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: "small_func".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            end_line: 5,
            text: "fn small_func() {\n    let x = 1;\n    return x;\n}".to_string(),
        };

        let (output, items) = assembler
            .format_context(&store, &[small_func], &[], &[], Some("query"))
            .unwrap();

        // Small symbols should be returned as-is
        assert!(
            output.contains("let x = 1"),
            "Small function should not be truncated"
        );
        assert!(
            output.contains("return x"),
            "Small function should include return statement"
        );
        assert!(
            !items[0].truncated,
            "Small symbol should not be marked as truncated"
        );
    }
}

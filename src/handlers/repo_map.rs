use super::AppState;
use crate::storage::sqlite::RepoMapSymbolRow;
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;

const DEFAULT_BUDGET_TOKENS: u32 = 4000;
const DEFAULT_MAX_FILES: u32 = 40;
const DEFAULT_MAX_SYMBOLS_PER_FILE: u32 = 8;
const MAX_SCAN_SYMBOLS: usize = 20_000;

#[derive(Debug, Clone, Copy)]
pub struct RepoMapOptions {
    pub budget_tokens: Option<u32>,
    pub max_files: Option<u32>,
    pub max_symbols_per_file: Option<u32>,
}

#[derive(Debug, Clone)]
struct FileEntry {
    file_path: String,
    language: String,
    importance: f64,
    symbol_count: usize,
    exported_count: usize,
    symbols: Vec<RepoMapSymbolRow>,
}

pub fn handle_repo_map(state: &AppState, options: RepoMapOptions) -> Result<Value, anyhow::Error> {
    let rows = state.sqlite.list_repo_map_symbols(MAX_SCAN_SYMBOLS)?;
    Ok(build_repo_map(rows, options))
}

fn build_repo_map(rows: Vec<RepoMapSymbolRow>, options: RepoMapOptions) -> Value {
    let budget_tokens = options
        .budget_tokens
        .unwrap_or(DEFAULT_BUDGET_TOKENS)
        .max(200);
    let max_files = options.max_files.unwrap_or(DEFAULT_MAX_FILES).max(1) as usize;
    let max_symbols_per_file = options
        .max_symbols_per_file
        .unwrap_or(DEFAULT_MAX_SYMBOLS_PER_FILE)
        .max(1) as usize;

    let total_symbols = rows.len();
    let mut grouped: BTreeMap<String, FileEntry> = BTreeMap::new();

    for row in rows {
        let score = symbol_score(&row);
        let entry = grouped
            .entry(row.file_path.clone())
            .or_insert_with(|| FileEntry {
                file_path: row.file_path.clone(),
                language: row.language.clone(),
                importance: 0.0,
                symbol_count: 0,
                exported_count: 0,
                symbols: Vec::new(),
            });
        entry.importance += score;
        entry.symbol_count += 1;
        if row.exported {
            entry.exported_count += 1;
        }
        entry.symbols.push(row);
    }

    let total_files = grouped.len();
    let mut files: Vec<FileEntry> = grouped.into_values().collect();
    for file in &mut files {
        file.symbols.sort_by(compare_symbols);
        file.symbols.truncate(max_symbols_per_file);
    }
    files.sort_by(compare_files);
    files.truncate(max_files);

    let mut selected = Vec::new();
    let mut used_tokens = approx_tokens_for_value(&json!({
        "budget_tokens": budget_tokens,
        "used_tokens": 0,
        "total_files": total_files,
        "total_symbols": total_symbols,
        "files": [],
    }));

    let mut truncated = files.len() < total_files;
    for file in files {
        let value = file_to_value(file);
        let cost = approx_tokens_for_value(&value);
        if !selected.is_empty() && used_tokens.saturating_add(cost) > budget_tokens {
            truncated = true;
            break;
        }
        used_tokens = used_tokens.saturating_add(cost);
        selected.push(value);
    }

    json!({
        "budget_tokens": budget_tokens,
        "used_tokens": used_tokens,
        "total_files": total_files,
        "total_symbols": total_symbols,
        "returned_files": selected.len(),
        "truncated": truncated,
        "files": selected,
    })
}

fn compare_files(a: &FileEntry, b: &FileEntry) -> Ordering {
    b.importance
        .partial_cmp(&a.importance)
        .unwrap_or(Ordering::Equal)
        .then_with(|| b.exported_count.cmp(&a.exported_count))
        .then_with(|| a.file_path.cmp(&b.file_path))
}

fn compare_symbols(a: &RepoMapSymbolRow, b: &RepoMapSymbolRow) -> Ordering {
    symbol_score(b)
        .partial_cmp(&symbol_score(a))
        .unwrap_or(Ordering::Equal)
        .then_with(|| b.exported.cmp(&a.exported))
        .then_with(|| a.start_line.cmp(&b.start_line))
        .then_with(|| a.name.cmp(&b.name))
}

fn symbol_score(symbol: &RepoMapSymbolRow) -> f64 {
    let exported_boost = if symbol.exported { 0.05 } else { 0.0 };
    symbol.pagerank + ((symbol.in_degree + symbol.out_degree) as f64 * 0.01) + exported_boost
}

fn file_to_value(file: FileEntry) -> Value {
    let symbols: Vec<Value> = file
        .symbols
        .into_iter()
        .map(|symbol| {
            json!({
                "id": symbol.id,
                "name": symbol.name,
                "kind": symbol.kind,
                "exported": symbol.exported,
                "signature": extract_signature(&symbol.text),
                "line": symbol.start_line,
                "importance": round_score(symbol_score(&symbol)),
            })
        })
        .collect();

    json!({
        "file_path": file.file_path,
        "language": file.language,
        "importance": round_score(file.importance),
        "symbol_count": file.symbol_count,
        "exported_count": file.exported_count,
        "symbols": symbols,
    })
}

fn extract_signature(text: &str) -> String {
    let signature = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string();
    if signature.chars().count() > 160 {
        let mut truncated = signature.chars().take(157).collect::<String>();
        truncated.push_str("...");
        truncated
    } else {
        signature
    }
}

fn round_score(value: f64) -> f64 {
    (value * 10000.0).round() / 10000.0
}

fn approx_tokens_for_value(value: &Value) -> u32 {
    let bytes = serde_json::to_vec(value).map(|v| v.len()).unwrap_or(0);
    ((bytes as u32).saturating_add(3) / 4).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        id: &str,
        file_path: &str,
        name: &str,
        exported: bool,
        pagerank: f64,
    ) -> RepoMapSymbolRow {
        RepoMapSymbolRow {
            id: id.to_string(),
            file_path: file_path.to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: name.to_string(),
            exported,
            start_line: 10,
            end_line: 12,
            text: format!("pub fn {name}() {{}}"),
            pagerank,
            in_degree: 0,
            out_degree: 0,
        }
    }

    #[test]
    fn repo_map_ranks_files_and_respects_caps() {
        let value = build_repo_map(
            vec![
                row("a1", "src/a.rs", "low", true, 0.1),
                row("b1", "src/b.rs", "high", true, 0.9),
                row("b2", "src/b.rs", "hidden", false, 0.2),
            ],
            RepoMapOptions {
                budget_tokens: Some(10_000),
                max_files: Some(1),
                max_symbols_per_file: Some(1),
            },
        );

        assert_eq!(value["total_files"], 2);
        assert_eq!(value["returned_files"], 1);
        assert_eq!(value["truncated"], true);
        assert_eq!(value["files"][0]["file_path"], "src/b.rs");
        assert_eq!(value["files"][0]["symbols"].as_array().unwrap().len(), 1);
        assert_eq!(value["files"][0]["symbols"][0]["name"], "high");
    }
}

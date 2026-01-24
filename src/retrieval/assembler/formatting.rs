use crate::storage::sqlite::SymbolRow;
use serde::Serialize;
use std::hash::{Hash, Hasher};

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
    pub tokens: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum FormatMode {
    Default,
    Full,
}

pub fn simplify_code(text: &str, kind: &str, is_root: bool) -> (String, bool) {
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

pub fn role_for_symbol(is_root: bool, is_extra: bool) -> String {
    if is_root {
        "root".to_string()
    } else if is_extra {
        "extra".to_string()
    } else {
        "expanded".to_string()
    }
}

pub fn fingerprint_text(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.to_lowercase().hash(&mut h);
    h.finish()
}

pub fn symbol_row_from_usage_example(
    root: &SymbolRow,
    ex: &crate::storage::sqlite::UsageExampleRow,
) -> SymbolRow {
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

fn stable_usage_id(root_id: &str, ex: &crate::storage::sqlite::UsageExampleRow) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root_id.hash(&mut h);
    ex.example_type.hash(&mut h);
    ex.file_path.hash(&mut h);
    ex.line.hash(&mut h);
    ex.snippet.hash(&mut h);
    format!("usage:{:016x}", h.finish())
}

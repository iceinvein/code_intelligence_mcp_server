use crate::storage::sqlite::SymbolRow;
use serde::Serialize;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use super::tokens::TokenCounter;

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

/// Score lines by relevance to query using BM25-like scoring
///
/// Returns a vec of (line_index, score) sorted by score descending.
/// Higher scores indicate more relevant lines.
pub fn rank_lines_by_relevance(lines: &[&str], query: &str) -> Vec<(usize, f32)> {
    let query_lower = query.to_lowercase();
    let query_terms: HashSet<&str> = query_lower
        .split_whitespace()
        .collect();

    let mut line_scores: Vec<(usize, f32)> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let line_lower = line.to_lowercase();
            let mut score = 0.0f32;

            // Exact term matches (case-insensitive)
            for term in &query_terms {
                if line_lower.contains(term) {
                    score += 1.0;
                }
                // Word boundary match (higher weight)
                for word in line_lower.split_whitespace() {
                    if word == *term {
                        score += 2.0;
                    }
                }
            }

            // Bonus for lines with structural keywords
            if line_lower.contains("fn ")
                || line_lower.contains("function ")
                || line_lower.contains("class ")
                || line_lower.contains("interface ")
                || line_lower.contains("struct ")
                || line_lower.contains("impl ")
                || line_lower.contains("type ")
                || line_lower.contains("trait ")
            {
                score += 0.5;
            }
            if line_lower.contains("return ") || line_lower.contains("pub ") || line_lower.contains("export ") {
                score += 0.3;
            }

            (i, score)
        })
        .collect();

    line_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    line_scores
}

/// Smart truncate: keep most relevant lines within token budget
///
/// Preserves header and footer lines while selecting the most query-relevant
/// lines from the middle section. Lines are ranked by relevance to the query
/// using `rank_lines_by_relevance`.
pub fn smart_truncate(text: &str, query: &str, max_tokens: usize, counter: &TokenCounter) -> String {
    let lines: Vec<&str> = text.lines().collect();

    // If within budget, return as-is
    if counter.count(text) <= max_tokens {
        return text.to_string();
    }

    // Always keep first few lines (header/signature) and last few lines (footer)
    let header_count = 5.min(lines.len());
    let footer_count = 3.min(lines.len());

    // Can't fit header + footer
    if header_count + footer_count > lines.len() {
        return text.to_string();
    }

    let mut selected_indices = HashSet::new();

    // Mark header lines as selected
    for i in 0..header_count {
        selected_indices.insert(i);
    }

    // Footer indices to add later (don't mark as selected yet)
    let footer_start = lines.len().saturating_sub(footer_count);
    let footer_indices: Vec<usize> = (footer_start..lines.len()).collect();

    // Score middle lines by relevance
    let middle_start = header_count;
    let middle_end = lines.len().saturating_sub(footer_count);
    if middle_end > middle_start {
        let middle_lines: Vec<&str> = lines[middle_start..middle_end].to_vec();
        let scored = rank_lines_by_relevance(&middle_lines, query);

        // Build result lines preserving original order
        let mut result_lines: Vec<(usize, &str)> = Vec::new();
        let mut current_tokens = 0usize;

        // Add header first
        for i in 0..header_count {
            result_lines.push((i, lines[i]));
            current_tokens += counter.count(lines[i]);
        }

        // Add relevant middle lines until budget exhausted
        for (relative_idx, _score) in scored {
            let abs_idx = middle_start + relative_idx;
            if selected_indices.contains(&abs_idx) {
                continue;
            }
            let line_tokens = counter.count(lines[abs_idx]);
            if current_tokens + line_tokens > max_tokens {
                break;
            }
            selected_indices.insert(abs_idx);
            result_lines.push((abs_idx, lines[abs_idx]));
            current_tokens += line_tokens;
        }

        // Add footer if space permits
        for i in footer_indices {
            if !selected_indices.contains(&i) {
                let line_tokens = counter.count(lines[i]);
                if current_tokens + line_tokens <= max_tokens {
                    result_lines.push((i, lines[i]));
                    current_tokens += line_tokens;
                }
            }
        }

        // Sort by original line number and join
        result_lines.sort_by_key(|(idx, _)| *idx);
        let result: String = result_lines
            .iter()
            .map(|(_, line)| *line)
            .collect::<Vec<_>>()
            .join("\n");

        return result;
    }

    // Fallback: just head + tail
    let mut result = String::new();
    for i in 0..header_count {
        result.push_str(lines[i]);
        result.push('\n');
    }
    if middle_end > middle_start {
        let omitted = middle_end - middle_start;
        result.push_str(&format!("... ({} lines omitted) ...\n", omitted));
    }
    for i in lines.len().saturating_sub(footer_count)..lines.len() {
        result.push_str(lines[i]);
        if i < lines.len() - 1 {
            result.push('\n');
        }
    }
    result
}

/// Simplify code with optional query-aware smart truncation
///
/// When query is provided, uses smart truncation to keep query-relevant lines.
/// When query is None, uses simple head/tail truncation.
pub fn simplify_code_with_query(
    text: &str,
    kind: &str,
    is_root: bool,
    query: Option<&str>,
    counter: &TokenCounter,
    max_tokens: usize,
) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();

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
        let tokens = counter.count(text);
        if tokens <= max_tokens {
            return (text.to_string(), false);
        }
    }

    // Use smart truncation when query is available
    if let Some(q) = query {
        return (smart_truncate(text, q, max_tokens, counter), true);
    }

    // Fallback to simple head/tail truncation
    let head_count = if kind == "file" { 50 } else { 15 };
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

/// Format a markdown section header (## Section Name)
pub fn format_section_header(name: &str) -> String {
    format!("\n## {}\n\n", name)
}

/// Format a single symbol as a markdown code block with metadata
///
/// Creates a markdown code block with symbol metadata header including
/// file path, line range, symbol name, kind, and language.
pub fn format_symbol_section(sym: &SymbolRow, text: &str, role: &str) -> String {
    format!(
        "### {}:{}-{} `{}` ({})\n```{}\n{}\n```\n\n",
        sym.file_path, sym.start_line, sym.end_line, sym.name, sym.kind, sym.language, text
    )
}

/// Format structured output with markdown sections (Definitions, Examples, Related)
///
/// Groups symbols by role and organizes them into clear markdown sections
/// for better LLM comprehension. Uses ## Section headers.
///
/// # Arguments
/// * `definitions` - Root symbols with their formatted text
/// * `examples` - Usage examples with their formatted text
/// * `related` - Expanded/extra symbols with their formatted text
pub fn format_structured_output(
    definitions: &[(SymbolRow, String)],
    examples: &[(SymbolRow, String)],
    related: &[(SymbolRow, String)],
) -> String {
    let mut out = String::new();

    // Definitions section (always present)
    out.push_str(&format_section_header("Definitions"));
    for (sym, text) in definitions {
        out.push_str(&format_symbol_section(sym, text, "root"));
    }

    // Examples section (if non-empty)
    if !examples.is_empty() {
        out.push_str(&format_section_header("Examples"));
        for (sym, text) in examples {
            out.push_str(&format_symbol_section(sym, text, "extra"));
        }
    }

    // Related section (if non-empty)
    if !related.is_empty() {
        out.push_str(&format_section_header("Related"));
        for (sym, text) in related {
            out.push_str(&format_symbol_section(sym, text, "expanded"));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rank_lines_by_relevance_basic() {
        let lines = vec![
            "fn process_data() {",
            "    let x = 1;",
            "    return x;",
            "}",
        ];
        let result = rank_lines_by_relevance(&lines, "process_data");
        assert_eq!(result[0].0, 0);
        assert_eq!(result[1].0, 2);
    }

    #[test]
    fn test_rank_lines_by_relevance_case_insensitive() {
        let lines = vec![
            "function processData() {",
            "    const x = 1;",
            "}",
        ];
        let result = rank_lines_by_relevance(&lines, "PROCESSDATA");
        assert!(result[0].1 > 0.0);
    }

    #[test]
    fn test_smart_truncate_within_budget() {
        let counter = TokenCounter::new("o200k_base").unwrap();
        let text = "fn test() {\n    let x = 1;\n    return x;\n}";
        let result = smart_truncate(text, "test", 1000, &counter);
        assert_eq!(result, text);
    }

    #[test]
    fn test_smart_truncate_preserves_header_footer() {
        let counter = TokenCounter::new("o200k_base").unwrap();
        let mut lines = vec![
            "fn big_function() {".to_string(),
            "    // setup".to_string(),
        ];
        for i in 0..50 {
            lines.push(format!("    let x{} = {};", i, i));
        }
        lines.push("    return 0;".to_string());
        lines.push("}".to_string());
        let text: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let text_joined = text.join("\n");

        let result = smart_truncate(&text_joined, "big_function", 100, &counter);
        assert!(result.contains("fn big_function() {"));
        // With a 100 token budget, should be able to keep the closing brace
        assert!(result.contains("}"));
    }

    #[test]
    fn test_simplify_code_with_query_no_query() {
        let counter = TokenCounter::new("o200k_base").unwrap();
        let text = "fn test() {\n    let x = 1;\n    return x;\n}";
        let (result, simplified) = simplify_code_with_query(text, "function", true, None, &counter, 1000);
        assert_eq!(result, text);
        assert!(!simplified);
    }

    #[test]
    fn test_format_section_header() {
        let result = format_section_header("Definitions");
        assert_eq!(result, "\n## Definitions\n\n");
    }

    #[test]
    fn test_format_symbol_section() {
        let sym = SymbolRow {
            id: "test".to_string(),
            file_path: "test.rs".to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: "test_func".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 50,
            start_line: 1,
            end_line: 5,
            text: "fn test_func() {}".to_string(),
        };
        let result = format_symbol_section(&sym, "fn test_func() {}", "root");
        assert!(result.contains("### test.rs:1-5"));
        assert!(result.contains("`test_func`"));
        assert!(result.contains("(function)"));
        assert!(result.contains("```rust"));
        assert!(result.contains("fn test_func() {}"));
    }

    #[test]
    fn test_format_structured_output() {
        let def_sym = SymbolRow {
            id: "def".to_string(),
            file_path: "def.rs".to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: "definition".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 50,
            start_line: 1,
            end_line: 5,
            text: "fn definition() {}".to_string(),
        };
        let ex_sym = SymbolRow {
            id: "ex".to_string(),
            file_path: "ex.rs".to_string(),
            language: "rust".to_string(),
            kind: "usage_call".to_string(),
            name: "example".to_string(),
            exported: false,
            start_byte: 0,
            end_byte: 30,
            start_line: 10,
            end_line: 10,
            text: "definition();".to_string(),
        };
        let rel_sym = SymbolRow {
            id: "rel".to_string(),
            file_path: "rel.rs".to_string(),
            language: "rust".to_string(),
            kind: "type".to_string(),
            name: "related_type".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 40,
            start_line: 1,
            end_line: 3,
            text: "type Related = ();".to_string(),
        };

        let definitions = vec![(def_sym, "fn definition() {}".to_string())];
        let examples = vec![(ex_sym, "definition();".to_string())];
        let related = vec![(rel_sym, "type Related = ();".to_string())];

        let result = format_structured_output(&definitions, &examples, &related);

        // Check section headers
        assert!(result.contains("## Definitions"));
        assert!(result.contains("## Examples"));
        assert!(result.contains("## Related"));

        // Check content in correct sections
        assert!(result.contains("### def.rs:1-5 `definition`"));
        assert!(result.contains("### ex.rs:10-10 `example`"));
        assert!(result.contains("### rel.rs:1-3 `related_type`"));
    }

    #[test]
    fn test_format_structured_output_empty_sections() {
        let def_sym = SymbolRow {
            id: "def".to_string(),
            file_path: "def.rs".to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: "definition".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 50,
            start_line: 1,
            end_line: 5,
            text: "fn definition() {}".to_string(),
        };

        let definitions = vec![(def_sym, "fn definition() {}".to_string())];
        let examples: Vec<(SymbolRow, String)> = vec![];
        let related: Vec<(SymbolRow, String)> = vec![];

        let result = format_structured_output(&definitions, &examples, &related);

        // Should have Definitions but not Examples or Related
        assert!(result.contains("## Definitions"));
        assert!(!result.contains("## Examples"));
        assert!(!result.contains("## Related"));
    }
}

//! Shared TODO/FIXME comment extraction for all languages.
//!
//! Each language extractor calls `extract_todo_from_tree` with its tree-sitter
//! comment node kinds. The core text scanning logic is language-agnostic.

use super::symbol::{TodoEntry, TodoKind};
use tree_sitter::{Node, TreeCursor};

/// Extract TODO/FIXME comments from a tree-sitter AST.
///
/// `comment_kinds` lists the node kinds that represent comments in this language
/// (e.g., `&["comment", "block_comment"]` for TypeScript, `&["line_comment", "block_comment"]` for Rust).
pub fn extract_todo_from_tree(
    cursor: TreeCursor,
    source: &str,
    file_path: &str,
    comment_kinds: &[&str],
) -> Vec<TodoEntry> {
    let mut todos = Vec::new();
    let mut comment_nodes = Vec::new();

    fn collect_comments<'a>(node: Node<'a>, comments: &mut Vec<Node<'a>>, kinds: &[&str]) {
        if kinds.contains(&node.kind()) {
            comments.push(node);
        }
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                collect_comments(cursor.node(), comments, kinds);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    collect_comments(cursor.node(), &mut comment_nodes, comment_kinds);

    for comment_node in comment_nodes {
        let text = &source[comment_node.byte_range()];
        let line_num = comment_node.start_position().row as u32;

        for line in text.lines() {
            if let Some((kind, todo_text)) = is_todo_comment(line) {
                todos.push(TodoEntry {
                    file_path: file_path.to_string(),
                    line: line_num,
                    kind,
                    text: todo_text,
                    associated_symbol: None,
                });
            }
        }
    }

    todos
}

/// Check if a comment line contains TODO or FIXME.
fn is_todo_comment(line: &str) -> Option<(TodoKind, String)> {
    let lower = line.to_lowercase();

    if let Some(pos) = lower.find("todo") {
        let after = &lower[pos..];
        if after.starts_with("todo:") || after.starts_with("todo ") || after.starts_with("todo(") {
            let text = extract_todo_text(line, pos, "TODO");
            return Some((TodoKind::Todo, text));
        }
    }

    if let Some(pos) = lower.find("fixme") {
        let after = &lower[pos..];
        if after.starts_with("fixme:") || after.starts_with("fixme ") || after.starts_with("fixme(")
        {
            let text = extract_todo_text(line, pos, "FIXME");
            return Some((TodoKind::Fixme, text));
        }
    }

    None
}

/// Extract the text portion after a TODO/FIXME keyword.
fn extract_todo_text(line: &str, keyword_start: usize, keyword: &str) -> String {
    let keyword_end = keyword_start + keyword.len();
    let rest = if keyword_end < line.len() {
        line[keyword_end..]
            .trim_start_matches([':', ' ', '-', '('])
            .trim_end_matches(')')
            .trim()
            .to_string()
    } else {
        String::new()
    };

    // Remove leading comment markers
    rest.trim_start_matches("//")
        .trim_start_matches("/*")
        .trim_start_matches('#')
        .trim_start_matches('*')
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_todo_variants() {
        assert!(is_todo_comment("// TODO: fix this").is_some());
        assert!(is_todo_comment("// TODO fix this").is_some());
        assert!(is_todo_comment("# TODO(user): fix this").is_some());
        assert!(is_todo_comment("// FIXME: broken").is_some());
        assert!(is_todo_comment("// FIXME broken").is_some());
        assert!(is_todo_comment("// nothing here").is_none());
        assert!(is_todo_comment("let todoList = []").is_none());
    }

    #[test]
    fn extracts_todo_text() {
        let (kind, text) = is_todo_comment("// TODO: fix the parser").unwrap();
        assert_eq!(kind, TodoKind::Todo);
        assert_eq!(text, "fix the parser");

        let (kind, text) = is_todo_comment("# FIXME: memory leak").unwrap();
        assert_eq!(kind, TodoKind::Fixme);
        assert_eq!(text, "memory leak");
    }
}

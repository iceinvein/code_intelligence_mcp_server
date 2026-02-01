use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind};

pub fn extract_c_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::C)?;
    extract_symbols_with_parser(&mut parser, source)
}

fn extract_symbols_with_parser(parser: &mut Parser, source: &str) -> Result<ExtractedFile> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse source"))?;
    let root = tree.root_node();

    let cursor = root.walk();
    let mut symbols = Vec::new();
    let mut imports = Vec::new();

    walk(cursor, &mut |node| {
        let kind = node.kind();
        match kind {
            "function_definition" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if let Some(name) = name_from_declarator(declarator, source) {
                        symbols.push(symbol_from_node(
                            name,
                            SymbolKind::Function,
                            true, // C functions are generally global/exported unless static, but we'll assume exported for now
                            node,
                        ));
                    }
                }
            }
            "struct_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap().to_string();
                    symbols.push(symbol_from_node(name, SymbolKind::Struct, true, node));
                }
            }
            "enum_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap().to_string();
                    symbols.push(symbol_from_node(name, SymbolKind::Enum, true, node));
                }
            }
            "type_definition" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if let Some(name) = name_from_declarator(declarator, source) {
                        symbols.push(symbol_from_node(name, SymbolKind::TypeAlias, true, node));
                    }
                }
            }
            "preproc_include" => {
                // #include <stdio.h> or "myheader.h"
                if let Some(path_node) = node.child_by_field_name("path") {
                    let path = path_node.utf8_text(source.as_bytes()).unwrap().to_string();
                    let name = path
                        .trim_matches(|c| c == '<' || c == '>' || c == '"')
                        .to_string();
                    imports.push(Import {
                        name: name.clone(),
                        source: name,
                        alias: None,
                    });
                }
            }
            _ => {}
        }
    });

    symbols.sort_by_key(|s| s.bytes.start);
    Ok(ExtractedFile {
        symbols,
        imports,
        type_edges: Vec::new(),
        dataflow_edges: Vec::new(),
        todos: Vec::new(),
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns: Vec::new(),
    })
}

fn walk(mut cursor: TreeCursor<'_>, f: &mut impl FnMut(Node<'_>)) {
    loop {
        let node = cursor.node();
        f(node);
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

fn name_from_declarator(node: Node, source: &str) -> Option<String> {
    // declarator can be:
    // identifier
    // function_declarator -> declarator: (identifier), parameters
    // pointer_declarator -> declarator
    // array_declarator -> declarator

    let kind = node.kind();
    if kind == "identifier" {
        return Some(node.utf8_text(source.as_bytes()).unwrap().to_string());
    }

    if let Some(child) = node.child_by_field_name("declarator") {
        return name_from_declarator(child, source);
    }

    // Sometimes it's direct child without field name "declarator" if grammar varies?
    // But tree-sitter-c usually nests declarators.

    None
}

fn symbol_from_node(name: String, kind: SymbolKind, exported: bool, node: Node) -> ExtractedSymbol {
    let start = node.start_position();
    let end = node.end_position();
    ExtractedSymbol {
        name,
        kind,
        exported,
        bytes: ByteSpan {
            start: node.start_byte(),
            end: node.end_byte(),
        },
        lines: LineSpan {
            start: start.row as u32,
            end: end.row as u32,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_c_symbols() {
        let source = r#"
#include <stdio.h>
#include "myheader.h"

struct Point {
    int x;
    int y;
};

typedef struct Point Point2D;

enum Color {
    RED,
    GREEN,
    BLUE
};

int add(int a, int b) {
    return a + b;
}

void main() {
    printf("Hello");
}
"#;
        let extracted = extract_c_symbols(source).unwrap();

        assert_eq!(extracted.symbols.len(), 5); // Point, Point2D, Color, add, main

        let point = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Point")
            .unwrap();
        assert_eq!(point.kind, SymbolKind::Struct);

        let color = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Color")
            .unwrap();
        assert_eq!(color.kind, SymbolKind::Enum);

        let add = extracted.symbols.iter().find(|s| s.name == "add").unwrap();
        assert_eq!(add.kind, SymbolKind::Function);

        // Imports
        assert_eq!(extracted.imports.len(), 2);
        assert!(extracted.imports.iter().any(|i| i.name == "stdio.h"));
        assert!(extracted.imports.iter().any(|i| i.name == "myheader.h"));
    }
}

use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind};

pub fn extract_cpp_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::Cpp)?;
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
                        // Check if it's a method (inside a class/struct) - simplistic check
                        // For now just assume Function. Method vs Function distinction requires parent context check which walk doesn't easily give unless we pass state.
                        // But since we flatten symbols, Function is okay.
                        symbols.push(symbol_from_node(name, SymbolKind::Function, true, node));
                    }
                }
            }
            "class_specifier" | "struct_specifier" => {
                let kind_type = if kind == "class_specifier" {
                    SymbolKind::Class
                } else {
                    SymbolKind::Struct
                };
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap().to_string();
                    symbols.push(symbol_from_node(name, kind_type, true, node));
                }
            }
            "enum_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap().to_string();
                    symbols.push(symbol_from_node(name, SymbolKind::Enum, true, node));
                }
            }
            "namespace_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap().to_string();
                    symbols.push(symbol_from_node(name, SymbolKind::Module, true, node));
                }
            }
            "type_definition" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if let Some(name) = name_from_declarator(declarator, source) {
                        symbols.push(symbol_from_node(name, SymbolKind::TypeAlias, true, node));
                    }
                }
            }
            "alias_declaration" => {
                // using X = Y;
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap().to_string();
                    symbols.push(symbol_from_node(name, SymbolKind::TypeAlias, true, node));
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
    let kind = node.kind();
    if kind == "identifier" || kind == "field_identifier" || kind == "type_identifier" {
        return Some(node.utf8_text(source.as_bytes()).unwrap().to_string());
    }

    // qualified_identifier: MyClass::myMethod
    if kind == "qualified_identifier" {
        return Some(node.utf8_text(source.as_bytes()).unwrap().to_string());
    }

    if let Some(child) = node.child_by_field_name("declarator") {
        return name_from_declarator(child, source);
    }

    // function_declarator might have declarator as a child

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
    fn test_extract_cpp_symbols() {
        let source = r#"
#include <iostream>
#include "myheader.h"

namespace MyNamespace {
    class MyClass {
    public:
        void myMethod();
    };
}

struct Point {
    int x;
    int y;
};

using Point2D = Point;

void MyNamespace::MyClass::myMethod() {
    std::cout << "Hello";
}

int main() {
    return 0;
}
"#;
        let extracted = extract_cpp_symbols(source).unwrap();

        // Symbols: MyNamespace, MyClass, Point, Point2D, MyNamespace::MyClass::myMethod, main
        // Note: myMethod declaration inside class is not currently extracted because we walk flat.
        // Wait, we walk the tree. "function_definition" is for implementation. "function_declarator" inside "field_declaration" inside class is for declaration.
        // My implementation only looks for "function_definition" (implementation) and "class_specifier", etc.
        // So MyNamespace, MyClass, Point, Point2D, MyNamespace::MyClass::myMethod, main.

        assert!(extracted
            .symbols
            .iter()
            .any(|s| s.name == "MyNamespace" && s.kind == SymbolKind::Module));
        assert!(extracted
            .symbols
            .iter()
            .any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class));
        assert!(extracted
            .symbols
            .iter()
            .any(|s| s.name == "Point" && s.kind == SymbolKind::Struct));
        assert!(extracted
            .symbols
            .iter()
            .any(|s| s.name == "Point2D" && s.kind == SymbolKind::TypeAlias));
        assert!(extracted
            .symbols
            .iter()
            .any(|s| s.name == "main" && s.kind == SymbolKind::Function));

        // Check for the method implementation
        // The name extractor for qualified_identifier should return the full name
        assert!(extracted
            .symbols
            .iter()
            .any(|s| s.name.contains("myMethod") && s.kind == SymbolKind::Function));

        // Imports
        assert!(extracted.imports.iter().any(|i| i.name == "iostream"));
        assert!(extracted.imports.iter().any(|i| i.name == "myheader.h"));
    }
}

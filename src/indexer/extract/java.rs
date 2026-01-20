use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind};

pub fn extract_java_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::Java)?;
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

    walk(cursor, &mut |node| match node.kind() {
        "class_declaration" => {
            if let Some(name) = symbol_name(node, source) {
                symbols.push(symbol_from_node(
                    name,
                    SymbolKind::Class,
                    is_public(node),
                    node,
                ));
            }
        }
        "interface_declaration" => {
            if let Some(name) = symbol_name(node, source) {
                symbols.push(symbol_from_node(
                    name,
                    SymbolKind::Interface,
                    is_public(node),
                    node,
                ));
            }
        }
        "enum_declaration" => {
            if let Some(name) = symbol_name(node, source) {
                symbols.push(symbol_from_node(
                    name,
                    SymbolKind::Enum,
                    is_public(node),
                    node,
                ));
            }
        }
        "method_declaration" => {
            if let Some(name) = symbol_name(node, source) {
                symbols.push(symbol_from_node(
                    name,
                    SymbolKind::Function,
                    is_public(node),
                    node,
                ));
            }
        }
        "import_declaration" => {
            extract_import(node, source, &mut imports);
        }
        _ => {}
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

fn symbol_name(node: Node, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .map(|n| n.utf8_text(source.as_bytes()).unwrap().to_string())
}

fn is_public(node: Node) -> bool {
    // Check for "modifiers" child node (it might not be a field in some versions of grammar)
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut mod_cursor = child.walk();
            for mod_child in child.children(&mut mod_cursor) {
                if mod_child.kind() == "public" {
                    return true;
                }
            }
        }
    }
    false
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

fn extract_import(node: Node, source: &str, imports: &mut Vec<Import>) {
    // import_declaration: import (static)? name ;
    // name is usually a scoped_identifier or identifier
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
            let name = child.utf8_text(source.as_bytes()).unwrap().to_string();
            // Java imports are usually full package paths
            // We can treat the full path as source
            // And the last part as name (unless it's import static or *)

            let last_part = name.split('.').last().unwrap_or(&name).to_string();

            imports.push(Import {
                name: last_part,
                source: name,
                alias: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_java_symbols() {
        let source = r#"
package com.example;

import java.util.List;

public class MyClass {
    public void myMethod() {}
    private void internalMethod() {}
}

interface MyInterface {
    void doSomething();
}

public enum Color {
    RED, GREEN
}
"#;
        let extracted = extract_java_symbols(source).unwrap();

        // Symbols: MyClass, myMethod, internalMethod, MyInterface, doSomething, Color
        // Note: doSomething inside interface is "method_declaration"? Yes usually.
        // Enum constants? I don't extract them currently (enum_constant)

        // Current logic:
        // class_declaration MyClass (public)
        // method_declaration myMethod (public)
        // method_declaration internalMethod (private)
        // interface_declaration MyInterface (package-private -> exported=false)
        // method_declaration doSomething (implicitly public in interface, but check is_public logic? Interface methods don't have modifiers node usually if implicit)
        // enum_declaration Color (public)

        // Wait, is_public only checks "modifiers" -> "public".
        // Interface methods are public by default, but my is_public will return false if "public" keyword is missing.
        // For now, that's acceptable behavior (following strict "public" keyword presence).

        assert_eq!(extracted.symbols.len(), 6);

        let cls = extracted
            .symbols
            .iter()
            .find(|s| s.name == "MyClass")
            .unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(cls.exported, "Class should be exported (public)");

        let method = extracted
            .symbols
            .iter()
            .find(|s| s.name == "myMethod")
            .unwrap();
        assert_eq!(method.kind, SymbolKind::Function);
        assert!(method.exported);

        let internal = extracted
            .symbols
            .iter()
            .find(|s| s.name == "internalMethod")
            .unwrap();
        assert_eq!(internal.kind, SymbolKind::Function);
        assert!(!internal.exported);

        let iface = extracted
            .symbols
            .iter()
            .find(|s| s.name == "MyInterface")
            .unwrap();
        assert_eq!(iface.kind, SymbolKind::Interface);
        assert!(!iface.exported); // no public keyword

        let color = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Color")
            .unwrap();
        assert_eq!(color.kind, SymbolKind::Enum);
        assert!(color.exported);

        // Imports
        assert_eq!(extracted.imports.len(), 1);
        assert_eq!(extracted.imports[0].name, "List");
        assert_eq!(extracted.imports[0].source, "java.util.List");
    }
}

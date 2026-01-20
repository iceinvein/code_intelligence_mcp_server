use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind};

pub fn extract_python_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::Python)?;
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
        "function_definition" => {
            if let Some(name) = symbol_name(node, source) {
                let exported = !name.starts_with('_');
                symbols.push(symbol_from_node(name, SymbolKind::Function, exported, node));
            }
        }
        "class_definition" => {
            if let Some(name) = symbol_name(node, source) {
                let exported = !name.starts_with('_');
                symbols.push(symbol_from_node(name, SymbolKind::Class, exported, node));
            }
        }
        "import_statement" => {
            extract_imports(node, source, &mut imports);
        }
        "import_from_statement" => {
            extract_from_imports(node, source, &mut imports);
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

fn extract_imports(node: Node, source: &str, imports: &mut Vec<Import>) {
    // import_statement can contain multiple imports: import x, y as z
    // children can be dotted_name or aliased_import
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "dotted_name" {
            let name = child.utf8_text(source.as_bytes()).unwrap().to_string();
            imports.push(Import {
                name: name.clone(),
                source: name,
                alias: None,
            });
        } else if child.kind() == "aliased_import" {
            let name_node = child.child_by_field_name("name");
            let alias_node = child.child_by_field_name("alias");
            if let (Some(name_n), Some(alias_n)) = (name_node, alias_node) {
                let name = name_n.utf8_text(source.as_bytes()).unwrap().to_string();
                let alias = alias_n.utf8_text(source.as_bytes()).unwrap().to_string();
                imports.push(Import {
                    name: name.clone(),
                    source: name,
                    alias: Some(alias),
                });
            }
        }
    }
}

fn extract_from_imports(node: Node, source: &str, imports: &mut Vec<Import>) {
    // from_import_statement: from module import x, y as z
    let module_name = node
        .child_by_field_name("module_name")
        .map(|n| n.utf8_text(source.as_bytes()).unwrap().to_string())
        .unwrap_or_default(); // handle relative imports later

    let mut cursor = node.walk();

    let mut seen_import = false;
    for child in node.children(&mut cursor) {
        if child.kind() == "import" {
            seen_import = true;
            continue;
        }
        if !seen_import {
            continue;
        }

        if child.kind() == "dotted_name" {
            let name = child.utf8_text(source.as_bytes()).unwrap().to_string();
            imports.push(Import {
                name: name.clone(),
                source: module_name.clone(),
                alias: None,
            });
        } else if child.kind() == "aliased_import" {
            let name_node = child.child_by_field_name("name");
            let alias_node = child.child_by_field_name("alias");
            if let (Some(name_n), Some(alias_n)) = (name_node, alias_node) {
                let name = name_n.utf8_text(source.as_bytes()).unwrap().to_string();
                let alias = alias_n.utf8_text(source.as_bytes()).unwrap().to_string();
                imports.push(Import {
                    name: name.clone(),          // This is the symbol name being imported
                    source: module_name.clone(), // From this module
                    alias: Some(alias),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_python_symbols() {
        let source = r#"
import os
from sys import path

def hello():
    pass

class MyClass:
    def method(self):
        pass

def _private():
    pass
"#;
        let extracted = extract_python_symbols(source).unwrap();

        // 4 symbols: hello, MyClass, MyClass.method (actually just "method" in flat list), _private
        assert_eq!(extracted.symbols.len(), 4);

        let hello = extracted
            .symbols
            .iter()
            .find(|s| s.name == "hello")
            .unwrap();
        assert_eq!(hello.kind, SymbolKind::Function);
        assert!(hello.exported);

        let my_class = extracted
            .symbols
            .iter()
            .find(|s| s.name == "MyClass")
            .unwrap();
        assert_eq!(my_class.kind, SymbolKind::Class);
        assert!(my_class.exported);

        let method = extracted
            .symbols
            .iter()
            .find(|s| s.name == "method")
            .unwrap();
        assert_eq!(method.kind, SymbolKind::Function);
        assert!(method.exported);

        let private = extracted
            .symbols
            .iter()
            .find(|s| s.name == "_private")
            .unwrap();
        assert_eq!(private.kind, SymbolKind::Function);
        assert!(!private.exported);

        // Imports
        assert_eq!(extracted.imports.len(), 2);
        assert!(extracted.imports.iter().any(|i| i.name == "os"));
        assert!(extracted
            .imports
            .iter()
            .any(|i| i.name == "path" && i.source == "sys"));
    }
}

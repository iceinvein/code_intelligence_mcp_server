use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind};

pub fn extract_go_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::Go)?;
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
        "function_declaration" => {
            if let Some(name) = symbol_name(node, source) {
                let exported = is_exported(&name);
                symbols.push(symbol_from_node(
                    name,
                    SymbolKind::Function,
                    exported,
                    node,
                ));
            }
        }
        "method_declaration" => {
            if let Some(name) = symbol_name(node, source) {
                let exported = is_exported(&name);
                // For methods, we might want to capture the receiver too, but for now just the method name
                symbols.push(symbol_from_node(
                    name,
                    SymbolKind::Function, // Or create Method kind? Existing is Function/Impl
                    exported,
                    node,
                ));
            }
        }
        "type_spec" => {
            // type_spec inside type_declaration
            if let Some(name) = symbol_name(node, source) {
                let exported = is_exported(&name);
                // Check what kind of type it is
                let kind = if node.child_by_field_name("type").map(|n| n.kind() == "struct_type").unwrap_or(false) {
                    SymbolKind::Struct
                } else if node.child_by_field_name("type").map(|n| n.kind() == "interface_type").unwrap_or(false) {
                    SymbolKind::Interface
                } else {
                    SymbolKind::TypeAlias // or just Type
                };
                
                symbols.push(symbol_from_node(
                    name,
                    kind,
                    exported,
                    node,
                ));
            }
        }
        "import_spec" => {
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

fn is_exported(name: &str) -> bool {
    // In Go, exported symbols start with an uppercase letter
    name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

fn symbol_from_node(
    name: String,
    kind: SymbolKind,
    exported: bool,
    node: Node,
) -> ExtractedSymbol {
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
    // import_spec: (name)? (path)
    let path_node = node.child_by_field_name("path");
    let name_node = node.child_by_field_name("name");
    
    if let Some(path_n) = path_node {
        let path_str = path_n.utf8_text(source.as_bytes()).unwrap().to_string();
        // path_str includes quotes, e.g. "fmt"
        let source_path = path_str.trim_matches('"').to_string();
        
        let alias = name_node.map(|n| n.utf8_text(source.as_bytes()).unwrap().to_string());
        
        // If no alias, the name is the last component of the path (usually)
        // But Import struct has name, source, alias.
        // name: local name used in file
        // source: import path
        
        let name = if let Some(a) = &alias {
            a.clone()
        } else {
            // derive from source path
            source_path.split('/').last().unwrap_or(&source_path).to_string()
        };
        
        imports.push(Import {
            name,
            source: source_path,
            alias,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_go_symbols() {
        let source = r#"
package main

import (
    "fmt"
    my_os "os"
)

func main() {
    fmt.Println("Hello")
}

func ExportedFunc() {}

type MyStruct struct {
    Field int
}

type MyInterface interface {
    Method()
}
"#;
        let extracted = extract_go_symbols(source).unwrap();
        
        // Symbols: main, ExportedFunc, MyStruct, MyInterface
        // Note: Field and Method inside struct/interface are not extracted by top-level walk unless we handle field_declaration/method_spec inside type_spec, 
        // but walk visits all nodes, so if we have matchers for them we could.
        // But currently I only match function_declaration, method_declaration, type_spec.
        // Struct fields are "field_declaration" inside "struct_type".
        // Interface methods are "method_spec" inside "interface_type".
        // I haven't added matchers for those yet.
        
        assert_eq!(extracted.symbols.len(), 4);
        
        let main = extracted.symbols.iter().find(|s| s.name == "main").unwrap();
        assert_eq!(main.kind, SymbolKind::Function);
        assert!(!main.exported); // lowercase
        
        let exported = extracted.symbols.iter().find(|s| s.name == "ExportedFunc").unwrap();
        assert_eq!(exported.kind, SymbolKind::Function);
        assert!(exported.exported); // Uppercase
        
        let my_struct = extracted.symbols.iter().find(|s| s.name == "MyStruct").unwrap();
        assert_eq!(my_struct.kind, SymbolKind::Struct);
        assert!(my_struct.exported);
        
        let my_iface = extracted.symbols.iter().find(|s| s.name == "MyInterface").unwrap();
        assert_eq!(my_iface.kind, SymbolKind::Interface);
        assert!(my_iface.exported);

        // Imports
        assert_eq!(extracted.imports.len(), 2);
        assert!(extracted.imports.iter().any(|i| i.source == "fmt" && i.name == "fmt"));
        assert!(extracted.imports.iter().any(|i| i.source == "os" && i.alias.as_deref() == Some("my_os")));
    }
}

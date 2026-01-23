use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind};

pub fn extract_javascript_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::Javascript)?;
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
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    symbols.push(symbol_from_node(
                        name,
                        SymbolKind::Function,
                        exported,
                        def_node,
                    ));
                }
            }
            "class_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    symbols.push(symbol_from_node(
                        name,
                        SymbolKind::Class,
                        exported,
                        def_node,
                    ));
                }
            }
            "method_definition" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    // Methods inside class are generally not "exported" in the module sense
                    symbols.push(symbol_from_node(name, SymbolKind::Function, false, node));
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                // const x = ..., let y = ..., var z = ...
                extract_variable_declarators(node, source, &mut symbols);
            }
            "import_statement" => {
                extract_imports(node, source, &mut imports);
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

fn symbol_name_from_declaration(node: Node, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("property")) // for method_definition sometimes? No, name is usually "name" or "property_name"
        .map(|n| n.utf8_text(source.as_bytes()).unwrap().to_string())
}

fn definition_node_for_declaration(node: Node) -> (Node, bool) {
    // Check if parent is export_statement
    if let Some(parent) = node.parent() {
        if parent.kind() == "export_statement" {
            return (parent, true);
        }
    }
    (node, false)
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

fn extract_variable_declarators(node: Node, source: &str, symbols: &mut Vec<ExtractedSymbol>) {
    // lexical_declaration -> variable_declarator*
    // variable_declarator -> name: identifier, value: ...

    // Check export status of the declaration
    let (def_node, exported) = definition_node_for_declaration(node);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            if let Some(name_node) = child.child_by_field_name("name") {
                if name_node.kind() == "identifier" {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap().to_string();
                    symbols.push(symbol_from_node(
                        name,
                        SymbolKind::Const, // or Variable
                        exported,
                        def_node, // We use the declaration node as the definition extent? Or the declarator?
                                  // Usually declarator is better for position, but declaration for export status.
                                  // Let's use declarator for position but pass exported status.
                    ));
                    // Fix position to be declarator
                    if let Some(last) = symbols.last_mut() {
                        let start = child.start_position();
                        let end = child.end_position();
                        last.bytes = ByteSpan {
                            start: child.start_byte(),
                            end: child.end_byte(),
                        };
                        last.lines = LineSpan {
                            start: start.row as u32,
                            end: end.row as u32,
                        };
                    }
                }
            }
        }
    }
}

fn extract_imports(node: Node, source: &str, imports: &mut Vec<Import>) {
    // import_statement: import { x } from "mod"; import x from "mod";
    // source: string
    // import_clause: (named_imports | namespace_import | identifier)

    let source_node = node.child_by_field_name("source");
    if let Some(src_n) = source_node {
        let source_path = src_n
            .utf8_text(source.as_bytes())
            .unwrap()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();

        // Check for import_clause child by kind (it might not be a field)
        let mut cursor = node.walk();
        let mut import_clause_node = None;
        for child in node.children(&mut cursor) {
            if child.kind() == "import_clause" {
                import_clause_node = Some(child);
                break;
            }
        }

        // If import_clause is present
        if let Some(clause) = import_clause_node {
            // clause can be identifier (default import)
            // or named_imports
            // or namespace_import

            // It can be mixed in some grammars? import d, { n } from ...

            // Check for default import (identifier)
            // Actually clause children can be identifier, named_imports, namespace_import

            let mut cursor = clause.walk();
            for child in clause.children(&mut cursor) {
                if child.kind() == "identifier" {
                    // Default import
                    let name = child.utf8_text(source.as_bytes()).unwrap().to_string();
                    imports.push(Import {
                        name: name.clone(),
                        source: source_path.clone(),
                        alias: Some(name), // It is aliased to local name
                    });
                } else if child.kind() == "named_imports" {
                    // { x, y as z }
                    let mut named_cursor = child.walk();
                    for specifier in child.children(&mut named_cursor) {
                        if specifier.kind() == "import_specifier" {
                            let name_node = specifier.child_by_field_name("name");
                            let alias_node = specifier.child_by_field_name("alias");

                            if let Some(name_n) = name_node {
                                let name = name_n.utf8_text(source.as_bytes()).unwrap().to_string();
                                let alias = alias_node
                                    .map(|n| n.utf8_text(source.as_bytes()).unwrap().to_string());

                                imports.push(Import {
                                    name: name.clone(),
                                    source: source_path.clone(),
                                    alias,
                                });
                            }
                        }
                    }
                } else if child.kind() == "namespace_import" {
                    // * as ns
                    // child has field "alias"? or just "*" and identifier?
                    // namespace_import -> "*" "as" identifier
                    let mut ns_cursor = child.walk();
                    for ns_child in child.children(&mut ns_cursor) {
                        if ns_child.kind() == "identifier" {
                            let name = ns_child.utf8_text(source.as_bytes()).unwrap().to_string();
                            imports.push(Import {
                                name: "*".to_string(),
                                source: source_path.clone(),
                                alias: Some(name),
                            });
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_javascript_symbols() {
        let source = r#"
import React, { useState } from 'react';
import * as utils from './utils';

export function MyComponent() {
    const [val, setVal] = useState(0);
}

class MyClass {
    method() {}
}

export const CONSTANT = 42;
"#;
        let extracted = extract_javascript_symbols(source).unwrap();

        // Symbols: MyComponent, MyClass, method, CONSTANT
        // Note: useState variable declarator?
        // "const [val, setVal] = ..." -> This is array_pattern, not identifier.
        // My extract_variable_declarators checks for identifier.
        // So [val, setVal] won't be extracted as a symbol "val" or "setVal" currently.
        // That's acceptable for now (complex destructuring is hard).

        assert_eq!(extracted.symbols.len(), 4);

        let comp = extracted
            .symbols
            .iter()
            .find(|s| s.name == "MyComponent")
            .unwrap();
        assert_eq!(comp.kind, SymbolKind::Function);
        assert!(comp.exported);

        let cls = extracted
            .symbols
            .iter()
            .find(|s| s.name == "MyClass")
            .unwrap();
        assert_eq!(cls.kind, SymbolKind::Class);
        assert!(!cls.exported);

        let method = extracted
            .symbols
            .iter()
            .find(|s| s.name == "method")
            .unwrap();
        assert_eq!(method.kind, SymbolKind::Function);

        let constant = extracted
            .symbols
            .iter()
            .find(|s| s.name == "CONSTANT")
            .unwrap();
        assert_eq!(constant.kind, SymbolKind::Const);
        assert!(constant.exported);

        // Imports
        // React (default), useState (named), utils (namespace)
        assert_eq!(extracted.imports.len(), 3);

        assert!(extracted
            .imports
            .iter()
            .any(|i| i.name == "React" && i.source == "react"));
        assert!(extracted
            .imports
            .iter()
            .any(|i| i.name == "useState" && i.source == "react"));
        assert!(extracted
            .imports
            .iter()
            .any(|i| i.alias.as_deref() == Some("utils") && i.source == "./utils"));
    }
}

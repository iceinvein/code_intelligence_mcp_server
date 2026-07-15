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
    let mut type_edges: Vec<(String, String)> = Vec::new();

    walk(cursor, &mut |node| {
        let kind = node.kind();
        match kind {
            "function_definition" => {
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if let Some(name) = name_from_declarator(declarator, source) {
                        extract_function_param_types(node, source, &name, &mut type_edges);
                        symbols.push(symbol_from_node(
                            name,
                            SymbolKind::Function,
                            !is_static(node, source),
                            node,
                        ));
                    }
                }
            }
            "union_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap().to_string();
                    extract_struct_field_types(node, source, &name, &mut type_edges);
                    symbols.push(symbol_from_node(name, SymbolKind::Struct, true, node));
                }
            }
            "struct_specifier" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source.as_bytes()).unwrap().to_string();
                    extract_struct_field_types(node, source, &name, &mut type_edges);
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
            "declaration" => {
                // Only extract top-level declarations (direct children of translation_unit)
                if let Some(parent) = node.parent() {
                    if parent.kind() == "translation_unit" {
                        let exported = !is_static(node, source);
                        let mut cursor2 = node.walk();
                        for child in node.children(&mut cursor2) {
                            // Determine which node holds the declarator name.
                            // - `init_declarator` wraps `declarator` + initializer
                            // - `pointer_declarator`, `array_declarator`, `identifier` are
                            //   direct declarator children (no-init case)
                            // Skip type-specifier and storage-class nodes.
                            let decl_node: Option<Node> = match child.kind() {
                                "init_declarator" => child.child_by_field_name("declarator"),
                                // Type / qualifier nodes — not declarators
                                "type_specifier"
                                | "type_qualifier"
                                | "storage_class_specifier"
                                | "primitive_type"
                                | "type_identifier"
                                | "sized_type_specifier"
                                | "struct_specifier"
                                | "union_specifier"
                                | "enum_specifier"
                                | ";"
                                | "comment" => None,
                                // Everything else that can carry a name (identifier,
                                // pointer_declarator, array_declarator, …)
                                _ => Some(child),
                            };
                            if let Some(decl) = decl_node {
                                // Skip function prototypes
                                if is_function_declaration(decl) {
                                    continue;
                                }
                                if let Some(name) = name_from_declarator(decl, source) {
                                    symbols.push(symbol_from_node(
                                        name,
                                        SymbolKind::Const,
                                        exported,
                                        node,
                                    ));
                                }
                            }
                        }
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
    let todo_cursor = root.walk();
    let todos = super::comments::extract_todo_from_tree(todo_cursor, source, "", &["comment"]);
    Ok(ExtractedFile {
        symbols,
        imports,
        type_edges,
        extends_edges: Vec::new(),
        dataflow_edges: Vec::new(),
        todos,
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

/// Extract the base type name from a C type node, skipping primitives.
fn extract_c_type_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(node.utf8_text(source.as_bytes()).unwrap().to_string()),
        "struct_specifier" | "union_specifier" | "enum_specifier" => node
            .child_by_field_name("name")
            .map(|n| n.utf8_text(source.as_bytes()).unwrap().to_string()),
        // Skip int, char, float, double, void, etc.
        "sized_type_specifier" | "primitive_type" => None,
        _ => None,
    }
}

/// Extract type edges from struct/union field declarations.
fn extract_struct_field_types(
    node: Node, // struct_specifier or union_specifier
    source: &str,
    struct_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "field_declaration" {
                if let Some(type_node) = child.child_by_field_name("type") {
                    if let Some(type_name) = extract_c_type_name(type_node, source) {
                        type_edges.push((struct_name.to_string(), type_name));
                    }
                }
            }
        }
    }
}

/// Extract type edges from function parameter types and return type.
fn extract_function_param_types(
    node: Node, // function_definition
    source: &str,
    fn_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    // The declarator contains the parameter_list
    if let Some(declarator) = node.child_by_field_name("declarator") {
        extract_param_types_from_declarator(declarator, source, fn_name, type_edges);
    }

    // Return type — the type specifier of the function
    if let Some(type_node) = node.child_by_field_name("type") {
        if let Some(type_name) = extract_c_type_name(type_node, source) {
            type_edges.push((fn_name.to_string(), type_name));
        }
    }
}

fn extract_param_types_from_declarator(
    node: Node,
    source: &str,
    fn_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    match node.kind() {
        "function_declarator" => {
            if let Some(params) = node.child_by_field_name("parameters") {
                let mut cursor = params.walk();
                for child in params.children(&mut cursor) {
                    if child.kind() == "parameter_declaration" {
                        if let Some(type_node) = child.child_by_field_name("type") {
                            if let Some(type_name) = extract_c_type_name(type_node, source) {
                                type_edges.push((fn_name.to_string(), type_name));
                            }
                        }
                    }
                }
            }
        }
        "pointer_declarator" => {
            // Recurse to find the actual function_declarator
            if let Some(inner) = node.child_by_field_name("declarator") {
                extract_param_types_from_declarator(inner, source, fn_name, type_edges);
            }
        }
        _ => {}
    }
}

/// Returns true if the given declarator node represents a function prototype.
fn is_function_declaration(decl_node: Node) -> bool {
    match decl_node.kind() {
        "function_declarator" => true,
        "pointer_declarator" => {
            if let Some(inner) = decl_node.child_by_field_name("declarator") {
                is_function_declaration(inner)
            } else {
                false
            }
        }
        _ => false,
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

fn is_static(node: Node, source: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "storage_class_specifier"
            && child.utf8_text(source.as_bytes()).unwrap() == "static"
        {
            return true;
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
            start: start.row as u32 + 1,
            end: end.row as u32 + 1,
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

    #[test]
    fn test_c_line_numbers_1_indexed() {
        let source = "int add(int a, int b) {\n    return a + b;\n}\n";
        let extracted = extract_c_symbols(source).unwrap();
        let add = extracted.symbols.iter().find(|s| s.name == "add").unwrap();
        assert_eq!(
            add.lines.start, 1,
            "Expected line 1, got {}",
            add.lines.start
        );
    }

    #[test]
    fn test_c_static_not_exported() {
        let source = "static int helper(int x) { return x; }\nint public_fn() { return 0; }\n";
        let extracted = extract_c_symbols(source).unwrap();
        let helper = extracted
            .symbols
            .iter()
            .find(|s| s.name == "helper")
            .unwrap();
        assert!(!helper.exported, "static functions should not be exported");
        let public_fn = extracted
            .symbols
            .iter()
            .find(|s| s.name == "public_fn")
            .unwrap();
        assert!(
            public_fn.exported,
            "non-static functions should be exported"
        );
    }

    #[test]
    fn test_c_union() {
        let source = "union Data {\n    int i;\n    float f;\n};\n";
        let extracted = extract_c_symbols(source).unwrap();
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Data" && s.kind == SymbolKind::Struct),
            "Expected union Data as Struct, got: {:?}",
            extracted
                .symbols
                .iter()
                .map(|s| &s.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_c_type_edges() {
        let source = r#"
struct Config {
    Database *db;
    Logger *log;
};

int process(User *user, Config *cfg) {
    return 0;
}
"#;
        let extracted = extract_c_symbols(source).unwrap();

        // Struct field types: Database and Logger are type_identifiers
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Config" && e.1 == "Database"),
            "Expected Config->Database, got: {:?}",
            extracted.type_edges
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Config" && e.1 == "Logger"),
            "Expected Config->Logger, got: {:?}",
            extracted.type_edges
        );

        // Function param types
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "process" && e.1 == "User"),
            "Expected process->User, got: {:?}",
            extracted.type_edges
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "process" && e.1 == "Config"),
            "Expected process->Config, got: {:?}",
            extracted.type_edges
        );
    }

    #[test]
    fn test_c_global_variables() {
        let source = "int global_count = 0;\nstatic char *internal_buf;\n";
        let extracted = extract_c_symbols(source).unwrap();
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "global_count" && s.kind == SymbolKind::Const),
            "Expected global_count, got: {:?}",
            extracted
                .symbols
                .iter()
                .map(|s| (&s.name, &s.kind))
                .collect::<Vec<_>>()
        );
        let internal = extracted
            .symbols
            .iter()
            .find(|s| s.name == "internal_buf")
            .unwrap();
        assert!(!internal.exported, "static global should not be exported");
    }
}

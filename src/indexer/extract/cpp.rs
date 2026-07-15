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
    let mut type_edges: Vec<(String, String)> = Vec::new();

    walk(cursor, &mut |node| {
        let kind = node.kind();
        match kind {
            "function_definition" => {
                // Skip methods inside class/struct bodies — handled by the class/struct arms.
                if let Some(parent) = node.parent() {
                    if parent.kind() == "field_declaration_list" {
                        return;
                    }
                }
                if let Some(declarator) = node.child_by_field_name("declarator") {
                    if let Some(name) = name_from_declarator(declarator, source) {
                        extract_cpp_function_type_edges(node, source, &name, &mut type_edges);
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
                    symbols.push(symbol_from_node(name.clone(), kind_type, true, node));

                    // Extract inheritance edges from base_class_clause.
                    let mut node_cursor = node.walk();
                    for child in node.children(&mut node_cursor) {
                        if child.kind() == "base_class_clause" {
                            let mut base_cursor = child.walk();
                            for base in child.children(&mut base_cursor) {
                                // base_class_clause contains access_specifier keywords and
                                // type nodes — only extract the type identifiers.
                                if let Some(type_name) = extract_cpp_type_name(base, source) {
                                    type_edges.push((name.clone(), type_name));
                                }
                            }
                        }
                    }

                    // Walk the body and extract methods with access specifier tracking.
                    if let Some(body) = node.child_by_field_name("body") {
                        // Default access: private for class, public for struct.
                        let mut current_access = if kind == "class_specifier" {
                            "private"
                        } else {
                            "public"
                        };

                        let mut body_cursor = body.walk();
                        for child in body.children(&mut body_cursor) {
                            match child.kind() {
                                "access_specifier" => {
                                    // Node text is e.g. "public:", "private:", "protected:"
                                    let text = child
                                        .utf8_text(source.as_bytes())
                                        .unwrap()
                                        .trim_end_matches(':')
                                        .trim();
                                    current_access = match text {
                                        "public" => "public",
                                        "private" => "private",
                                        "protected" => "protected",
                                        _ => current_access,
                                    };
                                }
                                "function_definition" => {
                                    // Inline method definition with body.
                                    if let Some(declarator) =
                                        child.child_by_field_name("declarator")
                                    {
                                        if let Some(method_name) =
                                            name_from_declarator(declarator, source)
                                        {
                                            let prefixed = format!("{name}.{method_name}");
                                            let exported = current_access == "public";
                                            extract_cpp_function_type_edges(
                                                child,
                                                source,
                                                &prefixed,
                                                &mut type_edges,
                                            );
                                            symbols.push(symbol_from_node(
                                                prefixed,
                                                SymbolKind::Function,
                                                exported,
                                                child,
                                            ));
                                        }
                                    }
                                }
                                "declaration" => {
                                    // Method declaration (no body) — may contain a
                                    // function_declarator or pointer_declarator wrapping one.
                                    let mut decl_cursor = child.walk();
                                    for sub in child.children(&mut decl_cursor) {
                                        if sub.kind() == "function_declarator"
                                            || (sub.kind() == "pointer_declarator"
                                                && contains_function_declarator(sub))
                                        {
                                            if let Some(method_name) =
                                                name_from_declarator(sub, source)
                                            {
                                                let prefixed = format!("{name}.{method_name}");
                                                let exported = current_access == "public";
                                                extract_params_from_cpp_declarator(
                                                    sub,
                                                    source,
                                                    &prefixed,
                                                    &mut type_edges,
                                                );
                                                // Return type is on the declaration node itself.
                                                if let Some(type_node) =
                                                    child.child_by_field_name("type")
                                                {
                                                    if let Some(type_name) =
                                                        extract_cpp_type_name(type_node, source)
                                                    {
                                                        type_edges
                                                            .push((prefixed.clone(), type_name));
                                                    }
                                                }
                                                symbols.push(symbol_from_node(
                                                    prefixed,
                                                    SymbolKind::Function,
                                                    exported,
                                                    child,
                                                ));
                                            }
                                        }
                                    }
                                }
                                "field_declaration" => {
                                    // May contain a function declarator (e.g. virtual methods
                                    // written as field_declaration in some tree-sitter grammars).
                                    // Also extract field type edges for non-function fields.
                                    let mut has_function_decl = false;
                                    let mut decl_cursor = child.walk();
                                    for sub in child.children(&mut decl_cursor) {
                                        if sub.kind() == "function_declarator" {
                                            has_function_decl = true;
                                            if let Some(method_name) =
                                                name_from_declarator(sub, source)
                                            {
                                                let prefixed = format!("{name}.{method_name}");
                                                let exported = current_access == "public";
                                                extract_params_from_cpp_declarator(
                                                    sub,
                                                    source,
                                                    &prefixed,
                                                    &mut type_edges,
                                                );
                                                // Return type on the field_declaration itself.
                                                if let Some(type_node) =
                                                    child.child_by_field_name("type")
                                                {
                                                    if let Some(type_name) =
                                                        extract_cpp_type_name(type_node, source)
                                                    {
                                                        type_edges
                                                            .push((prefixed.clone(), type_name));
                                                    }
                                                }
                                                symbols.push(symbol_from_node(
                                                    prefixed,
                                                    SymbolKind::Function,
                                                    exported,
                                                    child,
                                                ));
                                            }
                                        }
                                    }

                                    // Non-function field: record a type edge from the class to
                                    // the field's type (e.g. `Dog d;` → Dog→d is not useful, but
                                    // recording the *field type* as a dependency of the class is).
                                    if !has_function_decl {
                                        if let Some(type_node) = child.child_by_field_name("type") {
                                            if let Some(type_name) =
                                                extract_cpp_type_name(type_node, source)
                                            {
                                                type_edges.push((name.clone(), type_name));
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
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

/// Extract the base type name from a C++ type node, skipping primitives.
fn extract_cpp_type_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(node.utf8_text(source.as_bytes()).unwrap().to_string()),
        "qualified_identifier" | "scoped_identifier" => {
            // std::string or MyNamespace::MyType — take the full qualified name.
            Some(node.utf8_text(source.as_bytes()).unwrap().to_string())
        }
        "template_type" => {
            // vector<User> — extract the base template name (first child).
            node.child(0).and_then(|n| extract_cpp_type_name(n, source))
        }
        "struct_specifier" | "class_specifier" | "enum_specifier" => node
            .child_by_field_name("name")
            .map(|n| n.utf8_text(source.as_bytes()).unwrap().to_string()),
        // Skip primitive/built-in types.
        "sized_type_specifier" | "primitive_type" | "auto" => None,
        _ => None,
    }
}

/// Extract type edges from a function/method node: return type and parameter types.
///
/// `fn_name` should already be the prefixed name (e.g. `"Dog.fetch"` for a method).
fn extract_cpp_function_type_edges(
    node: Node,
    source: &str,
    fn_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    // Return type lives on the `type` field of the function_definition / declaration.
    if let Some(type_node) = node.child_by_field_name("type") {
        if let Some(type_name) = extract_cpp_type_name(type_node, source) {
            type_edges.push((fn_name.to_string(), type_name));
        }
    }

    // Parameter types live inside the function_declarator.
    if let Some(declarator) = node.child_by_field_name("declarator") {
        extract_params_from_cpp_declarator(declarator, source, fn_name, type_edges);
    }
}

/// Recurse through declarator wrappers to reach `function_declarator` and collect
/// parameter type edges.
fn extract_params_from_cpp_declarator(
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
                    if child.kind() == "parameter_declaration"
                        || child.kind() == "optional_parameter_declaration"
                    {
                        if let Some(type_node) = child.child_by_field_name("type") {
                            if let Some(type_name) = extract_cpp_type_name(type_node, source) {
                                type_edges.push((fn_name.to_string(), type_name));
                            }
                        }
                    }
                }
            }
        }
        "pointer_declarator" | "reference_declarator" => {
            if let Some(inner) = node.child_by_field_name("declarator") {
                extract_params_from_cpp_declarator(inner, source, fn_name, type_edges);
            }
        }
        _ => {}
    }
}

/// Returns `true` if `node` or any of its descendants is a `function_declarator`.
fn contains_function_declarator(node: Node) -> bool {
    if node.kind() == "function_declarator" {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if contains_function_declarator(child) {
            return true;
        }
    }
    false
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
            start: start.row as u32 + 1,
            end: end.row as u32 + 1,
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

        // Namespace, class, struct, type alias, out-of-class method impl, standalone main.
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

        // Out-of-class qualified method definition — extracted as a standalone function_definition
        // with the full qualified name returned by name_from_declarator.
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name.contains("myMethod") && s.kind == SymbolKind::Function),
            "Expected a symbol whose name contains 'myMethod', got: {:?}",
            extracted
                .symbols
                .iter()
                .map(|s| &s.name)
                .collect::<Vec<_>>()
        );

        // The in-class declaration should now be extracted with the prefixed name.
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "MyClass.myMethod"),
            "Expected MyClass.myMethod from declaration, got: {:?}",
            extracted
                .symbols
                .iter()
                .map(|s| &s.name)
                .collect::<Vec<_>>()
        );

        // Imports
        assert!(extracted.imports.iter().any(|i| i.name == "iostream"));
        assert!(extracted.imports.iter().any(|i| i.name == "myheader.h"));
    }

    #[test]
    fn test_cpp_line_numbers_1_indexed() {
        let source = "class Foo {};\n";
        let extracted = extract_cpp_symbols(source).unwrap();
        let foo = extracted.symbols.iter().find(|s| s.name == "Foo").unwrap();
        assert_eq!(
            foo.lines.start, 1,
            "Expected line 1, got {}",
            foo.lines.start
        );
    }

    #[test]
    fn test_cpp_method_prefixing() {
        let source = r#"
class Server {
public:
    void start() {}
private:
    void stop() {}
};
"#;
        let extracted = extract_cpp_symbols(source).unwrap();
        assert!(
            extracted.symbols.iter().any(|s| s.name == "Server.start"),
            "Expected Server.start, got: {:?}",
            extracted
                .symbols
                .iter()
                .map(|s| &s.name)
                .collect::<Vec<_>>()
        );
        assert!(
            extracted.symbols.iter().any(|s| s.name == "Server.stop"),
            "Expected Server.stop"
        );

        // Visibility check.
        let start = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Server.start")
            .unwrap();
        assert!(start.exported, "public method should be exported");

        let stop = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Server.stop")
            .unwrap();
        assert!(!stop.exported, "private method should not be exported");
    }

    #[test]
    fn test_cpp_method_declarations() {
        let source = r#"
class Widget {
public:
    void render();
    int width();
};
"#;
        let extracted = extract_cpp_symbols(source).unwrap();
        assert!(
            extracted.symbols.iter().any(|s| s.name == "Widget.render"),
            "Expected Widget.render declaration, got: {:?}",
            extracted
                .symbols
                .iter()
                .map(|s| &s.name)
                .collect::<Vec<_>>()
        );
        assert!(
            extracted.symbols.iter().any(|s| s.name == "Widget.width"),
            "Expected Widget.width declaration"
        );
    }

    #[test]
    fn test_cpp_struct_default_public() {
        let source = r#"
struct Point {
    void scale() {}
};
"#;
        let extracted = extract_cpp_symbols(source).unwrap();
        let scale = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Point.scale")
            .unwrap_or_else(|| {
                panic!(
                    "Expected Point.scale, got: {:?}",
                    extracted
                        .symbols
                        .iter()
                        .map(|s| &s.name)
                        .collect::<Vec<_>>()
                )
            });
        assert!(scale.exported, "struct members are public by default");
    }

    #[test]
    fn test_cpp_type_edges() {
        let source = r#"
class Animal {
public:
    virtual void speak() = 0;
};

class Dog : public Animal {
public:
    void fetch(Ball *ball) {}
};
"#;
        let extracted = extract_cpp_symbols(source).unwrap();

        // Inheritance
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Dog" && e.1 == "Animal"),
            "Expected Dog->Animal inheritance edge, got: {:?}",
            extracted.type_edges
        );

        // Method param types
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Dog.fetch" && e.1 == "Ball"),
            "Expected Dog.fetch->Ball type edge, got: {:?}",
            extracted.type_edges
        );
    }

    #[test]
    fn test_cpp_type_edges_multiple_inheritance() {
        let source = r#"
class A {};
class B {};
class C : public A, public B {};
"#;
        let extracted = extract_cpp_symbols(source).unwrap();
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "C" && e.1 == "A"),
            "Expected C->A, got: {:?}",
            extracted.type_edges
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "C" && e.1 == "B"),
            "Expected C->B, got: {:?}",
            extracted.type_edges
        );
    }
}

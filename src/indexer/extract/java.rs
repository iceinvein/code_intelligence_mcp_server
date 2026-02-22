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
    let mut type_edges: Vec<(String, String)> = Vec::new();

    walk(cursor, &mut |node| match node.kind() {
        "class_declaration" => {
            if let Some(name) = symbol_name(node, source) {
                let exported = is_public(node);
                symbols.push(symbol_from_node(name.clone(), SymbolKind::Class, exported, node));

                // extends — superclass field wraps `extends <type>` as a child sequence;
                // the type is NOT a named sub-field, so we iterate children of the
                // superclass node to find the type node.
                if let Some(superclass) = node.child_by_field_name("superclass") {
                    let mut sc_cursor = superclass.walk();
                    for child in superclass.children(&mut sc_cursor) {
                        if let Some(type_name) = extract_java_type_name(child, source) {
                            type_edges.push((name.clone(), type_name));
                        }
                    }
                }

                // implements — interfaces field points to super_interfaces which contains
                // `implements type_list`; type_list children are the actual type nodes.
                if let Some(interfaces) = node.child_by_field_name("interfaces") {
                    let mut iface_cursor = interfaces.walk();
                    for child in interfaces.children(&mut iface_cursor) {
                        // type_list is an intermediate wrapper; descend one more level
                        if child.kind() == "type_list" {
                            let mut tl_cursor = child.walk();
                            for type_node in child.children(&mut tl_cursor) {
                                if let Some(type_name) = extract_java_type_name(type_node, source) {
                                    type_edges.push((name.clone(), type_name));
                                }
                            }
                        } else if let Some(type_name) = extract_java_type_name(child, source) {
                            type_edges.push((name.clone(), type_name));
                        }
                    }
                }

                // Walk class body for methods, constructors, and field declarations
                if let Some(body) = node.child_by_field_name("body") {
                    let mut body_cursor = body.walk();
                    for child in body.children(&mut body_cursor) {
                        if child.kind() == "method_declaration" {
                            if let Some(method_name) = symbol_name(child, source) {
                                let prefixed = format!("{name}.{method_name}");
                                symbols.push(symbol_from_node(
                                    prefixed.clone(),
                                    SymbolKind::Function,
                                    is_public(child),
                                    child,
                                ));
                                extract_method_type_edges(child, source, &prefixed, &mut type_edges);
                            }
                        }
                        if child.kind() == "constructor_declaration" {
                            if let Some(ctor_name) = symbol_name(child, source) {
                                let prefixed = format!("{name}.{ctor_name}");
                                symbols.push(symbol_from_node(
                                    prefixed.clone(),
                                    SymbolKind::Function,
                                    is_public(child),
                                    child,
                                ));
                                extract_method_type_edges(child, source, &prefixed, &mut type_edges);
                            }
                        }
                        if child.kind() == "field_declaration" {
                            if let Some(type_node) = child.child_by_field_name("type") {
                                if let Some(type_name) = extract_java_type_name(type_node, source) {
                                    type_edges.push((name.clone(), type_name));
                                }
                            }
                        }
                    }
                }
            }
        }
        "interface_declaration" => {
            if let Some(name) = symbol_name(node, source) {
                let exported = is_public(node);
                symbols.push(symbol_from_node(
                    name.clone(),
                    SymbolKind::Interface,
                    exported,
                    node,
                ));

                // Interface extends — `extends_interfaces` is an unnamed child node of
                // kind "extends_interfaces" containing `extends type_list`.
                let mut node_cursor = node.walk();
                for child in node.children(&mut node_cursor) {
                    if child.kind() == "extends_interfaces" {
                        let mut ext_cursor = child.walk();
                        for ext_child in child.children(&mut ext_cursor) {
                            if ext_child.kind() == "type_list" {
                                let mut tl_cursor = ext_child.walk();
                                for type_node in ext_child.children(&mut tl_cursor) {
                                    if let Some(type_name) = extract_java_type_name(type_node, source) {
                                        type_edges.push((name.clone(), type_name));
                                    }
                                }
                            } else if let Some(type_name) = extract_java_type_name(ext_child, source) {
                                type_edges.push((name.clone(), type_name));
                            }
                        }
                    }
                }

                // Walk interface body for methods (all implicitly public)
                if let Some(body) = node.child_by_field_name("body") {
                    let mut body_cursor = body.walk();
                    for child in body.children(&mut body_cursor) {
                        if child.kind() == "method_declaration" {
                            if let Some(method_name) = symbol_name(child, source) {
                                let prefixed = format!("{name}.{method_name}");
                                symbols.push(symbol_from_node(
                                    prefixed.clone(),
                                    SymbolKind::Function,
                                    is_public(child),
                                    child,
                                ));
                                extract_method_type_edges(child, source, &prefixed, &mut type_edges);
                            }
                        }
                    }
                }
            }
        }
        "enum_declaration" => {
            if let Some(name) = symbol_name(node, source) {
                let exported = is_public(node);
                symbols.push(symbol_from_node(name.clone(), SymbolKind::Enum, exported, node));

                // Walk enum body for method declarations inside enum_body_declarations
                if let Some(body) = node.child_by_field_name("body") {
                    let mut body_cursor = body.walk();
                    for child in body.children(&mut body_cursor) {
                        if child.kind() == "enum_body_declarations" {
                            let mut decl_cursor = child.walk();
                            for decl in child.children(&mut decl_cursor) {
                                if decl.kind() == "method_declaration" {
                                    if let Some(method_name) = symbol_name(decl, source) {
                                        let prefixed = format!("{name}.{method_name}");
                                        symbols.push(symbol_from_node(
                                            prefixed,
                                            SymbolKind::Function,
                                            is_public(decl),
                                            decl,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        "method_declaration" => {
            // Skip methods inside classes/interfaces/enums — handled by those arms.
            // Only extract standalone methods (rare in Java, but guard for completeness).
            if let Some(parent) = node.parent() {
                if matches!(
                    parent.kind(),
                    "class_body" | "interface_body" | "enum_body" | "enum_body_declarations"
                ) {
                    return;
                }
            }
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
        type_edges,
        dataflow_edges: Vec::new(),
        todos: Vec::new(),
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns: Vec::new(),
    })
}

/// Extract the base type name from a Java type node, stripping generics and array
/// brackets. Returns `None` for primitive types (`void`, `int`, `long`, etc.).
fn extract_java_type_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "type_identifier" => {
            let name = node.utf8_text(source.as_bytes()).unwrap_or("").to_string();
            if name.is_empty() {
                None
            } else {
                Some(name)
            }
        }
        "generic_type" => {
            // `List<User>` — first child is the base type (`type_identifier`)
            node.child(0).and_then(|n| extract_java_type_name(n, source))
        }
        "array_type" => {
            // `User[]` — element type is under field "element" or is the first child
            node.child_by_field_name("element")
                .or_else(|| node.child(0))
                .and_then(|n| extract_java_type_name(n, source))
        }
        "scoped_type_identifier" => {
            // `java.io.InputStream` — take the trailing "name" field (the simple name)
            node.child_by_field_name("name")
                .and_then(|n| extract_java_type_name(n, source))
        }
        // Skip Java primitive and void types
        "void_type" | "integral_type" | "floating_point_type" | "boolean_type" => None,
        _ => None,
    }
}

/// Extract type edges from a method or constructor declaration's signature:
/// parameter types and the return type.
///
/// The `method_name` should already be the fully prefixed name (e.g. `"MyClass.findById"`).
fn extract_method_type_edges(
    node: Node,
    source: &str,
    method_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    // Parameter types
    if let Some(params) = node.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for child in params.children(&mut cursor) {
            if child.kind() == "formal_parameter" || child.kind() == "spread_parameter" {
                if let Some(type_node) = child.child_by_field_name("type") {
                    if let Some(type_name) = extract_java_type_name(type_node, source) {
                        type_edges.push((method_name.to_string(), type_name));
                    }
                }
            }
        }
    }

    // Return type — the "type" field is defined in the inlined `_method_header` rule
    // and is accessible directly on the `method_declaration` node.
    // `constructor_declaration` has no return type, so this is a no-op for them.
    if let Some(return_type) = node.child_by_field_name("type") {
        if let Some(type_name) = extract_java_type_name(return_type, source) {
            type_edges.push((method_name.to_string(), type_name));
        }
    }
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
    // Interface methods are implicitly public even without an explicit `public` modifier.
    if node.kind() == "method_declaration" {
        if let Some(parent) = node.parent() {
            if parent.kind() == "interface_body" {
                return true;
            }
        }
    }
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
            start: start.row as u32 + 1,
            end: end.row as u32 + 1,
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

            let last_part = name.split('.').next_back().unwrap_or(&name).to_string();

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

        // Symbols: MyClass, MyClass.myMethod, MyClass.internalMethod,
        //          MyInterface, MyInterface.doSomething, Color
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
            .find(|s| s.name == "MyClass.myMethod")
            .unwrap();
        assert_eq!(method.kind, SymbolKind::Function);
        assert!(method.exported);

        let internal = extracted
            .symbols
            .iter()
            .find(|s| s.name == "MyClass.internalMethod")
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

        // Standalone (unprefixed) method names must NOT appear
        assert!(
            !extracted.symbols.iter().any(|s| s.name == "myMethod"),
            "myMethod should be prefixed as MyClass.myMethod"
        );
        assert!(
            !extracted.symbols.iter().any(|s| s.name == "internalMethod"),
            "internalMethod should be prefixed as MyClass.internalMethod"
        );
        assert!(
            !extracted.symbols.iter().any(|s| s.name == "doSomething"),
            "doSomething should be prefixed as MyInterface.doSomething"
        );

        // Imports
        assert_eq!(extracted.imports.len(), 1);
        assert_eq!(extracted.imports[0].name, "List");
        assert_eq!(extracted.imports[0].source, "java.util.List");
    }

    #[test]
    fn test_java_line_numbers_1_indexed() {
        let source = "public class Foo {\n    public void bar() {}\n}\n";
        let extracted = extract_java_symbols(source).unwrap();
        let foo = extracted.symbols.iter().find(|s| s.name == "Foo").unwrap();
        assert_eq!(foo.lines.start, 1, "Expected line 1, got {}", foo.lines.start);
    }

    #[test]
    fn test_interface_methods_exported() {
        let source =
            "public interface Service {\n    void process();\n    String getName();\n}\n";
        let extracted = extract_java_symbols(source).unwrap();
        let process = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Service.process")
            .unwrap();
        assert!(process.exported, "Interface methods are implicitly public");
        let get_name = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Service.getName")
            .unwrap();
        assert!(get_name.exported, "Interface methods are implicitly public");
    }

    #[test]
    fn test_java_method_prefixing() {
        let source = r#"
public class UserService {
    public void save() {}
    private void validate() {}
}
"#;
        let extracted = extract_java_symbols(source).unwrap();
        assert!(
            extracted.symbols.iter().any(|s| s.name == "UserService.save"),
            "Expected UserService.save, got: {:?}",
            extracted
                .symbols
                .iter()
                .map(|s| &s.name)
                .collect::<Vec<_>>()
        );
        assert!(
            extracted.symbols.iter().any(|s| s.name == "UserService.validate"),
            "Expected UserService.validate"
        );
        // Standalone names should NOT exist
        assert!(
            !extracted.symbols.iter().any(|s| s.name == "save"),
            "save should be prefixed"
        );
        assert!(
            !extracted.symbols.iter().any(|s| s.name == "validate"),
            "validate should be prefixed"
        );
    }

    #[test]
    fn test_java_constructors() {
        let source = r#"
public class User {
    public User(String name) {}
    private User() {}
}
"#;
        let extracted = extract_java_symbols(source).unwrap();
        // Both constructors should be present as User.User
        let ctors: Vec<_> = extracted
            .symbols
            .iter()
            .filter(|s| s.name == "User.User" && s.kind == SymbolKind::Function)
            .collect();
        assert!(
            !ctors.is_empty(),
            "Expected User.User constructor, got: {:?}",
            extracted
                .symbols
                .iter()
                .map(|s| &s.name)
                .collect::<Vec<_>>()
        );
        // At least one constructor should be public
        let public_ctor = ctors.iter().any(|s| s.exported);
        assert!(public_ctor, "At least one constructor should be public");
    }

    #[test]
    fn test_java_interface_method_prefixing() {
        let source = r#"
public interface Repository {
    void save(Object entity);
    Object findById(Long id);
}
"#;
        let extracted = extract_java_symbols(source).unwrap();
        assert!(
            extracted.symbols.iter().any(|s| s.name == "Repository.save"),
            "Expected Repository.save"
        );
        assert!(
            extracted.symbols.iter().any(|s| s.name == "Repository.findById"),
            "Expected Repository.findById"
        );
    }

    #[test]
    fn test_java_enum_methods() {
        let source = r#"
public enum Status {
    ACTIVE, INACTIVE;

    public String display() {
        return name().toLowerCase();
    }
}
"#;
        let extracted = extract_java_symbols(source).unwrap();
        assert!(
            extracted.symbols.iter().any(|s| s.name == "Status"),
            "Expected Status enum"
        );
        assert!(
            extracted.symbols.iter().any(|s| s.name == "Status.display"),
            "Expected Status.display method, got: {:?}",
            extracted
                .symbols
                .iter()
                .map(|s| &s.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_java_type_edges() {
        let source = r#"
public class UserService extends BaseService implements Serializable {
    private UserRepository repo;
    public User findById(Long id) { return null; }
}
"#;
        let extracted = extract_java_symbols(source).unwrap();

        // extends
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "UserService" && e.1 == "BaseService"),
            "Expected type edge UserService->BaseService, got: {:?}",
            extracted.type_edges
        );
        // implements
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "UserService" && e.1 == "Serializable"),
            "Expected type edge UserService->Serializable, got: {:?}",
            extracted.type_edges
        );
        // Method return type
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "UserService.findById" && e.1 == "User"),
            "Expected type edge UserService.findById->User, got: {:?}",
            extracted.type_edges
        );
        // Method parameter type
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "UserService.findById" && e.1 == "Long"),
            "Expected type edge UserService.findById->Long, got: {:?}",
            extracted.type_edges
        );
        // Field type
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "UserService" && e.1 == "UserRepository"),
            "Expected type edge UserService->UserRepository, got: {:?}",
            extracted.type_edges
        );
    }

    #[test]
    fn test_java_type_edges_interface_extends() {
        let source = r#"
public interface ReadableRepository extends Repository, Closeable {
    Object findById(Long id);
}
"#;
        let extracted = extract_java_symbols(source).unwrap();

        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "ReadableRepository" && e.1 == "Repository"),
            "Expected type edge ReadableRepository->Repository, got: {:?}",
            extracted.type_edges
        );
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "ReadableRepository" && e.1 == "Closeable"),
            "Expected type edge ReadableRepository->Closeable, got: {:?}",
            extracted.type_edges
        );
    }

    #[test]
    fn test_java_type_edges_primitives_skipped() {
        let source = r#"
public class Calculator {
    private int count;
    public double compute(int x, boolean flag) { return 0.0; }
}
"#;
        let extracted = extract_java_symbols(source).unwrap();

        // Primitive types (int, double, boolean) must not appear in type_edges
        let primitives = ["int", "double", "boolean", "float", "long", "char", "byte", "short"];
        for primitive in primitives {
            assert!(
                !extracted.type_edges.iter().any(|e| e.1 == primitive),
                "Primitive type '{}' should not appear in type_edges, got: {:?}",
                primitive,
                extracted.type_edges
            );
        }
    }

    #[test]
    fn test_java_type_edges_generics() {
        let source = r#"
public class OrderService {
    private List<Order> orders;
    public Optional<User> findUser(Map<String, Long> params) { return null; }
}
"#;
        let extracted = extract_java_symbols(source).unwrap();

        // Generic base types should be extracted (stripped of type parameters)
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "OrderService" && e.1 == "List"),
            "Expected type edge OrderService->List (from List<Order> field), got: {:?}",
            extracted.type_edges
        );
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "OrderService.findUser" && e.1 == "Optional"),
            "Expected type edge OrderService.findUser->Optional (return type), got: {:?}",
            extracted.type_edges
        );
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "OrderService.findUser" && e.1 == "Map"),
            "Expected type edge OrderService.findUser->Map (param type), got: {:?}",
            extracted.type_edges
        );
    }
}

use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{
    ByteSpan, DataFlowEdge, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind,
};

/// Extract symbols from Swift source code.
///
/// Handles: classes, structs, enums, protocols (interfaces), extensions,
/// free functions, methods, `import` declarations, and
/// public/private/internal/open visibility modifiers.
///
/// Note: In the tree-sitter-swift grammar, `class`, `struct`, and `enum`
/// declarations all use the `class_declaration` node kind.  The actual Swift
/// keyword (`class`, `struct`, `enum`, `extension`) is available as a child
/// token and is used to determine the `SymbolKind`.
///
/// # Errors
///
/// Returns an error if the source cannot be parsed.
///
/// # Examples
///
/// ```
/// use code_intelligence_mcp_server::indexer::extract::swift::extract_swift_symbols;
/// let src = "class Greeter { func greet() {} }";
/// let file = extract_swift_symbols(src).unwrap();
/// assert!(file.symbols.iter().any(|s| s.name == "Greeter"));
/// ```
pub fn extract_swift_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::Swift)?;
    extract_symbols_with_parser(&mut parser, source)
}

fn extract_symbols_with_parser(parser: &mut Parser, source: &str) -> Result<ExtractedFile> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse Swift source"))?;
    let root = tree.root_node();

    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut type_edges: Vec<(String, String)> = Vec::new();

    walk(root.walk(), &mut |node| match node.kind() {
        "import_declaration" => {
            extract_import(node, source, &mut imports);
        }
        "class_declaration" => {
            // tree-sitter-swift uses class_declaration for class / struct / enum / extension
            if !node_is_inside_class_body(node) {
                extract_class_declaration(node, source, &mut symbols, &mut type_edges);
            }
        }
        "protocol_declaration" => {
            if !node_is_inside_class_body(node) {
                extract_protocol_declaration(node, source, &mut symbols, &mut type_edges);
            }
        }
        "function_declaration" => {
            // Top-level functions
            if !node_is_inside_class_body(node) {
                extract_function(node, source, None, &mut symbols);
            }
        }
        _ => {}
    });

    symbols.sort_by_key(|s| s.bytes.start);

    let todo_cursor = root.walk();
    let todos = super::comments::extract_todo_from_tree(
        todo_cursor,
        source,
        "",
        &["comment", "multiline_comment"],
    );

    Ok(ExtractedFile {
        symbols,
        imports,
        module_bindings: Vec::new(),
        type_edges,
        extends_edges: Vec::new(),
        dataflow_edges: Vec::<DataFlowEdge>::new(),
        todos,
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Class / struct / enum / extension extraction
// ---------------------------------------------------------------------------

/// Extract a `class_declaration` node, which in tree-sitter-swift also covers
/// `struct`, `enum`, and `extension` declarations.
fn extract_class_declaration(
    node: Node<'_>,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let kind = swift_type_kind(node, source);
    let is_extension = kind == SwiftTypeKind::Extension;

    let name = match type_name_from_declaration(node, source) {
        Some(n) => n,
        None => return,
    };

    let exported = is_exported_node(node, source);

    // For extensions, we don't create a new symbol — the base type already has one.
    // We do however extract the methods inside and add protocol conformance edges.
    if !is_extension {
        let symbol_kind = match kind {
            SwiftTypeKind::Struct => SymbolKind::Struct,
            SwiftTypeKind::Enum => SymbolKind::Enum,
            _ => SymbolKind::Class,
        };
        symbols.push(symbol_from_node(name.clone(), symbol_kind, exported, node));
    }

    // Inheritance / protocol conformance → type edges
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "inheritance_specifier" {
            let type_name = extract_inheritance_type(child, source);
            if !type_name.is_empty() && !is_swift_builtin(&type_name) {
                type_edges.push((name.clone(), type_name));
            }
        }
    }

    // Walk the class_body
    if let Some(body) = node.child_by_field_name("body") {
        extract_class_body(body, &name, source, symbols, type_edges);
    } else {
        // Some variants place the body directly as a child `class_body` without
        // a named field.  Scan children for it.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "class_body" || child.kind() == "enum_class_body" {
                extract_class_body(child, &name, source, symbols, type_edges);
                break;
            }
        }
    }
}

/// Extract a `protocol_declaration` node.
fn extract_protocol_declaration(
    node: Node<'_>,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let name = match type_name_from_declaration(node, source) {
        Some(n) => n,
        None => return,
    };

    let exported = is_exported_node(node, source);
    symbols.push(symbol_from_node(
        name.clone(),
        SymbolKind::Interface,
        exported,
        node,
    ));

    // Protocol inheritance
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "inheritance_specifier" {
            let type_name = extract_inheritance_type(child, source);
            if !type_name.is_empty() && !is_swift_builtin(&type_name) {
                type_edges.push((name.clone(), type_name));
            }
        }
    }

    // Walk protocol_body for required method signatures
    if let Some(body) = node.child_by_field_name("body") {
        extract_protocol_body(body, &name, source, symbols);
    } else {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "protocol_body" {
                extract_protocol_body(child, &name, source, symbols);
                break;
            }
        }
    }
}

fn extract_protocol_body(
    body: Node<'_>,
    parent_name: &str,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "protocol_function_declaration" | "function_declaration" => {
                extract_function(child, source, Some(parent_name), symbols);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Class body extraction
// ---------------------------------------------------------------------------

fn extract_class_body(
    body: Node<'_>,
    parent_name: &str,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "function_declaration" | "protocol_function_declaration" => {
                extract_function(child, source, Some(parent_name), symbols);
            }
            "class_declaration" => {
                // Nested type
                extract_class_declaration(child, source, symbols, type_edges);
            }
            "protocol_declaration" => {
                extract_protocol_declaration(child, source, symbols, type_edges);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Function extraction
// ---------------------------------------------------------------------------

fn extract_function(
    node: Node<'_>,
    source: &str,
    parent_name: Option<&str>,
    symbols: &mut Vec<ExtractedSymbol>,
) {
    let Some(name) = function_name(node, source) else {
        return;
    };

    let qualified = match parent_name {
        Some(parent) => format!("{parent}.{name}"),
        None => name.clone(),
    };

    let exported = is_exported_node(node, source);
    let is_test = is_test_function(node, source);
    let exported = exported && !is_test;

    symbols.push(symbol_from_node(
        qualified,
        SymbolKind::Function,
        exported,
        node,
    ));
}

/// Return `true` if the function has a `@Test`, `@testCase`, or name starts
/// with `test` (Swift XCTest convention).
fn is_test_function(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "attribute" {
            let text = text_for_node(child, source);
            if text.contains("Test") {
                return true;
            }
        }
    }
    // XCTest naming convention: methods starting with "test"
    if let Some(name) = function_name(node, source) {
        if name.starts_with("test") {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Import extraction
// ---------------------------------------------------------------------------

fn extract_import(node: Node<'_>, source: &str, imports: &mut Vec<Import>) {
    // import_declaration: `import` import_kind? identifier (`.` identifier)*
    // The identifier children give the import path components.

    let mut parts: Vec<String> = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                // Recurse into identifier which may contain simple_identifier
                let text = extract_identifier_text(child, source);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
            "simple_identifier" => {
                let text = text_for_node(child, source);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
            _ => {}
        }
    }

    if parts.is_empty() {
        return;
    }

    let source_path = parts.join(".");
    let name = parts.last().cloned().unwrap_or_else(|| source_path.clone());

    imports.push(Import {
        name,
        source: source_path,
        alias: None,
        at_line: node.start_position().row as u32 + 1,
    });
}

/// Extract the text from an `identifier` node, which in tree-sitter-swift may
/// contain `simple_identifier` children.
fn extract_identifier_text(node: Node<'_>, source: &str) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "simple_identifier" {
            let text = text_for_node(child, source);
            if !text.is_empty() {
                return text;
            }
        }
    }
    // Fallback to raw text of the node
    text_for_node(node, source)
}

// ---------------------------------------------------------------------------
// Type / kind helpers
// ---------------------------------------------------------------------------

#[derive(PartialEq)]
enum SwiftTypeKind {
    Class,
    Struct,
    Enum,
    Extension,
}

/// Determine whether a `class_declaration` node represents a class, struct,
/// enum, or extension by inspecting the leading keyword token.
fn swift_type_kind(node: Node<'_>, source: &str) -> SwiftTypeKind {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let text = text_for_node(child, source);
        match text.as_str() {
            "struct" => return SwiftTypeKind::Struct,
            "enum" => return SwiftTypeKind::Enum,
            "extension" => return SwiftTypeKind::Extension,
            _ => {}
        }
    }
    SwiftTypeKind::Class
}

/// Extract the declared type name from a `class_declaration` or
/// `protocol_declaration` node.
///
/// tree-sitter-swift places the name as the first `type_identifier` child (for
/// `class_declaration`) or as an explicit `name` field.
fn type_name_from_declaration(node: Node<'_>, source: &str) -> Option<String> {
    // Try `name` field
    if let Some(n) = node.child_by_field_name("name") {
        let text = text_for_node(n, source);
        if !text.is_empty() {
            return Some(text);
        }
    }
    // First `type_identifier` child
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => {
                let text = text_for_node(child, source);
                if !text.is_empty() {
                    return Some(text);
                }
            }
            "user_type" => {
                // user_type → type_identifier
                let mut uc = child.walk();
                for uc_child in child.children(&mut uc) {
                    if uc_child.kind() == "type_identifier" {
                        let text = text_for_node(uc_child, source);
                        if !text.is_empty() {
                            return Some(text);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract the type name from an `inheritance_specifier` node.
fn extract_inheritance_type(spec: Node<'_>, source: &str) -> String {
    let mut cursor = spec.walk();
    for child in spec.children(&mut cursor) {
        match child.kind() {
            "user_type" => {
                let mut uc = child.walk();
                for uc_child in child.children(&mut uc) {
                    if uc_child.kind() == "type_identifier" {
                        return text_for_node(uc_child, source);
                    }
                }
                // Fallback: strip generics from raw text
                return text_for_node(child, source)
                    .split('<')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
            }
            "type_identifier" => {
                return text_for_node(child, source);
            }
            _ => {}
        }
    }
    // Fallback: strip generics from raw node text
    text_for_node(spec, source)
        .split('<')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Extract the function name from a `function_declaration` or
/// `protocol_function_declaration` node.
fn function_name(node: Node<'_>, source: &str) -> Option<String> {
    // The `name` field is not present in tree-sitter-swift; names appear as
    // `simple_identifier` children.
    if let Some(n) = node.child_by_field_name("name") {
        let text = text_for_node(n, source);
        if !text.is_empty() {
            return Some(text);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "simple_identifier" {
            let text = text_for_node(child, source);
            if !text.is_empty() && text != "func" {
                return Some(text);
            }
        }
    }
    None
}

/// A node is exported unless it carries an explicit `private` or `internal`
/// modifier. In Swift, `public` and `open` both represent exported symbols.
/// The default (no modifier) for type members is `internal` — not exported.
/// Top-level declarations with no modifier are also `internal`, but we treat
/// them as exported for discoverability.
fn is_exported_node(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let text = text_for_node(child, source);
            if text.contains("private") || text.contains("fileprivate") {
                return false;
            }
            if text.contains("public") || text.contains("open") {
                return true;
            }
            // `internal` (explicit or default) → not exported
            if text.contains("internal") {
                return false;
            }
        }
    }
    // No explicit modifier — treat top-level declarations as exported for search
    // discoverability.
    true
}

fn is_swift_builtin(name: &str) -> bool {
    matches!(
        name,
        "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Float"
            | "Double"
            | "Bool"
            | "String"
            | "Character"
            | "Void"
            | "Any"
            | "Never"
            | "Array"
            | "Dictionary"
            | "Set"
            | "Optional"
    )
    // Note: AnyObject and AnyClass are intentionally NOT excluded — protocol
    // conformance to AnyObject (class-only protocol) is a meaningful type edge.
}

fn node_is_inside_class_body(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "class_body" | "enum_class_body" | "protocol_body" => return true,
            "source_file" => return false,
            _ => {}
        }
        current = parent;
    }
    false
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn text_for_node(node: Node<'_>, source: &str) -> String {
    node.utf8_text(source.as_bytes()).unwrap_or("").to_string()
}

fn symbol_from_node(
    name: String,
    kind: SymbolKind,
    exported: bool,
    node: Node<'_>,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_swift_class_and_methods() {
        let source = r#"
import Foundation

class MyClass: BaseClass, MyProtocol {
    func publicMethod() -> Int { return 0 }
    private func privateMethod() {}
    static func staticMethod() {}
}
"#;
        let file = extract_swift_symbols(source).unwrap();

        let class_sym = file.symbols.iter().find(|s| s.name == "MyClass").unwrap();
        assert_eq!(class_sym.kind, SymbolKind::Class);

        // Type edges
        assert!(
            file.type_edges
                .iter()
                .any(|e| e.0 == "MyClass" && e.1 == "BaseClass"),
            "MyClass should inherit BaseClass"
        );
        assert!(
            file.type_edges
                .iter()
                .any(|e| e.0 == "MyClass" && e.1 == "MyProtocol"),
            "MyClass should conform to MyProtocol"
        );

        // Methods
        assert!(
            file.symbols
                .iter()
                .any(|s| s.name == "MyClass.publicMethod"),
            "publicMethod should be extracted"
        );

        let priv_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "MyClass.privateMethod")
            .unwrap();
        assert!(!priv_sym.exported);
    }

    #[test]
    fn test_extract_swift_struct() {
        let source = r#"
struct Point {
    var x: Int
    var y: Int
    func distance() -> Double { return 0.0 }
}
"#;
        let file = extract_swift_symbols(source).unwrap();

        let sym = file.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(sym.kind, SymbolKind::Struct);

        assert!(
            file.symbols.iter().any(|s| s.name == "Point.distance"),
            "Struct method should be extracted"
        );
    }

    #[test]
    fn test_extract_swift_enum() {
        let source = r#"
enum Direction {
    case north, south, east, west
    func opposite() -> Direction { return .north }
}
"#;
        let file = extract_swift_symbols(source).unwrap();

        let sym = file.symbols.iter().find(|s| s.name == "Direction").unwrap();
        assert_eq!(sym.kind, SymbolKind::Enum);

        assert!(
            file.symbols.iter().any(|s| s.name == "Direction.opposite"),
            "Enum method should be extracted"
        );
    }

    #[test]
    fn test_extract_swift_protocol() {
        let source = r#"
protocol Drawable: AnyObject {
    func draw()
    func resize(factor: Double)
}
"#;
        let file = extract_swift_symbols(source).unwrap();

        let sym = file.symbols.iter().find(|s| s.name == "Drawable").unwrap();
        assert_eq!(sym.kind, SymbolKind::Interface);

        // Protocol inheritance edge
        assert!(
            file.type_edges
                .iter()
                .any(|e| e.0 == "Drawable" && e.1 == "AnyObject"),
            "Drawable should have edge to AnyObject"
        );

        // Protocol methods
        assert!(
            file.symbols.iter().any(|s| s.name == "Drawable.draw"),
            "Protocol method 'draw' should be extracted"
        );
        assert!(
            file.symbols.iter().any(|s| s.name == "Drawable.resize"),
            "Protocol method 'resize' should be extracted"
        );
    }

    #[test]
    fn test_extract_swift_extension() {
        let source = r#"
class Widget {}

extension Widget {
    func extended() {}
    func anotherExtension() -> String { return "" }
}
"#;
        let file = extract_swift_symbols(source).unwrap();

        // Widget class symbol
        assert!(
            file.symbols.iter().any(|s| s.name == "Widget"),
            "Widget should be extracted"
        );

        // Extension methods placed under Widget
        assert!(
            file.symbols.iter().any(|s| s.name == "Widget.extended"),
            "Extension method should be extracted under Widget"
        );
        assert!(
            file.symbols
                .iter()
                .any(|s| s.name == "Widget.anotherExtension"),
            "Second extension method should be extracted"
        );
    }

    #[test]
    fn test_extract_swift_imports() {
        let source = r#"
import Foundation
import UIKit
import SwiftUI
"#;
        let file = extract_swift_symbols(source).unwrap();

        assert!(
            file.imports.iter().any(|i| i.source == "Foundation"),
            "Should extract Foundation"
        );
        assert!(
            file.imports.iter().any(|i| i.source == "UIKit"),
            "Should extract UIKit"
        );
        assert!(
            file.imports.iter().any(|i| i.source == "SwiftUI"),
            "Should extract SwiftUI"
        );
    }

    #[test]
    fn test_extract_swift_top_level_function() {
        let source = r#"
func greet(name: String) -> String {
    return "Hello, \(name)!"
}
"#;
        let file = extract_swift_symbols(source).unwrap();

        let sym = file.symbols.iter().find(|s| s.name == "greet").unwrap();
        assert_eq!(sym.kind, SymbolKind::Function);
    }
}

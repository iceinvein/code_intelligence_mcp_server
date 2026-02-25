use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{
    ByteSpan, DataFlowEdge, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind,
};

/// Extract symbols from C# source code.
///
/// Handles: namespaces, classes (with base-type edges), interfaces, structs,
/// enums, methods (instance and static), `using` directives, and
/// public/private/protected/internal visibility modifiers.
///
/// # Errors
///
/// Returns an error if the source cannot be parsed.
///
/// # Examples
///
/// ```
/// let src = "namespace App { public class Greeter { public void Greet() {} } }";
/// let file = extract_csharp_symbols(src).unwrap();
/// assert!(file.symbols.iter().any(|s| s.name == "Greeter"));
/// ```
pub fn extract_csharp_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::CSharp)?;
    extract_symbols_with_parser(&mut parser, source)
}

fn extract_symbols_with_parser(parser: &mut Parser, source: &str) -> Result<ExtractedFile> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse C# source"))?;
    let root = tree.root_node();

    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut type_edges: Vec<(String, String)> = Vec::new();

    walk(root.walk(), &mut |node| match node.kind() {
        "using_directive" => {
            extract_using_directive(node, source, &mut imports);
        }
        "namespace_declaration" | "file_scoped_namespace_declaration" => {
            if !node_is_inside_type_declaration(node) {
                extract_namespace(node, source, &mut symbols, &mut type_edges);
            }
        }
        "class_declaration" | "struct_declaration" | "interface_declaration"
        | "enum_declaration" | "record_declaration" => {
            // Only handle top-level type declarations here (not nested inside
            // namespaces or other types — those are handled recursively).
            if !node_is_inside_type_declaration(node) && !node_is_inside_namespace(node) {
                extract_type_declaration(node, source, &mut symbols, &mut type_edges);
            }
        }
        _ => {}
    });

    symbols.sort_by_key(|s| s.bytes.start);

    Ok(ExtractedFile {
        symbols,
        imports,
        type_edges,
        dataflow_edges: Vec::<DataFlowEdge>::new(),
        todos: Vec::new(),
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Namespace extraction
// ---------------------------------------------------------------------------

fn extract_namespace(
    node: Node<'_>,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let Some(name) = symbol_name(node, source) else {
        return;
    };
    symbols.push(symbol_from_node(name.clone(), SymbolKind::Module, true, node));

    // Walk the declaration_list inside the namespace
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "declaration_list" {
            extract_declaration_list(child, source, symbols, type_edges);
        }
    }
}

fn extract_declaration_list(
    decl_list: Node<'_>,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let mut cursor = decl_list.walk();
    for child in decl_list.children(&mut cursor) {
        match child.kind() {
            "class_declaration"
            | "struct_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration" => {
                extract_type_declaration(child, source, symbols, type_edges);
            }
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                extract_namespace(child, source, symbols, type_edges);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Type declaration extraction (class, struct, interface, enum, record)
// ---------------------------------------------------------------------------

fn extract_type_declaration(
    node: Node<'_>,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let Some(name) = symbol_name(node, source) else {
        return;
    };

    let kind = match node.kind() {
        "interface_declaration" => SymbolKind::Interface,
        "enum_declaration" => SymbolKind::Enum,
        "struct_declaration" => SymbolKind::Struct,
        _ => SymbolKind::Class, // class_declaration or record_declaration
    };

    let exported = is_exported_node(node, source);
    symbols.push(symbol_from_node(name.clone(), kind, exported, node));

    // Base list (superclass / interfaces) → type edges
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "base_list" {
            extract_base_list(child, source, &name, type_edges);
        }
    }

    // Walk the declaration_list body for methods and nested types
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "declaration_list" {
            extract_member_declaration_list(child, source, &name, exported, symbols, type_edges);
        }
    }
}

fn extract_base_list(
    base_list: Node<'_>,
    source: &str,
    type_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    let mut cursor = base_list.walk();
    for child in base_list.children(&mut cursor) {
        match child.kind() {
            "identifier" | "qualified_name" => {
                let base_name = text_for_node(child, source)
                    .split('<')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !base_name.is_empty() && !is_csharp_builtin(&base_name) {
                    type_edges.push((type_name.to_string(), base_name));
                }
            }
            "generic_name" => {
                // generic_name: identifier type_argument_list
                if let Some(id_node) = child.child_by_field_name("name") {
                    let base_name = text_for_node(id_node, source);
                    if !base_name.is_empty() && !is_csharp_builtin(&base_name) {
                        type_edges.push((type_name.to_string(), base_name));
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Member declaration extraction
// ---------------------------------------------------------------------------

fn extract_member_declaration_list(
    decl_list: Node<'_>,
    source: &str,
    parent_name: &str,
    _parent_exported: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let mut cursor = decl_list.walk();
    for child in decl_list.children(&mut cursor) {
        match child.kind() {
            "method_declaration" | "constructor_declaration" => {
                extract_method(child, source, parent_name, symbols);
            }
            "class_declaration"
            | "struct_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration" => {
                // Nested type
                extract_type_declaration(child, source, symbols, type_edges);
            }
            _ => {}
        }
    }
}

fn extract_method(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    symbols: &mut Vec<ExtractedSymbol>,
) {
    let Some(name) = symbol_name(node, source) else {
        return;
    };

    let qualified = format!("{parent_name}.{name}");
    let exported = is_exported_node(node, source);
    let is_test = is_test_method(node, source);
    let exported = exported && !is_test;

    symbols.push(symbol_from_node(qualified, SymbolKind::Function, exported, node));
}

/// Return `true` if the method has a `[Test]`, `[Fact]`, or `[TestMethod]` attribute.
fn is_test_method(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "attribute_list" {
            let text = text_for_node(child, source);
            if text.contains("Test")
                || text.contains("Fact")
                || text.contains("Theory")
                || text.contains("TestMethod")
            {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Using directive extraction
// ---------------------------------------------------------------------------

fn extract_using_directive(node: Node<'_>, source: &str, imports: &mut Vec<Import>) {
    // using_directive: `using` (static)? identifier_or_qualified_name (= alias)?
    let mut qname = String::new();
    let mut alias: Option<String> = None;
    let mut is_alias = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                let text = text_for_node(child, source);
                if text == "static" || text == "using" {
                    continue;
                }
                if is_alias {
                    alias = Some(text);
                } else {
                    qname = text;
                }
            }
            "qualified_name" => {
                let text = text_for_node(child, source);
                if qname.is_empty() {
                    qname = text;
                }
            }
            "name_equals" => {
                // The LHS of `Alias = Namespace.Type`
                if let Some(n) = child.child_by_field_name("name") {
                    qname = text_for_node(n, source);
                }
                is_alias = true;
            }
            _ => {}
        }
    }

    if qname.is_empty() {
        return;
    }

    let name = qname
        .split('.')
        .next_back()
        .unwrap_or(&qname)
        .to_string();

    imports.push(Import {
        name: alias.clone().unwrap_or_else(|| name),
        source: qname,
        alias,
    });
}

// ---------------------------------------------------------------------------
// Visibility helpers
// ---------------------------------------------------------------------------

/// A C# member is exported if it has `public` modifier (or no explicit modifier
/// in an interface context, where members are implicitly public).
fn is_exported_node(node: Node<'_>, source: &str) -> bool {
    let mut has_modifier = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier" {
            has_modifier = true;
            let text = text_for_node(child, source);
            if text == "public" {
                return true;
            }
        }
    }
    // If no modifier, assume package-private (internal) — not exported
    // Exception: types directly inside an interface are implicitly public
    !has_modifier && node_is_inside_interface(node)
}

fn node_is_inside_interface(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "interface_declaration" => return true,
            "class_declaration"
            | "struct_declaration"
            | "enum_declaration"
            | "record_declaration" => return false,
            _ => {}
        }
        current = parent;
    }
    false
}

fn node_is_inside_type_declaration(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "declaration_list" => {
                if let Some(gp) = parent.parent() {
                    match gp.kind() {
                        "class_declaration"
                        | "struct_declaration"
                        | "interface_declaration"
                        | "enum_declaration"
                        | "record_declaration" => return true,
                        _ => {}
                    }
                }
            }
            "compilation_unit" => return false,
            _ => {}
        }
        current = parent;
    }
    false
}

fn node_is_inside_namespace(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "namespace_declaration" | "file_scoped_namespace_declaration" => return true,
            "compilation_unit" => return false,
            _ => {}
        }
        current = parent;
    }
    false
}

fn is_csharp_builtin(name: &str) -> bool {
    matches!(
        name,
        "object"
            | "string"
            | "int"
            | "long"
            | "float"
            | "double"
            | "bool"
            | "byte"
            | "char"
            | "void"
            | "uint"
            | "ulong"
            | "short"
            | "ushort"
            | "decimal"
            | "sbyte"
            | "Object"
            | "String"
            | "Int32"
            | "Int64"
            | "Boolean"
            | "Char"
            | "Void"
    )
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn text_for_node(node: Node<'_>, source: &str) -> String {
    node.utf8_text(source.as_bytes()).unwrap_or("").to_string()
}

fn symbol_name(node: Node<'_>, source: &str) -> Option<String> {
    // The `name` field is present on all declaration nodes in tree-sitter-c-sharp.
    if let Some(n) = node.child_by_field_name("name") {
        let text = text_for_node(n, source);
        if !text.is_empty() {
            return Some(text);
        }
    }
    // Fallback: first identifier child
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            let text = text_for_node(child, source);
            if !text.is_empty() && text != "namespace" && text != "class" {
                return Some(text);
            }
        }
    }
    None
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
    fn test_extract_csharp_class_and_methods() {
        let source = r#"
using System;
using System.Collections.Generic;

namespace MyApp {
    public class MyClass : BaseClass, IMyInterface {
        public void PublicMethod() {}
        private void PrivateMethod() {}
        protected void ProtectedMethod() {}
        public static void StaticMethod() {}
    }
}
"#;
        let file = extract_csharp_symbols(source).unwrap();

        // Namespace
        assert!(file.symbols.iter().any(|s| s.name == "MyApp" && s.kind == SymbolKind::Module));

        let class_sym = file.symbols.iter().find(|s| s.name == "MyClass").unwrap();
        assert_eq!(class_sym.kind, SymbolKind::Class);
        assert!(class_sym.exported);

        // Base type edges
        assert!(
            file.type_edges
                .iter()
                .any(|e| e.0 == "MyClass" && e.1 == "BaseClass"),
            "MyClass should have edge to BaseClass"
        );
        assert!(
            file.type_edges
                .iter()
                .any(|e| e.0 == "MyClass" && e.1 == "IMyInterface"),
            "MyClass should have edge to IMyInterface"
        );

        // Public method exported
        let pub_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "MyClass.PublicMethod")
            .unwrap();
        assert!(pub_sym.exported);

        // Private method not exported
        let priv_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "MyClass.PrivateMethod")
            .unwrap();
        assert!(!priv_sym.exported);
    }

    #[test]
    fn test_extract_csharp_interface() {
        let source = r#"
namespace App {
    public interface IRepository {
        void Save();
        IEnumerable<string> GetAll();
    }
}
"#;
        let file = extract_csharp_symbols(source).unwrap();

        let iface = file
            .symbols
            .iter()
            .find(|s| s.name == "IRepository")
            .unwrap();
        assert_eq!(iface.kind, SymbolKind::Interface);
        assert!(iface.exported);
    }

    #[test]
    fn test_extract_csharp_struct_and_enum() {
        let source = r#"
namespace App {
    public struct Point {
        public int X;
        public int Y;
    }
    public enum Color { Red, Green, Blue }
}
"#;
        let file = extract_csharp_symbols(source).unwrap();

        let struct_sym = file.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(struct_sym.kind, SymbolKind::Struct);

        let enum_sym = file.symbols.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(enum_sym.kind, SymbolKind::Enum);
    }

    #[test]
    fn test_extract_csharp_using_directives() {
        let source = r#"
using System;
using System.Collections.Generic;
using MyAlias = System.Text.StringBuilder;
"#;
        let file = extract_csharp_symbols(source).unwrap();

        assert!(
            file.imports.iter().any(|i| i.source == "System"),
            "Should extract 'using System'"
        );
        assert!(
            file.imports
                .iter()
                .any(|i| i.source == "System.Collections.Generic"),
            "Should extract 'using System.Collections.Generic'"
        );
    }

    #[test]
    fn test_extract_csharp_constructor() {
        let source = r#"
public class Service {
    public Service(string name) {}
    public void Execute() {}
}
"#;
        let file = extract_csharp_symbols(source).unwrap();

        let svc = file.symbols.iter().find(|s| s.name == "Service").unwrap();
        assert_eq!(svc.kind, SymbolKind::Class);

        // Constructor and method should be extracted
        assert!(
            file.symbols.iter().any(|s| s.name == "Service.Service"),
            "Constructor should be extracted"
        );
        assert!(
            file.symbols.iter().any(|s| s.name == "Service.Execute"),
            "Method should be extracted"
        );
    }

    #[test]
    fn test_extract_csharp_nested_namespace() {
        let source = r#"
namespace Outer.Inner {
    public class Widget {}
}
"#;
        let file = extract_csharp_symbols(source).unwrap();

        // Namespace symbol
        assert!(
            file.symbols.iter().any(|s| s.kind == SymbolKind::Module),
            "Namespace should produce a Module symbol"
        );

        // Widget class
        assert!(
            file.symbols.iter().any(|s| s.name == "Widget"),
            "Widget class should be extracted"
        );
    }
}

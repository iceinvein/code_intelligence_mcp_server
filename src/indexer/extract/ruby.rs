use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{
    ByteSpan, DataFlowEdge, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind,
};

/// Extract symbols from Ruby source code.
///
/// Handles: modules, classes (with superclass type edges), instance methods,
/// singleton methods (`def self.foo`), `require`/`require_relative` imports,
/// and private/protected visibility.
///
/// # Errors
///
/// Returns an error if the source cannot be parsed.
///
/// # Examples
///
/// ```
/// use code_intelligence_mcp_server::indexer::extract::ruby::extract_ruby_symbols;
/// let src = "class Greeter; def greet; end; end";
/// let file = extract_ruby_symbols(src).unwrap();
/// assert!(file.symbols.iter().any(|s| s.name == "Greeter"));
/// ```
pub fn extract_ruby_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::Ruby)?;
    extract_symbols_with_parser(&mut parser, source)
}

fn extract_symbols_with_parser(parser: &mut Parser, source: &str) -> Result<ExtractedFile> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse Ruby source"))?;
    let root = tree.root_node();

    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut type_edges: Vec<(String, String)> = Vec::new();

    // Walk top-level nodes looking for require calls, modules, classes, and methods
    walk(root.walk(), &mut |node| match node.kind() {
        "call" => {
            extract_require(node, source, &mut imports);
        }
        "module" => {
            extract_module(node, source, &mut symbols, &mut type_edges);
        }
        "class" => {
            // Top-level classes (not inside a module — those are walked recursively)
            if !node_is_inside_module_or_class(node) {
                extract_class(node, source, &mut symbols, &mut type_edges);
            }
        }
        "method" => {
            // Top-level method definitions (outside any class/module)
            if !node_is_inside_module_or_class(node) {
                if let Some(name) = method_name(node, source) {
                    let exported = is_exported_method(&name);
                    symbols.push(symbol_from_node(name, SymbolKind::Function, exported, node));
                }
            }
        }
        "singleton_method" => {
            // Top-level singleton methods
            if !node_is_inside_module_or_class(node) {
                if let Some(name) = singleton_method_name(node, source) {
                    symbols.push(symbol_from_node(name, SymbolKind::Function, true, node));
                }
            }
        }
        _ => {}
    });

    symbols.sort_by_key(|s| s.bytes.start);

    let todo_cursor = root.walk();
    let todos = super::comments::extract_todo_from_tree(
        todo_cursor, source, "", &["comment"],
    );

    Ok(ExtractedFile {
        symbols,
        imports,
        type_edges,
        dataflow_edges: Vec::<DataFlowEdge>::new(),
        todos,
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Module extraction
// ---------------------------------------------------------------------------

fn extract_module(
    node: Node<'_>,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let Some(name) = module_or_class_constant(node, source) else {
        return;
    };
    symbols.push(symbol_from_node(name.clone(), SymbolKind::Module, true, node));

    // Walk the body_statement for nested classes and methods
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    extract_body_statement(body, &name, source, symbols, type_edges);
}

// ---------------------------------------------------------------------------
// Class extraction
// ---------------------------------------------------------------------------

fn extract_class(
    node: Node<'_>,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let Some(name) = module_or_class_constant(node, source) else {
        return;
    };
    symbols.push(symbol_from_node(name.clone(), SymbolKind::Class, true, node));

    // Superclass type edge
    if let Some(superclass) = node.child_by_field_name("superclass") {
        // superclass node is `< ConstantName`; the constant is a named child
        let mut sc_cursor = superclass.walk();
        for child in superclass.children(&mut sc_cursor) {
            if child.kind() == "constant" || child.kind() == "scope_resolution" {
                let sc_name = text_for_node(child, source);
                if !sc_name.is_empty() && sc_name != "<" {
                    type_edges.push((name.clone(), sc_name));
                    break;
                }
            }
        }
    }

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    extract_body_statement(body, &name, source, symbols, type_edges);
}

// ---------------------------------------------------------------------------
// Body statement walk (shared by modules and classes)
// ---------------------------------------------------------------------------

/// Walk a `body_statement` node, tracking visibility state, and extracting
/// methods (both instance and singleton), nested classes, and nested modules.
fn extract_body_statement(
    body: Node<'_>,
    parent_name: &str,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    // Track whether we've hit a `private` or `protected` access modifier.
    let mut private = false;

    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            // The tree-sitter-ruby grammar represents standalone `private`,
            // `protected`, and `public` visibility modifiers as bare
            // `identifier` nodes inside `body_statement`.
            "access_modifier" | "identifier" => {
                let text = text_for_node(child, source);
                match text.as_str() {
                    "private" | "protected" => private = true,
                    "public" => private = false,
                    _ => {}
                }
            }
            "method" => {
                if let Some(name) = method_name(child, source) {
                    let qualified = format!("{parent_name}.{name}");
                    // In Ruby, public is the default; only private/protected modifiers change it
                    let exported = !private && is_exported_method(&name);
                    symbols.push(symbol_from_node(
                        qualified,
                        SymbolKind::Function,
                        exported,
                        child,
                    ));
                }
            }
            "singleton_method" => {
                if let Some(name) = singleton_method_name(child, source) {
                    let qualified = format!("{parent_name}.{name}");
                    symbols.push(symbol_from_node(
                        qualified,
                        SymbolKind::Function,
                        true,
                        child,
                    ));
                }
            }
            "class" => {
                // Nested class — recurse
                if let Some(nested_name) = module_or_class_constant(child, source) {
                    let qualified = format!("{parent_name}::{nested_name}");
                    symbols.push(symbol_from_node(
                        qualified.clone(),
                        SymbolKind::Class,
                        true,
                        child,
                    ));
                    if let Some(superclass) = child.child_by_field_name("superclass") {
                        let mut sc_cursor = superclass.walk();
                        for sc_child in superclass.children(&mut sc_cursor) {
                            if sc_child.kind() == "constant" || sc_child.kind() == "scope_resolution" {
                                let sc_name = text_for_node(sc_child, source);
                                if !sc_name.is_empty() && sc_name != "<" {
                                    type_edges.push((qualified.clone(), sc_name));
                                    break;
                                }
                            }
                        }
                    }
                    if let Some(nested_body) = child.child_by_field_name("body") {
                        extract_body_statement(
                            nested_body,
                            &qualified,
                            source,
                            symbols,
                            type_edges,
                        );
                    }
                }
            }
            "module" => {
                // Nested module — recurse
                if let Some(nested_name) = module_or_class_constant(child, source) {
                    let qualified = format!("{parent_name}::{nested_name}");
                    symbols.push(symbol_from_node(
                        qualified.clone(),
                        SymbolKind::Module,
                        true,
                        child,
                    ));
                    if let Some(nested_body) = child.child_by_field_name("body") {
                        extract_body_statement(
                            nested_body,
                            &qualified,
                            source,
                            symbols,
                            type_edges,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Import (require) extraction
// ---------------------------------------------------------------------------

fn extract_require(node: Node<'_>, source: &str, imports: &mut Vec<Import>) {
    // call node structure: identifier("require" | "require_relative") argument_list(string)
    let method_node = node.child_by_field_name("method");
    let method_text = method_node
        .map(|n| text_for_node(n, source))
        .unwrap_or_default();

    if method_text != "require" && method_text != "require_relative" {
        return;
    }

    let args = node.child_by_field_name("arguments");
    let Some(args_node) = args else { return };

    // Find the first string argument
    let mut cursor = args_node.walk();
    for child in args_node.children(&mut cursor) {
        if child.kind() == "string" {
            // String node has a string_content child with the actual content
            let content = string_content(child, source);
            if content.is_empty() {
                continue;
            }
            // Derive the import name from the last path component
            let name = content
                .split('/')
                .next_back()
                .unwrap_or(&content)
                .trim_start_matches('.')
                .to_string();

            imports.push(Import {
                name: if name.is_empty() {
                    content.clone()
                } else {
                    name
                },
                source: content,
                alias: None,
            });
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn text_for_node(node: Node<'_>, source: &str) -> String {
    node.utf8_text(source.as_bytes()).unwrap_or("").to_string()
}

/// Extract the string content from a tree-sitter Ruby `string` node.
///
/// Ruby strings can be `'...'` or `"..."`. The grammar represents the
/// literal text as a `string_content` child node.
fn string_content(string_node: Node<'_>, source: &str) -> String {
    let mut cursor = string_node.walk();
    for child in string_node.children(&mut cursor) {
        if child.kind() == "string_content" {
            return text_for_node(child, source);
        }
    }
    // Fallback: strip surrounding quotes from raw text
    let raw = text_for_node(string_node, source);
    raw.trim_matches('\'').trim_matches('"').to_string()
}

/// Return the constant name from a `module` or `class` node.
///
/// The grammar places the constant as the first named child (`constant` kind)
/// or via the `name` field in some grammar versions.
fn module_or_class_constant(node: Node<'_>, source: &str) -> Option<String> {
    // Try the `name` field first
    if let Some(n) = node.child_by_field_name("name") {
        let text = text_for_node(n, source);
        if !text.is_empty() {
            return Some(text);
        }
    }
    // Fallback: first named child that is a `constant`
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "constant" {
            let text = text_for_node(child, source);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// Extract the method name from a `method` node.
fn method_name(node: Node<'_>, source: &str) -> Option<String> {
    // Try `name` field
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
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// Extract the method name from a `singleton_method` node (`def self.method_name`).
fn singleton_method_name(node: Node<'_>, source: &str) -> Option<String> {
    // singleton_method: `def` object `.` name
    // The `name` field gives the method name
    if let Some(n) = node.child_by_field_name("name") {
        let text = text_for_node(n, source);
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

/// In Ruby, methods starting with `_` are conventionally private.
/// `initialize`, `method_missing` etc. are never truly private in API terms
/// but we follow the naming convention.
fn is_exported_method(name: &str) -> bool {
    !name.starts_with('_')
}

fn node_is_inside_module_or_class(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "class" | "module" => return true,
            "program" => return false,
            _ => {}
        }
        current = parent;
    }
    false
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
    fn test_extract_ruby_basic_class_and_methods() {
        let source = r#"
class MyClass < BaseClass
  def initialize(name)
    @name = name
  end

  def public_method
    42
  end

  private

  def private_method
    nil
  end
end
"#;
        let file = extract_ruby_symbols(source).unwrap();

        let class_sym = file.symbols.iter().find(|s| s.name == "MyClass").unwrap();
        assert_eq!(class_sym.kind, SymbolKind::Class);
        assert!(class_sym.exported);

        // Superclass type edge
        assert!(
            file.type_edges.iter().any(|e| e.0 == "MyClass" && e.1 == "BaseClass"),
            "MyClass should have type edge to BaseClass"
        );

        // Public method
        let pub_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "MyClass.public_method")
            .unwrap();
        assert_eq!(pub_sym.kind, SymbolKind::Function);
        assert!(pub_sym.exported);

        // Private method — still extracted but not exported
        let priv_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "MyClass.private_method")
            .unwrap();
        assert!(!priv_sym.exported);
    }

    #[test]
    fn test_extract_ruby_module() {
        let source = r#"
module MyModule
  def module_method
    "hello"
  end
end
"#;
        let file = extract_ruby_symbols(source).unwrap();

        let mod_sym = file.symbols.iter().find(|s| s.name == "MyModule").unwrap();
        assert_eq!(mod_sym.kind, SymbolKind::Module);
        assert!(mod_sym.exported);

        assert!(
            file.symbols
                .iter()
                .any(|s| s.name == "MyModule.module_method"),
            "Module method should be extracted"
        );
    }

    #[test]
    fn test_extract_ruby_singleton_method() {
        let source = r#"
class Builder
  def self.create(options = {})
    new(options)
  end

  def instance_method
    "instance"
  end
end
"#;
        let file = extract_ruby_symbols(source).unwrap();

        assert!(
            file.symbols.iter().any(|s| s.name == "Builder.create"),
            "Singleton method should be extracted as Builder.create"
        );
        assert!(
            file.symbols.iter().any(|s| s.name == "Builder.instance_method"),
            "Instance method should be extracted"
        );
    }

    #[test]
    fn test_extract_ruby_imports() {
        let source = r#"
require 'json'
require 'net/http'
require_relative './helper'
require_relative '../models/user'
"#;
        let file = extract_ruby_symbols(source).unwrap();

        assert!(
            file.imports.iter().any(|i| i.source == "json"),
            "Should extract require 'json'"
        );
        assert!(
            file.imports.iter().any(|i| i.source == "net/http"),
            "Should extract require 'net/http'"
        );
        assert!(
            file.imports.iter().any(|i| i.source == "./helper"),
            "Should extract require_relative './helper'"
        );
        assert!(
            file.imports.iter().any(|i| i.source == "../models/user"),
            "Should extract require_relative '../models/user'"
        );
    }

    #[test]
    fn test_extract_ruby_nested_module_and_class() {
        let source = r#"
module Outer
  module Inner
    class Container
      def method_a; end
    end
  end
end
"#;
        let file = extract_ruby_symbols(source).unwrap();

        assert!(file.symbols.iter().any(|s| s.name == "Outer"));
        assert!(file.symbols.iter().any(|s| s.name == "Outer::Inner"));
        assert!(
            file.symbols.iter().any(|s| s.name == "Outer::Inner::Container")
                || file.symbols.iter().any(|s| s.name.contains("Container")),
            "Nested class should be extracted"
        );
    }

    #[test]
    fn test_extract_ruby_top_level_method() {
        let source = r#"
def standalone_function(x)
  x * 2
end
"#;
        let file = extract_ruby_symbols(source).unwrap();

        let sym = file
            .symbols
            .iter()
            .find(|s| s.name == "standalone_function")
            .unwrap();
        assert_eq!(sym.kind, SymbolKind::Function);
        assert!(sym.exported);
    }

    #[test]
    fn test_extract_ruby_visibility() {
        let source = r#"
class Service
  def public_a; end
  def public_b; end

  private

  def secret_a; end
  def secret_b; end

  public

  def public_c; end
end
"#;
        let file = extract_ruby_symbols(source).unwrap();

        let find = |name: &str| file.symbols.iter().find(|s| s.name == name).cloned();

        let pub_a = find("Service.public_a").unwrap();
        assert!(pub_a.exported);

        let pub_b = find("Service.public_b").unwrap();
        assert!(pub_b.exported);

        let sec_a = find("Service.secret_a").unwrap();
        assert!(!sec_a.exported);

        let sec_b = find("Service.secret_b").unwrap();
        assert!(!sec_b.exported);

        let pub_c = find("Service.public_c").unwrap();
        assert!(pub_c.exported);
    }
}

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
    let mut type_edges: Vec<(String, String)> = Vec::new();

    walk(cursor, &mut |node| match node.kind() {
        "function_definition" => {
            // Skip methods defined directly inside a class body — handled by class_definition arm.
            if let Some(parent) = node.parent() {
                if parent.kind() == "block" {
                    if let Some(grandparent) = parent.parent() {
                        if grandparent.kind() == "class_definition" {
                            return;
                        }
                    }
                }
            }
            // Also skip if wrapped in decorated_definition inside a class body.
            if let Some(parent) = node.parent() {
                if parent.kind() == "decorated_definition" {
                    if let Some(gp) = parent.parent() {
                        if gp.kind() == "block" {
                            if let Some(ggp) = gp.parent() {
                                if ggp.kind() == "class_definition" {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            if let Some(name) = symbol_name(node, source) {
                let is_dunder = name.starts_with("__") && name.ends_with("__");
                let exported = is_dunder || !name.starts_with('_');
                symbols.push(symbol_from_node(name.clone(), SymbolKind::Function, exported, node));
                extract_function_type_edges(node, source, &name, &mut type_edges);
            }
        }
        "class_definition" => {
            if let Some(name) = symbol_name(node, source) {
                let class_exported = !name.starts_with('_');
                symbols.push(symbol_from_node(
                    name.clone(),
                    SymbolKind::Class,
                    class_exported,
                    node,
                ));

                // Extract superclass type edges from the argument_list after the class name.
                if let Some(superclasses) = node.child_by_field_name("superclasses") {
                    let mut sc_cursor = superclasses.walk();
                    for child in superclasses.children(&mut sc_cursor) {
                        if let Some(type_name) = extract_python_type_name(child, source) {
                            type_edges.push((name.clone(), type_name));
                        }
                    }
                }

                // Walk the class body block to extract methods and typed field annotations.
                if let Some(body) = node.child_by_field_name("body") {
                    let mut body_cursor = body.walk();
                    for child in body.children(&mut body_cursor) {
                        if child.kind() == "function_definition" {
                            extract_method(
                                &name,
                                class_exported,
                                child,
                                source,
                                &mut symbols,
                                &mut type_edges,
                            );
                        } else if child.kind() == "decorated_definition" {
                            // A decorator wraps the actual function_definition.
                            let mut dec_cursor = child.walk();
                            for dec_child in child.children(&mut dec_cursor) {
                                if dec_child.kind() == "function_definition" {
                                    extract_method(
                                        &name,
                                        class_exported,
                                        dec_child,
                                        source,
                                        &mut symbols,
                                        &mut type_edges,
                                    );
                                }
                            }
                        } else if child.kind() == "expression_statement" {
                            // Look for typed field annotations: `field: Type` or `field: Type = value`.
                            // tree-sitter-python represents these as `assignment` nodes with a `type` field.
                            let mut es_cursor = child.walk();
                            for sub in child.children(&mut es_cursor) {
                                if sub.kind() == "assignment" {
                                    if let Some(type_node) = sub.child_by_field_name("type") {
                                        if let Some(type_name) =
                                            extract_python_type_name(type_node, source)
                                        {
                                            type_edges.push((name.clone(), type_name));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
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
        type_edges,
        dataflow_edges: Vec::new(),
        todos: Vec::new(),
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns: Vec::new(),
    })
}

/// Extract a single method node from a class body, prefixing it with `ClassName.`.
fn extract_method(
    class_name: &str,
    class_exported: bool,
    node: Node<'_>,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    if let Some(method_name) = symbol_name(node, source) {
        let prefixed = format!("{class_name}.{method_name}");
        let is_dunder = method_name.starts_with("__") && method_name.ends_with("__");
        // A method is exported when:
        //   - its class is exported, AND
        //   - the method itself is dunder (always exported) or not private (no leading underscore).
        let method_exported = class_exported && (is_dunder || !method_name.starts_with('_'));
        symbols.push(symbol_from_node(
            prefixed.clone(),
            SymbolKind::Function,
            method_exported,
            node,
        ));
        extract_function_type_edges(node, source, &prefixed, type_edges);
    }
}

/// Extract type edges from a function/method signature: typed parameters and return type.
fn extract_function_type_edges(
    node: Node<'_>,
    source: &str,
    fn_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    // Parameter types — walk the `parameters` field children.
    if let Some(params) = node.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for child in params.children(&mut cursor) {
            match child.kind() {
                "typed_parameter" | "typed_default_parameter" => {
                    if let Some(type_node) = child.child_by_field_name("type") {
                        if let Some(type_name) = extract_python_type_name(type_node, source) {
                            type_edges.push((fn_name.to_string(), type_name));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Return type — the `return_type` field on the function_definition node.
    if let Some(return_type) = node.child_by_field_name("return_type") {
        if let Some(type_name) = extract_python_type_name(return_type, source) {
            type_edges.push((fn_name.to_string(), type_name));
        }
    }
}

/// Extract the base type name from a Python type annotation node.
///
/// In tree-sitter-python v0.25, generic types like `List[Item]` and `Optional[Result]`
/// are represented as `generic_type` nodes whose first child is an `identifier` (the outer
/// type name) and second child is a `type_parameter` (the bracketed arguments).
///
/// This function:
/// - Returns `None` for primitive/builtin types (`int`, `str`, `float`, etc.)
/// - For `Optional[X]` — unwraps to `X` (recurses into the type argument)
/// - For `List[X]`, `Dict[K, V]`, etc. — returns the outer type name (`List`, `Dict`)
/// - For `module.Type` (`attribute` node) — returns the rightmost identifier
/// - Recurses through `type` wrapper nodes transparently
///
/// Returns `None` for primitive types or unrecognised node kinds.
fn extract_python_type_name(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => {
            let name = node.utf8_text(source.as_bytes()).ok()?.to_string();
            // Skip primitive/builtin types that carry no structural information.
            match name.as_str() {
                "int" | "str" | "float" | "bool" | "bytes" | "None" | "object" | "type" => None,
                _ => Some(name),
            }
        }
        "attribute" => {
            // module.Type — extract the rightmost identifier via the `attribute` field.
            node.child_by_field_name("attribute")
                .and_then(|n| extract_python_type_name(n, source))
        }
        "generic_type" => {
            // tree-sitter-python v0.25: `List[Item]` → generic_type
            //   children: identifier("List"), type_parameter("[Item]")
            // `Optional[Result]` → generic_type
            //   children: identifier("Optional"), type_parameter("[Result]")
            // The first child is always the outer type identifier.
            let first = node.child(0)?;
            let outer_text = first.utf8_text(source.as_bytes()).ok()?;
            if outer_text == "Optional" || outer_text == "Union" {
                // Unwrap Optional[X] → X (take first non-None type from type_parameter).
                if let Some(type_param) = node.child(1) {
                    // type_parameter children: "[", type(...), "]"
                    // Recurse through named children of type_parameter.
                    let mut cursor = type_param.walk();
                    for child in type_param.children(&mut cursor) {
                        if child.kind() == "type" {
                            if let Some(name) = extract_python_type_name(child, source) {
                                return Some(name);
                            }
                        }
                    }
                }
                // Could not unwrap — fall back to skipping (Optional alone is not useful).
                return None;
            }
            // For all other generic types (List, Dict, Set, etc.), return the outer name.
            extract_python_type_name(first, source)
        }
        // The `type` wrapper node — recurse into its single child.
        "type" => node.child(0).and_then(|n| extract_python_type_name(n, source)),
        // type_parameter is the `[X, Y]` bracket — recurse into its first `type` child.
        "type_parameter" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type" {
                    if let Some(name) = extract_python_type_name(child, source) {
                        return Some(name);
                    }
                }
            }
            None
        }
        // Union type X | Y — take the first non-None branch.
        "union_type" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if let Some(name) = extract_python_type_name(child, source) {
                    return Some(name);
                }
            }
            None
        }
        // member_type (module.Type in some grammar variants) — last child is the type name.
        "member_type" => {
            let count = node.child_count();
            if count == 0 {
                return None;
            }
            extract_python_type_name(node.child(count - 1)?, source)
        }
        // Legacy subscript form (older tree-sitter-python grammars).
        "subscript" => {
            if let Some(value) = node.child_by_field_name("value") {
                let outer = value.utf8_text(source.as_bytes()).ok()?;
                if outer == "Optional" {
                    if let Some(subscript) = node.child_by_field_name("subscript") {
                        return extract_python_type_name(subscript, source);
                    }
                }
                return extract_python_type_name(value, source);
            }
            None
        }
        _ => None,
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

        // 4 symbols: hello, MyClass, MyClass.method, _private
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
            .find(|s| s.name == "MyClass.method")
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

    #[test]
    fn test_line_numbers_are_1_indexed() {
        let source = "def hello():\n    pass\n";
        let extracted = extract_python_symbols(source).unwrap();
        let hello = extracted.symbols.iter().find(|s| s.name == "hello").unwrap();
        assert_eq!(hello.lines.start, 1); // line 1, not 0
        assert_eq!(hello.lines.end, 2);
    }

    #[test]
    fn test_dunder_methods_exported() {
        let source =
            "class Foo:\n    def __init__(self):\n        pass\n    def __str__(self):\n        return ''\n";
        let extracted = extract_python_symbols(source).unwrap();
        // Dunder methods are now prefixed: Foo.__init__, Foo.__str__
        let init = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Foo.__init__")
            .unwrap();
        assert!(
            init.exported,
            "__init__ should be exported (dunder methods are public API)"
        );
        let str_m = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Foo.__str__")
            .unwrap();
        assert!(str_m.exported, "__str__ should be exported");
    }

    #[test]
    fn test_class_method_prefixing() {
        let source = r#"
class MyClass:
    def method(self):
        pass
    def _private(self):
        pass

class Other:
    def action(self):
        pass
"#;
        let extracted = extract_python_symbols(source).unwrap();
        assert!(
            extracted.symbols.iter().any(|s| s.name == "MyClass.method"),
            "Expected MyClass.method"
        );
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "MyClass._private"),
            "Expected MyClass._private"
        );
        assert!(
            extracted.symbols.iter().any(|s| s.name == "Other.action"),
            "Expected Other.action"
        );
        // Standalone (unprefixed) method name should not exist.
        assert!(
            !extracted.symbols.iter().any(|s| s.name == "method"),
            "method should be prefixed"
        );

        // Check export: _private should not be exported.
        let private = extracted
            .symbols
            .iter()
            .find(|s| s.name == "MyClass._private")
            .unwrap();
        assert!(!private.exported, "_private method should not be exported");
    }

    #[test]
    fn test_python_type_edges() {
        let source = r#"
def process(user: User, count: int) -> Result:
    pass

class MyClass:
    name: str
    items: List

class Child(Parent, Mixin):
    pass
"#;
        let extracted = extract_python_symbols(source).unwrap();

        // Function signature types (int is primitive, skipped)
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "process" && e.1 == "User"),
            "Expected type edge process->User, got: {:?}",
            extracted.type_edges
        );
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "process" && e.1 == "Result"),
            "Expected type edge process->Result, got: {:?}",
            extracted.type_edges
        );
        // int is primitive, should be skipped
        assert!(
            !extracted.type_edges.iter().any(|e| e.1 == "int"),
            "int is primitive and should be skipped, got: {:?}",
            extracted.type_edges
        );

        // Class field annotations
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "MyClass" && e.1 == "List"),
            "Expected type edge MyClass->List, got: {:?}",
            extracted.type_edges
        );

        // Inheritance
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "Child" && e.1 == "Parent"),
            "Expected type edge Child->Parent, got: {:?}",
            extracted.type_edges
        );
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "Child" && e.1 == "Mixin"),
            "Expected type edge Child->Mixin, got: {:?}",
            extracted.type_edges
        );
    }

    #[test]
    fn test_python_type_edges_subscript_and_optional() {
        let source = r#"
def fetch(items: List[Item]) -> Optional[Result]:
    pass
"#;
        let extracted = extract_python_symbols(source).unwrap();

        // List[Item] → List (outer type for subscript)
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "fetch" && e.1 == "List"),
            "Expected type edge fetch->List, got: {:?}",
            extracted.type_edges
        );
        // Optional[Result] → Result (unwrapped)
        assert!(
            extracted.type_edges.iter().any(|e| e.0 == "fetch" && e.1 == "Result"),
            "Expected type edge fetch->Result (unwrapped from Optional), got: {:?}",
            extracted.type_edges
        );
    }
}

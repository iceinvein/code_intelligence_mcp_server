use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::spring::extract_spring_patterns;
use super::symbol::{
    ByteSpan, DataFlowEdge, DataFlowType, ExtractedFile, ExtractedSymbol, Import, LineSpan,
    SymbolKind,
};

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
    let mut dataflow_edges: Vec<DataFlowEdge> = Vec::new();

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
                                extract_java_dataflow(child, source, &prefixed, &mut dataflow_edges);
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
                                extract_java_dataflow(child, source, &prefixed, &mut dataflow_edges);
                            }
                        }
                        if child.kind() == "field_declaration" {
                            if let Some(type_node) = child.child_by_field_name("type") {
                                if let Some(type_name) = extract_java_type_name(type_node, source) {
                                    type_edges.push((name.clone(), type_name));
                                }
                            }
                            // Extract static final fields as constants
                            let has_static = has_modifier(child, "static");
                            let has_final = has_modifier(child, "final");
                            if has_static && has_final {
                                let mut decl_cursor = child.walk();
                                for decl in child.children(&mut decl_cursor) {
                                    if decl.kind() == "variable_declarator" {
                                        if let Some(const_name) = symbol_name(decl, source) {
                                            let prefixed = format!("{name}.{const_name}");
                                            symbols.push(symbol_from_node(
                                                prefixed,
                                                SymbolKind::Const,
                                                is_public(child),
                                                child,
                                            ));
                                        }
                                    }
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

    let framework_patterns = extract_spring_patterns(root, source);

    // Extract TODO/FIXME from Java line and block comments
    let todo_cursor = root.walk();
    let todos = super::comments::extract_todo_from_tree(
        todo_cursor, source, "", &["line_comment", "block_comment"],
    );

    Ok(ExtractedFile {
        symbols,
        imports,
        type_edges,
        dataflow_edges,
        todos,
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns,
    })
}

// ---------------------------------------------------------------------------
// Data flow extraction
// ---------------------------------------------------------------------------

/// Extract data flow edges from a Java method body.
fn extract_java_dataflow(
    node: Node,
    source: &str,
    method_name: &str,
    edges: &mut Vec<DataFlowEdge>,
) {
    let body = match node.child_by_field_name("body") {
        Some(b) => b,
        None => return,
    };
    walk_java_dataflow(body, source, method_name, edges);
}

fn walk_java_dataflow(node: Node, source: &str, method_name: &str, edges: &mut Vec<DataFlowEdge>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "local_variable_declaration" => {
                extract_local_var_dataflow(child, source, method_name, edges);
            }
            "expression_statement" => {
                let mut es_cursor = child.walk();
                for sub in child.children(&mut es_cursor) {
                    match sub.kind() {
                        "assignment_expression" => {
                            extract_java_assignment_dataflow(sub, source, method_name, edges);
                        }
                        "method_invocation" => {
                            extract_java_call_reads(sub, source, method_name, edges);
                        }
                        "update_expression" => {
                            // i++ or ++i → reads and writes i
                            if let Some(operand) = sub.child(0).or_else(|| sub.child(1)) {
                                if operand.kind() == "identifier" {
                                    if let Ok(name) =
                                        operand.utf8_text(source.as_bytes())
                                    {
                                        if !is_java_keyword(name) {
                                            let line = operand.start_position().row as u32 + 1;
                                            edges.push(DataFlowEdge {
                                                from_symbol: name.to_string(),
                                                to_symbol: method_name.to_string(),
                                                flow_type: DataFlowType::Reads,
                                                at_line: line,
                                                scope: Some(method_name.to_string()),
                                            });
                                            edges.push(DataFlowEdge {
                                                from_symbol: name.to_string(),
                                                to_symbol: method_name.to_string(),
                                                flow_type: DataFlowType::Writes,
                                                at_line: line,
                                                scope: Some(method_name.to_string()),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            "return_statement" => {
                let mut ret_cursor = child.walk();
                for sub in child.children(&mut ret_cursor) {
                    if sub.kind() == "identifier" {
                        if let Ok(name) = sub.utf8_text(source.as_bytes()) {
                            if !is_java_keyword(name) {
                                edges.push(DataFlowEdge {
                                    from_symbol: name.to_string(),
                                    to_symbol: method_name.to_string(),
                                    flow_type: DataFlowType::Reads,
                                    at_line: sub.start_position().row as u32 + 1,
                                    scope: Some(method_name.to_string()),
                                });
                            }
                        }
                    } else if sub.kind() == "method_invocation" {
                        extract_java_call_reads(sub, source, method_name, edges);
                    }
                }
            }
            // Recurse into compound statements
            "if_statement"
            | "for_statement"
            | "enhanced_for_statement"
            | "while_statement"
            | "do_statement"
            | "try_statement"
            | "try_with_resources_statement"
            | "switch_expression"
            | "block"
            | "catch_clause"
            | "finally_clause"
            | "synchronized_statement" => {
                walk_java_dataflow(child, source, method_name, edges);
            }
            _ => {}
        }
    }
}

fn extract_local_var_dataflow(
    node: Node,
    source: &str,
    method_name: &str,
    edges: &mut Vec<DataFlowEdge>,
) {
    // local_variable_declaration: Type name = expr;
    // The declarator has name and value.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            let line = child.start_position().row as u32 + 1;
            // Name → write
            if let Some(name_node) = child.child_by_field_name("name") {
                if let Ok(name) = name_node.utf8_text(source.as_bytes()) {
                    edges.push(DataFlowEdge {
                        from_symbol: name.to_string(),
                        to_symbol: method_name.to_string(),
                        flow_type: DataFlowType::Writes,
                        at_line: line,
                        scope: Some(method_name.to_string()),
                    });
                }
            }
            // Value → reads
            if let Some(value) = child.child_by_field_name("value") {
                collect_java_reads(value, source, method_name, edges);
            }
        }
    }
}

fn extract_java_assignment_dataflow(
    node: Node,
    source: &str,
    method_name: &str,
    edges: &mut Vec<DataFlowEdge>,
) {
    let line = node.start_position().row as u32 + 1;
    // LHS → write
    if let Some(left) = node.child_by_field_name("left") {
        match left.kind() {
            "identifier" => {
                if let Ok(name) = left.utf8_text(source.as_bytes()) {
                    if !is_java_keyword(name) {
                        edges.push(DataFlowEdge {
                            from_symbol: name.to_string(),
                            to_symbol: method_name.to_string(),
                            flow_type: DataFlowType::Writes,
                            at_line: line,
                            scope: Some(method_name.to_string()),
                        });
                    }
                }
            }
            "field_access" => {
                // obj.field = value → write "field"
                if let Some(field) = left.child_by_field_name("field") {
                    if let Ok(name) = field.utf8_text(source.as_bytes()) {
                        edges.push(DataFlowEdge {
                            from_symbol: name.to_string(),
                            to_symbol: method_name.to_string(),
                            flow_type: DataFlowType::Writes,
                            at_line: line,
                            scope: Some(method_name.to_string()),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    // RHS → reads
    if let Some(right) = node.child_by_field_name("right") {
        collect_java_reads(right, source, method_name, edges);
    }
}

fn extract_java_call_reads(
    node: Node,
    source: &str,
    method_name: &str,
    edges: &mut Vec<DataFlowEdge>,
) {
    let line = node.start_position().row as u32 + 1;
    // Method name
    if let Some(name_node) = node.child_by_field_name("name") {
        if let Ok(name) = name_node.utf8_text(source.as_bytes()) {
            if !is_java_keyword(name) {
                edges.push(DataFlowEdge {
                    from_symbol: name.to_string(),
                    to_symbol: method_name.to_string(),
                    flow_type: DataFlowType::Reads,
                    at_line: line,
                    scope: Some(method_name.to_string()),
                });
            }
        }
    }
    // Object (receiver)
    if let Some(obj) = node.child_by_field_name("object") {
        if obj.kind() == "identifier" {
            if let Ok(name) = obj.utf8_text(source.as_bytes()) {
                if !is_java_keyword(name) && name != "this" && name != "super" {
                    edges.push(DataFlowEdge {
                        from_symbol: name.to_string(),
                        to_symbol: method_name.to_string(),
                        flow_type: DataFlowType::Reads,
                        at_line: line,
                        scope: Some(method_name.to_string()),
                    });
                }
            }
        }
    }
    // Arguments → reads
    if let Some(args) = node.child_by_field_name("arguments") {
        let mut cursor = args.walk();
        for arg in args.children(&mut cursor) {
            collect_java_reads(arg, source, method_name, edges);
        }
    }
}

fn collect_java_reads(
    node: Node,
    source: &str,
    method_name: &str,
    edges: &mut Vec<DataFlowEdge>,
) {
    match node.kind() {
        "identifier" => {
            if let Ok(name) = node.utf8_text(source.as_bytes()) {
                if !is_java_keyword(name)
                    && name != "this"
                    && name != "super"
                    && name != "null"
                    && name != "true"
                    && name != "false"
                {
                    edges.push(DataFlowEdge {
                        from_symbol: name.to_string(),
                        to_symbol: method_name.to_string(),
                        flow_type: DataFlowType::Reads,
                        at_line: node.start_position().row as u32 + 1,
                        scope: Some(method_name.to_string()),
                    });
                }
            }
        }
        "method_invocation" => {
            extract_java_call_reads(node, source, method_name, edges);
        }
        "field_access" => {
            if let Some(obj) = node.child_by_field_name("object") {
                if obj.kind() == "identifier" {
                    if let Ok(name) = obj.utf8_text(source.as_bytes()) {
                        if !is_java_keyword(name) && name != "this" && name != "super" {
                            edges.push(DataFlowEdge {
                                from_symbol: name.to_string(),
                                to_symbol: method_name.to_string(),
                                flow_type: DataFlowType::Reads,
                                at_line: obj.start_position().row as u32 + 1,
                                scope: Some(method_name.to_string()),
                            });
                        }
                    }
                }
            }
        }
        _ => {
            // Recurse into children for compound expressions
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if matches!(
                    child.kind(),
                    "identifier"
                        | "method_invocation"
                        | "field_access"
                        | "object_creation_expression"
                ) {
                    collect_java_reads(child, source, method_name, edges);
                }
            }
        }
    }
}

/// Returns `true` if `name` is a Java keyword or reserved literal.
fn is_java_keyword(name: &str) -> bool {
    matches!(
        name,
        "this" | "super" | "null" | "true" | "false"
        | "if" | "else" | "for" | "while" | "do" | "switch" | "case" | "default"
        | "return" | "break" | "continue" | "throw" | "throws"
        | "try" | "catch" | "finally" | "new" | "instanceof"
        | "class" | "interface" | "enum" | "extends" | "implements"
        | "public" | "private" | "protected" | "static" | "final" | "abstract"
        | "void" | "int" | "long" | "float" | "double" | "boolean" | "char" | "byte" | "short"
        | "import" | "package" | "synchronized" | "volatile" | "transient" | "native"
    )
}

// ---------------------------------------------------------------------------
// Modifier helpers
// ---------------------------------------------------------------------------

/// Returns `true` if `node` has a `modifiers` child that contains a modifier
/// node whose kind equals `modifier_kind` (e.g. `"static"`, `"final"`,
/// `"public"`).
///
/// In tree-sitter-java the `modifiers` node's immediate children have kinds
/// matching the modifier keyword text directly (e.g. kind `"static"`,
/// `"final"`, `"public"`).
fn has_modifier(node: Node, modifier_kind: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut mod_cursor = child.walk();
            for mod_child in child.children(&mut mod_cursor) {
                if mod_child.kind() == modifier_kind {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Type extraction helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tree walk utilities
// ---------------------------------------------------------------------------

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

    #[test]
    fn test_java_dataflow() {
        let source = r#"
public class Service {
    public void process() {
        User user = findUser();
        save(user);
    }
}
"#;
        let extracted = extract_java_symbols(source).unwrap();
        assert!(
            extracted.dataflow_edges.iter().any(|e| e.from_symbol == "user"
                && e.flow_type == DataFlowType::Writes),
            "Expected writes edge for user, got: {:?}",
            extracted.dataflow_edges
        );
        assert!(
            extracted.dataflow_edges.iter().any(|e| e.from_symbol == "findUser"
                && e.flow_type == DataFlowType::Reads),
            "Expected reads edge for findUser, got: {:?}",
            extracted.dataflow_edges
        );
        assert!(
            extracted.dataflow_edges.iter().any(|e| e.from_symbol == "save"
                && e.flow_type == DataFlowType::Reads),
            "Expected reads edge for save, got: {:?}",
            extracted.dataflow_edges
        );
    }

    #[test]
    fn test_java_constants() {
        let source = r#"
public class Config {
    public static final int MAX_RETRIES = 3;
    private static final String SECRET = "key";
    private String notConst = "value";
}
"#;
        let extracted = extract_java_symbols(source).unwrap();
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Config.MAX_RETRIES" && s.kind == SymbolKind::Const),
            "Expected Config.MAX_RETRIES constant, got: {:?}",
            extracted.symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Config.SECRET" && s.kind == SymbolKind::Const),
            "Expected Config.SECRET constant, got: {:?}",
            extracted.symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        let secret = extracted
            .symbols
            .iter()
            .find(|s| s.name == "Config.SECRET")
            .unwrap();
        assert!(!secret.exported, "SECRET should be private (not exported)");
        // notConst should not be a constant (no static final)
        assert!(
            !extracted
                .symbols
                .iter()
                .any(|s| s.name.contains("notConst") && s.kind == SymbolKind::Const),
            "notConst must not appear as a Const symbol"
        );
    }
}

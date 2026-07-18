use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::go_frameworks::extract_go_framework_patterns;
use super::symbol::{
    ByteSpan, DataFlowEdge, DataFlowType, ExtractedFile, ExtractedSymbol, Import, LineSpan,
    SymbolKind,
};

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
    let mut type_edges: Vec<(String, String)> = Vec::new();

    walk(cursor, &mut |node| match node.kind() {
        "function_declaration" => {
            if let Some(name) = symbol_name(node, source) {
                let exported = is_exported(&name);
                symbols.push(symbol_from_node(
                    name.clone(),
                    SymbolKind::Function,
                    exported,
                    node,
                ));
                // Extract param/return type edges (Task 6)
                extract_func_signature_types(node, source, &name, &mut type_edges);
            }
        }
        "method_declaration" => {
            // Resolve receiver type name and build "ReceiverType.MethodName" (Task 7)
            let receiver_type = receiver_type_name(node, source);
            let method_name = symbol_name(node, source);
            if let (Some(recv), Some(meth)) = (receiver_type, method_name) {
                let qualified = format!("{recv}.{meth}");
                let exported = is_exported(&meth);
                symbols.push(symbol_from_node(
                    qualified.clone(),
                    SymbolKind::Function,
                    exported,
                    node,
                ));
                // Receiver type edge (Task 7)
                type_edges.push((qualified.clone(), recv));
                // Param/return type edges (Task 7)
                extract_func_signature_types(node, source, &qualified, &mut type_edges);
            }
        }
        "type_spec" => {
            // type_spec inside type_declaration
            if let Some(name) = symbol_name(node, source) {
                let exported = is_exported(&name);
                let type_node = node.child_by_field_name("type");
                let kind = match type_node.map(|n| n.kind()) {
                    Some("struct_type") => SymbolKind::Struct,
                    Some("interface_type") => SymbolKind::Interface,
                    _ => SymbolKind::TypeAlias,
                };

                symbols.push(symbol_from_node(name.clone(), kind, exported, node));

                // Extract fields / embedded types / interface methods
                if let Some(body) = type_node {
                    match body.kind() {
                        "struct_type" => {
                            extract_struct_body(body, source, &name, &mut type_edges);
                        }
                        "interface_type" => {
                            extract_interface_body(
                                body,
                                source,
                                &name,
                                &mut symbols,
                                &mut type_edges,
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
        "import_spec" => {
            extract_import(node, source, &mut imports);
        }
        _ => {}
    });

    symbols.sort_by_key(|s| s.bytes.start);

    let framework_patterns = extract_go_framework_patterns(root, source);
    let dataflow_edges = extract_go_goroutine_edges(root, source);

    // Extract TODO/FIXME from Go comments
    let todo_cursor = root.walk();
    let todos = super::comments::extract_todo_from_tree(todo_cursor, source, "", &["comment"]);

    Ok(ExtractedFile {
        symbols,
        imports,
        module_bindings: Vec::new(),
        type_edges,
        extends_edges: Vec::new(),
        dataflow_edges,
        todos,
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns,
    })
}

// ---------------------------------------------------------------------------
// Struct body extraction (Tasks 6 and 8)
// ---------------------------------------------------------------------------

/// Walk a `struct_type` node and emit type edges for:
/// - Named fields whose type is a non-primitive (Task 6)
/// - Embedded types, i.e. field_declaration with no field_identifier child (Task 8)
///
/// Grammar (tree-sitter-go):
/// ```text
/// struct_type → struct field_declaration_list
/// field_declaration_list → { field_declaration* }
/// field_declaration → field_identifier? type_node
/// ```
/// An embedded type is a `field_declaration` whose first non-punctuation child
/// is a type node (type_identifier / pointer_type) with no preceding `field_identifier`.
fn extract_struct_body(
    struct_node: Node<'_>,
    source: &str,
    parent_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    // struct_type → child[1] is field_declaration_list (child[0] is "struct" keyword)
    let mut outer_cursor = struct_node.walk();
    for child in struct_node.children(&mut outer_cursor) {
        if child.kind() != "field_declaration_list" {
            continue;
        }

        let mut cursor = child.walk();
        for field in child.children(&mut cursor) {
            if field.kind() != "field_declaration" {
                continue;
            }

            // Walk children of field_declaration to find field_identifier and type node.
            let mut has_field_identifier = false;
            let mut type_node: Option<Node<'_>> = None;

            let mut fc = field.walk();
            for fchild in field.children(&mut fc) {
                match fchild.kind() {
                    "field_identifier" => {
                        has_field_identifier = true;
                    }
                    // Type nodes we care about
                    "type_identifier" | "pointer_type" | "slice_type" | "array_type"
                    | "map_type" | "channel_type" | "qualified_type" => {
                        if type_node.is_none() {
                            type_node = Some(fchild);
                        }
                    }
                    _ => {}
                }
            }

            // Both embedded and named fields emit type edges.
            // For embedded fields (no field_identifier), the type IS the type node.
            // For named fields, the type node follows the field_identifier.
            if !has_field_identifier {
                // Embedded type — direct child is the type node.
                if let Some(tn) = type_node {
                    extract_go_type_ref(tn, source, parent_name, type_edges);
                }
            } else if let Some(tn) = type_node {
                // Named field — emit type edge for the field's type.
                extract_go_type_ref(tn, source, parent_name, type_edges);
            }
        }
        break; // Only one field_declaration_list per struct
    }
}

// ---------------------------------------------------------------------------
// Interface body extraction (Task 8)
// ---------------------------------------------------------------------------

/// Walk an `interface_type` node and collect:
/// - Abstract method symbols named "InterfaceName.MethodName" (Task 8)
/// - Type edges for embedded interfaces (Task 8)
///
/// Grammar (tree-sitter-go, newer versions):
/// ```text
/// interface_type → interface { method_elem* | type_elem* }
/// method_elem → field_identifier parameter_list result?
/// type_elem → type_identifier   (embedded interface)
/// ```
fn extract_interface_body(
    iface_node: Node<'_>,
    source: &str,
    parent_name: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let mut cursor = iface_node.walk();
    for child in iface_node.children(&mut cursor) {
        match child.kind() {
            // Abstract method declaration inside an interface.
            // method_elem: field_identifier parameter_list result?
            "method_elem" => {
                // First child that is a field_identifier is the method name.
                let mut mc = child.walk();
                let mut method_name_opt: Option<String> = None;
                let mut params_opt: Option<Node<'_>> = None;
                let mut result_opt: Option<Node<'_>> = None;

                for mchild in child.children(&mut mc) {
                    match mchild.kind() {
                        "field_identifier" if method_name_opt.is_none() => {
                            method_name_opt = Some(text_for_node(mchild, source));
                        }
                        "parameter_list" if params_opt.is_none() => {
                            params_opt = Some(mchild);
                        }
                        // Result can be a type_identifier or a parameter_list
                        "parameter_list" => {
                            result_opt = Some(mchild);
                        }
                        "type_identifier" | "pointer_type" | "slice_type" | "array_type"
                        | "map_type" | "qualified_type" => {
                            result_opt = Some(mchild);
                        }
                        _ => {}
                    }
                }

                if let Some(method_name) = method_name_opt {
                    let qualified = format!("{parent_name}.{method_name}");
                    let exported = is_exported(&method_name);
                    symbols.push(symbol_from_node(
                        qualified.clone(),
                        SymbolKind::Function,
                        exported,
                        child,
                    ));
                    // Param types
                    if let Some(params) = params_opt {
                        extract_param_list_types(params, source, &qualified, type_edges);
                    }
                    // Return types
                    if let Some(result) = result_opt {
                        extract_go_type_ref(result, source, &qualified, type_edges);
                    }
                }
            }
            // Embedded interface in an interface body — tree-sitter-go represents
            // these as `type_elem` containing a bare `type_identifier`.
            "type_elem" => {
                // type_elem may wrap a type_identifier (embedded interface name).
                let mut tc = child.walk();
                for tchild in child.children(&mut tc) {
                    if tchild.kind() == "type_identifier" {
                        let name = text_for_node(tchild, source);
                        if !is_go_builtin(&name) {
                            type_edges.push((parent_name.to_string(), name));
                        }
                    }
                }
            }
            // Fallback for older grammar versions that use bare type_identifier.
            "type_identifier" => {
                let name = text_for_node(child, source);
                if !is_go_builtin(&name) {
                    type_edges.push((parent_name.to_string(), name));
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Function / method signature type extraction (Tasks 6 and 7)
// ---------------------------------------------------------------------------

/// Extract type edges from a `function_declaration` or `method_declaration`
/// parameter list and return types.
fn extract_func_signature_types(
    node: Node<'_>,
    source: &str,
    symbol_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    // Parameters
    if let Some(params) = node.child_by_field_name("parameters") {
        extract_param_list_types(params, source, symbol_name, type_edges);
    }
    // Return type(s) — field name is "result" in tree-sitter-go
    if let Some(result) = node.child_by_field_name("result") {
        extract_go_type_ref(result, source, symbol_name, type_edges);
    }
}

/// Walk a `parameter_list` node and emit type edges for every parameter type.
fn extract_param_list_types(
    param_list: Node<'_>,
    source: &str,
    symbol_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    let mut cursor = param_list.walk();
    for child in param_list.children(&mut cursor) {
        if child.kind() == "parameter_declaration"
            || child.kind() == "variadic_parameter_declaration"
        {
            if let Some(type_node) = child.child_by_field_name("type") {
                extract_go_type_ref(type_node, source, symbol_name, type_edges);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Receiver resolution (Task 7)
// ---------------------------------------------------------------------------

/// Return the struct/type name from a method receiver, stripping pointer `*`.
///
/// Grammar: `method_declaration` → `receiver: parameter_list` →
///   `parameter_declaration` → `type: (type_identifier | pointer_type)`
fn receiver_type_name(node: Node<'_>, source: &str) -> Option<String> {
    let receiver = node.child_by_field_name("receiver")?;
    // receiver is a parameter_list; find the first parameter_declaration
    let mut cursor = receiver.walk();
    for child in receiver.children(&mut cursor) {
        if child.kind() == "parameter_declaration" {
            if let Some(type_node) = child.child_by_field_name("type") {
                return Some(bare_type_name(type_node, source));
            }
        }
    }
    None
}

/// Return the unqualified type name, stripping pointer `*` and package prefix.
fn bare_type_name(node: Node<'_>, source: &str) -> String {
    match node.kind() {
        "pointer_type" => {
            // pointer_type children: "*" then the pointee type node
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() != "*" {
                    return bare_type_name(child, source);
                }
            }
            text_for_node(node, source)
        }
        "qualified_type" => {
            // pkg.Type — keep only the Type part
            if let Some(name_node) = node.child_by_field_name("name") {
                text_for_node(name_node, source)
            } else {
                text_for_node(node, source)
            }
        }
        _ => text_for_node(node, source),
    }
}

// ---------------------------------------------------------------------------
// Core type-ref walker
// ---------------------------------------------------------------------------

/// Recursively walk a type node and emit `(parent_name, TypeName)` edges for
/// every non-primitive named type encountered.
fn extract_go_type_ref(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    match node.kind() {
        "type_identifier" => {
            let name = text_for_node(node, source);
            if !is_go_builtin(&name) {
                out.push((parent_name.to_string(), name));
            }
        }
        "pointer_type" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() != "*" {
                    extract_go_type_ref(child, source, parent_name, out);
                }
            }
        }
        "slice_type" | "array_type" | "map_type" | "channel_type" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_go_type_ref(child, source, parent_name, out);
            }
        }
        "qualified_type" => {
            // pkg.Type — extract the type name (the RHS)
            if let Some(name_node) = node.child_by_field_name("name") {
                out.push((parent_name.to_string(), text_for_node(name_node, source)));
            }
        }
        "parameter_list" => {
            // Return type can be a parenthesised list: (string, error)
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "parameter_declaration" {
                    if let Some(type_node) = child.child_by_field_name("type") {
                        extract_go_type_ref(type_node, source, parent_name, out);
                    }
                } else {
                    // Might be a bare type_identifier inside the parens
                    extract_go_type_ref(child, source, parent_name, out);
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn text_for_node(node: Node<'_>, source: &str) -> String {
    node.utf8_text(source.as_bytes()).unwrap_or("").to_string()
}

/// Return `true` for Go primitive/builtin type names that should NOT produce
/// type edges.
///
/// Note: `error` is intentionally absent — it is a built-in *interface*, and
/// emitting a type edge for it is useful for graph analysis (callers of
/// functions that return `error` can be traced).
fn is_go_builtin(name: &str) -> bool {
    matches!(
        name,
        "string"
            | "int"
            | "bool"
            | "byte"
            | "rune"
            | "float32"
            | "float64"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
            | "any"
            | "complex64"
            | "complex128"
    )
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
        .map(|n| text_for_node(n, source))
}

fn is_exported(name: &str) -> bool {
    // In Go, exported symbols start with an uppercase letter
    name.chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
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

// ---------------------------------------------------------------------------
// Goroutine / async boundary detection
// ---------------------------------------------------------------------------

/// Walk the entire AST and emit `spawn:<callee>` data-flow edges for every
/// `go_statement` (goroutine spawn).
///
/// The tree-sitter-go grammar represents `go f()` as:
/// ```text
/// go_statement
///   "go"            ← keyword token
///   call_expression ← the spawned call (function field = callee)
/// ```
///
/// The enclosing function name is resolved by walking up to the nearest
/// `function_declaration` or `method_declaration` ancestor.
fn extract_go_goroutine_edges(root: Node<'_>, source: &str) -> Vec<DataFlowEdge> {
    let mut edges = Vec::new();
    collect_goroutine_edges(root, source, &mut edges);
    edges
}

fn collect_goroutine_edges(node: Node<'_>, source: &str, out: &mut Vec<DataFlowEdge>) {
    if node.kind() == "go_statement" {
        let line = node.start_position().row as u32 + 1;

        // The second child (after the "go" keyword token) is the call expression.
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            // Skip the "go" keyword token.
            cursor.goto_next_sibling();
            let call_node = cursor.node();
            if call_node.kind() == "call_expression" {
                if let Some(callee) = extract_go_call_callee(call_node, source) {
                    let enclosing = enclosing_function_name(node, source)
                        .unwrap_or_else(|| "<module>".to_string());
                    out.push(DataFlowEdge {
                        from_symbol: format!("spawn:{callee}"),
                        to_symbol: enclosing.clone(),
                        flow_type: DataFlowType::Reads,
                        at_line: line,
                        scope: Some(enclosing),
                    });
                }
            }
        }
    }

    // Recurse into all children.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_goroutine_edges(child, source, out);
    }
}

/// Extract the callee name from a Go `call_expression` node.
///
/// Handles:
/// - `identifier` — bare function calls like `handleRequest()`
/// - `selector_expression` — qualified calls like `pkg.Func()` or method calls
///   like `s.Handle()`
fn extract_go_call_callee(call_node: Node<'_>, source: &str) -> Option<String> {
    let func = call_node.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => Some(text_for_node(func, source)),
        "selector_expression" => {
            // operand.field — e.g. `wg.Done` or `http.ListenAndServe`
            let operand = func.child_by_field_name("operand")?;
            let field = func.child_by_field_name("field")?;
            Some(format!(
                "{}.{}",
                text_for_node(operand, source),
                text_for_node(field, source)
            ))
        }
        _ => {
            let t = text_for_node(func, source);
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        }
    }
}

/// Walk up the tree from `node` to find the nearest enclosing
/// `function_declaration` or `method_declaration` and return its qualified name.
///
/// For `method_declaration` this returns `"ReceiverType.MethodName"` (matching
/// the symbol names emitted by the main extractor walk).  For
/// `function_declaration` it returns the bare function name.
fn enclosing_function_name(node: Node<'_>, source: &str) -> Option<String> {
    let mut current = node.parent()?;
    loop {
        match current.kind() {
            "function_declaration" => {
                return current
                    .child_by_field_name("name")
                    .map(|n| text_for_node(n, source));
            }
            "method_declaration" => {
                let recv = receiver_type_name(current, source).unwrap_or_else(|| "_".to_string());
                let meth = current
                    .child_by_field_name("name")
                    .map(|n| text_for_node(n, source))
                    .unwrap_or_else(|| "_".to_string());
                return Some(format!("{recv}.{meth}"));
            }
            _ => {}
        }
        current = current.parent()?;
    }
}

fn extract_import(node: Node, source: &str, imports: &mut Vec<Import>) {
    // import_spec: (name)? (path)
    let path_node = node.child_by_field_name("path");
    let name_node = node.child_by_field_name("name");

    if let Some(path_n) = path_node {
        let path_str = text_for_node(path_n, source);
        // path_str includes quotes, e.g. "fmt"
        let source_path = path_str.trim_matches('"').to_string();

        let alias = name_node.map(|n| text_for_node(n, source));

        // If no alias, the name is the last component of the path (usually)
        let name = if let Some(a) = &alias {
            a.clone()
        } else {
            // derive from source path
            source_path
                .split('/')
                .next_back()
                .unwrap_or(&source_path)
                .to_string()
        };

        imports.push(Import {
            name,
            source: source_path,
            alias,
            at_line: node.start_position().row as u32 + 1,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // Diagnostic helpers (not a test themselves)
    // ---------------------------------------------------------------------------

    #[allow(dead_code)]
    fn debug_print_tree(node: Node<'_>, source: &str, depth: usize) {
        let indent = "  ".repeat(depth);
        let text = node
            .utf8_text(source.as_bytes())
            .unwrap_or("")
            .replace('\n', "\\n");
        let short = if text.len() > 40 { &text[..40] } else { &text };
        println!("{indent}[{depth}] kind={} text={short:?}", node.kind());
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            debug_print_tree(child, source, depth + 1);
        }
    }

    // ---------------------------------------------------------------------------
    // Existing tests (updated for Task 7 naming change)
    // ---------------------------------------------------------------------------

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

        // Symbols: main, ExportedFunc, MyStruct, MyInterface, MyInterface.Method (Task 8)
        assert_eq!(extracted.symbols.len(), 5);

        let main_sym = extracted.symbols.iter().find(|s| s.name == "main").unwrap();
        assert_eq!(main_sym.kind, SymbolKind::Function);
        assert!(!main_sym.exported); // lowercase

        let exported = extracted
            .symbols
            .iter()
            .find(|s| s.name == "ExportedFunc")
            .unwrap();
        assert_eq!(exported.kind, SymbolKind::Function);
        assert!(exported.exported);

        let my_struct = extracted
            .symbols
            .iter()
            .find(|s| s.name == "MyStruct")
            .unwrap();
        assert_eq!(my_struct.kind, SymbolKind::Struct);
        assert!(my_struct.exported);

        let my_iface = extracted
            .symbols
            .iter()
            .find(|s| s.name == "MyInterface")
            .unwrap();
        assert_eq!(my_iface.kind, SymbolKind::Interface);
        assert!(my_iface.exported);

        // Task 8: interface method extracted as qualified symbol
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "MyInterface.Method"),
            "MyInterface.Method should be extracted"
        );

        // Imports
        assert_eq!(extracted.imports.len(), 2);
        assert!(extracted
            .imports
            .iter()
            .any(|i| i.source == "fmt" && i.name == "fmt"));
        assert!(extracted
            .imports
            .iter()
            .any(|i| i.source == "os" && i.alias.as_deref() == Some("my_os")));
    }

    #[test]
    fn test_extract_go_struct_method() {
        let source = r#"
package main

import "fmt"

type GoGreeter struct {
    Name string
}

func (g *GoGreeter) Greet() {
    fmt.Println("Hello from Go")
}
"#;
        let extracted = extract_go_symbols(source).unwrap();

        let greeter = extracted
            .symbols
            .iter()
            .find(|s| s.name == "GoGreeter")
            .unwrap();
        assert_eq!(greeter.kind, SymbolKind::Struct);

        // Task 7: method name is now "GoGreeter.Greet" (dot-separated, not bare "Greet")
        let greet = extracted
            .symbols
            .iter()
            .find(|s| s.name == "GoGreeter.Greet")
            .unwrap();
        assert_eq!(greet.kind, SymbolKind::Function);

        // Task 7: receiver type edge must exist
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "GoGreeter.Greet" && e.1 == "GoGreeter"),
            "GoGreeter.Greet should have a receiver type edge to GoGreeter"
        );
    }

    // ---------------------------------------------------------------------------
    // Task 6: Type edges for functions and structs
    // ---------------------------------------------------------------------------

    #[test]
    fn extracts_go_type_edges() {
        let source = r#"
package main

type User struct {
    Name    string
    Age     int
    Address *Address
}

func Process(u User, count int) (string, error) {
    return "", nil
}
"#;
        let extracted = extract_go_symbols(source).unwrap();
        let has_edge = |p: &str, t: &str| extracted.type_edges.iter().any(|e| e.0 == p && e.1 == t);

        assert!(
            has_edge("User", "Address"),
            "User → Address struct field edge"
        );
        assert!(has_edge("Process", "User"), "Process → User param edge");
        // error is a Go built-in interface and should appear as a type edge
        assert!(has_edge("Process", "error"), "Process → error return edge");
    }

    // ---------------------------------------------------------------------------
    // Task 7: Method receiver linkage
    // ---------------------------------------------------------------------------

    #[test]
    fn extracts_go_method_receiver_linkage() {
        let source = r#"
package main

type Server struct {
    Port int
}

func (s *Server) Start() error {
    return nil
}

func (s Server) GetPort() int {
    return s.Port
}
"#;
        let extracted = extract_go_symbols(source).unwrap();

        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Server.Start" && s.kind == SymbolKind::Function),
            "Server.Start should be a Function symbol"
        );
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Server.GetPort" && s.kind == SymbolKind::Function),
            "Server.GetPort should be a Function symbol"
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Server.Start" && e.1 == "Server"),
            "Server.Start → Server receiver edge"
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Server.GetPort" && e.1 == "Server"),
            "Server.GetPort → Server receiver edge"
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Server.Start" && e.1 == "error"),
            "Server.Start → error return edge"
        );
    }

    // ---------------------------------------------------------------------------
    // Goroutine spawn detection
    // ---------------------------------------------------------------------------

    #[test]
    fn test_goroutine_spawn_detection() {
        let source = "package main\n\nfunc process() {\n\tgo handleRequest()\n}\n";
        let file = extract_go_symbols(source).unwrap();
        let spawn_edges: Vec<_> = file
            .dataflow_edges
            .iter()
            .filter(|e| e.from_symbol.starts_with("spawn:"))
            .collect();
        assert_eq!(
            spawn_edges.len(),
            1,
            "Should detect goroutine spawn, edges: {:?}",
            file.dataflow_edges
        );
        assert_eq!(
            spawn_edges[0].from_symbol, "spawn:handleRequest",
            "Expected spawn:handleRequest"
        );
        assert_eq!(
            spawn_edges[0].to_symbol, "process",
            "Enclosing function should be 'process'"
        );
    }

    #[test]
    fn test_goroutine_spawn_method_receiver() {
        let source = r#"package main

type Server struct{}

func (s *Server) Run() {
    go s.handleConn()
}
"#;
        let file = extract_go_symbols(source).unwrap();
        let spawn_edges: Vec<_> = file
            .dataflow_edges
            .iter()
            .filter(|e| e.from_symbol.starts_with("spawn:"))
            .collect();
        assert_eq!(
            spawn_edges.len(),
            1,
            "Should detect goroutine spawn inside method, edges: {:?}",
            file.dataflow_edges
        );
        assert_eq!(spawn_edges[0].from_symbol, "spawn:s.handleConn");
        assert_eq!(spawn_edges[0].to_symbol, "Server.Run");
    }

    // ---------------------------------------------------------------------------
    // Task 8: Interface methods + struct/interface embedding
    // ---------------------------------------------------------------------------

    #[test]
    fn extracts_go_interface_methods_and_embedding() {
        let source = r#"
package main

type Reader interface {
    Read(p []byte) (int, error)
}

type Writer interface {
    Write(p []byte) (int, error)
}

type ReadWriter interface {
    Reader
    Writer
}

type BufferedReader struct {
    Reader
    bufSize int
}
"#;
        let extracted = extract_go_symbols(source).unwrap();

        // Interface methods (Task 8)
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Reader.Read" && s.kind == SymbolKind::Function),
            "Reader.Read should be a Function symbol"
        );
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Writer.Write" && s.kind == SymbolKind::Function),
            "Writer.Write should be a Function symbol"
        );

        // Type edges from interface method return types (Task 8)
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Reader.Read" && e.1 == "error"),
            "Reader.Read → error return edge"
        );

        // Embedded interfaces in ReadWriter (Task 8)
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "ReadWriter" && e.1 == "Reader"),
            "ReadWriter → Reader embedding edge"
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "ReadWriter" && e.1 == "Writer"),
            "ReadWriter → Writer embedding edge"
        );

        // Struct embedding (Task 8)
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "BufferedReader" && e.1 == "Reader"),
            "BufferedReader → Reader embedding edge"
        );
    }
}

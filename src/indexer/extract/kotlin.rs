use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{
    ByteSpan, DataFlowEdge, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind,
};

/// Extract symbols from Kotlin source code.
///
/// Handles: classes, interfaces, objects (singletons), top-level functions,
/// member functions, companion object functions, `import` declarations, and
/// private/internal visibility modifiers.
///
/// # Errors
///
/// Returns an error if the source cannot be parsed.
///
/// # Examples
///
/// ```
/// use code_intelligence_mcp_server::indexer::extract::kotlin::extract_kotlin_symbols;
/// let src = "class Hello { fun greet() = \"hi\" }";
/// let file = extract_kotlin_symbols(src).unwrap();
/// assert!(file.symbols.iter().any(|s| s.name == "Hello"));
/// ```
pub fn extract_kotlin_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::Kotlin)?;
    extract_symbols_with_parser(&mut parser, source)
}

fn extract_symbols_with_parser(parser: &mut Parser, source: &str) -> Result<ExtractedFile> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse Kotlin source"))?;
    let root = tree.root_node();

    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut type_edges: Vec<(String, String)> = Vec::new();

    // Walk only top-level nodes — recursive descent handles nested declarations.
    walk(root.walk(), &mut |node| match node.kind() {
        "import_header" | "import" => {
            extract_import(node, source, &mut imports);
        }
        "class_declaration" => {
            if !node_is_inside_class_body(node) {
                extract_class_declaration(node, source, &mut symbols, &mut type_edges);
            }
        }
        "object_declaration" => {
            if !node_is_inside_class_body(node) {
                extract_object_declaration(node, source, &mut symbols);
            }
        }
        "function_declaration" => {
            // Top-level functions (class members extracted recursively)
            if !node_is_inside_class_body(node) {
                extract_function_declaration(node, source, None, &mut symbols, &mut Vec::new());
            }
        }
        _ => {}
    });

    symbols.sort_by_key(|s| s.bytes.start);

    let todo_cursor = root.walk();
    let todos = super::comments::extract_todo_from_tree(
        todo_cursor, source, "", &["line_comment", "multiline_comment"],
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
// Class / interface / object extraction
// ---------------------------------------------------------------------------

fn extract_class_declaration(
    node: Node<'_>,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let Some(name) = symbol_name(node, source) else {
        return;
    };

    let kind = classify_class_node(node, source);
    let exported = is_exported_node(node, source);
    symbols.push(symbol_from_node(name.clone(), kind, exported, node));

    // Delegation specifiers (superclass / interface list) → type edges
    extract_delegation_type_edges(node, source, &name, type_edges);

    // In tree-sitter-kotlin-ng the class body is a direct child of kind
    // `class_body`, accessible via the `class_body` field name.
    let body = node
        .child_by_field_name("class_body")
        .or_else(|| find_child_by_kind(node, "class_body"));

    if let Some(b) = body {
        extract_class_body(b, &name, exported, source, symbols, type_edges);
    }
}

fn extract_object_declaration(
    node: Node<'_>,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
) {
    let Some(name) = symbol_name(node, source) else {
        return;
    };
    let exported = is_exported_node(node, source);
    symbols.push(symbol_from_node(name.clone(), SymbolKind::Class, exported, node));

    let body = node
        .child_by_field_name("class_body")
        .or_else(|| find_child_by_kind(node, "class_body"));

    if let Some(b) = body {
        let mut cursor = b.walk();
        for child in b.children(&mut cursor) {
            if child.kind() == "function_declaration" {
                extract_function_declaration(
                    child,
                    source,
                    Some(&name),
                    symbols,
                    &mut Vec::new(),
                );
            }
        }
    }
}

fn classify_class_node(node: Node<'_>, source: &str) -> SymbolKind {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let text = text_for_node(child, source);
        match text.as_str() {
            "interface" => return SymbolKind::Interface,
            "enum" => return SymbolKind::Enum,
            _ => {}
        }
    }
    SymbolKind::Class
}

fn extract_delegation_type_edges(
    node: Node<'_>,
    source: &str,
    class_name: &str,
    type_edges: &mut Vec<(String, String)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "delegation_specifiers" {
            let mut ds_cursor = child.walk();
            for spec in child.children(&mut ds_cursor) {
                if spec.kind() == "delegation_specifier" {
                    let type_name = extract_type_name_from_specifier(spec, source);
                    if !type_name.is_empty() && !is_kotlin_builtin(&type_name) {
                        type_edges.push((class_name.to_string(), type_name));
                    }
                }
            }
        }
    }
}

fn extract_type_name_from_specifier(spec: Node<'_>, source: &str) -> String {
    let mut cursor = spec.walk();
    for child in spec.children(&mut cursor) {
        match child.kind() {
            "user_type" => return extract_user_type_name(child, source),
            "constructor_invocation" => {
                let mut ci_cursor = child.walk();
                for ci_child in child.children(&mut ci_cursor) {
                    if ci_child.kind() == "user_type" {
                        return extract_user_type_name(ci_child, source);
                    }
                }
            }
            _ => {}
        }
    }
    String::new()
}

fn extract_user_type_name(user_type: Node<'_>, source: &str) -> String {
    // user_type contains simple_user_type or identifier children
    let mut cursor = user_type.walk();
    for child in user_type.children(&mut cursor) {
        match child.kind() {
            "simple_user_type" | "type_identifier" | "identifier" => {
                let text = text_for_node(child, source);
                if !text.is_empty() {
                    // Return just the first component (base name without generics)
                    return text.split('<').next().unwrap_or("").trim().to_string();
                }
            }
            _ => {}
        }
    }
    // Fallback: raw text without generics
    text_for_node(user_type, source)
        .split('<')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

// ---------------------------------------------------------------------------
// Class body walk
// ---------------------------------------------------------------------------

fn extract_class_body(
    body: Node<'_>,
    class_name: &str,
    _class_exported: bool,
    source: &str,
    symbols: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                extract_function_declaration(
                    child,
                    source,
                    Some(class_name),
                    symbols,
                    type_edges,
                );
            }
            "companion_object" => {
                // companion object { … } — functions go under the parent class name
                let companion_body = child
                    .child_by_field_name("class_body")
                    .or_else(|| find_child_by_kind(child, "class_body"));
                if let Some(cb) = companion_body {
                    let mut cb_cursor = cb.walk();
                    for cb_child in cb.children(&mut cb_cursor) {
                        if cb_child.kind() == "function_declaration" {
                            extract_function_declaration(
                                cb_child,
                                source,
                                Some(class_name),
                                symbols,
                                type_edges,
                            );
                        }
                    }
                }
            }
            "class_declaration" => {
                extract_class_declaration(child, source, symbols, type_edges);
            }
            "object_declaration" => {
                extract_object_declaration(child, source, symbols);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Function extraction
// ---------------------------------------------------------------------------

fn extract_function_declaration(
    node: Node<'_>,
    source: &str,
    parent_name: Option<&str>,
    symbols: &mut Vec<ExtractedSymbol>,
    _type_edges: &mut Vec<(String, String)>,
) {
    let Some(name) = symbol_name(node, source) else {
        return;
    };

    let qualified = match parent_name {
        Some(parent) => format!("{parent}.{name}"),
        None => name.clone(),
    };

    let exported = is_exported_node(node, source);
    let is_test = is_test_function(node, source);
    let exported = exported && !is_test;

    symbols.push(symbol_from_node(qualified, SymbolKind::Function, exported, node));
}

fn is_test_function(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let text = text_for_node(child, source);
            if text.contains("@Test") || text.contains("@test") {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Import extraction
// ---------------------------------------------------------------------------

fn extract_import(node: Node<'_>, source: &str, imports: &mut Vec<Import>) {
    // In tree-sitter-kotlin-ng the import node is `import` with a
    // `qualified_identifier` child (containing multiple `identifier` children
    // joined by `.`) and an optional `identifier` alias after `as`.
    let mut qname = String::new();
    let mut alias: Option<String> = None;
    let mut seen_as = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "qualified_identifier" => {
                // Build the full dotted name from identifier children
                let mut parts: Vec<String> = Vec::new();
                let mut qi_cursor = child.walk();
                for qi_child in child.children(&mut qi_cursor) {
                    if qi_child.kind() == "identifier" {
                        let text = text_for_node(qi_child, source);
                        if !text.is_empty() {
                            parts.push(text);
                        }
                    }
                }
                if !parts.is_empty() {
                    qname = parts.join(".");
                }
            }
            "identifier" => {
                let text = text_for_node(child, source);
                if text == "as" {
                    seen_as = true;
                } else if seen_as {
                    alias = Some(text);
                }
                // First identifier before `as` is the simple module name
                // (only applies to single-segment imports without qualified_identifier)
                else if qname.is_empty() {
                    qname = text;
                }
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

/// A Kotlin node is exported if it lacks `private` or `internal` modifiers.
///
/// In tree-sitter-kotlin-ng, modifiers are represented as a `modifiers` child
/// node containing `visibility_modifier` children.
fn is_exported_node(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut mc = child.walk();
            for modifier in child.children(&mut mc) {
                if modifier.kind() == "visibility_modifier" {
                    let text = text_for_node(modifier, source);
                    match text.as_str() {
                        "private" | "internal" => return false,
                        "public" | "protected" => return true,
                        _ => {}
                    }
                }
            }
        }
    }
    // Default in Kotlin is public
    true
}

fn is_kotlin_builtin(name: &str) -> bool {
    matches!(
        name,
        "Any"
            | "Unit"
            | "Nothing"
            | "Boolean"
            | "Byte"
            | "Short"
            | "Int"
            | "Long"
            | "Float"
            | "Double"
            | "Char"
            | "String"
            | "Array"
            | "List"
            | "Map"
            | "Set"
    )
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn text_for_node(node: Node<'_>, source: &str) -> String {
    node.utf8_text(source.as_bytes()).unwrap_or("").to_string()
}

fn symbol_name(node: Node<'_>, source: &str) -> Option<String> {
    // In tree-sitter-kotlin-ng `name` field is set on class_declaration,
    // object_declaration, function_declaration.
    if let Some(n) = node.child_by_field_name("name") {
        let text = text_for_node(n, source);
        if !text.is_empty() {
            return Some(text);
        }
    }
    // Fallback: first identifier child
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind() == "simple_identifier" {
            let text = text_for_node(child, source);
            // Skip keyword tokens
            if !text.is_empty()
                && !matches!(
                    text.as_str(),
                    "class" | "interface" | "object" | "fun" | "enum" | "data" | "sealed"
                )
            {
                return Some(text);
            }
        }
    }
    None
}

/// Find the first direct child of `node` with the given node kind.
fn find_child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
    }
    None
}

fn node_is_inside_class_body(node: Node<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "class_body" | "enum_class_body" => return true,
            "source_file" => return false,
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
    fn test_extract_kotlin_class_and_methods() {
        let source = r#"
class MyClass(val name: String) : BaseClass() {
    fun publicMethod(): String = name

    private fun privateMethod() {}
}
"#;
        let file = extract_kotlin_symbols(source).unwrap();

        let class_sym = file.symbols.iter().find(|s| s.name == "MyClass").unwrap();
        assert_eq!(class_sym.kind, SymbolKind::Class);
        assert!(class_sym.exported);

        // Type edge to BaseClass
        assert!(
            file.type_edges
                .iter()
                .any(|e| e.0 == "MyClass" && e.1 == "BaseClass"),
            "MyClass should have type edge to BaseClass"
        );

        // Public method
        let pub_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "MyClass.publicMethod")
            .unwrap();
        assert!(pub_sym.exported);

        // Private method — extracted but not exported
        let priv_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "MyClass.privateMethod")
            .unwrap();
        assert!(!priv_sym.exported);
    }

    #[test]
    fn test_extract_kotlin_interface() {
        let source = r#"
interface MyInterface {
    fun required()
    fun optional(): String
}
"#;
        let file = extract_kotlin_symbols(source).unwrap();

        let iface_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "MyInterface")
            .unwrap();
        assert_eq!(iface_sym.kind, SymbolKind::Interface);
        assert!(iface_sym.exported);

        assert!(
            file.symbols
                .iter()
                .any(|s| s.name == "MyInterface.required"),
            "Interface method should be extracted"
        );
    }

    #[test]
    fn test_extract_kotlin_object_declaration() {
        let source = r#"
object Singleton {
    fun getInstance(): Singleton = this
}
"#;
        let file = extract_kotlin_symbols(source).unwrap();

        let obj_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "Singleton")
            .unwrap();
        assert_eq!(obj_sym.kind, SymbolKind::Class);

        assert!(
            file.symbols
                .iter()
                .any(|s| s.name == "Singleton.getInstance"),
            "Object method should be extracted"
        );
    }

    #[test]
    fn test_extract_kotlin_top_level_function() {
        let source = r#"
fun topLevel(x: Int): Int = x * 2
"#;
        let file = extract_kotlin_symbols(source).unwrap();

        let sym = file.symbols.iter().find(|s| s.name == "topLevel").unwrap();
        assert_eq!(sym.kind, SymbolKind::Function);
        assert!(sym.exported);
    }

    #[test]
    fn test_extract_kotlin_imports() {
        let source = r#"
import java.util.List
import kotlinx.coroutines.flow.Flow
import com.example.MyClass as AliasClass
"#;
        let file = extract_kotlin_symbols(source).unwrap();

        assert!(
            file.imports.iter().any(|i| i.source == "java.util.List"),
            "Should extract java.util.List"
        );
        assert!(
            file.imports
                .iter()
                .any(|i| i.source == "kotlinx.coroutines.flow.Flow"),
            "Should extract Flow"
        );
        assert!(
            file.imports
                .iter()
                .any(|i| i.source == "com.example.MyClass"),
            "Should extract aliased import"
        );
    }

    #[test]
    fn test_extract_kotlin_visibility() {
        let source = r#"
class Service {
    fun publicMethod() {}
    private fun secret() {}
    internal fun internalFn() {}
}
"#;
        let file = extract_kotlin_symbols(source).unwrap();

        let pub_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "Service.publicMethod")
            .unwrap();
        assert!(pub_sym.exported);

        let priv_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "Service.secret")
            .unwrap();
        assert!(!priv_sym.exported);

        let internal_sym = file
            .symbols
            .iter()
            .find(|s| s.name == "Service.internalFn")
            .unwrap();
        assert!(!internal_sym.exported);
    }

    #[test]
    fn test_extract_kotlin_companion_object() {
        let source = r#"
class Widget {
    companion object {
        fun create(): Widget = Widget()
    }
    fun render() {}
}
"#;
        let file = extract_kotlin_symbols(source).unwrap();

        assert!(
            file.symbols.iter().any(|s| s.name == "Widget.create"),
            "Companion object function should be extracted under parent class name"
        );
        assert!(
            file.symbols.iter().any(|s| s.name == "Widget.render"),
            "Instance method should be extracted"
        );
    }
}

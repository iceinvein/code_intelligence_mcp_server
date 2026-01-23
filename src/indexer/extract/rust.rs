use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, ExtractedFile, ExtractedSymbol, LineSpan, SymbolKind};

pub fn extract_rust_symbols(source: &str) -> Result<ExtractedFile> {
    let mut parser = parser_for_id(LanguageId::Rust)?;
    extract_symbols_with_parser(&mut parser, source)
}

fn extract_symbols_with_parser(parser: &mut Parser, source: &str) -> Result<ExtractedFile> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse source"))?;
    let root = tree.root_node();

    let cursor = root.walk();
    let mut symbols = Vec::new();
    let mut type_edges = Vec::new();

    walk(cursor, &mut |node| match node.kind() {
        "function_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                symbols.push(symbol_from_node(
                    name.clone(),
                    SymbolKind::Function,
                    is_public(node, source),
                    node,
                ));
                extract_function_signature_types(node, source, &name, &mut type_edges);
            }
        }
        "struct_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                symbols.push(symbol_from_node(
                    name.clone(),
                    SymbolKind::Struct,
                    is_public(node, source),
                    node,
                ));
                extract_struct_fields(node, source, &name, &mut type_edges);
            }
        }
        "enum_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                symbols.push(symbol_from_node(
                    name,
                    SymbolKind::Enum,
                    is_public(node, source),
                    node,
                ));
                // TODO: extract enum variants fields?
            }
        }
        "trait_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                symbols.push(symbol_from_node(
                    name,
                    SymbolKind::Trait,
                    is_public(node, source),
                    node,
                ));
            }
        }
        "impl_item" => {
            let name = impl_display_name(node, source);
            symbols.push(symbol_from_node(
                name,
                SymbolKind::Impl,
                is_public(node, source),
                node,
            ));
        }
        "mod_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                symbols.push(symbol_from_node(
                    name,
                    SymbolKind::Module,
                    is_public(node, source),
                    node,
                ));
            }
        }
        _ => {}
    });

    symbols.sort_by_key(|s| s.bytes.start);
    Ok(ExtractedFile {
        symbols,
        imports: Vec::new(),
        type_edges,
        dataflow_edges: Vec::new(),
    })
}

fn walk(mut cursor: TreeCursor<'_>, f: &mut impl FnMut(Node<'_>)) {
    loop {
        let node = cursor.node();
        f(node);

        if cursor.goto_first_child() {
            continue;
        }

        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

fn extract_function_signature_types(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    if let Some(params) = node.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for param in params.children(&mut cursor) {
            if param.kind() == "parameter" {
                if let Some(type_node) = param.child_by_field_name("type") {
                    extract_type_ref(type_node, source, parent_name, out);
                }
            }
        }
    }

    if let Some(ret) = node.child_by_field_name("return_type") {
        extract_type_ref(ret, source, parent_name, out);
    }
}

fn extract_struct_fields(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "field_declaration_list" {
            let mut f_cursor = child.walk();
            for field in child.children(&mut f_cursor) {
                if field.kind() == "field_declaration" {
                    if let Some(type_node) = field.child_by_field_name("type") {
                        extract_type_ref(type_node, source, parent_name, out);
                    }
                }
            }
        } else if child.kind() == "ordered_field_declaration_list" {
            let mut f_cursor = child.walk();
            for field in child.children(&mut f_cursor) {
                if field.kind() == "field_declaration" {
                    if let Some(type_node) = field.child_by_field_name("type") {
                        extract_type_ref(type_node, source, parent_name, out);
                    }
                }
            }
        }
    }
}

fn extract_type_ref(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    let kind = node.kind();

    if kind == "type_identifier" || kind == "primitive_type" {
        out.push((parent_name.to_string(), text_for_node(node, source)));
    } else if kind == "generic_type" {
        // generic_type -> type (name), type_arguments
        if let Some(name) = node.child_by_field_name("type") {
            extract_type_ref(name, source, parent_name, out);
        }

        let mut found_args = false;
        if let Some(args) = node.child_by_field_name("type_arguments") {
            found_args = true;
            let mut cursor = args.walk();
            for arg in args.children(&mut cursor) {
                extract_type_ref(arg, source, parent_name, out);
            }
        }
        if !found_args {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type_arguments" {
                    let mut a_cursor = child.walk();
                    for arg in child.children(&mut a_cursor) {
                        extract_type_ref(arg, source, parent_name, out);
                    }
                }
            }
        }
    } else if kind == "type_arguments" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            extract_type_ref(child, source, parent_name, out);
        }
    } else if kind == "reference_type" || kind == "pointer_type" || kind == "array_type" {
        if let Some(_inner) = node.child_by_field_name("type") {
            // reference_type has 'type' field? Not always in grammar
            // Let's iterate children to be safe, skipping & or *
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                let k = child.kind();
                if k != "&" && k != "*" && k != "mut" && k != "[" && k != "]" && k != ";" {
                    extract_type_ref(child, source, parent_name, out);
                }
            }
        } else {
            // If child_by_field_name fails, try children loop
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                let k = child.kind();
                if k != "&" && k != "*" && k != "mut" && k != "[" && k != "]" && k != ";" {
                    extract_type_ref(child, source, parent_name, out);
                }
            }
        }
    } else if kind == "tuple_type" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() != "(" && child.kind() != ")" && child.kind() != "," {
                extract_type_ref(child, source, parent_name, out);
            }
        }
    }
    // Handle plain type_arguments if passed directly?
    // recursion usually handles it via children loop above?
    // But generic_type handler manually walks type_arguments.
}

fn symbol_from_node(
    name: String,
    kind: SymbolKind,
    exported: bool,
    node: Node<'_>,
) -> ExtractedSymbol {
    let start_byte = node.start_byte();
    let end_byte = node.end_byte();

    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;

    ExtractedSymbol {
        name,
        kind,
        exported,
        bytes: ByteSpan {
            start: start_byte,
            end: end_byte,
        },
        lines: LineSpan {
            start: start_line,
            end: end_line,
        },
    }
}

fn symbol_name_from_declaration(node: Node<'_>, source: &str) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    Some(text_for_node(name_node, source))
}

fn impl_display_name(node: Node<'_>, source: &str) -> String {
    let type_name = node
        .child_by_field_name("type")
        .map(|n| text_for_node(n, source))
        .unwrap_or_else(|| "unknown".to_string());

    let trait_name = node
        .child_by_field_name("trait")
        .map(|n| text_for_node(n, source));

    match trait_name {
        Some(t) => format!("impl {t} for {type_name}"),
        None => format!("impl {type_name}"),
    }
}

fn text_for_node(node: Node<'_>, source: &str) -> String {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or("")
        .to_string()
}

fn is_public(node: Node<'_>, source: &str) -> bool {
    if let Some(vis) = node.child_by_field_name("visibility") {
        let v = text_for_node(vis, source);
        return v.trim_start().starts_with("pub");
    }

    let slice = source.get(node.start_byte()..node.end_byte()).unwrap_or("");
    slice.trim_start().starts_with("pub ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet<'a>(source: &'a str, sym: &ExtractedSymbol) -> &'a str {
        &source[sym.bytes.start..sym.bytes.end]
    }

    #[test]
    fn extracts_rust_items_with_spans() {
        let source = r#"
pub struct Foo {
  a: i32,
}

enum E { A, B }

pub trait T {
  fn x(&self);
}

impl Foo {
  pub fn new() -> Self { Self { a: 1 } }
}

pub fn top() {}

mod inner {
  pub fn a() {}
}
"#;

        let syms = extract_rust_symbols(source).unwrap();
        assert!(syms
            .symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Struct && s.name == "Foo"));
        assert!(syms
            .symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Enum && s.name == "E"));
        assert!(syms
            .symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Trait && s.name == "T"));
        assert!(syms
            .symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Function && s.name == "top"));
        assert!(syms
            .symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Module && s.name == "inner"));
        assert!(syms
            .symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Impl && s.name.contains("impl Foo")));

        let foo = syms.symbols.iter().find(|s| s.name == "Foo").unwrap();
        assert!(snippet(source, foo).contains("pub struct Foo"));
        assert!(foo.exported);
    }

    #[test]
    fn extracts_rust_type_edges() {
        let source = r#"
        struct User { name: String }
        fn process(u: User) -> Result<(), Error> {}
        impl User {
            fn new(name: String) -> Self { Self { name } }
        }
        "#;

        let extracted = extract_rust_symbols(source).unwrap();
        let edges = extracted.type_edges;

        let has_edge =
            |parent: &str, ty: &str| edges.contains(&(parent.to_string(), ty.to_string()));

        assert!(has_edge("User", "String"));
        assert!(has_edge("process", "User"));
        assert!(has_edge("process", "Result"));
        assert!(has_edge("process", "Error"));
        assert!(has_edge("new", "String"));
        assert!(has_edge("new", "Self"));
    }
}

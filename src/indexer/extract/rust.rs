use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, ExtractedSymbol, LineSpan, SymbolKind};

pub fn extract_rust_symbols(source: &str) -> Result<Vec<ExtractedSymbol>> {
    let mut parser = parser_for_id(LanguageId::Rust)?;
    extract_symbols_with_parser(&mut parser, source)
}

fn extract_symbols_with_parser(parser: &mut Parser, source: &str) -> Result<Vec<ExtractedSymbol>> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse source"))?;
    let root = tree.root_node();

    let cursor = root.walk();
    let mut out = Vec::new();
    walk(cursor, &mut |node| match node.kind() {
        "function_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                out.push(symbol_from_node(
                    name,
                    SymbolKind::Function,
                    is_public(node, source),
                    node,
                ));
            }
        }
        "struct_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                out.push(symbol_from_node(
                    name,
                    SymbolKind::Struct,
                    is_public(node, source),
                    node,
                ));
            }
        }
        "enum_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                out.push(symbol_from_node(
                    name,
                    SymbolKind::Enum,
                    is_public(node, source),
                    node,
                ));
            }
        }
        "trait_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                out.push(symbol_from_node(
                    name,
                    SymbolKind::Trait,
                    is_public(node, source),
                    node,
                ));
            }
        }
        "impl_item" => {
            let name = impl_display_name(node, source);
            out.push(symbol_from_node(
                name,
                SymbolKind::Impl,
                is_public(node, source),
                node,
            ));
        }
        "mod_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                out.push(symbol_from_node(
                    name,
                    SymbolKind::Module,
                    is_public(node, source),
                    node,
                ));
            }
        }
        _ => {}
    });

    out.sort_by_key(|s| s.bytes.start);
    Ok(out)
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
            .iter()
            .any(|s| s.kind == SymbolKind::Struct && s.name == "Foo"));
        assert!(syms
            .iter()
            .any(|s| s.kind == SymbolKind::Enum && s.name == "E"));
        assert!(syms
            .iter()
            .any(|s| s.kind == SymbolKind::Trait && s.name == "T"));
        assert!(syms
            .iter()
            .any(|s| s.kind == SymbolKind::Function && s.name == "top"));
        assert!(syms
            .iter()
            .any(|s| s.kind == SymbolKind::Module && s.name == "inner"));
        assert!(syms
            .iter()
            .any(|s| s.kind == SymbolKind::Impl && s.name.contains("impl Foo")));

        let foo = syms.iter().find(|s| s.name == "Foo").unwrap();
        assert!(snippet(source, foo).contains("pub struct Foo"));
        assert!(foo.exported);
    }
}

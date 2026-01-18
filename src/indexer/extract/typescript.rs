use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, ExtractedSymbol, LineSpan, SymbolKind};

pub fn extract_typescript_symbols(
    language_id: LanguageId,
    source: &str,
) -> Result<Vec<ExtractedSymbol>> {
    if !matches!(language_id, LanguageId::Typescript | LanguageId::Tsx) {
        return Err(anyhow!("LanguageId must be Typescript or Tsx"));
    }

    let mut parser = parser_for_id(language_id)?;
    extract_symbols_with_parser(&mut parser, source)
}

fn extract_symbols_with_parser(parser: &mut Parser, source: &str) -> Result<Vec<ExtractedSymbol>> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse source"))?;
    let root = tree.root_node();

    let cursor = root.walk();
    let mut out = Vec::new();
    walk(cursor, &mut |node| {
        let kind = node.kind();
        match kind {
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    out.push(symbol_from_node(
                        name,
                        SymbolKind::Function,
                        exported,
                        def_node,
                    ));
                }
            }
            "class_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    out.push(symbol_from_node(
                        name,
                        SymbolKind::Class,
                        exported,
                        def_node,
                    ));
                }
            }
            "interface_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    out.push(symbol_from_node(
                        name,
                        SymbolKind::Interface,
                        exported,
                        def_node,
                    ));
                }
            }
            "type_alias_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    out.push(symbol_from_node(
                        name,
                        SymbolKind::TypeAlias,
                        exported,
                        def_node,
                    ));
                }
            }
            "enum_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    out.push(symbol_from_node(name, SymbolKind::Enum, exported, def_node));
                }
            }
            "lexical_declaration" => {
                extract_const_declarators(node, source, &mut out);
            }
            _ => {}
        }
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

fn text_for_node(node: Node<'_>, source: &str) -> String {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or("")
        .to_string()
}

fn definition_node_for_declaration(node: Node<'_>) -> (Node<'_>, bool) {
    let mut current = node;
    let mut exported = false;
    for _ in 0..4 {
        let Some(parent) = current.parent() else {
            break;
        };
        if parent.kind() == "export_statement" {
            exported = true;
            return (parent, exported);
        }
        current = parent;
    }
    (node, exported)
}

fn extract_const_declarators(node: Node<'_>, source: &str, out: &mut Vec<ExtractedSymbol>) {
    if !is_const_lexical_declaration(node) {
        return;
    }

    let export_statement = export_ancestor(node);
    let exported = export_statement.is_some();
    let mut cursor = node.walk();

    loop {
        let current = cursor.node();
        if current.kind() == "variable_declarator" && current.child_by_field_name("value").is_some()
        {
            if let Some(name_node) = current.child_by_field_name("name") {
                let name = text_for_node(name_node, source);
                let def_node = const_definition_node(export_statement, node, current);
                out.push(symbol_from_node(
                    name,
                    SymbolKind::Const,
                    exported,
                    def_node,
                ));
            }
        }

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

fn is_const_lexical_declaration(node: Node<'_>) -> bool {
    if node.kind() != "lexical_declaration" {
        return false;
    }
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return false;
    }
    loop {
        if cursor.node().kind() == "const" {
            return true;
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
    false
}

fn export_ancestor(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node;
    for _ in 0..4 {
        let parent = current.parent()?;
        if parent.kind() == "export_statement" {
            return Some(parent);
        }
        current = parent;
    }
    None
}

fn const_definition_node<'a>(
    export_statement: Option<Node<'a>>,
    lexical_declaration: Node<'a>,
    variable_declarator: Node<'a>,
) -> Node<'a> {
    if let Some(export_node) = export_statement {
        if export_node.start_byte() <= variable_declarator.start_byte()
            && export_node.end_byte() >= variable_declarator.end_byte()
        {
            return export_node;
        }
    }

    if lexical_declaration.start_byte() <= variable_declarator.start_byte()
        && lexical_declaration.end_byte() >= variable_declarator.end_byte()
    {
        lexical_declaration
    } else {
        variable_declarator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet<'a>(source: &'a str, sym: &ExtractedSymbol) -> &'a str {
        &source[sym.bytes.start..sym.bytes.end]
    }

    #[test]
    fn extracts_declarations_and_const_initializers() {
        let source = r#"
export function foo(x: number) { return x * 2; }

class Bar {
  method() {}
}

export interface Baz { a: number }
export type Qux = { a: number, b: string }
export enum E { A = 1, B = 2 }

export const BIG = {
  nested: { x: 1, y: 2 },
  arr: [1,2,3],
};
"#;

        let symbols = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let names: Vec<_> = symbols
            .iter()
            .map(|s| (s.kind, s.name.as_str(), s.exported))
            .collect();

        assert!(names.contains(&(SymbolKind::Function, "foo", true)));
        assert!(names.contains(&(SymbolKind::Class, "Bar", false)));
        assert!(names.contains(&(SymbolKind::Interface, "Baz", true)));
        assert!(names.contains(&(SymbolKind::TypeAlias, "Qux", true)));
        assert!(names.contains(&(SymbolKind::Enum, "E", true)));
        assert!(names.contains(&(SymbolKind::Const, "BIG", true)));

        let foo = symbols.iter().find(|s| s.name == "foo").unwrap();
        assert!(snippet(source, foo).contains("export function foo"));

        let big = symbols.iter().find(|s| s.name == "BIG").unwrap();
        let big_snip = snippet(source, big);
        assert!(big_snip.contains("export const BIG"));
        assert!(big_snip.contains("nested"));
        assert!(big_snip.contains("arr"));
    }

    #[test]
    fn tsx_parses_and_extracts() {
        let source = r#"
export function Comp() {
  return <div className="x">Hi</div>;
}
"#;
        let symbols = extract_typescript_symbols(LanguageId::Tsx, source).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Function && s.name == "Comp" && s.exported));
    }
}

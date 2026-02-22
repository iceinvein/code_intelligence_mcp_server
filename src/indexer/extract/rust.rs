use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind};

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
    let mut imports = Vec::new();

    walk(cursor, &mut |node| match node.kind() {
        "use_declaration" => {
            extract_use_imports(node, source, &mut imports);
        }
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
        imports,
        type_edges,
        dataflow_edges: Vec::new(),
        todos: Vec::new(),
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns: Vec::new(),
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
        if child.kind() == "field_declaration_list"
            || child.kind() == "ordered_field_declaration_list"
        {
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

/// Extract all `Import` entries from a single `use_declaration` node.
///
/// A `use_declaration` has one child that is the use tree; the tree can be:
/// - `scoped_identifier`   — `std::collections::HashMap`
/// - `identifier`          — `HashMap` (bare, no path prefix)
/// - `scoped_use_list`     — `std::io::{Read, Write}`
/// - `use_list`            — `{Read, Write}` (rare at top level, but valid)
/// - `use_as_clause`       — `foo as bar`
/// - `use_wildcard`        — `path::*`
fn extract_use_imports(node: Node<'_>, source: &str, out: &mut Vec<Import>) {
    // The first non-punctuation child of use_declaration is the use tree.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let k = child.kind();
        if k == "use" || k == ";" || k == "pub" || k == "pub(crate)" {
            continue;
        }
        // Visibility nodes: pub, pub(crate), pub(super), pub(self), pub(in ...)
        if k == "visibility_modifier" {
            continue;
        }
        extract_use_tree(child, source, "", out);
        break; // only one use tree per declaration
    }
}

/// Recursively expand a use tree node into individual `Import` entries.
///
/// `prefix` accumulates the path segments found so far (e.g. `"std::io"`).
fn extract_use_tree(node: Node<'_>, source: &str, prefix: &str, out: &mut Vec<Import>) {
    match node.kind() {
        // `crate::path::Name` or `std::collections::HashMap`
        "scoped_identifier" => {
            let full = text_for_node(node, source);
            // The last segment after the final `::` is the imported name.
            let name = full
                .rsplit("::")
                .next()
                .unwrap_or(&full)
                .to_string();
            // `source` is the full path; if there is a prefix from a parent
            // scoped_use_list we join it, otherwise use the full scoped text.
            let source_path = if prefix.is_empty() {
                full.clone()
            } else {
                format!("{prefix}::{full}")
            };
            out.push(Import {
                name,
                source: source_path,
                alias: None,
            });
        }

        // A plain identifier with no path prefix (e.g. `use HashMap;` or an
        // item inside `{HashMap, BTreeMap}`)
        "identifier" => {
            let name = text_for_node(node, source);
            let source_path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}::{name}")
            };
            out.push(Import {
                name,
                source: source_path,
                alias: None,
            });
        }

        // `std::io::{Read, Write}` — has `path` and `list` children fields.
        "scoped_use_list" => {
            // Build the new prefix from the path part.
            let path_text = node
                .child_by_field_name("path")
                .map(|p| text_for_node(p, source))
                .unwrap_or_default();
            let new_prefix = if prefix.is_empty() {
                path_text
            } else if path_text.is_empty() {
                prefix.to_string()
            } else {
                format!("{prefix}::{path_text}")
            };
            // Now recurse into the list.
            if let Some(list) = node.child_by_field_name("list") {
                extract_use_tree(list, source, &new_prefix, out);
            }
        }

        // `{Read, Write}` — iterate children, skipping punctuation.
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                let k = child.kind();
                if k == "{" || k == "}" || k == "," {
                    continue;
                }
                extract_use_tree(child, source, prefix, out);
            }
        }

        // `HashMap as HM` or `crate::path as alias`
        "use_as_clause" => {
            // `path` field is the imported path; `alias` field is the local name.
            let path_node = node.child_by_field_name("path");
            let alias_node = node.child_by_field_name("alias");

            let (name, source_path) = if let Some(p) = path_node {
                let path_text = text_for_node(p, source);
                let last = path_text
                    .rsplit("::")
                    .next()
                    .unwrap_or(&path_text)
                    .to_string();
                let sp = if prefix.is_empty() {
                    path_text.clone()
                } else {
                    format!("{prefix}::{path_text}")
                };
                (last, sp)
            } else {
                // Fallback: extract from raw text before " as ".
                let raw = text_for_node(node, source);
                let before_as = raw.split(" as ").next().unwrap_or(&raw).trim().to_string();
                let last = before_as
                    .rsplit("::")
                    .next()
                    .unwrap_or(&before_as)
                    .to_string();
                let sp = if prefix.is_empty() {
                    before_as.clone()
                } else {
                    format!("{prefix}::{before_as}")
                };
                (last, sp)
            };

            let alias = alias_node.map(|a| text_for_node(a, source));

            out.push(Import {
                name,
                source: source_path,
                alias,
            });
        }

        // `path::*`
        //
        // tree-sitter represents `use super::symbol::*` as:
        //   use_wildcard
        //     scoped_identifier ("super::symbol")
        //     :: ("::"),
        //     * ("*")
        //
        // The path is embedded as the first child (scoped_identifier or
        // identifier), NOT passed down via the `prefix` argument (the wildcard
        // is a direct child of `use_declaration`, not of `scoped_use_list`).
        "use_wildcard" => {
            // Walk children to find the path node (anything that isn't "::" or "*").
            let mut path_text = prefix.to_string();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                let k = child.kind();
                if k == "::" || k == "*" {
                    continue;
                }
                let child_text = text_for_node(child, source);
                path_text = if path_text.is_empty() {
                    child_text
                } else {
                    format!("{path_text}::{child_text}")
                };
            }
            // `path_text` is now the module path; emit name="*".
            let source_path = if path_text.is_empty() {
                "*".to_string() // bare `use *;` — extremely rare
            } else {
                path_text
            };
            out.push(Import {
                name: "*".to_string(),
                source: source_path,
                alias: None,
            });
        }

        _ => {}
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
    fn extracts_rust_use_imports() {
        let source = r#"
use std::collections::HashMap;
use crate::path::PathNormalizer;
use super::symbol::{ExtractedFile, Import};
use anyhow::Result;
use std::io::{Read, Write};
use foo as bar;
use super::symbol::*;
"#;

        let extracted = extract_rust_symbols(source).unwrap();
        let imports = &extracted.imports;

        // Helper: find an import by name
        let find = |name: &str| imports.iter().find(|i| i.name == name);

        // use std::collections::HashMap;
        let h = find("HashMap").expect("HashMap import");
        assert_eq!(h.source, "std::collections::HashMap");
        assert_eq!(h.alias, None);

        // use crate::path::PathNormalizer;
        let pn = find("PathNormalizer").expect("PathNormalizer import");
        assert_eq!(pn.source, "crate::path::PathNormalizer");
        assert_eq!(pn.alias, None);

        // use super::symbol::{ExtractedFile, Import};  — two entries, same source
        let ef = find("ExtractedFile").expect("ExtractedFile import");
        assert_eq!(ef.source, "super::symbol::ExtractedFile");
        assert_eq!(ef.alias, None);

        let imp = find("Import").expect("Import import");
        assert_eq!(imp.source, "super::symbol::Import");
        assert_eq!(imp.alias, None);

        // use anyhow::Result;
        let r = find("Result").expect("Result import");
        assert_eq!(r.source, "anyhow::Result");
        assert_eq!(r.alias, None);

        // use std::io::{Read, Write};
        let rd = find("Read").expect("Read import");
        assert_eq!(rd.source, "std::io::Read");
        assert_eq!(rd.alias, None);

        let wr = find("Write").expect("Write import");
        assert_eq!(wr.source, "std::io::Write");
        assert_eq!(wr.alias, None);

        // use foo as bar;
        let fb = find("foo").expect("foo as bar import");
        assert_eq!(fb.source, "foo");
        assert_eq!(fb.alias.as_deref(), Some("bar"));

        // use super::symbol::*;
        let wc = find("*").expect("wildcard import");
        assert_eq!(wc.source, "super::symbol");
        assert_eq!(wc.alias, None);
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

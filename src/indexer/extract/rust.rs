use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::actix::extract_actix_patterns;
use super::axum::extract_axum_patterns;
use super::symbol::{
    ByteSpan, DataFlowEdge, DataFlowType, ExtractedFile, ExtractedSymbol, Import, LineSpan,
    SymbolKind,
};

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
    let mut dataflow_edges: Vec<DataFlowEdge> = Vec::new();

    walk(cursor, &mut |node| match node.kind() {
        "use_declaration" => {
            extract_use_imports(node, source, &mut imports);
        }
        "function_item" => {
            // Skip methods inside impl blocks or trait blocks — handled by those arms with type prefix
            if let Some(parent) = node.parent() {
                if parent.kind() == "declaration_list" {
                    if let Some(grandparent) = parent.parent() {
                        if grandparent.kind() == "impl_item" || grandparent.kind() == "trait_item" {
                            return;
                        }
                    }
                }
            }
            if let Some(name) = symbol_name_from_declaration(node, source) {
                symbols.push(symbol_from_node(
                    name.clone(),
                    SymbolKind::Function,
                    is_public(node, source),
                    node,
                ));
                extract_function_signature_types(node, source, &name, &mut type_edges);
                extract_rust_dataflow(node, source, &name, &mut dataflow_edges);
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
                    name.clone(),
                    SymbolKind::Trait,
                    is_public(node, source),
                    node,
                ));

                // Extract method signatures from trait body
                if let Some(body) = node.child_by_field_name("body") {
                    let mut body_cursor = body.walk();
                    for child in body.children(&mut body_cursor) {
                        // function_signature_item = trait method declaration (no body)
                        // function_item = default method implementation (has body)
                        if child.kind() == "function_signature_item"
                            || child.kind() == "function_item"
                        {
                            if let Some(method_name) = symbol_name_from_declaration(child, source) {
                                let prefixed = format!("{name}::{method_name}");
                                symbols.push(symbol_from_node(
                                    prefixed.clone(),
                                    SymbolKind::Function,
                                    is_public(child, source),
                                    child,
                                ));
                                extract_function_signature_types(
                                    child,
                                    source,
                                    &prefixed,
                                    &mut type_edges,
                                );
                                if child.kind() == "function_item" {
                                    extract_rust_dataflow(
                                        child,
                                        source,
                                        &prefixed,
                                        &mut dataflow_edges,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        "impl_item" => {
            let display_name = impl_display_name(node, source);
            symbols.push(symbol_from_node(
                display_name.clone(),
                SymbolKind::Impl,
                is_public(node, source),
                node,
            ));

            // Emit type edges to both the implemented type and the trait (if any)
            let type_name = node
                .child_by_field_name("type")
                .map(|n| text_for_node(n, source));
            let trait_name = node
                .child_by_field_name("trait")
                .map(|n| text_for_node(n, source));
            if let Some(ref tn) = type_name {
                type_edges.push((display_name.clone(), tn.clone()));
            }
            if let Some(ref tr) = trait_name {
                type_edges.push((display_name.clone(), tr.clone()));
            }

            // Extract methods with type-prefixed names
            let prefix = type_name.as_deref().unwrap_or("unknown");
            if let Some(body) = node.child_by_field_name("body") {
                let mut body_cursor = body.walk();
                for child in body.children(&mut body_cursor) {
                    if child.kind() == "function_item" {
                        if let Some(method_name) = symbol_name_from_declaration(child, source) {
                            let prefixed = format!("{prefix}::{method_name}");
                            symbols.push(symbol_from_node(
                                prefixed.clone(),
                                SymbolKind::Function,
                                is_public(child, source),
                                child,
                            ));
                            extract_function_signature_types(
                                child,
                                source,
                                &prefixed,
                                &mut type_edges,
                            );
                            extract_rust_dataflow(child, source, &prefixed, &mut dataflow_edges);
                        }
                    }
                }
            }
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
        "const_item" | "static_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                symbols.push(symbol_from_node(
                    name.clone(),
                    SymbolKind::Const,
                    is_public(node, source),
                    node,
                ));
                if let Some(type_node) = node.child_by_field_name("type") {
                    extract_type_ref(type_node, source, &name, &mut type_edges);
                }
            }
        }
        "type_item" => {
            if let Some(name) = symbol_name_from_declaration(node, source) {
                symbols.push(symbol_from_node(
                    name.clone(),
                    SymbolKind::TypeAlias,
                    is_public(node, source),
                    node,
                ));
                // Extract type edges for the aliased type
                if let Some(type_node) = node.child_by_field_name("type") {
                    extract_type_ref(type_node, source, &name, &mut type_edges);
                }
            }
        }
        _ => {}
    });

    symbols.sort_by_key(|s| s.bytes.start);

    let mut framework_patterns = Vec::new();
    framework_patterns.extend(extract_axum_patterns(root, source));
    framework_patterns.extend(extract_actix_patterns(root, source));

    // Extract TODO/FIXME comments from Rust line_comment and block_comment nodes
    let todo_cursor = root.walk();
    let todos = super::comments::extract_todo_from_tree(
        todo_cursor,
        source,
        "",
        &["line_comment", "block_comment"],
    );

    Ok(ExtractedFile {
        symbols,
        imports,
        type_edges,
        extends_edges: Vec::new(),
        dataflow_edges,
        todos,
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns,
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
            let name = full.rsplit("::").next().unwrap_or(&full).to_string();
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

/// Walk a function node's body block and emit data flow edges for all
/// statements and expressions found directly within it.
fn extract_rust_dataflow(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    let body = match node.child_by_field_name("body") {
        Some(b) if b.kind() == "block" => b,
        _ => return,
    };
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        extract_rust_dataflow_from_node(child, source, context_name, out);
    }
}

/// Recursively emit data flow edges from a single tree-sitter node.
fn extract_rust_dataflow_from_node(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    match node.kind() {
        "let_declaration" => {
            let line = node.start_position().row as u32;
            if let Some(pattern) = node.child_by_field_name("pattern") {
                if pattern.kind() == "identifier" {
                    out.push(DataFlowEdge {
                        from_symbol: text_for_node(pattern, source),
                        to_symbol: context_name.to_string(),
                        flow_type: DataFlowType::Writes,
                        at_line: line,
                        scope: Some(context_name.to_string()),
                    });
                }
            }
            if let Some(value) = node.child_by_field_name("value") {
                extract_rust_reads_from_expr(value, source, context_name, out);
            }
        }
        "assignment_expression" | "compound_assignment_expr" => {
            let line = node.start_position().row as u32;
            if let Some(left) = node.child_by_field_name("left") {
                if let Some(n) = extract_rust_lhs_identifier(left, source) {
                    out.push(DataFlowEdge {
                        from_symbol: n,
                        to_symbol: context_name.to_string(),
                        flow_type: DataFlowType::Writes,
                        at_line: line,
                        scope: Some(context_name.to_string()),
                    });
                }
            }
            if let Some(right) = node.child_by_field_name("right") {
                extract_rust_reads_from_expr(right, source, context_name, out);
            }
        }
        "await_expression" => {
            // In tree-sitter-rust, `.await` is a postfix expression represented as:
            //   await_expression
            //     <inner_expression>   (first named child — no field name)
            //     "."
            //     "await"
            // There is no named field; the inner expression is simply the first child.
            let line = node.start_position().row as u32;
            let inner = {
                let mut cur = node.walk();
                cur.goto_first_child();
                // The first child is the expression being awaited (skip if it's a keyword token).
                let first = cur.node();
                if first.kind() == "." || first.kind() == "await" {
                    None
                } else {
                    Some(first)
                }
            };
            if let Some(inner) = inner {
                let callee_name = match inner.kind() {
                    "call_expression" => inner
                        .child_by_field_name("function")
                        .and_then(|f| extract_rust_callee_name(f, source)),
                    "identifier" => Some(text_for_node(inner, source)),
                    "field_expression" => inner
                        .child_by_field_name("field")
                        .map(|f| text_for_node(f, source)),
                    _ => {
                        let t = text_for_node(inner, source);
                        if t.is_empty() {
                            None
                        } else {
                            Some(t)
                        }
                    }
                };
                if let Some(name) = callee_name {
                    out.push(DataFlowEdge {
                        from_symbol: format!("await:{name}"),
                        to_symbol: context_name.to_string(),
                        flow_type: DataFlowType::Reads,
                        at_line: line,
                        scope: Some(context_name.to_string()),
                    });
                }
                // Also recurse into the inner expression so regular dataflow edges
                // (callee reads, argument reads) are still captured.
                extract_rust_dataflow_from_node(inner, source, context_name, out);
            }
        }
        "call_expression" => {
            let line = node.start_position().row as u32;

            // Detect tokio::spawn / tokio::spawn_blocking before the generic handler.
            if let Some(func) = node.child_by_field_name("function") {
                let func_text = text_for_node(func, source);
                let spawn_label =
                    if func_text == "tokio::spawn" || func_text == "tokio::spawn_blocking" {
                        Some(format!("spawn:{func_text}"))
                    } else {
                        None
                    };

                if let Some(label) = spawn_label {
                    out.push(DataFlowEdge {
                        from_symbol: label,
                        to_symbol: context_name.to_string(),
                        flow_type: DataFlowType::Reads,
                        at_line: line,
                        scope: Some(context_name.to_string()),
                    });
                } else if let Some(n) = extract_rust_callee_name(func, source) {
                    out.push(DataFlowEdge {
                        from_symbol: n,
                        to_symbol: context_name.to_string(),
                        flow_type: DataFlowType::Reads,
                        at_line: line,
                        scope: Some(context_name.to_string()),
                    });
                }
            }

            if let Some(args) = node.child_by_field_name("arguments") {
                let mut cursor = args.walk();
                for child in args.children(&mut cursor) {
                    extract_rust_reads_from_expr(child, source, context_name, out);
                }
            }
        }
        "expression_statement" | "block" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_rust_dataflow_from_node(child, source, context_name, out);
            }
        }
        _ => {
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    extract_rust_dataflow_from_node(cursor.node(), source, context_name, out);
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
    }
}

/// Emit `Reads` edges for all identifiers and call-sites within an expression.
fn extract_rust_reads_from_expr(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    let line = node.start_position().row as u32;
    match node.kind() {
        "await_expression" => {
            // Delegate to the main handler which knows how to emit the await: edge.
            extract_rust_dataflow_from_node(node, source, context_name, out);
        }
        "identifier" => {
            let name = text_for_node(node, source);
            // Skip Rust keywords and common enum variants that add no signal.
            if !matches!(
                name.as_str(),
                "self" | "Self" | "true" | "false" | "None" | "Some" | "Ok" | "Err"
            ) {
                out.push(DataFlowEdge {
                    from_symbol: name,
                    to_symbol: context_name.to_string(),
                    flow_type: DataFlowType::Reads,
                    at_line: line,
                    scope: Some(context_name.to_string()),
                });
            }
        }
        "call_expression" => {
            // Delegate to the main handler so we also capture the callee.
            extract_rust_dataflow_from_node(node, source, context_name, out);
        }
        "field_expression" => {
            // `obj.field` — read the receiver object, not the field name itself.
            if let Some(obj) = node.child_by_field_name("value") {
                if obj.kind() == "identifier" {
                    let name = text_for_node(obj, source);
                    if name != "self" {
                        out.push(DataFlowEdge {
                            from_symbol: name,
                            to_symbol: context_name.to_string(),
                            flow_type: DataFlowType::Reads,
                            at_line: line,
                            scope: Some(context_name.to_string()),
                        });
                    }
                }
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_rust_reads_from_expr(child, source, context_name, out);
            }
        }
    }
}

/// Extract the identifier name from the left-hand side of an assignment.
fn extract_rust_lhs_identifier(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_for_node(node, source)),
        "field_expression" => node
            .child_by_field_name("field")
            .map(|f| text_for_node(f, source)),
        _ => None,
    }
}

/// Extract the callable name from a call expression's function position.
fn extract_rust_callee_name(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_for_node(node, source)),
        "field_expression" => node
            .child_by_field_name("field")
            .map(|f| text_for_node(f, source)),
        "scoped_identifier" => Some(text_for_node(node, source)),
        _ => None,
    }
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
            .any(|s| s.kind == SymbolKind::Function && s.name == "Foo::new"));
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
        assert!(has_edge("User::new", "String"));
        assert!(has_edge("User::new", "Self"));
    }

    #[test]
    fn extracts_rust_const_static_type() {
        let source = r#"
pub const MAX_SIZE: usize = 100;
static INSTANCE: Mutex<Config> = Mutex::new(Config::default());
pub type Result<T> = std::result::Result<T, Error>;
"#;

        let extracted = extract_rust_symbols(source).unwrap();
        let syms = &extracted.symbols;
        let edges = &extracted.type_edges;

        // --- symbol checks ---

        let max_size = syms
            .iter()
            .find(|s| s.name == "MAX_SIZE")
            .expect("MAX_SIZE symbol");
        assert_eq!(max_size.kind, SymbolKind::Const, "MAX_SIZE should be Const");
        assert!(max_size.exported, "MAX_SIZE should be exported (pub const)");

        let instance = syms
            .iter()
            .find(|s| s.name == "INSTANCE")
            .expect("INSTANCE symbol");
        assert_eq!(instance.kind, SymbolKind::Const, "INSTANCE should be Const");
        assert!(
            !instance.exported,
            "INSTANCE should not be exported (no pub)"
        );

        let result_alias = syms
            .iter()
            .find(|s| s.name == "Result")
            .expect("Result type alias symbol");
        assert_eq!(
            result_alias.kind,
            SymbolKind::TypeAlias,
            "Result should be TypeAlias"
        );
        assert!(
            result_alias.exported,
            "Result should be exported (pub type)"
        );

        // --- type edge checks ---

        let has_edge =
            |parent: &str, ty: &str| edges.contains(&(parent.to_string(), ty.to_string()));

        assert!(
            has_edge("MAX_SIZE", "usize"),
            "expected type edge (MAX_SIZE, usize)"
        );
        assert!(
            has_edge("INSTANCE", "Mutex"),
            "expected type edge (INSTANCE, Mutex)"
        );
        assert!(
            has_edge("INSTANCE", "Config"),
            "expected type edge (INSTANCE, Config)"
        );
    }

    #[test]
    fn extracts_rust_impl_edges_and_method_prefix() {
        let source = r#"
pub struct Foo { x: i32 }

pub trait Display {
    fn fmt(&self) -> String;
}

impl Display for Foo {
    fn fmt(&self) -> String {
        format!("{}", self.x)
    }
}

impl Foo {
    pub fn new(x: i32) -> Self {
        Self { x }
    }

    pub fn value(&self) -> i32 {
        self.x
    }
}
"#;
        let extracted = extract_rust_symbols(source).unwrap();

        // impl Display for Foo should have type edges to both Display and Foo
        let impl_display = extracted
            .symbols
            .iter()
            .find(|s| s.name.contains("impl Display for Foo"))
            .expect("impl Display for Foo symbol");
        assert_eq!(impl_display.kind, SymbolKind::Impl);
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == impl_display.name && e.1 == "Display"),
            "expected type edge from impl to Display trait"
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == impl_display.name && e.1 == "Foo"),
            "expected type edge from impl to Foo type"
        );

        // Methods inside impl should be prefixed with the type name
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Foo::new" && s.kind == SymbolKind::Function),
            "expected Foo::new symbol"
        );
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Foo::value" && s.kind == SymbolKind::Function),
            "expected Foo::value symbol"
        );
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Foo::fmt" && s.kind == SymbolKind::Function),
            "expected Foo::fmt symbol"
        );

        // Method type edges should use prefixed name
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Foo::new" && e.1 == "Self"),
            "expected type edge Foo::new -> Self"
        );

        // Unprefixed bare method names must not exist
        assert!(
            !extracted
                .symbols
                .iter()
                .any(|s| s.name == "new" && s.kind == SymbolKind::Function),
            "bare 'new' must not exist — should be 'Foo::new'"
        );
        assert!(
            !extracted
                .symbols
                .iter()
                .any(|s| s.name == "fmt" && s.kind == SymbolKind::Function),
            "bare 'fmt' must not exist — should be 'Foo::fmt'"
        );
    }

    #[test]
    fn extracts_rust_trait_method_types() {
        let source = r#"
pub trait Processor {
    fn process(&self, input: Input) -> Output;
    fn validate(&self, data: &Data) -> Result<(), Error>;
}
"#;
        let extracted = extract_rust_symbols(source).unwrap();

        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Processor" && s.kind == SymbolKind::Trait),
            "expected Processor trait symbol"
        );
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Processor::process" && s.kind == SymbolKind::Function),
            "expected Processor::process symbol"
        );
        assert!(
            extracted
                .symbols
                .iter()
                .any(|s| s.name == "Processor::validate" && s.kind == SymbolKind::Function),
            "expected Processor::validate symbol"
        );

        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Processor::process" && e.1 == "Input"),
            "expected type edge Processor::process -> Input"
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Processor::process" && e.1 == "Output"),
            "expected type edge Processor::process -> Output"
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Processor::validate" && e.1 == "Data"),
            "expected type edge Processor::validate -> Data"
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Processor::validate" && e.1 == "Result"),
            "expected type edge Processor::validate -> Result"
        );
        assert!(
            extracted
                .type_edges
                .iter()
                .any(|e| e.0 == "Processor::validate" && e.1 == "Error"),
            "expected type edge Processor::validate -> Error"
        );

        // Bare unprefixed method names must not be emitted
        assert!(
            !extracted
                .symbols
                .iter()
                .any(|s| s.name == "process" && s.kind == SymbolKind::Function),
            "bare 'process' must not exist — should be 'Processor::process'"
        );
        assert!(
            !extracted
                .symbols
                .iter()
                .any(|s| s.name == "validate" && s.kind == SymbolKind::Function),
            "bare 'validate' must not exist — should be 'Processor::validate'"
        );
    }

    #[test]
    fn test_async_boundary_detection() {
        let source = r#"
async fn process() {
    let data = fetch_data().await;
    tokio::spawn(async { background_work() });
}
"#;
        let file = extract_rust_symbols(source).unwrap();
        let async_edges: Vec<_> = file
            .dataflow_edges
            .iter()
            .filter(|e| e.from_symbol.starts_with("await:") || e.from_symbol.starts_with("spawn:"))
            .collect();
        assert!(
            !async_edges.is_empty(),
            "Should detect await/spawn expressions, got edges: {:?}",
            file.dataflow_edges
                .iter()
                .map(|e| &e.from_symbol)
                .collect::<Vec<_>>()
        );

        // Verify await:fetch_data edge.
        assert!(
            file.dataflow_edges
                .iter()
                .any(|e| e.from_symbol == "await:fetch_data"),
            "Expected await:fetch_data edge"
        );

        // Verify spawn:tokio::spawn edge.
        assert!(
            file.dataflow_edges
                .iter()
                .any(|e| e.from_symbol == "spawn:tokio::spawn"),
            "Expected spawn:tokio::spawn edge"
        );
    }

    #[test]
    fn extracts_rust_dataflow_edges() {
        let source = r#"
fn process(data: Vec<u8>) -> String {
    let result = transform(data);
    let count = result.len();
    output = format_output(result, count);
    output
}
"#;
        let extracted = extract_rust_symbols(source).unwrap();

        let has_read = |sym: &str| {
            extracted
                .dataflow_edges
                .iter()
                .any(|e| e.from_symbol == sym && matches!(e.flow_type, DataFlowType::Reads))
        };
        let has_write = |sym: &str| {
            extracted
                .dataflow_edges
                .iter()
                .any(|e| e.from_symbol == sym && matches!(e.flow_type, DataFlowType::Writes))
        };

        // let result = transform(data)
        assert!(has_write("result"), "expected write edge for 'result'");
        assert!(has_read("transform"), "expected read edge for 'transform'");
        assert!(has_read("data"), "expected read edge for 'data'");

        // let count = result.len()
        assert!(has_write("count"), "expected write edge for 'count'");

        // output = format_output(result, count)
        assert!(has_write("output"), "expected write edge for 'output'");
        assert!(
            has_read("format_output"),
            "expected read edge for 'format_output'"
        );
    }
}

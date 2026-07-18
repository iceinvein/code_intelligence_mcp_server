use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::convex;
use super::elysia::extract_elysia_patterns;
use super::express::extract_express_patterns;
use super::fastify::extract_fastify_patterns;
use super::hono::extract_hono_patterns;
use super::nestjs::extract_nestjs_patterns;
use super::nextjs;
use super::symbol::{
    ByteSpan, DataFlowEdge, DataFlowType, DecoratorEntry, DecoratorType, ExtractedFile,
    ExtractedSymbol, Import, JSDocEntry, JSDocParam, LineSpan, ModuleBinding, ModuleBindingKind,
    SymbolKind, TodoEntry, TodoKind,
};
use super::trpc::extract_trpc_patterns;

pub fn extract_typescript_symbols(language_id: LanguageId, source: &str) -> Result<ExtractedFile> {
    extract_typescript_symbols_with_path(language_id, source, "<unknown>")
}

pub fn extract_typescript_symbols_with_path(
    language_id: LanguageId,
    source: &str,
    file_path: &str,
) -> Result<ExtractedFile> {
    if !matches!(language_id, LanguageId::Typescript | LanguageId::Tsx) {
        return Err(anyhow!("LanguageId must be Typescript or Tsx"));
    }

    let mut parser = parser_for_id(language_id)?;
    extract_symbols_with_parser(&mut parser, source, file_path, language_id)
}

fn extract_symbols_with_parser(
    parser: &mut Parser,
    source: &str,
    file_path: &str,
    language_id: LanguageId,
) -> Result<ExtractedFile> {
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse source"))?;
    let root = tree.root_node();

    let cursor = root.walk();
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut module_bindings = Vec::new();
    let mut type_edges = Vec::new();
    let mut dataflow_edges = Vec::new();

    walk(cursor, &mut |node| {
        let kind = node.kind();
        match kind {
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    let sym =
                        symbol_from_node(name.clone(), SymbolKind::Function, exported, def_node);
                    symbols.push(sym);
                    extract_function_signature_types(node, source, &name, &mut type_edges);
                    extract_dataflow_from_function_body(node, source, &name, &mut dataflow_edges);
                }
            }
            "method_definition" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    // Methods are not "exported" from the module, so false.
                    // We define their symbol kind as Function for now (or maybe add Method if needed, but Function is fine).
                    symbols.push(symbol_from_node(
                        name.clone(),
                        SymbolKind::Function,
                        false,
                        node,
                    ));
                    extract_function_signature_types(node, source, &name, &mut type_edges);
                    extract_dataflow_from_function_body(node, source, &name, &mut dataflow_edges);
                }
            }
            "arrow_function" => {
                // Extract data flow from arrow functions that are part of const declarations
                // These will be handled when we process the parent lexical_declaration
            }
            "class_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    symbols.push(symbol_from_node(
                        name.clone(),
                        SymbolKind::Class,
                        exported,
                        def_node,
                    ));
                    // We don't extract types for the class itself (generics?) yet,
                    // but we could. For now, we rely on methods.
                }
            }
            "interface_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    symbols.push(symbol_from_node(
                        name.clone(),
                        SymbolKind::Interface,
                        exported,
                        def_node,
                    ));
                    extract_interface_types(node, source, &name, &mut type_edges);
                }
            }
            "type_alias_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    symbols.push(symbol_from_node(
                        name.clone(),
                        SymbolKind::TypeAlias,
                        exported,
                        def_node,
                    ));
                    extract_type_alias_types(node, source, &name, &mut type_edges);
                }
            }
            "enum_declaration" => {
                if let Some(name) = symbol_name_from_declaration(node, source) {
                    let (def_node, exported) = definition_node_for_declaration(node);
                    symbols.push(symbol_from_node(name, SymbolKind::Enum, exported, def_node));
                }
            }
            "lexical_declaration" => {
                // Only extract as a symbol when at module/namespace scope —
                // local variables inside function bodies are not top-level symbols.
                if !super::is_inside_function_scope(node) {
                    extract_const_declarators(node, source, &mut symbols, &mut type_edges);
                }
                // Dataflow extraction ALWAYS runs (used by graph engine, not search)
                extract_dataflow_from_lexical_declaration(
                    node,
                    source,
                    &symbols,
                    &mut dataflow_edges,
                );
            }
            "import_statement" => {
                extract_imports(node, source, &mut imports);
                extract_import_bindings(node, source, &mut module_bindings);
            }
            "export_statement" => {
                // handle export ... from ...
                if node.child_by_field_name("source").is_some() {
                    extract_imports(node, source, &mut imports);
                }
                if default_export_local_name(node, source).as_deref() == Some("default") {
                    symbols.push(symbol_from_node(
                        "default".to_string(),
                        SymbolKind::Const,
                        true,
                        node,
                    ));
                }
                extract_export_bindings(node, source, &mut module_bindings);
            }
            "object" => {
                // Object literal: emit Property symbols for hook-shaped or
                // function-valued or exported-const-literal properties. The
                // walk visits every object literal, but property emission is
                // gated to keep symbol count bounded.
                extract_object_literal_properties(node, source, &mut symbols);
            }
            _ => {}
        }
    });

    symbols.sort_by_key(|s| s.bytes.start);

    // Create a new cursor from root for later extractions
    let cursor = root.walk();

    // Extract TODO/FIXME comments
    let todos = extract_todo_comments(cursor, source, file_path);

    // Extract JSDoc entries
    let jsdoc_entries = extract_jsdoc_entries(&symbols, source, file_path, language_id);

    // Extract decorators
    let cursor = root.walk();
    let decorators = extract_decorators_for_symbols(&symbols, source, cursor);

    // Extract framework patterns from all supported frameworks
    let mut framework_patterns = Vec::new();
    framework_patterns.extend(extract_elysia_patterns(root, source));
    framework_patterns.extend(extract_hono_patterns(root, source));
    framework_patterns.extend(extract_express_patterns(root, source));
    framework_patterns.extend(extract_fastify_patterns(root, source));
    framework_patterns.extend(extract_nestjs_patterns(root, source));
    framework_patterns.extend(extract_trpc_patterns(root, source));
    if nextjs::is_nextjs_convention_file(file_path) {
        framework_patterns.extend(nextjs::extract_nextjs_patterns(root, source, file_path));
    }
    if convex::is_convex_file(file_path) {
        framework_patterns.extend(convex::extract_convex_patterns(root, source, file_path));
    }

    Ok(ExtractedFile {
        symbols,
        imports,
        module_bindings,
        type_edges,
        extends_edges: Vec::new(),
        dataflow_edges,
        todos,
        jsdoc_entries,
        decorators,
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

fn extract_function_signature_types(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>, // (parent_name, type_name)
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // Parameters
        if child.kind() == "formal_parameters" {
            let mut p_cursor = child.walk();
            for param in child.children(&mut p_cursor) {
                // p: Type
                if param.kind() == "required_parameter" || param.kind() == "optional_parameter" {
                    if let Some(type_node) = param.child_by_field_name("type") {
                        // type_annotation -> type identifier
                        extract_types_from_annotation(type_node, source, parent_name, out);
                    }
                }
            }
        }
        // Return type
        if child.kind() == "type_annotation"
            && child.prev_sibling().map(|n| n.kind()) == Some("formal_parameters")
        {
            extract_types_from_annotation(child, source, parent_name, out);
        }
    }
}

fn extract_types_from_annotation(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    // If the node itself is a type identifier (e.g. from a type alias value)
    if node.kind() == "type_identifier" || node.kind() == "predefined_type" {
        let type_name = text_for_node(node, source);
        out.push((parent_name.to_string(), type_name));
        return;
    }

    // node is "type_annotation", child is the actual type
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_identifier" || child.kind() == "predefined_type" {
            let type_name = text_for_node(child, source);
            out.push((parent_name.to_string(), type_name));
        }
        // recursive for generics? e.g. Promise<User>
        if child.kind() == "generic_type" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let type_name = text_for_node(name_node, source);
                out.push((parent_name.to_string(), type_name));
            }
            if let Some(args) = child.child_by_field_name("type_arguments") {
                extract_types_from_annotation(args, source, parent_name, out);
            }
        }
        if child.kind() == "type_arguments" {
            let mut arg_cursor = child.walk();
            for arg in child.children(&mut arg_cursor) {
                extract_types_from_annotation(arg, source, parent_name, out);
            }
        }
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
    // Tree-sitter wraps a module-level exported declaration directly in an
    // export_statement. Walking arbitrary ancestors incorrectly marks nested
    // declarations as exported and gives them the outer declaration's span.
    if let Some(parent) = node.parent() {
        if parent.kind() == "export_statement" {
            return (parent, true);
        }
    }
    (node, false)
}

fn extract_const_declarators(
    node: Node<'_>,
    source: &str,
    out: &mut Vec<ExtractedSymbol>,
    type_edges: &mut Vec<(String, String)>,
) {
    if !is_const_lexical_declaration(node) {
        return;
    }

    let export_statement = export_ancestor(node);
    let exported = export_statement.is_some();
    let mut cursor = node.walk();

    // Only iterate direct children of the lexical_declaration — do NOT
    // recurse into value expressions (arrow function bodies, etc.).
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" && child.child_by_field_name("value").is_some() {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = text_for_node(name_node, source);
                let def_node = const_definition_node(export_statement, node, child);
                out.push(symbol_from_node(
                    name.clone(),
                    SymbolKind::Const,
                    exported,
                    def_node,
                ));

                // Check if value is an arrow function to extract signature types
                if let Some(value_node) = child.child_by_field_name("value") {
                    if value_node.kind() == "arrow_function" {
                        extract_function_signature_types(value_node, source, &name, type_edges);
                    }
                }
            }
        }
    }
}

/// Detect hook-shaped property identifiers (onX/beforeX/afterX/willX/didX/handleX).
/// Used to gate `extract_object_literal_properties` so we only emit
/// Property symbols for callback-style keys and don't explode the
/// symbol count with every config option that happens to be in an
/// object literal.
fn is_hook_shaped_property_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    for prefix in ["on", "before", "after", "will", "did", "handle"] {
        if name.starts_with(prefix) && bytes.len() > prefix.len() {
            let next_byte = bytes[prefix.len()];
            if next_byte.is_ascii_uppercase() {
                return true;
            }
        }
    }
    false
}

/// Walk a single `object` (object literal) node and emit `Property`
/// symbols for its pair children. Three independent acceptance gates --
/// any one passing is enough -- keep the symbol count bounded:
///
///   1. The key is hook-shaped (`onX/beforeX/afterX/willX/didX/handleX`).
///   2. The value is a function expression (`arrow_function`,
///      `function_expression`, `function`, or method shorthand).
///   3. The enclosing const declaration is `export`ed AND the value is
///      a primitive literal (string/number/true/false/null) -- this is
///      the "channel constant" pattern (`export const IPC = { SESSION_MESSAGE: 'session:message' }`).
///
/// Properties whose key is not a `property_identifier` (computed keys,
/// string keys, etc.) are skipped to avoid name collisions.
fn extract_object_literal_properties(
    object_node: Node<'_>,
    source: &str,
    out: &mut Vec<ExtractedSymbol>,
) {
    let in_exported_const = is_inside_exported_const(object_node);

    let mut cursor = object_node.walk();
    for child in object_node.children(&mut cursor) {
        if child.kind() != "pair" {
            continue;
        }
        let Some(key_node) = child.child_by_field_name("key") else {
            continue;
        };
        if key_node.kind() != "property_identifier" {
            continue;
        }
        let name = text_for_node(key_node, source);
        let value_node = child.child_by_field_name("value");
        let value_kind = value_node.map(|v| v.kind());

        let is_function_value = matches!(
            value_kind,
            Some("arrow_function") | Some("function_expression") | Some("function")
        );
        let is_literal_value = matches!(
            value_kind,
            Some("string")
                | Some("number")
                | Some("true")
                | Some("false")
                | Some("null")
                | Some("template_string")
        );

        let accept = is_hook_shaped_property_name(&name)
            || is_function_value
            || (in_exported_const && is_literal_value);
        if !accept {
            continue;
        }

        // Span: from the key through the value (or just the key if no value).
        let span_node = value_node
            .map(|v| {
                if v.start_byte() < key_node.start_byte() {
                    key_node
                } else {
                    child
                }
            })
            .unwrap_or(key_node);
        // Properties on object literals are not directly "exported" --
        // export status flows from the enclosing const. We surface this
        // through the in_exported_const flag at gating time only.
        out.push(symbol_from_node(
            name,
            SymbolKind::Property,
            in_exported_const,
            span_node,
        ));
    }
}

/// Walk up the AST from an `object` node to determine whether the
/// nearest enclosing const declaration is exported. Returns false if
/// the object is not the right-hand-side of a const declarator.
fn is_inside_exported_const(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "variable_declarator" => {
                // Walk further up to find the lexical_declaration and
                // its export wrapper.
                let mut up = parent.parent();
                while let Some(p) = up {
                    match p.kind() {
                        "lexical_declaration" => {
                            return is_const_lexical_declaration(p) && export_ancestor(p).is_some();
                        }
                        "export_statement" => return true,
                        _ => up = p.parent(),
                    }
                }
                return false;
            }
            "lexical_declaration" => {
                return is_const_lexical_declaration(parent) && export_ancestor(parent).is_some();
            }
            // Stop ascending once we leave the declaration context.
            "function_declaration"
            | "method_definition"
            | "arrow_function"
            | "function_expression"
            | "class_body"
            | "program" => return false,
            _ => current = parent.parent(),
        }
    }
    false
}

fn extract_interface_types(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "interface_body" {
            let mut body_cursor = child.walk();
            for member in child.children(&mut body_cursor) {
                // property_signature: name, type_annotation
                if member.kind() == "property_signature" {
                    if let Some(type_node) = member.child_by_field_name("type") {
                        extract_types_from_annotation(type_node, source, parent_name, out);
                    }
                }
                // method_signature: name, formal_parameters, type_annotation
                if member.kind() == "method_signature" {
                    // Extract param types
                    if let Some(params) = member.child_by_field_name("parameters") {
                        // reuse logic? extract_function_signature_types iterates over children of node looking for formal_parameters
                        // Here params IS formal_parameters.
                        // We can manually iterate.
                        let mut p_cursor = params.walk();
                        for param in params.children(&mut p_cursor) {
                            if param.kind() == "required_parameter"
                                || param.kind() == "optional_parameter"
                            {
                                if let Some(type_node) = param.child_by_field_name("type") {
                                    extract_types_from_annotation(
                                        type_node,
                                        source,
                                        parent_name,
                                        out,
                                    );
                                }
                            }
                        }
                    }
                    // Extract return type
                    if let Some(ret_type) = member.child_by_field_name("type") {
                        extract_types_from_annotation(ret_type, source, parent_name, out);
                    }
                }
            }
        }
    }
}

fn extract_type_alias_types(
    node: Node<'_>,
    source: &str,
    parent_name: &str,
    out: &mut Vec<(String, String)>,
) {
    if let Some(value_node) = node.child_by_field_name("value") {
        extract_types_from_annotation(value_node, source, parent_name, out);
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

fn extract_imports(node: Node<'_>, source: &str, out: &mut Vec<Import>) {
    let Some(source_node) = node.child_by_field_name("source") else {
        return;
    };
    let source_path = text_for_node(source_node, source)
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string();

    // tree-sitter-typescript does NOT register field names on import_clause
    // children, so `child_by_field_name("name")` / `("named_imports")` /
    // `("namespace_import")` all return None for every TS file. Match by
    // node KIND instead, which is what the grammar actually exposes.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "import_clause" {
            let mut clause_cursor = child.walk();
            for clause_child in child.children(&mut clause_cursor) {
                match clause_child.kind() {
                    "identifier" => {
                        // default import: `import A from "..."`
                        out.push(Import {
                            name: "default".to_string(),
                            source: source_path.clone(),
                            alias: Some(text_for_node(clause_child, source)),
                            at_line: clause_child.start_position().row as u32 + 1,
                        });
                    }
                    "named_imports" => {
                        extract_import_specifiers(clause_child, source, &source_path, out);
                    }
                    "namespace_import" => {
                        // `import * as ns from "..."`
                        let mut ns_cursor = clause_child.walk();
                        let alias_node = clause_child
                            .children(&mut ns_cursor)
                            .find(|n| n.kind() == "identifier");
                        if let Some(alias_node) = alias_node {
                            out.push(Import {
                                name: "*".to_string(),
                                source: source_path.clone(),
                                alias: Some(text_for_node(alias_node, source)),
                                at_line: clause_child.start_position().row as u32 + 1,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        // export clause: `export { A } from "..."`
        if child.kind() == "export_clause" {
            extract_export_specifiers(child, source, &source_path, out);
        }
    }
}

fn extract_import_specifiers(
    node: Node<'_>,
    source: &str,
    source_path: &str,
    out: &mut Vec<Import>,
) {
    // tree-sitter-typescript does not register field names on
    // import_specifier children either. Walk identifier children: the
    // first identifier is the remote name, an optional second is the
    // local alias (`import { A as B }`).
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "import_specifier" {
            let mut spec_cursor = child.walk();
            let idents: Vec<_> = child
                .children(&mut spec_cursor)
                .filter(|n| n.kind() == "identifier")
                .collect();
            let Some(name_node) = idents.first() else {
                continue;
            };
            let name = text_for_node(*name_node, source);
            let alias = idents
                .get(1)
                .map(|alias_node| text_for_node(*alias_node, source));

            out.push(Import {
                name,
                source: source_path.to_string(),
                alias,
                at_line: child.start_position().row as u32 + 1,
            });
        }
    }
}

fn extract_export_specifiers(
    node: Node<'_>,
    source: &str,
    source_path: &str,
    out: &mut Vec<Import>,
) {
    // Same field-name caveat as extract_import_specifiers above.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "export_specifier" {
            let mut spec_cursor = child.walk();
            let name_node = child
                .children(&mut spec_cursor)
                .find(|n| n.kind() == "identifier");
            let Some(name_node) = name_node else {
                continue;
            };
            let name = text_for_node(name_node, source);

            out.push(Import {
                name,
                source: source_path.to_string(),
                alias: None,
                at_line: child.start_position().row as u32 + 1,
            });
        }
    }
}

pub(super) fn extract_import_bindings(node: Node<'_>, source: &str, out: &mut Vec<ModuleBinding>) {
    let Some(source_node) = node.child_by_field_name("source") else {
        return;
    };
    let source_path = text_for_node(source_node, source)
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "import_clause" {
            continue;
        }
        let mut clause_cursor = child.walk();
        for clause_child in child.children(&mut clause_cursor) {
            match clause_child.kind() {
                "identifier" => out.push(ModuleBinding {
                    kind: ModuleBindingKind::Import,
                    source: source_path.clone(),
                    imported_name: "default".to_string(),
                    local_name: text_for_node(clause_child, source),
                    exported_name: String::new(),
                    at_line: clause_child.start_position().row as u32 + 1,
                }),
                "named_imports" => {
                    let mut named_cursor = clause_child.walk();
                    for specifier in clause_child.children(&mut named_cursor) {
                        if specifier.kind() != "import_specifier" {
                            continue;
                        }
                        let mut spec_cursor = specifier.walk();
                        let identifiers = specifier
                            .children(&mut spec_cursor)
                            .filter(|node| node.kind() == "identifier")
                            .collect::<Vec<_>>();
                        let Some(imported) = identifiers.first() else {
                            continue;
                        };
                        let imported_name = text_for_node(*imported, source);
                        let local_name = identifiers
                            .get(1)
                            .map(|node| text_for_node(*node, source))
                            .unwrap_or_else(|| imported_name.clone());
                        out.push(ModuleBinding {
                            kind: ModuleBindingKind::Import,
                            source: source_path.clone(),
                            imported_name,
                            local_name,
                            exported_name: String::new(),
                            at_line: specifier.start_position().row as u32 + 1,
                        });
                    }
                }
                "namespace_import" => {
                    let mut ns_cursor = clause_child.walk();
                    if let Some(alias) = clause_child
                        .children(&mut ns_cursor)
                        .find(|node| node.kind() == "identifier")
                    {
                        out.push(ModuleBinding {
                            kind: ModuleBindingKind::Import,
                            source: source_path.clone(),
                            imported_name: "*".to_string(),
                            local_name: text_for_node(alias, source),
                            exported_name: String::new(),
                            at_line: clause_child.start_position().row as u32 + 1,
                        });
                    };
                }
                _ => {}
            }
        }
    }
}

pub(super) fn extract_export_bindings(node: Node<'_>, source: &str, out: &mut Vec<ModuleBinding>) {
    if let Some(local_name) = default_export_local_name(node, source) {
        out.push(ModuleBinding {
            kind: ModuleBindingKind::Export,
            source: String::new(),
            imported_name: String::new(),
            local_name,
            exported_name: "default".to_string(),
            at_line: node.start_position().row as u32 + 1,
        });
        return;
    }

    let source_path = node
        .child_by_field_name("source")
        .map(|source_node| {
            text_for_node(source_node, source)
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string()
        })
        .unwrap_or_default();

    let mut found_clause = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "export_clause" {
            continue;
        }
        found_clause = true;
        let mut clause_cursor = child.walk();
        for specifier in child.children(&mut clause_cursor) {
            if specifier.kind() != "export_specifier" {
                continue;
            }
            let mut spec_cursor = specifier.walk();
            let identifiers = specifier
                .children(&mut spec_cursor)
                .filter(|node| node.kind() == "identifier")
                .collect::<Vec<_>>();
            let Some(local) = identifiers.first() else {
                continue;
            };
            let local_name = text_for_node(*local, source);
            let exported_name = identifiers
                .get(1)
                .map(|node| text_for_node(*node, source))
                .unwrap_or_else(|| local_name.clone());
            out.push(ModuleBinding {
                kind: if source_path.is_empty() {
                    ModuleBindingKind::Export
                } else {
                    ModuleBindingKind::ReExport
                },
                source: source_path.clone(),
                imported_name: if source_path.is_empty() {
                    String::new()
                } else {
                    local_name.clone()
                },
                local_name: if source_path.is_empty() {
                    local_name
                } else {
                    String::new()
                },
                exported_name,
                at_line: specifier.start_position().row as u32 + 1,
            });
        }
    }

    if source_path.is_empty() || found_clause {
        return;
    }

    // `export * from` and `export * as Namespace from` do not contain an
    // export_clause in tree-sitter-typescript. Parse only the small prefix
    // before `from`; the module source still comes from the AST field above.
    let statement = text_for_node(node, source);
    let export_head = statement
        .strip_prefix("export")
        .and_then(|rest| rest.split_once("from").map(|(head, _)| head.trim()))
        .unwrap_or_default();
    if let Some(namespace) = export_head.strip_prefix("* as ") {
        out.push(ModuleBinding {
            kind: ModuleBindingKind::ReExport,
            source: source_path,
            imported_name: "*".to_string(),
            local_name: String::new(),
            exported_name: namespace.trim().to_string(),
            at_line: node.start_position().row as u32 + 1,
        });
    } else if export_head == "*" {
        out.push(ModuleBinding {
            kind: ModuleBindingKind::ExportAll,
            source: source_path,
            imported_name: "*".to_string(),
            local_name: String::new(),
            exported_name: "*".to_string(),
            at_line: node.start_position().row as u32 + 1,
        });
    }
}

/// Return the local declaration/expression name behind an ECMAScript default
/// export. Anonymous declarations and expressions receive the stable public
/// identity `default`, while `export default Worker` and named declarations
/// retain their local symbol name.
pub(super) fn default_export_local_name(node: Node<'_>, source: &str) -> Option<String> {
    let statement = text_for_node(node, source);
    let after_export = statement.trim_start().strip_prefix("export")?.trim_start();
    if !after_export.starts_with("default") {
        return None;
    }
    let boundary = after_export.as_bytes().get("default".len()).copied();
    if boundary.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        return None;
    }

    if let Some(declaration) = node.child_by_field_name("declaration") {
        return Some(
            declaration
                .child_by_field_name("name")
                .map(|name| text_for_node(name, source))
                .unwrap_or_else(|| "default".to_string()),
        );
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor).filter(|child| child.is_named()) {
        match child.kind() {
            "function_declaration" | "generator_function_declaration" | "class_declaration" => {
                return Some(
                    child
                        .child_by_field_name("name")
                        .map(|name| text_for_node(name, source))
                        .unwrap_or_else(|| "default".to_string()),
                );
            }
            "identifier" => return Some(text_for_node(child, source)),
            _ => {}
        }
    }

    Some("default".to_string())
}

/// Extract data flow edges from function/method bodies
/// Tracks reads and writes of identifiers within function scopes
fn extract_dataflow_from_function_body(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    // Find the statement block (body) of the function
    let body = match node.child_by_field_name("body") {
        Some(b) if b.kind() == "statement_block" => b,
        _ => return,
    };

    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        extract_dataflow_from_node(child, source, context_name, out);
    }
}

/// Extract data flow from lexical declarations (const/let)
/// Handles arrow functions and direct assignments
fn extract_dataflow_from_lexical_declaration(
    node: Node<'_>,
    source: &str,
    _symbols: &[ExtractedSymbol],
    out: &mut Vec<DataFlowEdge>,
) {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            // Get the name being declared
            let name = if let Some(name_node) = child.child_by_field_name("name") {
                text_for_node(name_node, source)
            } else {
                continue;
            };

            // Check if this is an arrow function (we want to extract dataflow from its body)
            if let Some(value_node) = child.child_by_field_name("value") {
                if value_node.kind() == "arrow_function" {
                    // Extract data flow from arrow function body using the const name as context
                    extract_dataflow_from_arrow_function(value_node, source, &name, out);
                } else {
                    // For non-arrow function values, track what's being read to initialize this
                    extract_reads_from_expression(value_node, source, &name, out);
                    // Track write to the variable being declared
                    out.push(DataFlowEdge {
                        from_symbol: name.clone(),
                        to_symbol: "<scope>".to_string(),
                        flow_type: DataFlowType::Writes,
                        at_line: node.start_position().row as u32 + 1,
                        scope: Some(name.clone()),
                    });
                }
            }
        }
    }
}

/// Extract data flow from arrow function body
fn extract_dataflow_from_arrow_function(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    let body = match node.child_by_field_name("body") {
        Some(b) => b,
        None => return,
    };

    // Arrow function body can be a statement_block or a single expression
    if body.kind() == "statement_block" {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            extract_dataflow_from_node(child, source, context_name, out);
        }
    } else {
        // Single expression body
        extract_reads_from_expression(body, source, context_name, out);
    }
}

/// Extract the full dotted name from a member_expression node, e.g. "Promise.all".
/// Returns `None` when the expression is not a simple `Object.property` form.
fn extract_member_expression_full_name(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() != "member_expression" {
        return None;
    }
    let obj = node.child_by_field_name("object")?;
    let prop = node.child_by_field_name("property")?;
    if obj.kind() == "identifier" && prop.kind() == "property_identifier" {
        Some(format!(
            "{}.{}",
            text_for_node(obj, source),
            text_for_node(prop, source)
        ))
    } else {
        None
    }
}

/// Recursively extract data flow from a node
fn extract_dataflow_from_node(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    match node.kind() {
        "assignment_expression" => {
            extract_dataflow_from_assignment(node, source, context_name, out);
        }
        "await_expression" => {
            let line = node.start_position().row as u32 + 1;
            // The child of an await_expression is the expression being awaited.
            // It is typically a call_expression; extract its callee name.
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    // Skip the "await" keyword token itself.
                    if child.kind() == "await" {
                        if !cursor.goto_next_sibling() {
                            break;
                        }
                        continue;
                    }
                    // Extract callee name from a call_expression child.
                    if child.kind() == "call_expression" {
                        if let Some(func) = child.child_by_field_name("function") {
                            if let Some(callee) = extract_callee_name(func, source) {
                                out.push(DataFlowEdge {
                                    from_symbol: format!("await:{callee}"),
                                    to_symbol: context_name.to_string(),
                                    flow_type: DataFlowType::Reads,
                                    at_line: line,
                                    scope: Some(context_name.to_string()),
                                });
                            }
                        }
                        // Also recurse into the awaited call's arguments.
                        extract_dataflow_from_node(child, source, context_name, out);
                    } else {
                        // For other awaited expressions (identifier, member_expression, etc.)
                        // emit a generic await edge using whatever text we can extract.
                        let name = text_for_node(child, source);
                        if !name.is_empty() {
                            out.push(DataFlowEdge {
                                from_symbol: format!("await:{name}"),
                                to_symbol: context_name.to_string(),
                                flow_type: DataFlowType::Reads,
                                at_line: line,
                                scope: Some(context_name.to_string()),
                            });
                        }
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
        "call_expression" => {
            // Before delegating to the generic call handler, check for Promise.all/race/allSettled.
            let is_promise_combinator = if let Some(func) = node.child_by_field_name("function") {
                if let Some(full_name) = extract_member_expression_full_name(func, source) {
                    matches!(
                        full_name.as_str(),
                        "Promise.all" | "Promise.race" | "Promise.allSettled" | "Promise.any"
                    )
                } else {
                    false
                }
            } else {
                false
            };

            if is_promise_combinator {
                if let Some(func) = node.child_by_field_name("function") {
                    if let Some(full_name) = extract_member_expression_full_name(func, source) {
                        let line = node.start_position().row as u32 + 1;
                        out.push(DataFlowEdge {
                            from_symbol: format!("spawn:{full_name}"),
                            to_symbol: context_name.to_string(),
                            flow_type: DataFlowType::Reads,
                            at_line: line,
                            scope: Some(context_name.to_string()),
                        });
                    }
                }
            }

            extract_dataflow_from_call(node, source, context_name, out);
        }
        "statement_block" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_dataflow_from_node(child, source, context_name, out);
            }
        }
        "if_statement" | "for_statement" | "while_statement" | "do_statement" => {
            // Handle control flow bodies
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind().ends_with("body")
                    || child.kind() == "consequence"
                    || child.kind() == "alternative"
                {
                    extract_dataflow_from_node(child, source, context_name, out);
                }
            }
        }
        _ => {
            // Recursively process children to find nested assignments/calls
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    extract_dataflow_from_node(cursor.node(), source, context_name, out);
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
    }
}

/// Extract data flow from assignment expressions
/// Pattern: left = right  -> left is written, identifiers in right are read
fn extract_dataflow_from_assignment(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    let left = match node.child_by_field_name("left") {
        Some(l) => l,
        None => return,
    };
    let right = match node.child_by_field_name("right") {
        Some(r) => r,
        None => return,
    };

    let line = node.start_position().row as u32 + 1;

    // Extract what's being written (left side)
    if let Some(name) = extract_identifier_from_assignment_left(left, source) {
        out.push(DataFlowEdge {
            from_symbol: name,
            to_symbol: context_name.to_string(),
            flow_type: DataFlowType::Writes,
            at_line: line,
            scope: Some(context_name.to_string()),
        });
    }

    // Extract what's being read (right side)
    for ident in extract_identifiers_from_expression(right, source) {
        out.push(DataFlowEdge {
            from_symbol: ident,
            to_symbol: context_name.to_string(),
            flow_type: DataFlowType::Reads,
            at_line: line,
            scope: Some(context_name.to_string()),
        });
    }
}

/// Extract data flow from function/method calls
fn extract_dataflow_from_call(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    let line = node.start_position().row as u32 + 1;

    // The function being called is being read
    if let Some(func_node) = node.child_by_field_name("function") {
        if let Some(name) = extract_callee_name(func_node, source) {
            out.push(DataFlowEdge {
                from_symbol: name,
                to_symbol: context_name.to_string(),
                flow_type: DataFlowType::Reads,
                at_line: line,
                scope: Some(context_name.to_string()),
            });
        }
    }

    // Arguments are being read
    if let Some(args_node) = node.child_by_field_name("arguments") {
        let mut cursor = args_node.walk();
        for child in args_node.children(&mut cursor) {
            for ident in extract_identifiers_from_expression(child, source) {
                out.push(DataFlowEdge {
                    from_symbol: ident,
                    to_symbol: context_name.to_string(),
                    flow_type: DataFlowType::Reads,
                    at_line: line,
                    scope: Some(context_name.to_string()),
                });
            }
        }
    }
}

/// Extract all identifiers read from an expression (right side of assignments, arguments, etc.)
fn extract_reads_from_expression(
    node: Node<'_>,
    source: &str,
    context_name: &str,
    out: &mut Vec<DataFlowEdge>,
) {
    let line = node.start_position().row as u32 + 1;
    for ident in extract_identifiers_from_expression(node, source) {
        out.push(DataFlowEdge {
            from_symbol: ident,
            to_symbol: context_name.to_string(),
            flow_type: DataFlowType::Reads,
            at_line: line,
            scope: Some(context_name.to_string()),
        });
    }
}

/// Extract identifier name from the left side of an assignment
/// Handles: identifier, member_expression (obj.prop), etc.
fn extract_identifier_from_assignment_left(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_for_node(node, source)),
        "member_expression" => {
            // For obj.prop, we track the object being accessed
            if let Some(obj_node) = node.child_by_field_name("object") {
                if obj_node.kind() == "identifier" {
                    Some(text_for_node(obj_node, source))
                } else {
                    extract_identifier_from_assignment_left(obj_node, source)
                }
            } else {
                None
            }
        }
        "array_pattern" | "object_pattern" => {
            // Destructuring: extract identifiers from the pattern
            let mut ids = Vec::new();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    ids.push(text_for_node(child, source));
                } else if child.kind() == "pair" {
                    // Object destructuring with key: value
                    if let Some(value_node) = child.child_by_field_name("value") {
                        if value_node.kind() == "identifier" {
                            ids.push(text_for_node(value_node, source));
                        }
                    }
                }
            }
            // Return first identifier or join them
            ids.into_iter().next()
        }
        _ => None,
    }
}

/// Extract all identifiers from an expression
/// Recursively finds identifiers in nested expressions
fn extract_identifiers_from_expression(node: Node<'_>, source: &str) -> Vec<String> {
    let mut identifiers = Vec::new();

    match node.kind() {
        "identifier" => {
            identifiers.push(text_for_node(node, source));
        }
        "member_expression" => {
            // Extract object being accessed
            if let Some(obj_node) = node.child_by_field_name("object") {
                identifiers.extend(extract_identifiers_from_expression(obj_node, source));
            }
            // Extract property if it's a computed property
            if let Some(prop_node) = node.child_by_field_name("property") {
                if prop_node.kind() == "identifier"
                    && node
                        .child_by_field_name("object")
                        .is_some_and(|o| o.kind() != "member_expression")
                {
                    // Only add property if it's not part of a chain
                }
                identifiers.extend(extract_identifiers_from_expression(prop_node, source));
            }
        }
        "call_expression" => {
            // Extract function being called
            if let Some(func_node) = node.child_by_field_name("function") {
                if let Some(name) = extract_callee_name(func_node, source) {
                    identifiers.push(name);
                }
            }
            // Extract arguments
            if let Some(args_node) = node.child_by_field_name("arguments") {
                let mut cursor = args_node.walk();
                for child in args_node.children(&mut cursor) {
                    identifiers.extend(extract_identifiers_from_expression(child, source));
                }
            }
        }
        "binary_expression" | "unary_expression" | "logical_expression" => {
            // Process both sides
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                identifiers.extend(extract_identifiers_from_expression(child, source));
            }
        }
        "parenthesized_expression" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                identifiers.extend(extract_identifiers_from_expression(child, source));
            }
        }
        "array" | "array_expression" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                identifiers.extend(extract_identifiers_from_expression(child, source));
            }
        }
        "object" | "object_expression" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "pair" {
                    if let Some(value_node) = child.child_by_field_name("value") {
                        identifiers.extend(extract_identifiers_from_expression(value_node, source));
                    }
                }
            }
        }
        "arrow_function" | "function_expression" => {
            // Don't extract identifiers from nested function declarations
            // They are separate scopes
        }
        _ => {
            // Recursively process children for unknown node types
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    identifiers.extend(extract_identifiers_from_expression(cursor.node(), source));
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
    }

    identifiers
}

/// Extract the function name from a callee node
/// Handles: identifier, member_expression (obj.method), etc.
fn extract_callee_name(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_for_node(node, source)),
        "member_expression" => {
            // For obj.method(), return the method name
            if let Some(prop_node) = node.child_by_field_name("property") {
                if prop_node.kind() == "property_identifier" {
                    return Some(text_for_node(prop_node, source));
                }
            }
            // Otherwise return the object
            if let Some(obj_node) = node.child_by_field_name("object") {
                return extract_callee_name(obj_node, source);
            }
            None
        }
        _ => None,
    }
}

/// Check if comment line contains TODO or FIXME
fn is_todo_comment(line: &str) -> Option<(TodoKind, String)> {
    let lower = line.to_lowercase();

    // Match "TODO:" or "TODO " patterns
    if let Some(pos) = lower.find("todo") {
        let after_todo = &lower[pos..];
        if after_todo.starts_with("todo:") || after_todo.starts_with("todo ") {
            let text = extract_todo_text(line, pos, "TODO");
            return Some((TodoKind::Todo, text));
        }
    }

    // Match "FIXME:" or "FIXME " patterns
    if let Some(pos) = lower.find("fixme") {
        let after_fixme = &lower[pos..];
        if after_fixme.starts_with("fixme:") || after_fixme.starts_with("fixme ") {
            let text = extract_todo_text(line, pos, "FIXME");
            return Some((TodoKind::Fixme, text));
        }
    }

    None
}

/// Extract the text portion of a TODO comment
fn extract_todo_text(line: &str, keyword_start: usize, keyword: &str) -> String {
    // Find the end of the keyword
    let keyword_end = keyword_start + keyword.len();

    // Get the rest of the line after the keyword and its separator
    let rest = if keyword_end < line.len() {
        let after_keyword = &line[keyword_end..];
        // Skip colon, spaces, or dash
        let trimmed = after_keyword
            .trim_start_matches(':')
            .trim_start_matches(' ')
            .trim_start_matches('-')
            .trim_start_matches('-')
            .trim();
        trimmed.to_string()
    } else {
        String::new()
    };

    // Also remove leading comment markers
    let cleaned = rest
        .trim_start_matches("//")
        .trim_start_matches("/*")
        .trim_start_matches("*")
        .trim()
        .to_string();

    cleaned
}

/// Extract TODO/FIXME comments from the file
fn extract_todo_comments(cursor: TreeCursor, source: &str, file_path: &str) -> Vec<TodoEntry> {
    let mut todos = Vec::new();

    // First pass: find all comment nodes
    let mut comment_nodes = Vec::new();

    fn collect_comments<'a>(node: Node<'a>, comments: &mut Vec<Node<'a>>) {
        if node.kind() == "comment" || node.kind() == "block_comment" {
            comments.push(node);
        }
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                collect_comments(cursor.node(), comments);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    collect_comments(cursor.node(), &mut comment_nodes);

    // Process each comment
    for comment_node in comment_nodes {
        let text = text_for_node(comment_node, source);
        let line_num = comment_node.start_position().row as u32;

        // Check each line for TODO/FIXME
        for line in text.lines() {
            if let Some((kind, todo_text)) = is_todo_comment(line) {
                // Find next symbol after this TODO for association
                let mut current = comment_node;
                let associated_symbol = loop {
                    current = match current.next_sibling() {
                        Some(s) => s,
                        None => break None,
                    };

                    // Skip non-named nodes (whitespace, punctuation)
                    if !current.is_named() {
                        continue;
                    }

                    // Found a named node - try to get its symbol name
                    match current.kind() {
                        "function_declaration"
                        | "class_declaration"
                        | "interface_declaration"
                        | "type_alias_declaration"
                        | "enum_declaration"
                        | "lexical_declaration"
                        | "method_definition" => {
                            if let Some(name_node) = current.child_by_field_name("name") {
                                break Some(text_for_node(name_node, source));
                            }
                        }
                        _ => break None,
                    }
                };

                todos.push(TodoEntry {
                    kind,
                    text: todo_text,
                    file_path: file_path.to_string(),
                    line: line_num,
                    associated_symbol,
                });
            }
        }
    }

    todos
}

/// Find JSDoc comment immediately preceding a node
fn find_jsdoc_for_node(node: Node, source: &str) -> Option<String> {
    let mut current = node.prev_sibling();
    while let Some(sibling) = current {
        match sibling.kind() {
            "comment" | "block_comment" => {
                let text = text_for_node(sibling, source);
                // Check if it looks like JSDoc (starts with /**)
                if text.trim_start().starts_with("/**") {
                    return Some(text);
                }
                // For non-JSDoc comments, continue searching
                current = sibling.prev_sibling();
            }
            // Whitespace and unnamed nodes are OK to skip
            "" if !sibling.is_named() => {
                current = sibling.prev_sibling();
            }
            // Any other named node means no JSDoc attached
            _ => return None,
        }
    }
    None
}

/// Parse JSDoc tags from raw comment text
fn parse_jsdoc(raw: &str, symbol_id: &str) -> JSDocEntry {
    let mut params = Vec::new();
    let mut returns = None;
    let mut examples = Vec::new();
    let mut throws = Vec::new();
    let mut see_also = Vec::new();
    let mut since = None;
    let mut deprecated = false;
    let mut summary_lines = Vec::new();

    let mut current_example = None;

    for line in raw.lines() {
        let trimmed = line
            .trim_start_matches("/**")
            .trim_start_matches("*")
            .trim();

        // Skip empty lines during summary collection
        if trimmed.is_empty() && summary_lines.is_empty() && current_example.is_none() {
            continue;
        }

        // Check for tags
        if let Some(rest) = trimmed.strip_prefix("@") {
            // Flush summary if we hit first tag
            if !summary_lines.is_empty() && current_example.is_none() {
                // Summary done
            }

            if let Some(param_str) = rest.strip_prefix("param ") {
                // Parse @param {type} name description
                let parts: Vec<&str> = param_str.splitn(3, ' ').collect();
                if parts.len() >= 2 {
                    let type_anno = if parts[0].starts_with('{') {
                        parts[0].trim_start_matches('{').trim_end_matches('}')
                    } else {
                        ""
                    };
                    let name = if !type_anno.is_empty() {
                        parts[1]
                    } else {
                        parts[0]
                    };
                    let desc = if parts.len() > 2 { parts[2] } else { "" };
                    params.push(JSDocParam {
                        name: name.to_string(),
                        type_annotation: if !type_anno.is_empty() {
                            Some(type_anno.to_string())
                        } else {
                            None
                        },
                        description: if desc.is_empty() {
                            None
                        } else {
                            Some(desc.to_string())
                        },
                    });
                }
            } else if let Some(ret) = rest.strip_prefix("returns ") {
                returns = Some(ret.trim().to_string());
            } else if rest.starts_with("example") {
                current_example = Some(String::new());
            } else if rest.starts_with("deprecated") {
                deprecated = true;
            } else if let Some(thrown) = rest.strip_prefix("throws ") {
                throws.push(thrown.trim().to_string());
            } else if let Some(see) = rest.strip_prefix("see ") {
                see_also.push(see.trim().to_string());
            } else if let Some(ver) = rest.strip_prefix("since ") {
                since = Some(ver.trim().to_string());
            }
        } else if let Some(example) = &mut current_example {
            // Accumulate example lines
            if !trimmed.is_empty() {
                example.push_str(trimmed);
                example.push('\n');
            }
        } else if !trimmed.is_empty() && current_example.is_none() {
            // Accumulate summary
            summary_lines.push(trimmed);
        }
    }

    if let Some(example) = current_example {
        examples.push(example.trim().to_string());
    }

    let summary = if summary_lines.is_empty() {
        None
    } else {
        Some(summary_lines.join(" "))
    };

    JSDocEntry {
        symbol_id: symbol_id.to_string(),
        raw_text: raw.to_string(),
        summary,
        params,
        returns,
        examples,
        deprecated,
        throws,
        see_also,
        since,
    }
}

/// Extract JSDoc entries for all symbols in the file
fn extract_jsdoc_entries(
    symbols: &[ExtractedSymbol],
    source: &str,
    file_path: &str,
    language_id: LanguageId,
) -> Vec<JSDocEntry> {
    let mut jsdoc_entries = Vec::new();

    let tree = {
        let mut parser = parser_for_id(language_id).unwrap_or_else(|_| {
            // Fallback for parsing failure - try to get a parser
            let mut p = tree_sitter::Parser::new();
            let lang = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
            p.set_language(&lang).ok();
            p
        });
        parser.parse(source, None)
    };

    let tree = match tree {
        Some(t) => t,
        None => return jsdoc_entries,
    };

    let root = tree.root_node();

    for sym in symbols {
        // Find the node for this symbol by position
        let sym_node = find_node_at_position(root, sym.bytes.start);
        if let Some(node) = sym_node {
            if let Some(jsdoc_raw) = find_jsdoc_for_node(node, source) {
                let symbol_id = format!("{}:{}:{}", file_path, sym.lines.start, sym.name);
                let entry = parse_jsdoc(&jsdoc_raw, &symbol_id);
                jsdoc_entries.push(entry);
            }
        }
    }

    jsdoc_entries
}

/// Find a node at a specific byte offset
fn find_node_at_position(root: Node, offset: usize) -> Option<Node> {
    let mut cursor = root.walk();

    // Navigate to the position
    loop {
        let node = cursor.node();
        if node.start_byte() <= offset && node.end_byte() > offset {
            // This node contains the offset
            if cursor.goto_first_child() {
                continue;
            }
            return Some(node);
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }

    None
}

/// Recognize well-known framework decorators
fn classify_decorator(name: &str) -> DecoratorType {
    match name {
        // Angular decorators
        "Component" | "Input" | "Output" | "HostListener" | "HostBinding" => {
            DecoratorType::Component
        }
        "Injectable" | "Inject" | "Optional" | "Self" => DecoratorType::Injectable,
        "NgModule" | "Module" => DecoratorType::Module,
        "Directive" | "Pipe" => DecoratorType::Directive,
        // NestJS decorators
        "Controller" => DecoratorType::Controller,
        "Get" | "Post" | "Put" | "Delete" | "Patch" | "Options" | "Head" | "All" => {
            DecoratorType::Get
        }
        "Param" | "Body" | "Query" | "Headers" | "Session" | "Ip" | "Req" | "Res" => {
            DecoratorType::Param
        }
        "UseGuards" | "UseInterceptors" | "UsePipes" => DecoratorType::Unknown,
        _ => DecoratorType::Unknown,
    }
}

/// Extract decorator name (identifier or call expression target)
fn extract_decorator_name(node: Node, source: &str) -> String {
    // @Decorator or @Decorator()
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "property_identifier" => {
                return text_for_node(child, source);
            }
            "call_expression" => {
                // @Decorator() - find the function being called
                if let Some(func) = child.child_by_field_name("function") {
                    if func.kind() == "identifier" || func.kind() == "member_expression" {
                        return text_for_node(func, source);
                    }
                }
            }
            _ => {}
        }
    }
    String::new()
}

/// Extract decorator arguments as raw text
fn extract_decorator_arguments(node: Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call_expression" {
            if let Some(args) = child.child_by_field_name("arguments") {
                let raw = text_for_node(args, source);
                let trimmed = raw.trim();
                if !trimmed.is_empty() && trimmed != "()" {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// Extract decorators from a class or method declaration
fn extract_decorators_from_node(node: Node, source: &str, symbol_id: &str) -> Vec<DecoratorEntry> {
    let mut decorators = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "decorator" {
            let name = extract_decorator_name(child, source);
            let args = extract_decorator_arguments(child, source);
            let dec_type = classify_decorator(&name);

            decorators.push(DecoratorEntry {
                symbol_id: symbol_id.to_string(),
                name: name.clone(),
                arguments: args,
                target_line: node.start_position().row as u32,
                decorator_type: dec_type,
            });
        }
    }

    decorators
}

/// Extract decorators for all symbols in the file
fn extract_decorators_for_symbols(
    symbols: &[ExtractedSymbol],
    source: &str,
    root_cursor: TreeCursor,
) -> Vec<DecoratorEntry> {
    let mut decorators = Vec::new();

    let root = root_cursor.node();

    for sym in symbols {
        // Only classes and methods can have decorators in TypeScript
        if !matches!(sym.kind, SymbolKind::Class | SymbolKind::Function) {
            continue;
        }

        if let Some(node) = find_node_at_position(root, sym.bytes.start) {
            let symbol_id = format!("<unknown>:{}:{}", sym.lines.start, sym.name);
            let sym_decorators = extract_decorators_from_node(node, source, &symbol_id);
            decorators.extend(sym_decorators);
        }
    }

    decorators
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
            .symbols
            .iter()
            .map(|s| (s.kind, s.name.as_str(), s.exported))
            .collect();

        assert!(names.contains(&(SymbolKind::Function, "foo", true)));
        assert!(names.contains(&(SymbolKind::Class, "Bar", false)));
        assert!(names.contains(&(SymbolKind::Interface, "Baz", true)));
        assert!(names.contains(&(SymbolKind::TypeAlias, "Qux", true)));
        assert!(names.contains(&(SymbolKind::Enum, "E", true)));
        assert!(names.contains(&(SymbolKind::Const, "BIG", true)));

        let foo = symbols.symbols.iter().find(|s| s.name == "foo").unwrap();
        assert!(snippet(source, foo).contains("export function foo"));

        let big = symbols.symbols.iter().find(|s| s.name == "BIG").unwrap();
        let big_snip = snippet(source, big);
        assert!(big_snip.contains("export const BIG"));
        assert!(big_snip.contains("nested"));
        assert!(big_snip.contains("arr"));
    }

    #[test]
    fn extracts_import_and_public_export_bindings_without_losing_aliases() {
        let source = r#"
import DefaultWorker, { Worker as LocalWorker } from "./worker";
export { Worker as PublicWorker } from "./worker";
export * from "./shared";
export * as SharedNamespace from "./shared";
export { LocalWorker as WorkerAlias };
export default class DefaultWorkerClass {}
"#;
        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();

        assert!(extracted.module_bindings.iter().any(|binding| {
            binding.kind == ModuleBindingKind::Import
                && binding.imported_name == "default"
                && binding.local_name == "DefaultWorker"
                && binding.source == "./worker"
        }));
        assert!(extracted.module_bindings.iter().any(|binding| {
            binding.kind == ModuleBindingKind::Import
                && binding.imported_name == "Worker"
                && binding.local_name == "LocalWorker"
        }));
        assert!(extracted.module_bindings.iter().any(|binding| {
            binding.kind == ModuleBindingKind::ReExport
                && binding.imported_name == "Worker"
                && binding.exported_name == "PublicWorker"
                && binding.at_line == 3
        }));
        assert!(extracted.module_bindings.iter().any(|binding| {
            binding.kind == ModuleBindingKind::ExportAll
                && binding.source == "./shared"
                && binding.exported_name == "*"
        }));
        assert!(extracted.module_bindings.iter().any(|binding| {
            binding.kind == ModuleBindingKind::ReExport
                && binding.imported_name == "*"
                && binding.exported_name == "SharedNamespace"
        }));
        assert!(extracted.module_bindings.iter().any(|binding| {
            binding.kind == ModuleBindingKind::Export
                && binding.local_name == "LocalWorker"
                && binding.exported_name == "WorkerAlias"
        }));
        assert!(extracted.module_bindings.iter().any(|binding| {
            binding.kind == ModuleBindingKind::Export
                && binding.local_name == "DefaultWorkerClass"
                && binding.exported_name == "default"
        }));
    }

    #[test]
    fn anonymous_default_export_gets_stable_public_symbol() {
        let extracted = extract_typescript_symbols(
            LanguageId::Typescript,
            "export default function () { return 1; }",
        )
        .unwrap();

        assert!(extracted
            .symbols
            .iter()
            .any(|symbol| symbol.name == "default" && symbol.exported));
        assert!(extracted.module_bindings.iter().any(|binding| {
            binding.kind == ModuleBindingKind::Export
                && binding.local_name == "default"
                && binding.exported_name == "default"
        }));
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
            .symbols
            .iter()
            .any(|s| s.kind == SymbolKind::Function && s.name == "Comp" && s.exported));
    }

    #[test]
    fn extracts_type_edges() {
        let source = r#"
        class User {}
        interface Props { u: User; }
        function process(p: Props): void {}
        const arrow = (u: User) => {};
        type MyType = User | string;
        class Manager {
            manage(u: User) {}
        }
        "#;

        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let edges = extracted.type_edges;

        let has_edge =
            |parent: &str, ty: &str| edges.contains(&(parent.to_string(), ty.to_string()));

        assert!(has_edge("Props", "User"));
        assert!(has_edge("process", "Props"));
        assert!(has_edge("process", "void"));
        assert!(has_edge("arrow", "User"));
        assert!(has_edge("MyType", "User"));
        assert!(has_edge("MyType", "string"));
        assert!(has_edge("manage", "User"));
    }

    #[test]
    fn extracts_data_flow_edges_from_assignments() {
        let source = r#"
export function processData() {
    let x = 1;
    let y = foo();
    let z = bar(x);
    x = 2;
    return z;
}

const arrowFunc = (input: string) => {
    let result = input.trim();
    return result.toUpperCase();
};
"#;

        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let df_edges = &extracted.dataflow_edges;

        // Check for writes to x
        assert!(df_edges
            .iter()
            .any(|e| { e.from_symbol == "x" && matches!(e.flow_type, DataFlowType::Writes) }));

        // Check for reads of foo (function call on right side of assignment)
        assert!(df_edges
            .iter()
            .any(|e| { e.from_symbol == "foo" && matches!(e.flow_type, DataFlowType::Reads) }));

        // Check for reads of x (used in bar(x))
        assert!(df_edges
            .iter()
            .any(|e| { e.from_symbol == "x" && matches!(e.flow_type, DataFlowType::Reads) }));

        let foo_read = df_edges
            .iter()
            .find(|edge| edge.from_symbol == "foo" && matches!(edge.flow_type, DataFlowType::Reads))
            .expect("foo read edge");
        assert_eq!(foo_read.at_line, 4, "data-flow lines are one-based");

        // Check that dataflow_edges is populated
        assert!(
            !df_edges.is_empty(),
            "Should have extracted data flow edges"
        );
    }

    #[test]
    fn extracts_data_flow_edges_from_member_expressions() {
        let source = r#"
export function processUser(user: any) {
    let name = user.name;
    let age = user.age;
    return name;
}
"#;

        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let df_edges = &extracted.dataflow_edges;

        // Check for reads of user (from user.name, user.age)
        assert!(df_edges
            .iter()
            .any(|e| { e.from_symbol == "user" && matches!(e.flow_type, DataFlowType::Reads) }));

        // Check for writes to name
        assert!(df_edges
            .iter()
            .any(|e| { e.from_symbol == "name" && matches!(e.flow_type, DataFlowType::Writes) }));
    }

    #[test]
    fn skips_local_variables_inside_functions() {
        let source = r#"
export const MODULE_CONST = "top-level";

export function handler() {
    const startTime = Date.now();
    const url = buildUrl("/api");
    let result = fetch(url);
    return result;
}

const topArrow = (x: number) => {
    const inner = x * 2;
    return inner;
};

class Service {
    process() {
        const local = this.getData();
        return local;
    }
}
"#;

        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let names: Vec<&str> = extracted.symbols.iter().map(|s| s.name.as_str()).collect();

        // Module-level symbols ARE extracted
        assert!(
            names.contains(&"MODULE_CONST"),
            "module const should be extracted"
        );
        assert!(names.contains(&"handler"), "function should be extracted");
        assert!(
            names.contains(&"topArrow"),
            "top-level arrow should be extracted"
        );
        assert!(names.contains(&"Service"), "class should be extracted");
        assert!(names.contains(&"process"), "method should be extracted");

        // Local variables inside function bodies are NOT extracted
        assert!(
            !names.contains(&"startTime"),
            "local var in function should be skipped"
        );
        assert!(
            !names.contains(&"url"),
            "local var in function should be skipped"
        );
        assert!(
            !names.contains(&"result"),
            "local let in function should be skipped"
        );
        assert!(
            !names.contains(&"inner"),
            "local var in arrow should be skipped"
        );
        assert!(
            !names.contains(&"local"),
            "local var in method should be skipped"
        );
    }

    #[test]
    fn preserves_dataflow_for_local_variables() {
        let source = r#"
export function handler() {
    const url = buildUrl("/api");
    return url;
}
"#;

        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();

        // url should NOT be a symbol
        assert!(
            !extracted.symbols.iter().any(|s| s.name == "url"),
            "local var should not be a symbol"
        );

        // But dataflow edges should still reference buildUrl
        assert!(
            extracted
                .dataflow_edges
                .iter()
                .any(|e| e.from_symbol == "buildUrl"),
            "dataflow edges should still be extracted for local vars"
        );
    }

    #[test]
    fn test_async_boundary_detection() {
        let source = r#"
async function fetchData() {
    const result = await fetch("/api");
    const items = await Promise.all([fetchA(), fetchB()]);
}
"#;
        let file = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let await_edges: Vec<_> = file
            .dataflow_edges
            .iter()
            .filter(|e| e.from_symbol.starts_with("await:"))
            .collect();
        assert!(
            !await_edges.is_empty(),
            "Should detect await expressions, got edges: {:?}",
            file.dataflow_edges
                .iter()
                .map(|e| &e.from_symbol)
                .collect::<Vec<_>>()
        );

        // Verify the specific await:fetch edge is present.
        assert!(
            file.dataflow_edges
                .iter()
                .any(|e| e.from_symbol == "await:fetch"),
            "Expected await:fetch edge"
        );

        // Verify spawn:Promise.all edge from await Promise.all(...).
        let spawn_edges: Vec<_> = file
            .dataflow_edges
            .iter()
            .filter(|e| e.from_symbol.starts_with("spawn:"))
            .collect();
        assert!(
            !spawn_edges.is_empty(),
            "Should detect Promise.all as a spawn edge"
        );
        assert!(
            file.dataflow_edges
                .iter()
                .any(|e| e.from_symbol == "spawn:Promise.all"),
            "Expected spawn:Promise.all edge"
        );
    }

    #[test]
    fn extracts_property_for_hook_shaped_callback() {
        let source = r#"
import { query } from "@anthropic/sdk";

export async function consumeStream() {
  for await (const event of query({
    onBeforeToolUse: async (toolEvent) => {
      console.log(toolEvent);
    },
    prompt: "hi",
  })) {
    // ...
  }
}
"#;
        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let props: Vec<_> = extracted
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Property)
            .map(|s| s.name.as_str())
            .collect();
        assert!(
            props.contains(&"onBeforeToolUse"),
            "should extract onBeforeToolUse property, got {props:?}"
        );
        // 'prompt' is neither hook-shaped, function-valued, nor inside an
        // exported const, so it must NOT be extracted (keeps the symbol
        // count bounded).
        assert!(
            !props.contains(&"prompt"),
            "should NOT extract non-hook prompt property, got {props:?}"
        );
    }

    #[test]
    fn extracts_property_for_exported_const_string_literal() {
        let source = r#"
export const IPC = {
  SESSION_MESSAGE: 'session:message',
  SESSION_TOOL_USE: 'session:tool-use',
};
"#;
        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let props: Vec<_> = extracted
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Property)
            .map(|s| (s.name.as_str(), s.exported))
            .collect();
        assert!(
            props.iter().any(|(n, exp)| *n == "SESSION_MESSAGE" && *exp),
            "should extract exported SESSION_MESSAGE property, got {props:?}"
        );
        assert!(
            props
                .iter()
                .any(|(n, exp)| *n == "SESSION_TOOL_USE" && *exp),
            "should extract exported SESSION_TOOL_USE property, got {props:?}"
        );
    }

    #[test]
    fn extracts_property_for_preload_renderer_api_hook() {
        let source = r#"
import { contextBridge, ipcRenderer } from 'electron';

contextBridge.exposeInMainWorld('api', {
  onSessionMessage: (handler: (msg: unknown) => void) => {
    ipcRenderer.on('session:message', (_e, msg) => handler(msg));
  },
  send: (channel: string, payload: unknown) => ipcRenderer.send(channel, payload),
});
"#;
        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let props: Vec<_> = extracted
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Property)
            .map(|s| s.name.as_str())
            .collect();
        // onSessionMessage matches both gate 1 (hook-shaped) and gate 2
        // (function-valued).
        assert!(
            props.contains(&"onSessionMessage"),
            "should extract onSessionMessage, got {props:?}"
        );
        // `send` is function-valued but not hook-shaped; gate 2 accepts
        // it because callback-style call-arg objects ARE the pattern we
        // care about.
        assert!(
            props.contains(&"send"),
            "should extract send (function-valued), got {props:?}"
        );
    }

    #[test]
    fn skips_plain_non_hook_property_in_local_object() {
        let source = r#"
function doStuff() {
  const cfg = {
    timeout: 1000,
    label: "x",
  };
  return cfg;
}
"#;
        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let props: Vec<_> = extracted
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Property)
            .map(|s| s.name.as_str())
            .collect();
        assert!(
            props.is_empty(),
            "non-hook properties in local object should be skipped, got {props:?}"
        );
    }

    #[test]
    fn extracts_namespace_level_consts() {
        let source = r#"
namespace Config {
    export const API_URL = "https://api.example.com";
    export const TIMEOUT = 5000;
}
"#;

        let extracted = extract_typescript_symbols(LanguageId::Typescript, source).unwrap();
        let names: Vec<&str> = extracted.symbols.iter().map(|s| s.name.as_str()).collect();

        // Namespace-level consts should be extracted (namespace/internal_module is transparent)
        assert!(
            names.contains(&"API_URL"),
            "namespace const should be extracted"
        );
        assert!(
            names.contains(&"TIMEOUT"),
            "namespace const should be extracted"
        );
    }
}

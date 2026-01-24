use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{
    ByteSpan, DataFlowEdge, DataFlowType, ExtractedFile, ExtractedSymbol, Import, LineSpan,
    SymbolKind,
};

pub fn extract_typescript_symbols(language_id: LanguageId, source: &str) -> Result<ExtractedFile> {
    if !matches!(language_id, LanguageId::Typescript | LanguageId::Tsx) {
        return Err(anyhow!("LanguageId must be Typescript or Tsx"));
    }

    let mut parser = parser_for_id(language_id)?;
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
                    extract_dataflow_from_function_body(
                        node,
                        source,
                        &name,
                        &mut dataflow_edges,
                    );
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
                    extract_dataflow_from_function_body(
                        node,
                        source,
                        &name,
                        &mut dataflow_edges,
                    );
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
                extract_const_declarators(node, source, &mut symbols, &mut type_edges);
                // Extract data flow from const/let declarations with arrow functions
                extract_dataflow_from_lexical_declaration(
                    node,
                    source,
                    &symbols,
                    &mut dataflow_edges,
                );
            }
            "import_statement" => {
                extract_imports(node, source, &mut imports);
            }
            "export_statement" => {
                // handle export ... from ...
                if node.child_by_field_name("source").is_some() {
                    extract_imports(node, source, &mut imports);
                }
            }
            _ => {}
        }
    });

    symbols.sort_by_key(|s| s.bytes.start);
    Ok(ExtractedFile {
        symbols,
        imports,
        type_edges,
        dataflow_edges,
        todos: Vec::new(),
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
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

    loop {
        let current = cursor.node();
        if current.kind() == "variable_declarator" && current.child_by_field_name("value").is_some()
        {
            if let Some(name_node) = current.child_by_field_name("name") {
                let name = text_for_node(name_node, source);
                let def_node = const_definition_node(export_statement, node, current);
                out.push(symbol_from_node(
                    name.clone(),
                    SymbolKind::Const,
                    exported,
                    def_node,
                ));

                // Check if value is an arrow function to extract signature types
                if let Some(value_node) = current.child_by_field_name("value") {
                    if value_node.kind() == "arrow_function" {
                        extract_function_signature_types(value_node, source, &name, type_edges);
                    }
                }
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

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "import_clause" {
            // default import: import A from "..."
            if let Some(name_node) = child.child_by_field_name("name") {
                out.push(Import {
                    name: text_for_node(name_node, source),
                    source: source_path.clone(),
                    alias: Some("default".to_string()), // It imports the 'default' export
                });
            }
            // named imports
            if let Some(named) = child.child_by_field_name("named_imports") {
                extract_import_specifiers(named, source, &source_path, out);
            }
            // namespace import
            if let Some(ns) = child.child_by_field_name("namespace_import") {
                // import * as ns from ...
                // The symbol * is imported as ns
                // This is tricky. We import "everything".
                // Let's treat it as name="*" alias="ns"
                if let Some(alias_node) = ns
                    .children(&mut ns.walk())
                    .find(|n| n.kind() == "identifier")
                {
                    out.push(Import {
                        name: "*".to_string(),
                        source: source_path.clone(),
                        alias: Some(text_for_node(alias_node, source)),
                    });
                }
            }
        }
        // export clause: export { A } from "..."
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
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "import_specifier" {
            let name_node = child.child_by_field_name("name").unwrap(); // This is the local name OR remote name?
                                                                        // import { A } -> name=A.
                                                                        // import { A as B } -> name=A, alias=B.

            let name = text_for_node(name_node, source);
            let alias_node = child.child_by_field_name("alias");
            let alias = alias_node.map(|n| text_for_node(n, source));

            out.push(Import {
                name,
                source: source_path.to_string(),
                alias,
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
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "export_specifier" {
            // export { A } from "..."
            // export { A as B } from "..."
            let name_node = child.child_by_field_name("name").unwrap();
            let name = text_for_node(name_node, source);
            // Logic for exports is similar, they depend on the remote file.

            out.push(Import {
                name,
                source: source_path.to_string(),
                alias: None, // We don't care about alias for graph edge purposes locally, it re-exports.
            });
        }
    }
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
                        at_line: node.start_position().row as u32,
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
        "call_expression" => {
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
                if child.kind().ends_with("body") || child.kind() == "consequence" || child.kind() == "alternative" {
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

    let line = node.start_position().row as u32;

    // Extract what's being written (left side)
    if let Some(name) = extract_identifier_from_assignment_left(left, source) {
        out.push(DataFlowEdge {
            from_symbol: name,
            to_symbol: context_name.to_string(),
            flow_type: DataFlowType::Writes,
            at_line: line,
        });
    }

    // Extract what's being read (right side)
    for ident in extract_identifiers_from_expression(right, source) {
        out.push(DataFlowEdge {
            from_symbol: ident,
            to_symbol: context_name.to_string(),
            flow_type: DataFlowType::Reads,
            at_line: line,
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
    let line = node.start_position().row as u32;

    // The function being called is being read
    if let Some(func_node) = node.child_by_field_name("function") {
        if let Some(name) = extract_callee_name(func_node, source) {
            out.push(DataFlowEdge {
                from_symbol: name,
                to_symbol: context_name.to_string(),
                flow_type: DataFlowType::Reads,
                at_line: line,
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
    let line = node.start_position().row as u32;
    for ident in extract_identifiers_from_expression(node, source) {
        out.push(DataFlowEdge {
            from_symbol: ident,
            to_symbol: context_name.to_string(),
            flow_type: DataFlowType::Reads,
            at_line: line,
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
                if prop_node.kind() == "identifier" && node.child_by_field_name("object").map_or(false, |o| o.kind() != "member_expression") {
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
        assert!(df_edges.iter().any(|e| {
            e.from_symbol == "x" && matches!(e.flow_type, DataFlowType::Writes)
        }));

        // Check for reads of foo (function call on right side of assignment)
        assert!(df_edges.iter().any(|e| {
            e.from_symbol == "foo" && matches!(e.flow_type, DataFlowType::Reads)
        }));

        // Check for reads of x (used in bar(x))
        assert!(df_edges.iter().any(|e| {
            e.from_symbol == "x" && matches!(e.flow_type, DataFlowType::Reads)
        }));

        // Check that dataflow_edges is populated
        assert!(!df_edges.is_empty(), "Should have extracted data flow edges");
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
        assert!(df_edges.iter().any(|e| {
            e.from_symbol == "user" && matches!(e.flow_type, DataFlowType::Reads)
        }));

        // Check for writes to name
        assert!(df_edges.iter().any(|e| {
            e.from_symbol == "name" && matches!(e.flow_type, DataFlowType::Writes)
        }));
    }
}

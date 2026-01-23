use crate::indexer::parser::{parser_for_id, LanguageId};
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser, TreeCursor};

use super::symbol::{ByteSpan, DataFlowEdge, DataFlowType, ExtractedFile, ExtractedSymbol, Import, LineSpan, SymbolKind};

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
                }
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
        dataflow_edges: Vec::new(), // Will be populated by Task 2
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
}

//! Shared tree-sitter utilities for framework pattern extraction.
//!
//! Common AST traversal helpers used by multiple framework extractors
//! (Elysia, Hono, Express, Fastify, NestJS, tRPC, Next.js).

use tree_sitter::Node;

/// HTTP methods recognized as route methods across frameworks
pub const ROUTE_METHODS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "options", "head", "all",
];

/// Extract text content for a tree-sitter node
pub fn text_for_node(node: Node, source: &str) -> String {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or("")
        .to_string()
}

/// Truncate text to max length, appending "..." if truncated
pub fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...", &text[..max_len])
    }
}

/// Extract string content without surrounding quotes (single, double, backtick)
pub fn extract_string_value(node: Node, source: &str) -> String {
    let text = text_for_node(node, source);
    text.trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string()
}

/// Check if a string looks like an HTTP path (starts with "/")
pub fn is_http_path(s: &str) -> bool {
    s.starts_with('/')
}

/// Try to extract handler function name from various node types
pub fn extract_handler_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_for_node(node, source)),
        "arrow_function" | "function_expression" => Some("<anonymous>".to_string()),
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                return Some(text_for_node(func, source));
            }
            None
        }
        _ => None,
    }
}

/// Extract object keys as comma-separated string from an object literal node
pub fn extract_object_keys(node: Node, source: &str) -> Option<String> {
    let mut keys = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "pair" {
            if let Some(key_node) = child.child_by_field_name("key") {
                keys.push(text_for_node(key_node, source));
            }
        }
        if child.kind() == "shorthand_property_identifier" {
            keys.push(text_for_node(child, source));
        }
    }

    if keys.is_empty() {
        None
    } else {
        Some(keys.join(", "))
    }
}

/// Extract plugin/function name from an identifier or call expression
pub fn extract_plugin_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_for_node(node, source)),
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                return Some(text_for_node(func, source));
            }
            None
        }
        _ => None,
    }
}

/// Walk up a member_expression chain to find the root variable.
/// e.g., `app.get('/').post('/')` → "app", `new Hono().get('/')` → "Hono"
pub fn find_chain_root(member_expr: Node, source: &str) -> Option<String> {
    let object = member_expr.child_by_field_name("object")?;

    match object.kind() {
        "identifier" => Some(text_for_node(object, source)),
        "call_expression" => {
            if let Some(inner_func) = object.child_by_field_name("function") {
                if inner_func.kind() == "member_expression" {
                    find_chain_root(inner_func, source)
                } else {
                    None
                }
            } else {
                None
            }
        }
        "member_expression" => find_chain_root(object, source),
        "new_expression" => object
            .child_by_field_name("constructor")
            .map(|constructor| text_for_node(constructor, source)),
        _ => None,
    }
}

/// Get the first string literal argument from a call's arguments node.
/// Returns the unquoted string value and the node.
pub fn first_string_arg<'a>(args_node: Node<'a>, source: &str) -> Option<(String, Node<'a>)> {
    let mut cursor = args_node.walk();
    let named: Vec<Node<'a>> = args_node
        .children(&mut cursor)
        .filter(|n| n.is_named())
        .collect();
    let first_named = named.into_iter().next()?;
    if first_named.kind() == "string" || first_named.kind() == "template_string" {
        Some((extract_string_value(first_named, source), first_named))
    } else {
        None
    }
}

/// Get the Nth named child from an arguments node (0-indexed).
pub fn nth_named_arg<'a>(args_node: Node<'a>, n: usize) -> Option<Node<'a>> {
    let mut cursor = args_node.walk();
    let named: Vec<Node<'a>> = args_node
        .children(&mut cursor)
        .filter(|c| c.is_named())
        .collect();
    named.into_iter().nth(n)
}

/// Recursively walk an AST, calling `visit` on every `call_expression` node.
/// Common pattern shared by Elysia, Hono, Express, Fastify, tRPC extractors.
pub fn walk_call_expressions(
    node: Node,
    source: &str,
    results: &mut Vec<super::symbol::ExtractedFrameworkPattern>,
    visit: &dyn Fn(Node, &str) -> Option<super::symbol::ExtractedFrameworkPattern>,
) {
    if node.kind() == "call_expression" {
        if let Some(pattern) = visit(node, source) {
            results.push(pattern);
        }
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_call_expressions(cursor.node(), source, results, visit);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Count the parameters of a function/arrow function node.
/// Used by Express to distinguish error handlers (4 params) from middleware (≤3).
pub fn count_function_params(node: Node) -> usize {
    match node.kind() {
        "arrow_function" | "function_expression" | "function_declaration" => {
            if let Some(params) = node.child_by_field_name("parameters") {
                let mut cursor = params.walk();
                params
                    .children(&mut cursor)
                    .filter(|c| c.is_named())
                    .count()
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// Find named exports in a file. Returns vec of (export_name, node).
/// Used by Next.js to detect `export function GET`, `export default function`, etc.
pub fn find_named_exports<'a>(root: Node<'a>, source: &str) -> Vec<(String, Node<'a>)> {
    let mut exports = Vec::new();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        if child.kind() == "export_statement" {
            let is_default = {
                let mut c = child.walk();
                let kids: Vec<_> = child.children(&mut c).collect();
                kids.iter().any(|n| n.kind() == "default")
            };

            if let Some(decl) = child.child_by_field_name("declaration") {
                match decl.kind() {
                    "function_declaration" | "generator_function_declaration" => {
                        if let Some(name_node) = decl.child_by_field_name("name") {
                            let name = text_for_node(name_node, source);
                            exports.push((name, decl));
                        } else if is_default {
                            exports.push(("default".to_string(), decl));
                        }
                    }
                    "lexical_declaration" => {
                        let mut vc = decl.walk();
                        for declarator in decl.children(&mut vc) {
                            if declarator.kind() == "variable_declarator" {
                                if let Some(name_node) = declarator.child_by_field_name("name") {
                                    let name = text_for_node(name_node, source);
                                    exports.push((name, declarator));
                                }
                            }
                        }
                    }
                    _ => {
                        if is_default {
                            exports.push(("default".to_string(), decl));
                        }
                    }
                }
            } else if let Some(value) = child.child_by_field_name("value") {
                if is_default {
                    exports.push(("default".to_string(), value));
                }
            }
        }
    }
    exports
}

/// Derive a URL path from a Next.js App Router file path.
/// e.g., "app/api/users/[id]/route.ts" → "/api/users/:id"
/// e.g., "app/dashboard/page.tsx" → "/dashboard"
/// e.g., "src/app/api/auth/route.ts" → "/api/auth"
pub fn derive_nextjs_route_path(file_path: &str) -> Option<String> {
    let path = file_path.replace('\\', "/");

    // Accept paths that start with "app/" (no leading slash) or contain "/app/".
    let after_app = if let Some(rest) = path.strip_prefix("app/") {
        rest
    } else {
        let app_idx = path.find("/app/")?;
        &path[app_idx + 5..]
    };

    let dir = after_app.rsplit_once('/').map(|(d, _)| d).unwrap_or("");

    let segments: Vec<String> = if dir.is_empty() {
        vec![]
    } else {
        dir.split('/')
            .map(|seg| {
                if seg.starts_with("[[...") && seg.ends_with("]]") {
                    format!("*{}", &seg[5..seg.len() - 2])
                } else if seg.starts_with("[...") && seg.ends_with(']') {
                    format!("*{}", &seg[4..seg.len() - 1])
                } else if seg.starts_with('(') && seg.ends_with(')') {
                    String::new()
                } else if seg.starts_with('[') && seg.ends_with(']') {
                    format!(":{}", &seg[1..seg.len() - 1])
                } else {
                    seg.to_string()
                }
            })
            .filter(|s| !s.is_empty())
            .collect()
    };

    let route = if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    };

    Some(route)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_http_path() {
        assert!(is_http_path("/users"));
        assert!(is_http_path("/"));
        assert!(!is_http_path("users"));
        assert!(!is_http_path(""));
        assert!(!is_http_path("key"));
    }

    #[test]
    fn test_derive_nextjs_route_path() {
        assert_eq!(
            derive_nextjs_route_path("app/api/users/route.ts"),
            Some("/api/users".to_string())
        );
        assert_eq!(
            derive_nextjs_route_path("app/api/users/[id]/route.ts"),
            Some("/api/users/:id".to_string())
        );
        assert_eq!(
            derive_nextjs_route_path("app/dashboard/page.tsx"),
            Some("/dashboard".to_string())
        );
        assert_eq!(
            derive_nextjs_route_path("src/app/api/auth/route.ts"),
            Some("/api/auth".to_string())
        );
        assert_eq!(
            derive_nextjs_route_path("app/page.tsx"),
            Some("/".to_string())
        );
        assert_eq!(
            derive_nextjs_route_path("app/blog/[...slug]/page.tsx"),
            Some("/blog/*slug".to_string())
        );
        assert_eq!(
            derive_nextjs_route_path("app/(marketing)/about/page.tsx"),
            Some("/about".to_string())
        );
    }

    #[test]
    fn test_truncate_text() {
        assert_eq!(truncate_text("hello", 10), "hello");
        assert_eq!(truncate_text("hello world", 5), "hello...");
    }
}

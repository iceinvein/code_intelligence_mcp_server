//! Hono framework pattern extraction
//!
//! Extracts routes, middleware, groups, and other patterns from Hono fluent API chains.
//! Hono's API closely mirrors Elysia (method-chaining on `new Hono()`), with a few
//! key differences:
//! - `.use()` → `Middleware` (Elysia uses `Plugin`)
//! - `.route('/prefix', subApp)` → `Group`
//! - `.onError(handler)` → `ErrorHandler`
//! - `.notFound(handler)` → `ErrorHandler`

use tree_sitter::Node;

use super::framework_utils::{
    extract_handler_name, extract_plugin_name, extract_string_value, find_chain_root, is_http_path,
    text_for_node, truncate_text, walk_call_expressions, ROUTE_METHODS,
};
use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

/// Extract Hono framework patterns from a TypeScript AST.
pub fn extract_hono_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    walk_call_expressions(root, source, &mut patterns, &try_extract_hono_call);
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

/// Try to extract a Hono pattern from a call expression node.
fn try_extract_hono_call(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    let func_node = node.child_by_field_name("function")?;

    if func_node.kind() != "member_expression" {
        return None;
    }

    let property = func_node.child_by_field_name("property")?;
    let method_name = text_for_node(property, source);

    let (kind, http_method) = classify_hono_method(&method_name)?;

    let args_node = node.child_by_field_name("arguments")?;
    let pos = property.start_position();
    let line = pos.row as u32 + 1;
    let column = pos.column as u32;

    let (path, name, handler, arguments) =
        extract_hono_pattern_details(kind.clone(), args_node, source);

    // Route and Group patterns must have an HTTP-path-like first argument.
    // This prevents generic `.get(key)` / `.delete(id)` calls from being
    // misidentified as HTTP routes.
    if matches!(kind, FrameworkPatternKind::Route | FrameworkPatternKind::Group) {
        match &path {
            Some(p) if is_http_path(p) => {}
            _ => return None,
        }
    }

    let parent_chain = find_chain_root(func_node, source);

    Some(ExtractedFrameworkPattern {
        line,
        column,
        framework: "hono".to_string(),
        kind,
        http_method,
        path,
        name,
        handler,
        arguments,
        parent_chain,
    })
}

/// Classify a Hono method name into a `FrameworkPatternKind`.
///
/// Returns `None` for methods that are not Hono-specific (and therefore should
/// be ignored by this extractor).
fn classify_hono_method(method: &str) -> Option<(FrameworkPatternKind, Option<String>)> {
    let lower = method.to_lowercase();

    // HTTP route methods share the same set as Elysia.
    if ROUTE_METHODS.contains(&lower.as_str()) {
        return Some((FrameworkPatternKind::Route, Some(lower.to_uppercase())));
    }

    match lower.as_str() {
        // `.use()` in Hono registers middleware (not a plugin like Elysia).
        "use" => Some((FrameworkPatternKind::Middleware, None)),
        // `.route('/prefix', subApp)` mounts a sub-application as a route group.
        "route" => Some((FrameworkPatternKind::Group, None)),
        // Error-handling callbacks.
        "onerror" | "notfound" => Some((FrameworkPatternKind::ErrorHandler, None)),
        _ => None,
    }
}

/// Extract pattern-specific field values from the call's arguments node.
fn extract_hono_pattern_details(
    kind: FrameworkPatternKind,
    args_node: Node,
    source: &str,
) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    let mut path = None;
    let mut name = None;
    let mut handler = None;
    let mut arguments = None;

    let mut cursor = args_node.walk();
    let children: Vec<Node> = args_node.children(&mut cursor).collect();
    let named: Vec<&Node> = children.iter().filter(|n| n.is_named()).collect();

    match kind {
        FrameworkPatternKind::Route | FrameworkPatternKind::Group => {
            // First arg: path string; second arg: handler / sub-app.
            if let Some(first) = named.first() {
                if first.kind() == "string" || first.kind() == "template_string" {
                    path = Some(extract_string_value(**first, source));
                }
            }
            if let Some(second) = named.get(1) {
                handler = extract_handler_name(**second, source);
            }
        }

        FrameworkPatternKind::Middleware => {
            // `.use(middleware)` or `.use('/path/*', middleware)`
            // If the first arg is a string path, the actual middleware is the second arg.
            if let Some(first) = named.first() {
                if first.kind() == "string" || first.kind() == "template_string" {
                    // Path-scoped middleware — store the path and extract handler from next arg.
                    path = Some(extract_string_value(**first, source));
                    if let Some(second) = named.get(1) {
                        name = extract_plugin_name(**second, source);
                    }
                } else {
                    // Unscoped middleware — extract its name directly.
                    name = extract_plugin_name(**first, source);
                }
            }
        }

        FrameworkPatternKind::ErrorHandler => {
            // `.onError(handler)` / `.notFound(handler)`
            if let Some(first) = named.first() {
                handler = extract_handler_name(**first, source);
                if handler.is_none() {
                    arguments = Some(truncate_text(&text_for_node(**first, source), 200));
                }
            }
        }

        _ => {}
    }

    (path, name, handler, arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parser::{parser_for_id, LanguageId};

    fn parse_and_extract(source: &str) -> Vec<ExtractedFrameworkPattern> {
        let mut parser = parser_for_id(LanguageId::Typescript).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_hono_patterns(tree.root_node(), source)
    }

    #[test]
    fn extracts_basic_routes() {
        let source = r#"
const app = new Hono()
    .get('/users', listUsers)
    .post('/users', createUser)
    .delete('/users/:id', deleteUser)
"#;
        let patterns = parse_and_extract(source);

        assert_eq!(patterns.len(), 3);

        assert_eq!(patterns[0].kind, FrameworkPatternKind::Route);
        assert_eq!(patterns[0].http_method, Some("GET".to_string()));
        assert_eq!(patterns[0].path, Some("/users".to_string()));
        assert_eq!(patterns[0].framework, "hono");

        assert_eq!(patterns[1].http_method, Some("POST".to_string()));
        assert_eq!(patterns[1].path, Some("/users".to_string()));

        assert_eq!(patterns[2].http_method, Some("DELETE".to_string()));
        assert_eq!(patterns[2].path, Some("/users/:id".to_string()));
    }

    #[test]
    fn extracts_middleware() {
        let source = r#"
const app = new Hono()
    .use(logger())
    .use('/api/*', cors())
"#;
        let patterns = parse_and_extract(source);

        assert_eq!(patterns.len(), 2);

        // Unscoped middleware
        assert_eq!(patterns[0].kind, FrameworkPatternKind::Middleware);
        assert_eq!(patterns[0].path, None);
        assert_eq!(patterns[0].name, Some("logger".to_string()));

        // Path-scoped middleware
        assert_eq!(patterns[1].kind, FrameworkPatternKind::Middleware);
        assert_eq!(patterns[1].path, Some("/api/*".to_string()));
        assert_eq!(patterns[1].name, Some("cors".to_string()));
    }

    #[test]
    fn extracts_route_group() {
        let source = r#"
const app = new Hono()
    .route('/api/v1', apiRouter)
"#;
        let patterns = parse_and_extract(source);

        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].kind, FrameworkPatternKind::Group);
        assert_eq!(patterns[0].path, Some("/api/v1".to_string()));
        assert_eq!(patterns[0].handler, Some("apiRouter".to_string()));
    }

    #[test]
    fn extracts_error_handlers() {
        let source = r#"
const app = new Hono()
    .onError((err, c) => c.json({ error: err.message }, 500))
    .notFound((c) => c.text('Not Found', 404))
"#;
        let patterns = parse_and_extract(source);

        assert_eq!(patterns.len(), 2);
        assert_eq!(patterns[0].kind, FrameworkPatternKind::ErrorHandler);
        assert_eq!(patterns[1].kind, FrameworkPatternKind::ErrorHandler);
    }

    #[test]
    fn ignores_generic_get_calls() {
        // `.get(key)` on a Map or similar must never be treated as an HTTP route.
        let source = r#"
const result = myMap.get(key);
searchParams.get('id');
headers.get('content-type');
"#;
        let patterns = parse_and_extract(source);
        assert_eq!(
            patterns.len(),
            0,
            "Generic .get() calls should not be extracted as Hono routes"
        );
    }
}

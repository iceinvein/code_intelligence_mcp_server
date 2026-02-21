//! Fastify framework pattern extraction
//!
//! Extracts routes, plugins, hooks, decorators, error handlers, and listen calls
//! from Fastify's plugin-oriented API.
//!
//! Key Fastify-specific behaviour:
//! - `.register(plugin, opts?)` → `Plugin`
//! - `.addHook('onRequest', handler)` → `Hook`
//! - `.decorate('name', value)` → `Decorate`
//! - `.decorateRequest(...)` / `.decorateReply(...)` → `Decorate`
//! - `.setErrorHandler(handler)` → `ErrorHandler`
//! - `.listen(port)` → `Listen`
//! - Routes may carry an options object between the path and the handler:
//!   `.get('/path', { schema: ... }, handler)`
//!
//! Note: tree-sitter returns method names in their original capitalisation, so
//! `classify_fastify_method` receives the lowercase form (e.g. `"addhook"`,
//! `"seterrorhandler"`) produced by `.to_lowercase()`.

use tree_sitter::Node;

use super::framework_utils::{
    extract_handler_name, extract_plugin_name, extract_string_value, find_chain_root, is_http_path,
    text_for_node, truncate_text, walk_call_expressions, ROUTE_METHODS,
};
use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

/// Extract Fastify framework patterns from a TypeScript/JavaScript AST.
pub fn extract_fastify_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    walk_call_expressions(root, source, &mut patterns, &try_extract_fastify_call);
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

/// Try to extract a Fastify pattern from a call expression node.
fn try_extract_fastify_call(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    let func_node = node.child_by_field_name("function")?;

    if func_node.kind() != "member_expression" {
        return None;
    }

    let property = func_node.child_by_field_name("property")?;
    let method_name = text_for_node(property, source);

    let (kind, http_method) = classify_fastify_method(&method_name)?;

    let args_node = node.child_by_field_name("arguments")?;
    let pos = property.start_position();
    let line = pos.row as u32 + 1;
    let column = pos.column as u32;

    let (path, name, handler, arguments) =
        extract_fastify_pattern_details(kind.clone(), args_node, source);

    // Route patterns must have a valid HTTP path as the first argument.
    if matches!(kind, FrameworkPatternKind::Route) {
        match &path {
            Some(p) if is_http_path(p) => {}
            _ => return None,
        }
    }

    let parent_chain = find_chain_root(func_node, source);

    Some(ExtractedFrameworkPattern {
        line,
        column,
        framework: "fastify".to_string(),
        kind,
        http_method,
        path,
        name,
        handler,
        arguments,
        parent_chain,
    })
}

/// Classify a Fastify method name into a `FrameworkPatternKind`.
///
/// The `method` parameter is the **original** (un-lowercased) text taken
/// directly from the AST.  Internally we lowercase it to produce a stable
/// match key (e.g. `"addHook"` → `"addhook"`).
fn classify_fastify_method(method: &str) -> Option<(FrameworkPatternKind, Option<String>)> {
    let lower = method.to_lowercase();

    // Standard HTTP route methods.
    if ROUTE_METHODS.contains(&lower.as_str()) {
        return Some((FrameworkPatternKind::Route, Some(lower.to_uppercase())));
    }

    match lower.as_str() {
        "register" => Some((FrameworkPatternKind::Plugin, None)),
        "addhook" => Some((FrameworkPatternKind::Hook, None)),
        "decorate" | "decoraterequest" | "decoratereply" => {
            Some((FrameworkPatternKind::Decorate, None))
        }
        "seterrorhandler" => Some((FrameworkPatternKind::ErrorHandler, None)),
        "listen" => Some((FrameworkPatternKind::Listen, None)),
        _ => None,
    }
}

/// Extract pattern-specific field values from the call's arguments node.
fn extract_fastify_pattern_details(
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
        FrameworkPatternKind::Route => {
            // `.get('/path', handler)` or `.get('/path', { schema: ... }, handler)`
            // First arg: path string.
            if let Some(first) = named.first() {
                if first.kind() == "string" || first.kind() == "template_string" {
                    path = Some(extract_string_value(**first, source));
                }
            }
            // Handler is the last named argument (skips optional opts object).
            if let Some(last) = named.last() {
                // Only use it as a handler when it is not the same as the path node.
                if named.len() >= 2 {
                    handler = extract_handler_name(**last, source);
                }
            }
        }

        FrameworkPatternKind::Plugin => {
            // `.register(plugin[, opts])`
            if let Some(first) = named.first() {
                name = extract_plugin_name(**first, source);
                if name.is_none() {
                    arguments = Some(truncate_text(&text_for_node(**first, source), 200));
                }
            }
        }

        FrameworkPatternKind::Hook => {
            // `.addHook('hookName', handler)`
            if let Some(first) = named.first() {
                if first.kind() == "string" || first.kind() == "template_string" {
                    name = Some(extract_string_value(**first, source));
                }
            }
            if let Some(second) = named.get(1) {
                handler = extract_handler_name(**second, source);
            }
        }

        FrameworkPatternKind::Decorate => {
            // `.decorate('name', value)` / `.decorateRequest('name', value)` / etc.
            if let Some(first) = named.first() {
                if first.kind() == "string" || first.kind() == "template_string" {
                    name = Some(extract_string_value(**first, source));
                } else {
                    // Identifier or function argument — store its text.
                    name = Some(text_for_node(**first, source));
                }
            }
        }

        FrameworkPatternKind::ErrorHandler => {
            // `.setErrorHandler(handler)`
            if let Some(first) = named.first() {
                handler = extract_handler_name(**first, source);
                if handler.is_none() {
                    arguments = Some(truncate_text(&text_for_node(**first, source), 200));
                }
            }
        }

        FrameworkPatternKind::Listen => {
            // `.listen(port[, host][, callback])` — store port as name.
            if let Some(first) = named.first() {
                name = Some(text_for_node(**first, source));
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
        extract_fastify_patterns(tree.root_node(), source)
    }

    #[test]
    fn extracts_basic_routes() {
        let source = r#"
const app = fastify()
app.get('/users', listUsers)
app.post('/users', { schema: userSchema }, createUser)
"#;
        let patterns = parse_and_extract(source);

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(routes.len(), 2);

        assert_eq!(routes[0].http_method, Some("GET".to_string()));
        assert_eq!(routes[0].path, Some("/users".to_string()));
        assert_eq!(routes[0].handler, Some("listUsers".to_string()));
        assert_eq!(routes[0].framework, "fastify");

        assert_eq!(routes[1].http_method, Some("POST".to_string()));
        assert_eq!(routes[1].path, Some("/users".to_string()));
        // Handler is the last arg, after the schema opts object.
        assert_eq!(routes[1].handler, Some("createUser".to_string()));
    }

    #[test]
    fn extracts_register() {
        let source = r#"
app.register(cors, { origin: true })
app.register(authPlugin)
"#;
        let patterns = parse_and_extract(source);

        let plugins: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Plugin)
            .collect();

        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].name, Some("cors".to_string()));
        assert_eq!(plugins[1].name, Some("authPlugin".to_string()));
    }

    #[test]
    fn extracts_hooks() {
        let source = r#"
app.addHook('onRequest', authenticate)
"#;
        let patterns = parse_and_extract(source);

        let hooks: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Hook)
            .collect();

        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, Some("onRequest".to_string()));
        assert_eq!(hooks[0].handler, Some("authenticate".to_string()));
    }

    #[test]
    fn extracts_decorate_and_error_handler() {
        let source = r#"
app.decorate('authenticate', authFn)
app.setErrorHandler(errorHandler)
"#;
        let patterns = parse_and_extract(source);

        let decorate = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::Decorate);
        assert!(decorate.is_some(), "Expected a Decorate pattern");
        assert_eq!(decorate.unwrap().name, Some("authenticate".to_string()));

        let error_handler = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::ErrorHandler);
        assert!(error_handler.is_some(), "Expected an ErrorHandler pattern");
        assert_eq!(
            error_handler.unwrap().handler,
            Some("errorHandler".to_string())
        );
    }

    #[test]
    fn ignores_generic_get_calls() {
        let source = r#"
const result = myMap.get(key);
searchParams.get('id');
headers.get('content-type');
"#;
        let patterns = parse_and_extract(source);

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(
            routes.len(),
            0,
            "Generic .get() calls must not be treated as Fastify routes"
        );
    }
}

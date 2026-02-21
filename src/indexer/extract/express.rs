//! Express framework pattern extraction
//!
//! Extracts routes, middleware, error handlers, router creation, and other patterns
//! from Express.js method chains.
//!
//! Key differences from Elysia/Hono:
//! - `.use()` is Middleware OR ErrorHandler, depending on the function arity.
//!   A handler with 4 parameters `(err, req, res, next)` is an error handler.
//! - `express.Router()` or plain `Router()` call → `Router` kind.
//! - `.set('key', value)` → `State` (app-level configuration).
//! - `.listen(port)` → `Listen`.

use tree_sitter::Node;

use super::framework_utils::{
    count_function_params, extract_handler_name, extract_plugin_name, extract_string_value,
    find_chain_root, is_http_path, text_for_node, truncate_text, walk_call_expressions,
    ROUTE_METHODS,
};
use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

/// Extract Express framework patterns from a TypeScript/JavaScript AST.
pub fn extract_express_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    walk_call_expressions(root, source, &mut patterns, &try_extract_express_call);
    detect_router_creation(root, source, &mut patterns);
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

/// Try to extract an Express pattern from a call expression node.
fn try_extract_express_call(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    let func_node = node.child_by_field_name("function")?;

    if func_node.kind() != "member_expression" {
        return None;
    }

    let property = func_node.child_by_field_name("property")?;
    let method_name = text_for_node(property, source);

    let args_node = node.child_by_field_name("arguments")?;

    let (kind, http_method) = classify_express_method(&method_name, args_node, source)?;

    let pos = property.start_position();
    let line = pos.row as u32 + 1;
    let column = pos.column as u32;

    let (path, name, handler, arguments) =
        extract_express_pattern_details(kind.clone(), args_node, source);

    // HTTP routes must have a path starting with "/".
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
        framework: "express".to_string(),
        kind,
        http_method,
        path,
        name,
        handler,
        arguments,
        parent_chain,
    })
}

/// Classify an Express method into a `FrameworkPatternKind`.
///
/// `args_node` is provided so that `.use()` can inspect the first callback's
/// parameter count: 4 parameters means an error handler `(err, req, res, next)`.
fn classify_express_method(
    method: &str,
    args_node: Node,
    _source: &str,
) -> Option<(FrameworkPatternKind, Option<String>)> {
    let lower = method.to_lowercase();

    if ROUTE_METHODS.contains(&lower.as_str()) {
        return Some((FrameworkPatternKind::Route, Some(lower.to_uppercase())));
    }

    match lower.as_str() {
        "use" => {
            // Determine whether this `.use()` call registers an error handler.
            // Express error handlers conventionally have 4 parameters:
            // `(err, req, res, next)`.  Find the first function-like argument.
            let mut cursor = args_node.walk();
            let named: Vec<Node> = args_node.children(&mut cursor).filter(|n| n.is_named()).collect();

            // The callback may be the first or second argument (after an optional path).
            let callback = named.iter().find(|n| {
                matches!(n.kind(), "arrow_function" | "function_expression" | "function_declaration")
            });

            if let Some(cb) = callback {
                if count_function_params(*cb) >= 4 {
                    return Some((FrameworkPatternKind::ErrorHandler, None));
                }
            }

            Some((FrameworkPatternKind::Middleware, None))
        }
        "set" => Some((FrameworkPatternKind::State, None)),
        "listen" => Some((FrameworkPatternKind::Listen, None)),
        _ => None,
    }
}

/// Extract pattern-specific field values from the call's arguments node.
fn extract_express_pattern_details(
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
            // First arg: path string; second (or later) arg: handler.
            if let Some(first) = named.first() {
                if first.kind() == "string" || first.kind() == "template_string" {
                    path = Some(extract_string_value(**first, source));
                }
            }
            if let Some(second) = named.get(1) {
                handler = extract_handler_name(**second, source);
            }
        }

        FrameworkPatternKind::Middleware | FrameworkPatternKind::ErrorHandler => {
            // `.use([path,] middleware)`
            // If the first arg is a string, it scopes the middleware to a path.
            if let Some(first) = named.first() {
                if first.kind() == "string" || first.kind() == "template_string" {
                    path = Some(extract_string_value(**first, source));
                    if let Some(second) = named.get(1) {
                        name = extract_plugin_name(**second, source);
                        if name.is_none() {
                            arguments =
                                Some(truncate_text(&text_for_node(**second, source), 200));
                        }
                    }
                } else {
                    name = extract_plugin_name(**first, source);
                    if name.is_none() {
                        arguments = Some(truncate_text(&text_for_node(**first, source), 200));
                    }
                }
            }
        }

        FrameworkPatternKind::State => {
            // `.set('key', value)` — store the key name.
            if let Some(first) = named.first() {
                if first.kind() == "string" || first.kind() == "template_string" {
                    name = Some(extract_string_value(**first, source));
                }
            }
        }

        FrameworkPatternKind::Listen => {
            // `.listen(port[, host][, callback])` — store the port as name.
            if let Some(first) = named.first() {
                name = Some(text_for_node(**first, source));
            }
        }

        _ => {}
    }

    (path, name, handler, arguments)
}

/// Walk the entire AST to find `express.Router()` or bare `Router()` call
/// expressions, emitting a `Router` pattern for each.
fn detect_router_creation(
    root: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    detect_router_creation_recursive(root, source, patterns);
}

fn detect_router_creation_recursive(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    if node.kind() == "call_expression" {
        if let Some(pattern) = try_extract_router_creation(node, source) {
            patterns.push(pattern);
        }
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            detect_router_creation_recursive(cursor.node(), source, patterns);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Try to match `express.Router()` or `Router()` call expressions.
fn try_extract_router_creation(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    let func_node = node.child_by_field_name("function")?;
    let pos;
    let router_name;

    match func_node.kind() {
        "identifier" => {
            // Bare `Router()` call.
            let name = text_for_node(func_node, source);
            if name != "Router" {
                return None;
            }
            router_name = name;
            pos = func_node.start_position();
        }
        "member_expression" => {
            // `express.Router()` call.
            let property = func_node.child_by_field_name("property")?;
            let prop_name = text_for_node(property, source);
            if prop_name != "Router" {
                return None;
            }
            router_name = prop_name;
            pos = property.start_position();
        }
        _ => return None,
    }

    Some(ExtractedFrameworkPattern {
        line: pos.row as u32 + 1,
        column: pos.column as u32,
        framework: "express".to_string(),
        kind: FrameworkPatternKind::Router,
        http_method: None,
        path: None,
        name: Some(router_name),
        handler: None,
        arguments: None,
        parent_chain: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parser::{parser_for_id, LanguageId};

    fn parse_and_extract(source: &str) -> Vec<ExtractedFrameworkPattern> {
        let mut parser = parser_for_id(LanguageId::Typescript).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_express_patterns(tree.root_node(), source)
    }

    #[test]
    fn extracts_basic_routes() {
        let source = r#"
const app = express()
app.get('/users', listUsers)
app.post('/users', createUser)
app.put('/users/:id', updateUser)
"#;
        let patterns = parse_and_extract(source);

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(routes.len(), 3);
        assert_eq!(routes[0].http_method, Some("GET".to_string()));
        assert_eq!(routes[0].path, Some("/users".to_string()));
        assert_eq!(routes[0].framework, "express");

        assert_eq!(routes[1].http_method, Some("POST".to_string()));
        assert_eq!(routes[2].http_method, Some("PUT".to_string()));
        assert_eq!(routes[2].path, Some("/users/:id".to_string()));
    }

    #[test]
    fn extracts_middleware() {
        let source = r#"
app.use(cors())
app.use('/api', authMiddleware)
"#;
        let patterns = parse_and_extract(source);

        let middleware: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Middleware)
            .collect();

        assert_eq!(middleware.len(), 2);

        // Unscoped middleware
        assert_eq!(middleware[0].path, None);
        assert_eq!(middleware[0].name, Some("cors".to_string()));

        // Path-scoped middleware
        assert_eq!(middleware[1].path, Some("/api".to_string()));
        assert_eq!(middleware[1].name, Some("authMiddleware".to_string()));
    }

    #[test]
    fn detects_error_handler() {
        let source = r#"
app.use((err, req, res, next) => {
    res.status(500).json({ error: err.message })
})
"#;
        let patterns = parse_and_extract(source);

        let error_handlers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::ErrorHandler)
            .collect();

        assert_eq!(
            error_handlers.len(),
            1,
            "Expected exactly one error handler"
        );
    }

    #[test]
    fn detects_router_creation() {
        let source = r#"
const apiRouter = express.Router()
const userRouter = Router()
"#;
        let patterns = parse_and_extract(source);

        let routers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Router)
            .collect();

        assert_eq!(routers.len(), 2, "Expected two Router() creations");
        assert!(routers.iter().all(|r| r.framework == "express"));
    }

    #[test]
    fn extracts_listen_and_set() {
        let source = r#"
app.set('view engine', 'pug')
app.listen(3000)
"#;
        let patterns = parse_and_extract(source);

        let state = patterns.iter().find(|p| p.kind == FrameworkPatternKind::State);
        assert!(state.is_some(), "Expected a State pattern for .set()");
        assert_eq!(state.unwrap().name, Some("view engine".to_string()));

        let listen = patterns.iter().find(|p| p.kind == FrameworkPatternKind::Listen);
        assert!(listen.is_some(), "Expected a Listen pattern for .listen()");
        assert_eq!(listen.unwrap().name, Some("3000".to_string()));
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
            "Generic .get() calls must not be treated as Express routes"
        );
    }
}

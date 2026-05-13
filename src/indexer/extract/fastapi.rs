//! FastAPI and Flask framework pattern extraction
//!
//! Extracts HTTP routes, middleware, and error handlers from Python source files
//! using the tree-sitter-python grammar. Supports both FastAPI and Flask
//! decorator-based routing patterns.
//!
//! ## Supported patterns
//!
//! **FastAPI HTTP verbs** (emit `framework = "fastapi"`, `kind = Route`):
//! ```python
//! @app.get("/users")
//! def get_users(): ...
//!
//! @router.post("/users")
//! async def create_user(): ...
//! ```
//!
//! **FastAPI middleware** (emit `framework = "fastapi"`, `kind = Middleware`):
//! ```python
//! @app.middleware("http")
//! async def log_requests(request, call_next): ...
//! ```
//!
//! **Flask route** (emit `framework = "flask"`, `kind = Route`):
//! ```python
//! @app.route("/login", methods=["POST"])
//! def login(): ...
//! ```
//!
//! **Flask lifecycle hooks** (emit `framework = "flask"`, `kind = Middleware`):
//! ```python
//! @app.before_request
//! def check_auth(): ...
//!
//! @app.after_request
//! def add_cors(response): ...
//! ```
//!
//! **Flask error handlers** (emit `framework = "flask"`, `kind = ErrorHandler`):
//! ```python
//! @app.errorhandler(404)
//! def not_found(error): ...
//! ```
//!
//! ## False-positive guard (R118)
//!
//! HTTP route patterns require the first positional argument to be a string
//! literal starting with `/`. This prevents false positives from generic
//! `.get()` / `.post()` calls on non-router objects.

use tree_sitter::Node;

use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// HTTP verb method names that both FastAPI and Flask support.
const HTTP_VERBS: &[&str] = &["get", "post", "put", "delete", "patch", "options", "head"];

/// Flask-specific decorator method names that are NOT HTTP verbs.
const FLASK_LIFECYCLE: &[&str] = &["before_request", "after_request"];

/// FastAPI-specific non-verb decorator method name.
const FASTAPI_MIDDLEWARE: &str = "middleware";

/// Flask error handler decorator method name.
const FLASK_ERRORHANDLER: &str = "errorhandler";

/// Flask catch-all route decorator that accepts an explicit `methods` kwarg.
const FLASK_ROUTE: &str = "route";

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Extract FastAPI and Flask framework patterns from a Python AST.
///
/// Walks the tree-sitter Python AST and inspects every `decorated_definition`
/// node. Each decorator that matches a known FastAPI/Flask pattern is emitted
/// as an [`ExtractedFrameworkPattern`].
///
/// # Arguments
///
/// * `root` - Root node of the parsed Python tree-sitter tree.
/// * `source` - Full source text of the Python file (UTF-8).
///
/// # Returns
///
/// A vector of patterns sorted by `(line, column)`.
pub fn extract_fastapi_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    walk_for_decorated_definitions(root, source, &mut patterns);
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

// ---------------------------------------------------------------------------
// AST walking
// ---------------------------------------------------------------------------

/// Recursively walk the AST, calling the pattern extractor for every
/// `decorated_definition` node encountered.
fn walk_for_decorated_definitions(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    if node.kind() == "decorated_definition" {
        extract_patterns_from_decorated_definition(node, source, patterns);
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_for_decorated_definitions(cursor.node(), source, patterns);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Inspect a `decorated_definition` node, extracting one pattern per
/// FastAPI/Flask decorator found on the definition.
///
/// A `decorated_definition` in tree-sitter-python has the structure:
///
/// ```text
/// decorated_definition
///   decorator          (one per @-line)
///     ...decorator content...
///   function_definition | class_definition
///     name: identifier
///     ...
/// ```
fn extract_patterns_from_decorated_definition(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    // Collect all decorator child nodes.
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();

    // Determine the handler name from the definition (function_definition or
    // class_definition) — the last named child of the decorated_definition.
    let handler_name: Option<String> = children.iter().rev().find_map(|child| {
        if matches!(
            child.kind(),
            "function_definition" | "async_function_definition"
        ) {
            child
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(ToOwned::to_owned)
        } else {
            None
        }
    });

    // Process each decorator node.
    for child in &children {
        if child.kind() != "decorator" {
            continue;
        }

        if let Some(pattern) = try_extract_decorator(*child, source, handler_name.clone()) {
            patterns.push(pattern);
        }
    }
}

// ---------------------------------------------------------------------------
// Decorator analysis
// ---------------------------------------------------------------------------

/// Try to match a `decorator` node against FastAPI/Flask patterns.
///
/// The decorator node in tree-sitter-python wraps the `@expr` expression.
/// Its first (and only) named child is the decorator expression itself, which
/// may be:
///
/// - A plain `attribute` (no call): `@app.before_request`
/// - A `call` whose function is an `attribute`: `@app.get("/users")`
fn try_extract_decorator(
    decorator: Node,
    source: &str,
    handler: Option<String>,
) -> Option<ExtractedFrameworkPattern> {
    // The decorator node contains: "@" token + expression.
    // Walk children to find the expression (skip the "@" punctuation token).
    let mut cursor = decorator.walk();
    let children: Vec<Node> = decorator.children(&mut cursor).collect();

    // Find the first named child — the decorator expression.
    let expr = children.iter().find(|n| n.is_named())?;

    match expr.kind() {
        // `@app.before_request` — attribute without call arguments.
        "attribute" => try_extract_plain_attribute(expr, source, handler),

        // `@app.get("/users")` — call expression.
        "call" => try_extract_call_decorator(expr, source, handler),

        _ => None,
    }
}

/// Handle a plain attribute decorator (no call parentheses).
///
/// Example: `@app.before_request` or `@app.after_request`.
fn try_extract_plain_attribute(
    attr: &Node,
    source: &str,
    handler: Option<String>,
) -> Option<ExtractedFrameworkPattern> {
    let method_node = attr.child_by_field_name("attribute")?;
    let method_name = text_for_node(method_node, source);
    let pos = method_node.start_position();

    if FLASK_LIFECYCLE.contains(&method_name.as_str()) {
        return Some(ExtractedFrameworkPattern {
            line: pos.row as u32 + 1,
            column: pos.column as u32,
            framework: "flask".to_string(),
            kind: FrameworkPatternKind::Middleware,
            http_method: None,
            path: None,
            name: None,
            handler,
            arguments: None,
            parent_chain: None,
        });
    }

    None
}

/// Handle a call decorator: `@app.get("/users")` or `@app.errorhandler(404)`.
///
/// In tree-sitter-python a `call` node has:
/// - `function` field: the callee expression
/// - `arguments` field: `argument_list` node
fn try_extract_call_decorator(
    call: &Node,
    source: &str,
    handler: Option<String>,
) -> Option<ExtractedFrameworkPattern> {
    let func = call.child_by_field_name("function")?;

    // The callee must be an `attribute` (e.g., `app.get`, `router.post`).
    if func.kind() != "attribute" {
        return None;
    }

    let method_node = func.child_by_field_name("attribute")?;
    let method_name = text_for_node(method_node, source);
    let pos = method_node.start_position();

    let args_node = call.child_by_field_name("arguments")?;

    // Route HTTP verb: @app.get("/path") / @router.post("/path")
    if HTTP_VERBS.contains(&method_name.as_str()) {
        let path = extract_first_string_arg(args_node, source);
        // R118 guard: HTTP routes must have a path starting with "/".
        match &path {
            Some(p) if p.starts_with('/') => {}
            _ => return None,
        }

        return Some(ExtractedFrameworkPattern {
            line: pos.row as u32 + 1,
            column: pos.column as u32,
            framework: "fastapi".to_string(),
            kind: FrameworkPatternKind::Route,
            http_method: Some(method_name.to_uppercase()),
            path,
            name: None,
            handler,
            arguments: None,
            parent_chain: None,
        });
    }

    // Flask route decorator: @app.route("/path", methods=[...])
    if method_name == FLASK_ROUTE {
        let path = extract_first_string_arg(args_node, source);
        match &path {
            Some(p) if p.starts_with('/') => {}
            _ => return None,
        }

        return Some(ExtractedFrameworkPattern {
            line: pos.row as u32 + 1,
            column: pos.column as u32,
            framework: "flask".to_string(),
            kind: FrameworkPatternKind::Route,
            http_method: None, // Flask route has methods kwarg, not in method name
            path,
            name: None,
            handler,
            arguments: None,
            parent_chain: None,
        });
    }

    // FastAPI middleware: @app.middleware("http")
    if method_name == FASTAPI_MIDDLEWARE {
        return Some(ExtractedFrameworkPattern {
            line: pos.row as u32 + 1,
            column: pos.column as u32,
            framework: "fastapi".to_string(),
            kind: FrameworkPatternKind::Middleware,
            http_method: None,
            path: None,
            name: None,
            handler,
            arguments: None,
            parent_chain: None,
        });
    }

    // Flask error handler: @app.errorhandler(404)
    if method_name == FLASK_ERRORHANDLER {
        return Some(ExtractedFrameworkPattern {
            line: pos.row as u32 + 1,
            column: pos.column as u32,
            framework: "flask".to_string(),
            kind: FrameworkPatternKind::ErrorHandler,
            http_method: None,
            path: None,
            name: None,
            handler,
            arguments: None,
            parent_chain: None,
        });
    }

    None
}

// ---------------------------------------------------------------------------
// String argument extraction
// ---------------------------------------------------------------------------

/// Extract the first positional string literal from an `argument_list` node.
///
/// In tree-sitter-python, string literals are `string` nodes. The string
/// content (without surrounding quotes) lives in a `string_content` child, or
/// can be recovered by stripping quote characters from the full node text.
fn extract_first_string_arg(args_node: Node, source: &str) -> Option<String> {
    let mut cursor = args_node.walk();
    for child in args_node.children(&mut cursor) {
        // Skip non-named nodes (commas, parentheses).
        if !child.is_named() {
            continue;
        }
        // Skip keyword arguments — we only want positional strings.
        if child.kind() == "keyword_argument" {
            continue;
        }
        if child.kind() == "string" {
            return Some(extract_python_string(child, source));
        }
        // The first named non-string argument means no leading string path.
        break;
    }
    None
}

/// Extract the string value from a tree-sitter-python `string` node.
///
/// tree-sitter-python represents Python strings as:
/// ```text
/// string
///   string_start  (the opening quote token, e.g., `"` or `'`)
///   string_content  (the literal content without quotes)
///   string_end    (the closing quote token)
/// ```
///
/// This function tries the `string_content` child first (most reliable), then
/// falls back to stripping quote characters from the full node text.
fn extract_python_string(node: Node, source: &str) -> String {
    // Prefer the `string_content` child, which contains the unquoted value.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            return text_for_node(child, source);
        }
    }

    // Fallback: strip surrounding quote characters from the full node text.
    let raw = text_for_node(node, source);
    raw.trim_matches(|c| c == '"' || c == '\'' || c == '`' || c == 'f' || c == 'r' || c == 'b')
        .to_string()
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Return the source text slice corresponding to a node.
fn text_for_node(node: Node, source: &str) -> String {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or("")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parser::{parser_for_id, LanguageId};

    fn parse_and_extract(source: &str) -> Vec<ExtractedFrameworkPattern> {
        let mut parser = parser_for_id(LanguageId::Python).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_fastapi_patterns(tree.root_node(), source)
    }

    #[test]
    fn test_fastapi_routes() {
        let source = "@app.get(\"/users\")\ndef get_users(): pass\n\n@app.post(\"/users\")\ndef create_user(): pass\n";
        let patterns = parse_and_extract(source);

        let routes: Vec<&ExtractedFrameworkPattern> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(
            routes.len(),
            2,
            "Expected 2 Route patterns, got: {routes:?}"
        );

        let get_route = routes
            .iter()
            .find(|p| p.http_method == Some("GET".to_string()));
        let post_route = routes
            .iter()
            .find(|p| p.http_method == Some("POST".to_string()));

        assert!(get_route.is_some(), "Expected a GET route");
        assert_eq!(get_route.unwrap().path, Some("/users".to_string()));
        assert_eq!(get_route.unwrap().handler, Some("get_users".to_string()));
        assert_eq!(get_route.unwrap().framework, "fastapi");

        assert!(post_route.is_some(), "Expected a POST route");
        assert_eq!(post_route.unwrap().path, Some("/users".to_string()));
        assert_eq!(post_route.unwrap().handler, Some("create_user".to_string()));
        assert_eq!(post_route.unwrap().framework, "fastapi");
    }

    #[test]
    fn test_flask_routes() {
        let source = "@app.route(\"/login\", methods=[\"POST\"])\ndef login(): pass\n";
        let patterns = parse_and_extract(source);

        let routes: Vec<&ExtractedFrameworkPattern> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(routes.len(), 1, "Expected 1 Route pattern, got: {routes:?}");
        assert_eq!(routes[0].framework, "flask");
        assert_eq!(routes[0].path, Some("/login".to_string()));
        assert_eq!(routes[0].handler, Some("login".to_string()));
    }

    #[test]
    fn test_middleware_detection() {
        let source = "@app.before_request\ndef check_auth(): pass\n";
        let patterns = parse_and_extract(source);

        let middleware: Vec<&ExtractedFrameworkPattern> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Middleware)
            .collect();

        assert_eq!(
            middleware.len(),
            1,
            "Expected 1 Middleware pattern, got: {middleware:?}"
        );
        assert_eq!(middleware[0].framework, "flask");
        assert_eq!(middleware[0].handler, Some("check_auth".to_string()));
    }

    #[test]
    fn test_fastapi_middleware_call() {
        let source =
            "@app.middleware(\"http\")\nasync def log_requests(request, call_next): pass\n";
        let patterns = parse_and_extract(source);

        let middleware: Vec<&ExtractedFrameworkPattern> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Middleware)
            .collect();

        assert_eq!(
            middleware.len(),
            1,
            "Expected 1 Middleware pattern, got: {middleware:?}"
        );
        assert_eq!(middleware[0].framework, "fastapi");
        assert_eq!(middleware[0].handler, Some("log_requests".to_string()));
    }

    #[test]
    fn test_flask_errorhandler() {
        let source = "@app.errorhandler(404)\ndef not_found(error): pass\n";
        let patterns = parse_and_extract(source);

        let handlers: Vec<&ExtractedFrameworkPattern> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::ErrorHandler)
            .collect();

        assert_eq!(
            handlers.len(),
            1,
            "Expected 1 ErrorHandler pattern, got: {handlers:?}"
        );
        assert_eq!(handlers[0].framework, "flask");
        assert_eq!(handlers[0].handler, Some("not_found".to_string()));
    }

    #[test]
    fn test_flask_after_request() {
        let source = "@app.after_request\ndef add_cors(response): pass\n";
        let patterns = parse_and_extract(source);

        let middleware: Vec<&ExtractedFrameworkPattern> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Middleware)
            .collect();

        assert_eq!(
            middleware.len(),
            1,
            "Expected 1 Middleware pattern for after_request, got: {middleware:?}"
        );
        assert_eq!(middleware[0].handler, Some("add_cors".to_string()));
    }

    #[test]
    fn test_http_verb_methods() {
        let source = "\
@app.get(\"/a\")\ndef a(): pass\n\n\
@app.post(\"/b\")\ndef b(): pass\n\n\
@app.put(\"/c\")\ndef c(): pass\n\n\
@app.delete(\"/d\")\ndef d(): pass\n\n\
@app.patch(\"/e\")\ndef e(): pass\n";
        let patterns = parse_and_extract(source);
        let routes: Vec<&ExtractedFrameworkPattern> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 5, "Expected 5 routes, got: {routes:?}");

        let methods: Vec<&str> = routes
            .iter()
            .filter_map(|r| r.http_method.as_deref())
            .collect();
        assert!(methods.contains(&"GET"));
        assert!(methods.contains(&"POST"));
        assert!(methods.contains(&"PUT"));
        assert!(methods.contains(&"DELETE"));
        assert!(methods.contains(&"PATCH"));
    }

    /// R118 guard: a call like `cache.get(key)` where the first argument is
    /// NOT a string starting with "/" must NOT be emitted as a route.
    #[test]
    fn test_r118_no_false_positives_on_generic_get() {
        let source = "\
@cache.get(\"some_key\")\ndef cached_result(): pass\n\n\
@thing.get(\"not-a-path\")\ndef other(): pass\n";
        let patterns = parse_and_extract(source);
        let routes: Vec<&ExtractedFrameworkPattern> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(
            routes.len(),
            0,
            "Generic .get() with non-path arg must not produce routes, got: {routes:?}"
        );
    }

    #[test]
    fn test_async_fastapi_routes() {
        let source = "@router.get(\"/items/{item_id}\")\nasync def read_item(item_id: int): pass\n";
        let patterns = parse_and_extract(source);
        let routes: Vec<&ExtractedFrameworkPattern> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 1, "Expected 1 route, got: {routes:?}");
        assert_eq!(routes[0].path, Some("/items/{item_id}".to_string()));
        assert_eq!(routes[0].http_method, Some("GET".to_string()));
        assert_eq!(routes[0].handler, Some("read_item".to_string()));
    }

    #[test]
    fn test_line_numbers_are_1_indexed() {
        let source = "@app.get(\"/\")\ndef index(): pass\n";
        let patterns = parse_and_extract(source);
        assert!(!patterns.is_empty());
        // The decorator is on line 1 (1-indexed).
        assert_eq!(
            patterns[0].line, 1,
            "Line numbers must be 1-indexed, got: {}",
            patterns[0].line
        );
    }
}

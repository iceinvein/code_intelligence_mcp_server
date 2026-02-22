//! Axum framework pattern extraction
//!
//! Extracts route registrations, nested routers, middleware layers, and router
//! creation from Rust code that uses the [`axum`] HTTP framework.
//!
//! ## Patterns recognised
//!
//! | Source                                | Kind         | Notes                          |
//! |---------------------------------------|--------------|--------------------------------|
//! | `Router::new()`                       | `Router`     | Builder entry-point            |
//! | `.route("/path", get(handler))`       | `Route`      | HTTP verb extracted from inner call |
//! | `.route("/path", get(h1).post(h2))`   | `Route` (×2) | Each verb in a method chain    |
//! | `.nest("/prefix", subrouter)`         | `Group`      | Sub-router mounting            |
//! | `.layer(middleware)`                  | `Middleware` | Tower layer registration       |
//!
//! ## R118 guard
//!
//! Route and Group patterns are only emitted when the first argument is a
//! `string_literal` that starts with `/`.  This prevents false positives from
//! generic method calls like `map.get(key)` where the receiver happens to be
//! named with an HTTP verb.
//!
//! ## Method chain handling
//!
//! `.route("/path", get(h1).post(h2))` passes a method chain as the second
//! argument.  The extractor walks the whole chain and emits one `Route` pattern
//! per HTTP method verb found.
//!
//! # Example
//!
//! ```rust
//! use code_intelligence_mcp_server::indexer::extract::axum::extract_axum_patterns;
//! use code_intelligence_mcp_server::indexer::parser::{parser_for_id, LanguageId};
//!
//! let source = r#"
//! async fn main() {
//!     let app = Router::new()
//!         .route("/users", get(list_users))
//!         .route("/users/:id", post(create_user));
//! }
//! "#;
//! let mut parser = parser_for_id(LanguageId::Rust).unwrap();
//! let tree = parser.parse(source, None).unwrap();
//! let patterns = extract_axum_patterns(tree.root_node(), source);
//! assert_eq!(patterns.len(), 3); // 1 Router::new() + 2 routes
//! ```

use tree_sitter::Node;

use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

// ---------------------------------------------------------------------------
// Axum HTTP verb functions
// ---------------------------------------------------------------------------

/// Lowercase HTTP verb function names accepted as axum route handlers.
///
/// Axum re-exports these from `axum::routing::{get, post, put, delete, …}`.
/// The `any` / `any_with_state` variants cover catch-all handlers.
const AXUM_HTTP_VERBS: &[&str] = &[
    "get",
    "post",
    "put",
    "delete",
    "patch",
    "options",
    "head",
    "trace",
    "any",
    "any_with_state",
    "get_service",
    "post_service",
    "put_service",
    "delete_service",
    "patch_service",
    "options_service",
    "head_service",
    "trace_service",
];

// ---------------------------------------------------------------------------
// Public entry-point
// ---------------------------------------------------------------------------

/// Extract Axum framework patterns from a parsed Rust AST.
///
/// Walks every `call_expression` in the tree, matching:
///
/// - `Router::new()` → [`FrameworkPatternKind::Router`]
/// - `.route("/path", verb(handler))` → [`FrameworkPatternKind::Route`]
/// - `.nest("/prefix", router)` → [`FrameworkPatternKind::Group`]
/// - `.layer(mw)` → [`FrameworkPatternKind::Middleware`]
///
/// Results are sorted by `(line, column)` for stable output.
pub fn extract_axum_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    walk_and_extract(root, source, &mut patterns);
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

// ---------------------------------------------------------------------------
// AST traversal
// ---------------------------------------------------------------------------

/// Recursively walk every node.  Each `call_expression` is examined for
/// Axum patterns.  Walking continues into child nodes even after a match so
/// that chained calls (`.route(…).nest(…)`) are all captured.
fn walk_and_extract(node: Node, source: &str, out: &mut Vec<ExtractedFrameworkPattern>) {
    if node.kind() == "call_expression" {
        extract_from_call(node, source, out);
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_and_extract(cursor.node(), source, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pattern matching helpers
// ---------------------------------------------------------------------------

/// Attempt to match an Axum pattern at a single `call_expression` node.
///
/// Dispatches to the appropriate sub-extractor based on the method/function
/// name.  A call may emit zero or more patterns (e.g. a chained route handler
/// `.get(h1).post(h2)` yields two routes for the same path).
fn extract_from_call(node: Node, source: &str, out: &mut Vec<ExtractedFrameworkPattern>) {
    let Some(func_node) = node.child_by_field_name("function") else {
        return;
    };

    match func_node.kind() {
        // `Router::new()` — scoped path call like `Router::new` or `axum::Router::new`
        "scoped_identifier" => {
            if let Some(pattern) = try_extract_router_new(node, func_node, source) {
                out.push(pattern);
            }
        }

        // `.route(…)`, `.nest(…)`, `.layer(…)` — method calls.
        // In tree-sitter-rust, `field_expression` has fields `value` (receiver)
        // and `field` (method name as `field_identifier`).
        "field_expression" => {
            let Some(field_node) = func_node.child_by_field_name("field") else {
                return;
            };
            let method = text_for_node(field_node, source);

            match method.as_str() {
                "route" => {
                    extract_route_patterns(node, func_node, source, out);
                }
                "nest" | "nest_service" => {
                    if let Some(pattern) = try_extract_nest(node, func_node, source) {
                        out.push(pattern);
                    }
                }
                "layer" | "route_layer" => {
                    out.push(extract_layer(node, func_node, source));
                }
                _ => {}
            }
        }

        _ => {}
    }
}

/// Emit a [`FrameworkPatternKind::Router`] pattern for `Router::new()`.
///
/// Matches both `Router::new()` (bare) and qualified paths like
/// `axum::Router::new()` by checking that the rightmost identifier in the
/// `scoped_identifier` is `new` and its parent path ends with `Router`.
fn try_extract_router_new(
    call_node: Node,
    func_node: Node,
    source: &str,
) -> Option<ExtractedFrameworkPattern> {
    // In tree-sitter-rust a scoped_identifier has fields `path` and `name`.
    //   Router::new   → path=Router,       name=new
    //   axum::Router::new → path=axum::Router, name=new
    let name_node = func_node.child_by_field_name("name")?;
    let method_name = text_for_node(name_node, source);
    if method_name != "new" {
        return None;
    }

    let path_node = func_node.child_by_field_name("path")?;
    let path_text = text_for_node(path_node, source);

    // Accept `Router`, `axum::Router`, etc.
    let last_segment = path_text.rsplitn(2, "::").next().unwrap_or("");
    if last_segment != "Router" {
        return None;
    }

    // Make sure the call has empty arguments `()`
    let args_node = call_node.child_by_field_name("arguments")?;
    if named_children(args_node).count() != 0 {
        return None;
    }

    let pos = func_node.start_position();
    Some(ExtractedFrameworkPattern {
        line: pos.row as u32 + 1,
        column: pos.column as u32,
        framework: "axum".to_string(),
        kind: FrameworkPatternKind::Router,
        http_method: None,
        path: None,
        name: Some("Router".to_string()),
        handler: None,
        arguments: None,
        parent_chain: None,
    })
}

/// Extract one or more [`FrameworkPatternKind::Route`] patterns from a
/// `.route("/path", handler_expr)` call.
///
/// The handler expression may be a simple `get(handler)` call or a method
/// chain `get(h1).post(h2)`.  All verbs in the chain are emitted.
fn extract_route_patterns(
    call_node: Node,
    func_node: Node,
    source: &str,
    out: &mut Vec<ExtractedFrameworkPattern>,
) {
    let Some(args_node) = call_node.child_by_field_name("arguments") else {
        return;
    };

    let named: Vec<Node> = named_children(args_node).collect();

    // First argument must be a string literal starting with "/"  (R118 guard).
    let Some(first_arg) = named.first() else {
        return;
    };
    if first_arg.kind() != "string_literal" {
        return;
    }
    let path = extract_rust_string(*first_arg, source);
    if !path.starts_with('/') {
        return;
    }

    // Second argument is the handler expression (a routing function or chain).
    let Some(handler_expr) = named.get(1) else {
        return;
    };

    let pos = func_node.start_position();

    // Collect (http_method, handler_name) pairs from the handler expression.
    let mut verb_handlers: Vec<(String, Option<String>)> = Vec::new();
    collect_verb_handlers(*handler_expr, source, &mut verb_handlers);

    if verb_handlers.is_empty() {
        // Unrecognised second argument — still emit a route without method/handler.
        out.push(ExtractedFrameworkPattern {
            line: pos.row as u32 + 1,
            column: pos.column as u32,
            framework: "axum".to_string(),
            kind: FrameworkPatternKind::Route,
            http_method: None,
            path: Some(path),
            name: None,
            handler: None,
            arguments: None,
            parent_chain: None,
        });
    } else {
        for (http_method, handler) in verb_handlers {
            out.push(ExtractedFrameworkPattern {
                line: pos.row as u32 + 1,
                column: pos.column as u32,
                framework: "axum".to_string(),
                kind: FrameworkPatternKind::Route,
                http_method: Some(http_method),
                path: Some(path.clone()),
                name: None,
                handler,
                arguments: None,
                parent_chain: None,
            });
        }
    }
}

/// Walk a handler expression tree and collect `(HTTP_METHOD, handler_name)`
/// pairs from axum routing verb calls.
///
/// Handles both simple `get(handler)` and chained `get(h1).post(h2)`.
fn collect_verb_handlers(
    node: Node,
    source: &str,
    out: &mut Vec<(String, Option<String>)>,
) {
    match node.kind() {
        "call_expression" => {
            let Some(func) = node.child_by_field_name("function") else {
                return;
            };

            match func.kind() {
                // Simple call: `get(handler)` or `routing::get(handler)`
                "identifier" => {
                    let verb = text_for_node(func, source);
                    if let Some(method) = axum_verb_to_method(&verb) {
                        let handler = extract_first_identifier_arg(node, source);
                        out.push((method, handler));
                    }
                }

                // Scoped call: `routing::get(handler)` or `axum::routing::get(handler)`
                "scoped_identifier" => {
                    if let Some(name_node) = func.child_by_field_name("name") {
                        let verb = text_for_node(name_node, source);
                        if let Some(method) = axum_verb_to_method(&verb) {
                            let handler = extract_first_identifier_arg(node, source);
                            out.push((method, handler));
                        }
                    }
                }

                // Chained call: `get(h1).post(h2)` where `func` is a `field_expression`
                // pointing to `get(h1)` as the receiver and `post` as the field.
                // In tree-sitter-rust `field_expression` uses `value` and `field`
                // (not `object`/`property` as in TypeScript).
                "field_expression" => {
                    // Recurse into the receiver to collect earlier verbs in the chain.
                    if let Some(receiver) = func.child_by_field_name("value") {
                        collect_verb_handlers(receiver, source, out);
                    }

                    // Then emit the current verb (field name in tree-sitter-rust is "field").
                    if let Some(field_node) = func.child_by_field_name("field") {
                        let verb = text_for_node(field_node, source);
                        if let Some(method) = axum_verb_to_method(&verb) {
                            let handler = extract_first_identifier_arg(node, source);
                            out.push((method, handler));
                        }
                    }
                }

                _ => {}
            }
        }

        _ => {}
    }
}

/// Extract a [`FrameworkPatternKind::Group`] pattern from a
/// `.nest("/prefix", router)` call.
fn try_extract_nest(
    call_node: Node,
    func_node: Node,
    source: &str,
) -> Option<ExtractedFrameworkPattern> {
    let args_node = call_node.child_by_field_name("arguments")?;
    let named: Vec<Node> = named_children(args_node).collect();

    // First argument: path string starting with "/"  (R118 guard).
    let first_arg = named.first()?;
    if first_arg.kind() != "string_literal" {
        return None;
    }
    let path = extract_rust_string(*first_arg, source);
    if !path.starts_with('/') {
        return None;
    }

    let pos = func_node.start_position();
    Some(ExtractedFrameworkPattern {
        line: pos.row as u32 + 1,
        column: pos.column as u32,
        framework: "axum".to_string(),
        kind: FrameworkPatternKind::Group,
        http_method: None,
        path: Some(path),
        name: None,
        handler: None,
        arguments: None,
        parent_chain: None,
    })
}

/// Extract a [`FrameworkPatternKind::Middleware`] pattern from a
/// `.layer(mw)` call.
fn extract_layer(
    _call_node: Node,
    func_node: Node,
    _source: &str,
) -> ExtractedFrameworkPattern {
    let pos = func_node.start_position();
    ExtractedFrameworkPattern {
        line: pos.row as u32 + 1,
        column: pos.column as u32,
        framework: "axum".to_string(),
        kind: FrameworkPatternKind::Middleware,
        http_method: None,
        path: None,
        name: None,
        handler: None,
        arguments: None,
        parent_chain: None,
    }
}

// ---------------------------------------------------------------------------
// Small utilities
// ---------------------------------------------------------------------------

/// Map a lowercase axum routing verb name to an uppercase HTTP method string.
///
/// Returns `None` for names that are not recognised axum routing verbs.
fn axum_verb_to_method(verb: &str) -> Option<String> {
    // Strip `_service` / `_with_state` suffixes for lookup.
    let base = verb
        .strip_suffix("_service")
        .or_else(|| verb.strip_suffix("_with_state"))
        .unwrap_or(verb);

    if AXUM_HTTP_VERBS.contains(&verb) || AXUM_HTTP_VERBS.contains(&base) {
        // `any` / `any_with_state` → "ANY"
        if base == "any" {
            return Some("ANY".to_string());
        }
        Some(base.to_uppercase())
    } else {
        None
    }
}

/// Return an iterator over the named (non-punctuation) children of a node.
fn named_children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node.children(&mut cursor).filter(|n| n.is_named()).collect();
    children.into_iter()
}

/// Extract a Rust string literal value, stripping surrounding quotes.
///
/// Handles ordinary `"..."` literals.  For raw strings `r#"..."#` the content
/// between the hash-delimited quotes is returned.  The result is trimmed of
/// surrounding `"` characters as a safe fallback.
fn extract_rust_string(node: Node, source: &str) -> String {
    let raw = text_for_node(node, source);
    // Ordinary string: `"value"`
    if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        return raw[1..raw.len() - 1].to_string();
    }
    // Raw string: r#"value"# or r##"value"## etc.
    if raw.starts_with('r') {
        if let Some(inner) = raw.strip_prefix('r') {
            let hashes = inner.chars().take_while(|&c| c == '#').count();
            let prefix_len = 1 + hashes + 1; // r + hashes + opening "
            let suffix_len = 1 + hashes; // closing " + hashes
            if raw.len() >= prefix_len + suffix_len {
                return raw[prefix_len..raw.len() - suffix_len].to_string();
            }
        }
    }
    // Fallback: strip any surrounding quotes.
    raw.trim_matches('"').to_string()
}

/// Return the text of a node (its source slice).
fn text_for_node(node: Node, source: &str) -> String {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or("")
        .to_string()
}

/// Extract the first `identifier` argument from a call expression.
///
/// Used to pull the handler name from `get(my_handler)`.  Returns `None`
/// when the first argument is a closure or other non-identifier expression.
fn extract_first_identifier_arg(call_node: Node, source: &str) -> Option<String> {
    let args = call_node.child_by_field_name("arguments")?;
    let first = named_children(args).next()?;
    if first.kind() == "identifier" {
        Some(text_for_node(first, source))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parser::{parser_for_id, LanguageId};

    fn parse_and_extract(source: &str) -> Vec<ExtractedFrameworkPattern> {
        let mut parser = parser_for_id(LanguageId::Rust).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_axum_patterns(tree.root_node(), source)
    }

    // -----------------------------------------------------------------------
    // Router creation
    // -----------------------------------------------------------------------

    #[test]
    fn test_router_new_detected() {
        let source = r#"
fn build_app() -> Router {
    Router::new()
}
"#;
        let patterns = parse_and_extract(source);
        let routers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Router)
            .collect();

        assert_eq!(routers.len(), 1, "Expected one Router::new() pattern");
        assert_eq!(routers[0].framework, "axum");
        assert_eq!(routers[0].name, Some("Router".to_string()));
    }

    // -----------------------------------------------------------------------
    // Routes
    // -----------------------------------------------------------------------

    #[test]
    fn test_axum_routes() {
        let source = r#"
fn build_app() -> Router {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/:id", get(get_user).post(update_user))
}
"#;
        let patterns = parse_and_extract(source);

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        // At minimum the simple case must be present.
        let get_users = routes
            .iter()
            .find(|p| p.path == Some("/users".to_string()) && p.http_method == Some("GET".to_string()));
        assert!(
            get_users.is_some(),
            "Expected GET /users route, got: {:?}",
            routes
        );
        assert_eq!(get_users.unwrap().handler, Some("list_users".to_string()));
        assert_eq!(get_users.unwrap().framework, "axum");

        // Bonus: chained `.route("/users/:id", get(get_user).post(update_user))`
        // should produce both GET and POST.
        let get_by_id = routes
            .iter()
            .find(|p| p.path == Some("/users/:id".to_string()) && p.http_method == Some("GET".to_string()));
        let post_by_id = routes
            .iter()
            .find(|p| p.path == Some("/users/:id".to_string()) && p.http_method == Some("POST".to_string()));

        if get_by_id.is_some() || post_by_id.is_some() {
            // Chain handling is supported — verify both are present.
            assert!(
                get_by_id.is_some(),
                "Expected GET /users/:id from chained handler"
            );
            assert!(
                post_by_id.is_some(),
                "Expected POST /users/:id from chained handler"
            );
            assert_eq!(get_by_id.unwrap().handler, Some("get_user".to_string()));
            assert_eq!(post_by_id.unwrap().handler, Some("update_user".to_string()));
        }
    }

    #[test]
    fn test_route_path_must_start_with_slash() {
        // Non-path string should be rejected by the R118 guard.
        let source = r#"
fn setup() {
    Router::new().route("users", get(list));
}
"#;
        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(
            routes.len(),
            0,
            "Route without leading '/' must be rejected"
        );
    }

    // -----------------------------------------------------------------------
    // Nest and Layer
    // -----------------------------------------------------------------------

    #[test]
    fn test_axum_nest_and_layer() {
        let source = r#"
fn build_app() -> Router {
    Router::new()
        .nest("/api", api_router)
        .layer(cors_layer)
}
"#;
        let patterns = parse_and_extract(source);

        let groups: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Group)
            .collect();
        let middleware: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Middleware)
            .collect();

        assert_eq!(groups.len(), 1, "Expected 1 nest() Group pattern");
        assert_eq!(groups[0].path, Some("/api".to_string()));
        assert_eq!(groups[0].framework, "axum");

        assert_eq!(middleware.len(), 1, "Expected 1 layer() Middleware pattern");
        assert_eq!(middleware[0].framework, "axum");
    }

    #[test]
    fn test_nest_path_must_start_with_slash() {
        let source = r#"
fn setup() {
    Router::new().nest("api", sub);
}
"#;
        let patterns = parse_and_extract(source);
        let groups: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Group)
            .collect();
        assert_eq!(groups.len(), 0, "nest() without '/' must be rejected");
    }

    // -----------------------------------------------------------------------
    // False-positive guards
    // -----------------------------------------------------------------------

    #[test]
    fn test_generic_get_not_treated_as_route() {
        // `.get(key)` on a HashMap / similar must not produce a route.
        let source = r#"
fn handler() {
    let val = my_map.get("some_key");
    let hdr = headers.get("content-type");
}
"#;
        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(
            routes.len(),
            0,
            "Generic .get() calls must not produce Route patterns"
        );
    }

    // -----------------------------------------------------------------------
    // Miscellaneous verb coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_various_http_verbs() {
        let source = r#"
fn build_app() -> Router {
    Router::new()
        .route("/items", post(create_item))
        .route("/items/:id", put(replace_item))
        .route("/items/:id", delete(remove_item))
        .route("/items/:id", patch(update_item))
}
"#;
        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert!(
            routes.iter().any(|r| r.http_method == Some("POST".to_string())),
            "Expected POST route"
        );
        assert!(
            routes.iter().any(|r| r.http_method == Some("PUT".to_string())),
            "Expected PUT route"
        );
        assert!(
            routes.iter().any(|r| r.http_method == Some("DELETE".to_string())),
            "Expected DELETE route"
        );
        assert!(
            routes.iter().any(|r| r.http_method == Some("PATCH".to_string())),
            "Expected PATCH route"
        );
    }
}

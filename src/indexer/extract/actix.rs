//! Actix-web framework pattern extraction.
//!
//! Extracts routes and builder API patterns from Actix-web Rust source files using
//! two complementary strategies:
//!
//! **Phase 1 — Attribute macros:** Actix-web uses proc-macro attributes on handler
//! functions to declare HTTP routes.  A `#[get("/users")]` attribute followed by a
//! `fn` item produces a `Route` pattern with `http_method = "GET"` and the
//! function name as `handler`.
//!
//! **Phase 2 — Builder API calls:** The `web::resource("/path")` and
//! `web::scope("/path")` builder calls are detected by recognising `scoped_identifier`
//! nodes whose `path` is `web` and whose `name` is `resource` / `scope`.
//! `.app_data(...)` chains are captured as `State` patterns.
//!
//! # R118 Guard
//! Route paths are validated to start with `"/"` before being emitted, preventing
//! false positives from non-HTTP builder calls.

use tree_sitter::Node;

use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

// ---------------------------------------------------------------------------
// HTTP method attribute names recognised as Actix-web route macros
// ---------------------------------------------------------------------------

/// Actix-web HTTP method attribute names (lowercase).
const ACTIX_HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "head", "options", "route",
];

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Extract Actix-web framework patterns from a Rust AST.
///
/// Returns patterns sorted by `(line, column)` for deterministic ordering.
///
/// # Examples
///
/// ```ignore
/// use crate::indexer::parser::{parser_for_id, LanguageId};
/// use crate::indexer::extract::actix::extract_actix_patterns;
///
/// let mut parser = parser_for_id(LanguageId::Rust).unwrap();
/// let source = r#"
/// #[get("/users")]
/// async fn get_users() -> impl Responder { HttpResponse::Ok() }
/// "#;
/// let tree = parser.parse(source, None).unwrap();
/// let patterns = extract_actix_patterns(tree.root_node(), source);
/// assert_eq!(patterns.len(), 1);
/// assert_eq!(patterns[0].path, Some("/users".to_string()));
/// ```
pub fn extract_actix_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();

    // Phase 1: attribute macros on functions (walk at every scope level).
    extract_attribute_routes(root, source, &mut patterns);

    // Phase 2: web::resource / web::scope / .app_data builder calls.
    extract_builder_calls(root, source, &mut patterns);

    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

// ---------------------------------------------------------------------------
// Phase 1 — Attribute macro routes
// ---------------------------------------------------------------------------

/// Walk the AST collecting `attribute_item → function_item` pairs at every
/// scope level and emit a `Route` pattern for each Actix-web HTTP method
/// attribute.
fn extract_attribute_routes(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    extract_attribute_routes_in_scope(node, source, patterns);
}

/// Scan the direct named children of `scope_node` for attribute / function
/// pairs, then recurse into every child scope (impl blocks, mod items, etc.).
fn extract_attribute_routes_in_scope(
    scope_node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    // Collect all direct children so we can look ahead from attribute_item to
    // the following function_item sibling.
    let mut cursor = scope_node.walk();
    let children: Vec<Node> = scope_node.children(&mut cursor).collect();

    let mut pending_attrs: Vec<Node> = Vec::new();

    for child in &children {
        match child.kind() {
            "attribute_item" => {
                pending_attrs.push(*child);
            }
            "function_item" => {
                // Inspect pending attribute items for Actix HTTP method macros.
                for attr_node in &pending_attrs {
                    if let Some(pattern) =
                        try_extract_attribute_route(*attr_node, *child, source)
                    {
                        patterns.push(pattern);
                        // One route pattern per function — first match wins.
                        break;
                    }
                }
                pending_attrs.clear();

                // Recurse into the function body (nested handlers are rare but
                // technically valid in Rust).
                extract_attribute_routes_in_scope(*child, source, patterns);
            }
            // An impl block or mod item may itself contain attributed functions.
            "impl_item" | "mod_item" => {
                pending_attrs.clear();
                extract_attribute_routes_in_scope(*child, source, patterns);
            }
            // Any other node resets the pending-attribute window.
            _ => {
                if child.is_named() {
                    pending_attrs.clear();
                }
            }
        }
    }
}

/// Given an `attribute_item` node and the immediately following `function_item`
/// node, return an `ExtractedFrameworkPattern` if the attribute encodes an
/// Actix-web HTTP method macro, or `None` otherwise.
fn try_extract_attribute_route(
    attr_item: Node,
    fn_item: Node,
    source: &str,
) -> Option<ExtractedFrameworkPattern> {
    // attribute_item → attribute → (identifier | scoped_identifier) + token_tree
    let attribute = attr_item
        .named_children(&mut attr_item.walk())
        .find(|n| n.kind() == "attribute")?;

    // Extract the method name from either a plain `identifier` or a
    // `scoped_identifier` (e.g., `actix_web::get`).
    let (method_name, attr_path_node) = extract_attribute_method_name(attribute, source)?;

    // Check this is a recognised HTTP method macro.
    if !ACTIX_HTTP_METHODS.contains(&method_name.as_str()) {
        return None;
    }

    // Extract the route path from the token_tree argument: `("/users")`.
    let path = extract_path_from_token_tree(attribute, source)?;

    // R118 guard: path must start with "/".
    if !path.starts_with('/') {
        return None;
    }

    // Handler is the function name.
    let handler = fn_item
        .child_by_field_name("name")
        .map(|n| text_for_node(n, source).to_string());

    let pos = attr_path_node.start_position();

    Some(ExtractedFrameworkPattern {
        line: pos.row as u32 + 1,
        column: pos.column as u32,
        framework: "actix".to_string(),
        kind: FrameworkPatternKind::Route,
        http_method: Some(method_name.to_uppercase()),
        path: Some(path),
        name: None,
        handler,
        arguments: None,
        parent_chain: None,
    })
}

/// Extract the HTTP method name from an `attribute` node.
///
/// Handles two forms:
/// - `#[get(...)]` → plain `identifier` node named `"get"`
/// - `#[actix_web::get(...)]` → `scoped_identifier` whose last segment is `"get"`
///
/// Returns `(method_name_lowercase, the_node_used_for_position)` or `None`.
fn extract_attribute_method_name<'a>(
    attribute: Node<'a>,
    source: &str,
) -> Option<(String, Node<'a>)> {
    // The first named child of `attribute` is the path.
    let path_node = attribute.named_children(&mut attribute.walk()).next()?;

    match path_node.kind() {
        "identifier" => {
            let name = text_for_node(path_node, source).to_lowercase();
            Some((name, path_node))
        }
        "scoped_identifier" => {
            // scoped_identifier: `actix_web::get`
            // The last `identifier` child is the method name.
            let last_ident = path_node
                .named_children(&mut path_node.walk())
                .filter(|n| n.kind() == "identifier")
                .last()?;
            let name = text_for_node(last_ident, source).to_lowercase();
            Some((name, last_ident))
        }
        _ => None,
    }
}

/// Extract the route path string from the `token_tree` argument of an attribute.
///
/// The token tree looks like `("/users")` where the inner node is a
/// `string_literal` containing a `string_content` child.
fn extract_path_from_token_tree(attribute: Node, source: &str) -> Option<String> {
    let token_tree = attribute
        .named_children(&mut attribute.walk())
        .find(|n| n.kind() == "token_tree")?;

    // Find the first string_literal inside the token_tree.
    let string_lit = token_tree
        .named_children(&mut token_tree.walk())
        .find(|n| n.kind() == "string_literal")?;

    Some(extract_rust_string(string_lit, source))
}

// ---------------------------------------------------------------------------
// Phase 2 — Builder API calls
// ---------------------------------------------------------------------------

/// Recursively walk the entire AST looking for:
/// - `web::resource("/path")` → `Route`
/// - `web::scope("/path")` → `Group`
/// - `.app_data(...)` → `State`
fn extract_builder_calls(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    if node.kind() == "call_expression" {
        if let Some(pattern) = try_extract_web_call(node, source) {
            patterns.push(pattern);
        } else if let Some(pattern) = try_extract_app_data(node, source) {
            patterns.push(pattern);
        }
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            extract_builder_calls(cursor.node(), source, patterns);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Try to match `web::resource("/path")` or `web::scope("/path")`.
///
/// Tree structure:
/// ```text
/// call_expression
///   scoped_identifier  (function)
///     identifier "web"
///     identifier "resource" | "scope"
///   arguments
///     string_literal "/path"
/// ```
fn try_extract_web_call(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    let func_node = node.child_by_field_name("function")?;

    if func_node.kind() != "scoped_identifier" {
        return None;
    }

    // The path component must be "web".
    let path_id = func_node
        .named_children(&mut func_node.walk())
        .find(|n| n.kind() == "identifier")?;

    if text_for_node(path_id, source) != "web" {
        return None;
    }

    // The name component is the last identifier in the scoped_identifier.
    let name_id = func_node
        .named_children(&mut func_node.walk())
        .filter(|n| n.kind() == "identifier")
        .last()?;

    let call_name = text_for_node(name_id, source);

    let kind = match call_name {
        "resource" => FrameworkPatternKind::Route,
        "scope" => FrameworkPatternKind::Group,
        _ => return None,
    };

    let args = node.child_by_field_name("arguments")?;
    let path = first_string_arg_rust(args, source)?;

    // R118 guard: path must start with "/".
    if !path.starts_with('/') {
        return None;
    }

    let pos = func_node.start_position();

    Some(ExtractedFrameworkPattern {
        line: pos.row as u32 + 1,
        column: pos.column as u32,
        framework: "actix".to_string(),
        kind,
        http_method: None,
        path: Some(path),
        name: None,
        handler: None,
        arguments: None,
        parent_chain: None,
    })
}

/// Try to match `.app_data(...)` method calls.
///
/// Tree structure:
/// ```text
/// call_expression
///   field_expression  (function)
///     <receiver>
///     field_identifier "app_data"
///   arguments
///     ...
/// ```
fn try_extract_app_data(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    let func_node = node.child_by_field_name("function")?;

    if func_node.kind() != "field_expression" {
        return None;
    }

    // The field (method name) must be "app_data".
    let field = func_node
        .named_children(&mut func_node.walk())
        .find(|n| n.kind() == "field_identifier")?;

    if text_for_node(field, source) != "app_data" {
        return None;
    }

    let pos = field.start_position();

    Some(ExtractedFrameworkPattern {
        line: pos.row as u32 + 1,
        column: pos.column as u32,
        framework: "actix".to_string(),
        kind: FrameworkPatternKind::State,
        http_method: None,
        path: None,
        name: None,
        handler: None,
        arguments: None,
        parent_chain: None,
    })
}

// ---------------------------------------------------------------------------
// String extraction helpers for Rust AST
// ---------------------------------------------------------------------------

/// Extract the unquoted string content from a Rust `string_literal` node.
///
/// A `string_literal` in tree-sitter-rust has the structure:
/// ```text
/// string_literal
///   " (unnamed)
///   string_content (named) ← the actual text
///   " (unnamed)
/// ```
fn extract_rust_string(string_lit: Node, source: &str) -> String {
    // Prefer the named `string_content` child for correctness.
    if let Some(content) = string_lit
        .named_children(&mut string_lit.walk())
        .find(|n| n.kind() == "string_content")
    {
        return text_for_node(content, source).to_string();
    }

    // Fallback: strip quotes from the raw node text.
    let raw = text_for_node(string_lit, source);
    raw.trim_matches('"').to_string()
}

/// Get the first `string_literal` argument from a Rust `arguments` node,
/// returning its unquoted content.
fn first_string_arg_rust(args_node: Node, source: &str) -> Option<String> {
    args_node
        .named_children(&mut args_node.walk())
        .find(|n| n.kind() == "string_literal")
        .map(|n| extract_rust_string(n, source))
}

/// Return the source text for a node.
#[inline]
fn text_for_node<'a>(node: Node, source: &'a str) -> &'a str {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or("")
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
        extract_actix_patterns(tree.root_node(), source)
    }

    /// Phase 1: attribute macro routes are extracted with the correct HTTP method,
    /// path, and handler function name.
    #[test]
    fn test_attribute_routes() {
        let source = r#"
#[get("/users")]
async fn get_users() -> impl Responder {
    HttpResponse::Ok()
}

#[post("/items")]
async fn create_item() -> impl Responder {
    HttpResponse::Created()
}

#[put("/users/{id}")]
async fn update_user() -> impl Responder {
    HttpResponse::Ok()
}

#[delete("/users/{id}")]
async fn delete_user() -> impl Responder {
    HttpResponse::NoContent()
}
"#;

        let patterns = parse_and_extract(source);

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route && p.http_method.is_some())
            .collect();

        assert_eq!(routes.len(), 4, "Expected 4 route patterns, got {routes:?}");

        // GET /users → get_users
        let get_route = routes.iter().find(|p| p.http_method == Some("GET".to_string()));
        assert!(get_route.is_some(), "Expected a GET route");
        let get_route = get_route.unwrap();
        assert_eq!(get_route.path, Some("/users".to_string()));
        assert_eq!(get_route.handler, Some("get_users".to_string()));
        assert_eq!(get_route.framework, "actix");

        // POST /items → create_item
        let post_route = routes.iter().find(|p| p.http_method == Some("POST".to_string()));
        assert!(post_route.is_some(), "Expected a POST route");
        let post_route = post_route.unwrap();
        assert_eq!(post_route.path, Some("/items".to_string()));
        assert_eq!(post_route.handler, Some("create_item".to_string()));

        // PUT /users/{id}
        let put_route = routes.iter().find(|p| p.http_method == Some("PUT".to_string()));
        assert!(put_route.is_some(), "Expected a PUT route");
        assert_eq!(put_route.unwrap().path, Some("/users/{id}".to_string()));

        // DELETE /users/{id}
        let delete_route = routes.iter().find(|p| p.http_method == Some("DELETE".to_string()));
        assert!(delete_route.is_some(), "Expected a DELETE route");
    }

    /// Scoped attribute macros (`#[actix_web::get(...)]`) are handled alongside
    /// the short form.
    #[test]
    fn test_scoped_attribute_routes() {
        let source = r#"
#[actix_web::get("/health")]
async fn health_check() -> impl Responder {
    HttpResponse::Ok()
}

#[actix_web::post("/users")]
async fn create_user() -> impl Responder {
    HttpResponse::Created()
}
"#;

        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(routes.len(), 2, "Expected 2 scoped-attribute routes");
        assert_eq!(routes[0].http_method, Some("GET".to_string()));
        assert_eq!(routes[0].path, Some("/health".to_string()));
        assert_eq!(routes[0].handler, Some("health_check".to_string()));

        assert_eq!(routes[1].http_method, Some("POST".to_string()));
        assert_eq!(routes[1].path, Some("/users".to_string()));
    }

    /// Phase 2: `web::resource` and `web::scope` builder calls are extracted as
    /// `Route` and `Group` patterns respectively.
    #[test]
    fn test_builder_api() {
        let source = r#"
fn configure(cfg: &mut web::ServiceConfig) {
    web::resource("/users").route(web::get().to(list_users));
    web::scope("/api");
}
"#;

        let patterns = parse_and_extract(source);

        let resources: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route && p.http_method.is_none())
            .collect();

        let groups: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Group)
            .collect();

        assert_eq!(resources.len(), 1, "Expected 1 Route from web::resource");
        assert_eq!(resources[0].path, Some("/users".to_string()));
        assert_eq!(resources[0].framework, "actix");

        assert_eq!(groups.len(), 1, "Expected 1 Group from web::scope");
        assert_eq!(groups[0].path, Some("/api".to_string()));
        assert_eq!(groups[0].framework, "actix");
    }

    /// `.app_data(...)` calls are captured as `State` patterns.
    #[test]
    fn test_app_data_state() {
        let source = r#"
fn configure(cfg: &mut web::ServiceConfig) {
    web::scope("/api").app_data(web::Data::new(db_pool.clone()));
}
"#;

        let patterns = parse_and_extract(source);

        let state: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::State)
            .collect();

        assert!(!state.is_empty(), "Expected at least one State pattern for .app_data()");
        assert_eq!(state[0].framework, "actix");
    }

    /// Paths that do not start with "/" must be rejected (R118 guard).
    #[test]
    fn test_non_http_paths_rejected() {
        let source = r#"
fn not_a_route() {
    web::resource("users").route(web::get().to(handler));
}
"#;

        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert!(
            routes.is_empty(),
            "Paths not starting with '/' must not produce Route patterns"
        );
    }

    /// Non-HTTP attribute macros (e.g., `#[derive]`, `#[cfg]`) must not produce
    /// route patterns.
    #[test]
    fn test_non_route_attributes_ignored() {
        let source = r#"
#[derive(Debug, Clone)]
struct MyStruct {
    field: u32,
}

#[cfg(test)]
mod tests {
    fn helper() {}
}
"#;

        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert!(routes.is_empty(), "Non-HTTP attributes must not produce Route patterns");
    }

    /// `#[patch]` and `#[head]` are also recognised.
    #[test]
    fn test_patch_and_head_routes() {
        let source = r#"
#[patch("/users/{id}")]
async fn patch_user() -> impl Responder {
    HttpResponse::Ok()
}

#[head("/ping")]
async fn head_ping() -> impl Responder {
    HttpResponse::Ok()
}
"#;

        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(routes.len(), 2);
        assert!(routes.iter().any(|r| r.http_method == Some("PATCH".to_string())));
        assert!(routes.iter().any(|r| r.http_method == Some("HEAD".to_string())));
    }
}

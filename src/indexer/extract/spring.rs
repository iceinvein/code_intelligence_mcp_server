//! Spring Boot framework pattern extraction for Java.
//!
//! Extracts routes, controllers, injectable components, modules, and route
//! groups from Spring Boot annotation syntax.
//!
//! Spring Boot uses Java annotations (inside `modifiers` nodes) to define its
//! structure:
//!
//! - `@RestController` / `@Controller` on a class → `Controller` kind
//! - `@RequestMapping("/prefix")` on a class → `Group` kind (route prefix)
//! - `@GetMapping("/path")` on a method → `Route` kind, HTTP method GET
//! - `@PostMapping("/path")` on a method → `Route` kind, HTTP method POST
//! - `@PutMapping`, `@DeleteMapping`, `@PatchMapping` → `Route` kind
//! - `@RequestMapping` on a method → `Route` kind (method from args or ANY)
//! - `@Service` / `@Repository` / `@Component` → `Injectable` kind
//! - `@Configuration` → `Module` kind
//!
//! ## Tree-sitter-java AST layout
//!
//! Annotations in Java live inside a `modifiers` node that is a direct child
//! of the enclosing declaration:
//!
//! ```text
//! class_declaration
//!   modifiers
//!     marker_annotation          ← @RestController (no args)
//!       name: identifier
//!     annotation                 ← @RequestMapping("/api") (with args)
//!       name: identifier
//!       arguments: annotation_argument_list
//!         string_literal
//!   name: identifier "UserController"
//!   body: class_body
//!     method_declaration
//!       modifiers
//!         annotation
//!           name: identifier "GetMapping"
//!           arguments: annotation_argument_list
//!             string_literal "\"/users\""
//!       type: ...
//!       name: identifier "getUsers"
//! ```

use tree_sitter::Node;

use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Extract Spring Boot framework patterns from a Java AST.
///
/// Walks the tree looking for `annotation` and `marker_annotation` nodes inside
/// `modifiers` on class and method declarations, classifying them into Spring
/// Boot-specific pattern kinds.
///
/// # Examples
///
/// ```no_run
/// # use code_intelligence_mcp_server::indexer::extract::spring::extract_spring_patterns;
/// # use code_intelligence_mcp_server::indexer::parser::{parser_for_id, LanguageId};
/// let source = r#"
/// @RestController
/// public class UserController {
///     @GetMapping("/users")
///     public void getUsers() {}
/// }
/// "#;
/// let mut parser = parser_for_id(LanguageId::Java).unwrap();
/// let tree = parser.parse(source, None).unwrap();
/// let patterns = extract_spring_patterns(tree.root_node(), source);
/// assert!(!patterns.is_empty());
/// ```
pub fn extract_spring_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    collect_spring_patterns(root, source, &mut patterns);
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

// ---------------------------------------------------------------------------
// Recursive AST walk
// ---------------------------------------------------------------------------

/// Recursively walk every node, collecting patterns from `modifiers` children.
fn collect_spring_patterns(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    match node.kind() {
        "class_declaration" => {
            collect_from_declaration(node, source, patterns, DeclarationScope::Class);
            // Recurse into class body for nested method declarations.
            if let Some(body) = node.child_by_field_name("body") {
                let mut cursor = body.walk();
                for child in body.children(&mut cursor) {
                    collect_spring_patterns(child, source, patterns);
                }
            }
            // Don't recurse further via the default path to avoid revisiting.
            return;
        }
        "method_declaration" => {
            collect_from_declaration(node, source, patterns, DeclarationScope::Method);
            // Don't recurse into method bodies — no Spring annotations inside.
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            collect_spring_patterns(cursor.node(), source, patterns);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Declaration-scoped annotation processing
// ---------------------------------------------------------------------------

/// Whether annotations are being examined on a class or a method declaration.
#[derive(Clone, Copy)]
enum DeclarationScope {
    Class,
    Method,
}

/// Process all Spring annotations in the `modifiers` child of a declaration.
fn collect_from_declaration(
    decl_node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
    scope: DeclarationScope,
) {
    // The modifiers node is an (unnamed-field) child of the declaration node.
    let mut cursor = decl_node.walk();
    for child in decl_node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            collect_from_modifiers(child, source, patterns, decl_node, scope);
        }
    }
}

/// Walk the `modifiers` node and process each annotation child.
fn collect_from_modifiers(
    modifiers: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
    decl_node: Node,
    scope: DeclarationScope,
) {
    let mut cursor = modifiers.walk();
    for child in modifiers.children(&mut cursor) {
        match child.kind() {
            "marker_annotation" | "annotation" => {
                if let Some(pattern) =
                    parse_spring_annotation(child, source, decl_node, scope)
                {
                    patterns.push(pattern);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Annotation parsing
// ---------------------------------------------------------------------------

/// Parse a single Java annotation node into a Spring `ExtractedFrameworkPattern`.
///
/// Returns `None` when the annotation is not a recognised Spring Boot annotation.
fn parse_spring_annotation(
    annotation: Node,
    source: &str,
    decl_node: Node,
    scope: DeclarationScope,
) -> Option<ExtractedFrameworkPattern> {
    // Both `annotation` and `marker_annotation` have a `name` field.
    let name_node = annotation.child_by_field_name("name")?;
    let annotation_name = extract_simple_name(name_node, source);

    // `annotation` nodes have an `arguments` field; `marker_annotation` nodes
    // do not. We use the field rather than checking the node kind.
    let args_node = annotation.child_by_field_name("arguments");

    let pos = annotation.start_position();
    let line = pos.row as u32 + 1;
    let column = pos.column as u32;

    // Look up the declaring symbol name from the enclosing declaration.
    let handler = decl_node
        .child_by_field_name("name")
        .map(|n| node_text(n, source));

    match (annotation_name.as_str(), scope) {
        // ------------------------------------------------------------------
        // Route annotations — only meaningful on method declarations
        // ------------------------------------------------------------------
        ("GetMapping", DeclarationScope::Method) => {
            let path = extract_path_from_args(args_node, source).unwrap_or_else(|| "/".to_string());
            Some(make_route("GET", path, handler, line, column))
        }
        ("PostMapping", DeclarationScope::Method) => {
            let path = extract_path_from_args(args_node, source).unwrap_or_else(|| "/".to_string());
            Some(make_route("POST", path, handler, line, column))
        }
        ("PutMapping", DeclarationScope::Method) => {
            let path = extract_path_from_args(args_node, source).unwrap_or_else(|| "/".to_string());
            Some(make_route("PUT", path, handler, line, column))
        }
        ("DeleteMapping", DeclarationScope::Method) => {
            let path = extract_path_from_args(args_node, source).unwrap_or_else(|| "/".to_string());
            Some(make_route("DELETE", path, handler, line, column))
        }
        ("PatchMapping", DeclarationScope::Method) => {
            let path = extract_path_from_args(args_node, source).unwrap_or_else(|| "/".to_string());
            Some(make_route("PATCH", path, handler, line, column))
        }
        ("RequestMapping", DeclarationScope::Method) => {
            // Extract HTTP method from `method = RequestMethod.GET` argument,
            // falling back to a generic "ANY" sentinel when absent.
            let http_method = args_node
                .and_then(|a| extract_request_method_from_args(a, source))
                .unwrap_or_else(|| "ANY".to_string());
            let path = extract_path_from_args(args_node, source).unwrap_or_else(|| "/".to_string());
            Some(make_route(&http_method, path, handler, line, column))
        }

        // ------------------------------------------------------------------
        // Class-level annotations
        // ------------------------------------------------------------------
        ("RestController" | "Controller", DeclarationScope::Class) => {
            Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "spring".to_string(),
                kind: FrameworkPatternKind::Controller,
                http_method: None,
                path: None,
                name: handler,
                handler: None,
                arguments: None,
                parent_chain: None,
            })
        }
        ("RequestMapping", DeclarationScope::Class) => {
            // Class-level @RequestMapping defines the route prefix/group.
            let path = extract_path_from_args(args_node, source);
            Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "spring".to_string(),
                kind: FrameworkPatternKind::Group,
                http_method: None,
                path,
                name: handler,
                handler: None,
                arguments: None,
                parent_chain: None,
            })
        }
        ("Service" | "Repository" | "Component", DeclarationScope::Class) => {
            Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "spring".to_string(),
                kind: FrameworkPatternKind::Injectable,
                http_method: None,
                path: None,
                name: handler,
                handler: None,
                arguments: None,
                parent_chain: None,
            })
        }
        ("Configuration", DeclarationScope::Class) => {
            Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "spring".to_string(),
                kind: FrameworkPatternKind::Module,
                http_method: None,
                path: None,
                name: handler,
                handler: None,
                arguments: None,
                parent_chain: None,
            })
        }

        // Unknown or scope-mismatched annotation — skip.
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Path / HTTP method extraction from annotation arguments
// ---------------------------------------------------------------------------

/// Extract the route path string from an `annotation_argument_list` node.
///
/// Handles three forms:
/// - `@GetMapping("/users")` — single positional string literal
/// - `@GetMapping(value = "/users")` — named `value` element-value-pair
/// - `@GetMapping(path = "/users")` — named `path` element-value-pair
/// - `@GetMapping` (no args node) — caller defaults to "/"
fn extract_path_from_args(args: Option<Node>, source: &str) -> Option<String> {
    let args = args?;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            // Named argument: value="/users" or path="/users"
            "element_value_pair" => {
                if let Some(key) = child.child_by_field_name("key") {
                    let key_text = node_text(key, source);
                    if key_text == "value" || key_text == "path" {
                        if let Some(value) = child.child_by_field_name("value") {
                            return extract_string_literal(value, source);
                        }
                    }
                }
            }
            // Positional string literal: "/users"
            "string_literal" => {
                return Some(strip_java_string_quotes(node_text(child, source)));
            }
            _ => {}
        }
    }
    None
}

/// Extract the `RequestMethod.XXX` method value from `@RequestMapping` args.
///
/// Looks for `method = RequestMethod.GET` (element-value-pair) and returns
/// the method name (e.g., `"GET"`). Returns `None` when not present.
fn extract_request_method_from_args(args: Node, source: &str) -> Option<String> {
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "element_value_pair" {
            if let Some(key) = child.child_by_field_name("key") {
                if node_text(key, source) == "method" {
                    if let Some(value) = child.child_by_field_name("value") {
                        // Value is e.g. `RequestMethod.GET` — a field_access or identifier.
                        return extract_request_method_name(value, source);
                    }
                }
            }
        }
    }
    None
}

/// Extract the method name from a `RequestMethod.GET`-style node.
///
/// The node may be:
/// - `field_access`: `RequestMethod.GET` → extract the `field` child
/// - `identifier`: bare `GET` (rare but valid)
fn extract_request_method_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "field_access" => {
            // field_access has `object` and `field` children.
            // We want the `field` part: e.g. GET from RequestMethod.GET.
            node.child_by_field_name("field")
                .map(|f| node_text(f, source))
        }
        "identifier" => Some(node_text(node, source)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// String literal helpers
// ---------------------------------------------------------------------------

/// Attempt to extract a Java string literal value from a node.
///
/// Checks for a `string_literal` leaf or descends one level to find one.
fn extract_string_literal(node: Node, source: &str) -> Option<String> {
    if node.kind() == "string_literal" {
        return Some(strip_java_string_quotes(node_text(node, source)));
    }
    // Descend one level (e.g., for parenthesised expressions or arrays).
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_literal" {
            return Some(strip_java_string_quotes(node_text(child, source)));
        }
    }
    None
}

/// Remove surrounding double-quote characters from a Java string literal text.
///
/// The tree-sitter Java grammar represents string literals including their
/// quote characters, so `"\"hello\""` → `"hello"`.
fn strip_java_string_quotes(text: String) -> String {
    text.trim_matches('"').to_string()
}

// ---------------------------------------------------------------------------
// Name helpers
// ---------------------------------------------------------------------------

/// Extract the simple (last-segment) name from an `identifier` or
/// `scoped_identifier` node.
///
/// For `scoped_identifier` like `org.springframework.web.bind.annotation.GetMapping`
/// we return only `GetMapping`.
fn extract_simple_name(node: Node, source: &str) -> String {
    match node.kind() {
        "identifier" => node_text(node, source),
        "scoped_identifier" => {
            // The `name` field on scoped_identifier holds the trailing simple name.
            node.child_by_field_name("name")
                .map(|n| node_text(n, source))
                .unwrap_or_else(|| node_text(node, source))
        }
        _ => node_text(node, source),
    }
}

/// Return the source text covered by `node`.
#[inline]
fn node_text(node: Node, source: &str) -> String {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or("")
        .to_string()
}

// ---------------------------------------------------------------------------
// Pattern constructors
// ---------------------------------------------------------------------------

/// Build a `Route` pattern.
fn make_route(
    http_method: &str,
    path: String,
    handler: Option<String>,
    line: u32,
    column: u32,
) -> ExtractedFrameworkPattern {
    ExtractedFrameworkPattern {
        line,
        column,
        framework: "spring".to_string(),
        kind: FrameworkPatternKind::Route,
        http_method: Some(http_method.to_string()),
        path: Some(path),
        name: None,
        handler,
        arguments: None,
        parent_chain: None,
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
        let mut parser = parser_for_id(LanguageId::Java).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_spring_patterns(tree.root_node(), source)
    }

    /// Verify that `@GetMapping` and `@PostMapping` on methods produce Route
    /// patterns with the correct HTTP method and path.
    #[test]
    fn test_route_mappings() {
        let source = r#"
class Ctrl {
    @GetMapping("/users")
    public void getUsers() {}

    @PostMapping("/items")
    public void createItem() {}
}
"#;
        let patterns = parse_and_extract(source);

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(routes.len(), 2, "Expected 2 Route patterns, got: {routes:?}");

        let get_route = routes.iter().find(|r| r.http_method == Some("GET".to_string()));
        assert!(get_route.is_some(), "Expected a GET route");
        let get = get_route.unwrap();
        assert_eq!(get.path, Some("/users".to_string()));
        assert_eq!(get.handler, Some("getUsers".to_string()));
        assert_eq!(get.framework, "spring");

        let post_route = routes.iter().find(|r| r.http_method == Some("POST".to_string()));
        assert!(post_route.is_some(), "Expected a POST route");
        let post = post_route.unwrap();
        assert_eq!(post.path, Some("/items".to_string()));
        assert_eq!(post.handler, Some("createItem".to_string()));
    }

    /// Verify that `@RestController` produces a Controller pattern and
    /// class-level `@RequestMapping` produces a Group pattern with the prefix.
    #[test]
    fn test_controller_annotation() {
        let source = r#"
@RestController
@RequestMapping("/api")
public class UserController {}
"#;
        let patterns = parse_and_extract(source);

        let controllers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Controller)
            .collect();
        assert_eq!(controllers.len(), 1, "Expected 1 Controller pattern, got: {controllers:?}");
        assert_eq!(controllers[0].framework, "spring");
        assert_eq!(controllers[0].name, Some("UserController".to_string()));

        let groups: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Group)
            .collect();
        assert_eq!(groups.len(), 1, "Expected 1 Group pattern, got: {groups:?}");
        assert_eq!(groups[0].path, Some("/api".to_string()));
        assert_eq!(groups[0].name, Some("UserController".to_string()));
    }

    /// Verify that `@Service` and `@Repository` produce Injectable patterns.
    #[test]
    fn test_injectable_annotations() {
        let source = r#"
@Service
public class UserService {}

@Repository
public class UserRepo {}
"#;
        let patterns = parse_and_extract(source);

        let injectables: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Injectable)
            .collect();

        assert_eq!(
            injectables.len(),
            2,
            "Expected 2 Injectable patterns, got: {injectables:?}"
        );

        let service = injectables.iter().find(|p| p.name == Some("UserService".to_string()));
        assert!(service.is_some(), "Expected Injectable for UserService");
        assert_eq!(service.unwrap().framework, "spring");

        let repo = injectables.iter().find(|p| p.name == Some("UserRepo".to_string()));
        assert!(repo.is_some(), "Expected Injectable for UserRepo");
    }

    /// Verify all five HTTP method mapping annotations produce correct kinds.
    #[test]
    fn test_all_http_method_mappings() {
        let source = r#"
class Api {
    @GetMapping("/a") public void a() {}
    @PostMapping("/b") public void b() {}
    @PutMapping("/c") public void c() {}
    @DeleteMapping("/d") public void d() {}
    @PatchMapping("/e") public void e() {}
}
"#;
        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 5, "Expected 5 Route patterns, got: {routes:?}");

        let methods: Vec<&str> = routes
            .iter()
            .filter_map(|r| r.http_method.as_deref())
            .collect();
        for m in &["GET", "POST", "PUT", "DELETE", "PATCH"] {
            assert!(methods.contains(m), "Missing HTTP method {m} in {methods:?}");
        }
    }

    /// Verify `@Component` and `@Configuration` are also extracted.
    #[test]
    fn test_component_and_configuration() {
        let source = r#"
@Component
public class AuditLogger {}

@Configuration
public class AppConfig {}
"#;
        let patterns = parse_and_extract(source);

        let injectable = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::Injectable);
        assert!(injectable.is_some(), "Expected Injectable for @Component");
        assert_eq!(injectable.unwrap().name, Some("AuditLogger".to_string()));

        let module = patterns.iter().find(|p| p.kind == FrameworkPatternKind::Module);
        assert!(module.is_some(), "Expected Module for @Configuration");
        assert_eq!(module.unwrap().name, Some("AppConfig".to_string()));
    }

    /// Verify `@RequestMapping` on a method uses the extracted HTTP method.
    #[test]
    fn test_request_mapping_on_method() {
        let source = r#"
class Ctrl {
    @RequestMapping(value = "/orders", method = RequestMethod.POST)
    public void placeOrder() {}
}
"#;
        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 1, "Expected 1 Route, got: {routes:?}");
        assert_eq!(routes[0].path, Some("/orders".to_string()));
        assert_eq!(routes[0].http_method, Some("POST".to_string()));
        assert_eq!(routes[0].handler, Some("placeOrder".to_string()));
    }

    /// Verify `@RequestMapping` without a `method` argument defaults to ANY.
    #[test]
    fn test_request_mapping_defaults_to_any() {
        let source = r#"
class Ctrl {
    @RequestMapping("/fallback")
    public void fallback() {}
}
"#;
        let patterns = parse_and_extract(source);
        let route = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::Route);
        assert!(route.is_some(), "Expected a Route pattern");
        assert_eq!(route.unwrap().http_method, Some("ANY".to_string()));
        assert_eq!(route.unwrap().path, Some("/fallback".to_string()));
    }

    /// Verify that patterns are emitted in source order (line, column).
    #[test]
    fn test_patterns_sorted_by_position() {
        let source = r#"
@RestController
public class Ctrl {
    @GetMapping("/first")
    public void first() {}

    @PostMapping("/second")
    public void second() {}
}
"#;
        let patterns = parse_and_extract(source);
        for window in patterns.windows(2) {
            let a = &window[0];
            let b = &window[1];
            assert!(
                (a.line, a.column) <= (b.line, b.column),
                "Patterns not sorted: {a:?} comes before {b:?}"
            );
        }
    }
}

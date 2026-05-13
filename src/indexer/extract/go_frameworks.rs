//! Go framework pattern extraction for Gin, Echo, and Chi.
//!
//! Extracts route registrations, middleware, and route groups from Go HTTP
//! framework method calls by walking the tree-sitter Go AST.
//!
//! ## Supported frameworks
//!
//! | Framework | Route methods | Notes |
//! |-----------|--------------|-------|
//! | Gin       | `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `OPTIONS`, `HEAD`, `Any` | uppercase HTTP verbs |
//! | Echo      | same as Gin  | identical API surface |
//! | Chi       | `Get`, `Post`, `Put`, `Delete`, `Patch`, `Options`, `Head` | TitleCase verbs |
//!
//! `Use` → [`FrameworkPatternKind::Middleware`] (all three frameworks)
//! `Group` → [`FrameworkPatternKind::Group`] (Gin, Echo)
//! `Route` / `Mount` → [`FrameworkPatternKind::Group`] (Chi)
//!
//! ## R118 guard
//!
//! Route patterns are only emitted when the first argument is an
//! `interpreted_string_literal` that starts with `/`.  This prevents false
//! positives from generic `.Get(key)` or `.Post(body)` calls that are
//! unrelated to HTTP routing.
//!
//! # Examples
//!
//! ```rust
//! use code_intelligence_mcp_server::indexer::extract::go_frameworks::extract_go_framework_patterns;
//! use code_intelligence_mcp_server::indexer::parser::{parser_for_id, LanguageId};
//!
//! let source = r#"
//! func main() {
//!     r := gin.Default()
//!     r.GET("/users", listUsers)
//! }
//! "#;
//! let mut parser = parser_for_id(LanguageId::Go).unwrap();
//! let tree = parser.parse(source, None).unwrap();
//! let patterns = extract_go_framework_patterns(tree.root_node(), source);
//! assert_eq!(patterns.len(), 1);
//! assert_eq!(patterns[0].path, Some("/users".to_string()));
//! ```

use tree_sitter::Node;

use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

// ---------------------------------------------------------------------------
// HTTP verb sets
// ---------------------------------------------------------------------------

/// Uppercase HTTP verbs used by Gin and Echo (also covers `Any` which maps to
/// all methods).
const GIN_ECHO_VERBS: &[&str] = &[
    "GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD", "Any",
];

/// TitleCase HTTP verbs used by Chi.
const CHI_VERBS: &[&str] = &["Get", "Post", "Put", "Delete", "Patch", "Options", "Head"];

// ---------------------------------------------------------------------------
// Public entry-point
// ---------------------------------------------------------------------------

/// Extract Go framework patterns (routes, middleware, groups) from a parsed
/// Go AST.
///
/// Walks every `call_expression` node recursively and matches against known
/// Gin/Echo/Chi method names.  Results are sorted by `(line, column)`.
pub fn extract_go_framework_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    walk_and_extract(root, source, &mut patterns);
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

// ---------------------------------------------------------------------------
// AST traversal
// ---------------------------------------------------------------------------

/// Recursively walk the AST, visiting every `call_expression` node.
fn walk_and_extract(node: Node, source: &str, out: &mut Vec<ExtractedFrameworkPattern>) {
    if node.kind() == "call_expression" {
        if let Some(pattern) = try_extract_go_call(node, source) {
            out.push(pattern);
        }
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
// Pattern matching
// ---------------------------------------------------------------------------

/// Attempt to extract a Go framework pattern from a `call_expression` node.
///
/// Returns `None` when the call does not match any known Go framework method.
fn try_extract_go_call(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    // The function being called must be a selector_expression: `object.Method`
    let func_node = node.child_by_field_name("function")?;
    if func_node.kind() != "selector_expression" {
        return None;
    }

    // Extract the method name from the `field` child of the selector_expression.
    let field_node = func_node.child_by_field_name("field")?;
    let method_name = text_for_node(field_node, source);

    // The arguments are in an `argument_list` node.
    let args_node = node.child_by_field_name("arguments")?;

    // Classify the method and derive the framework label.
    let (kind, http_method, framework) = classify_go_method(&method_name)?;

    // Collect named children of argument_list.
    let named_args = named_children(args_node);

    // R118 guard: HTTP routes MUST have a first argument that is an
    // `interpreted_string_literal` starting with "/".
    let path = if matches!(kind, FrameworkPatternKind::Route) {
        let first = named_args.first()?;
        if first.kind() != "interpreted_string_literal" {
            return None;
        }
        let raw = text_for_node(*first, source);
        let unquoted = raw.trim_matches('"').to_string();
        if !unquoted.starts_with('/') {
            return None;
        }
        Some(unquoted)
    } else {
        // For Group / Middleware, also try to extract an optional path.
        named_args.first().and_then(|first| {
            if first.kind() == "interpreted_string_literal" {
                let raw = text_for_node(*first, source);
                let unquoted = raw.trim_matches('"').to_string();
                if unquoted.starts_with('/') {
                    return Some(unquoted);
                }
            }
            None
        })
    };

    // Handler: for Route, the second named argument (index 1) if it is an identifier.
    let handler = if matches!(kind, FrameworkPatternKind::Route) {
        named_args.get(1).and_then(|arg| {
            if arg.kind() == "identifier" {
                Some(text_for_node(*arg, source))
            } else {
                None
            }
        })
    } else {
        None
    };

    let pos = field_node.start_position();

    Some(ExtractedFrameworkPattern {
        line: pos.row as u32 + 1,
        column: pos.column as u32,
        framework,
        kind,
        http_method,
        path,
        name: None,
        handler,
        arguments: None,
        parent_chain: None,
    })
}

// ---------------------------------------------------------------------------
// Method classification
// ---------------------------------------------------------------------------

/// Classify a Go method name into a `(kind, http_method, framework)` triple.
///
/// Returns `None` when the method name is not recognised as a Go framework
/// method.
fn classify_go_method(method: &str) -> Option<(FrameworkPatternKind, Option<String>, String)> {
    // Gin / Echo: uppercase HTTP verbs.
    if GIN_ECHO_VERBS.contains(&method) {
        let http = if method == "Any" {
            Some("ANY".to_string())
        } else {
            Some(method.to_uppercase())
        };
        return Some((FrameworkPatternKind::Route, http, "gin".to_string()));
    }

    // Chi: TitleCase HTTP verbs.
    if CHI_VERBS.contains(&method) {
        let http = Some(method.to_uppercase());
        return Some((FrameworkPatternKind::Route, http, "chi".to_string()));
    }

    match method {
        // Middleware — all three frameworks share `.Use(...)`.
        "Use" => Some((FrameworkPatternKind::Middleware, None, "gin".to_string())),

        // Groups — Gin/Echo use `.Group(...)`.
        "Group" => Some((FrameworkPatternKind::Group, None, "gin".to_string())),

        // Chi route groups and sub-router mounting.
        "Route" | "Mount" => Some((FrameworkPatternKind::Group, None, "chi".to_string())),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// AST helpers
// ---------------------------------------------------------------------------

/// Return the text of a node by slicing the source bytes.
fn text_for_node(node: Node, source: &str) -> String {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or("")
        .to_string()
}

/// Collect all *named* children of a node into a `Vec`.
fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|n| n.is_named())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parser::{parser_for_id, LanguageId};

    fn parse_and_extract(source: &str) -> Vec<ExtractedFrameworkPattern> {
        let mut parser = parser_for_id(LanguageId::Go).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_go_framework_patterns(tree.root_node(), source)
    }

    // -----------------------------------------------------------------------
    // Gin routes
    // -----------------------------------------------------------------------

    #[test]
    fn test_gin_routes() {
        let source = r#"
package main

func main() {
    r := gin.Default()
    r.GET("/users", getUsers)
    r.POST("/users", createUser)
}
"#;
        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(routes.len(), 2, "Expected 2 Gin route patterns");

        let get_route = routes
            .iter()
            .find(|p| p.http_method.as_deref() == Some("GET"));
        assert!(get_route.is_some(), "Expected a GET route");
        let get_route = get_route.unwrap();
        assert_eq!(get_route.path, Some("/users".to_string()));
        assert_eq!(get_route.handler, Some("getUsers".to_string()));
        assert_eq!(get_route.framework, "gin");

        let post_route = routes
            .iter()
            .find(|p| p.http_method.as_deref() == Some("POST"));
        assert!(post_route.is_some(), "Expected a POST route");
        let post_route = post_route.unwrap();
        assert_eq!(post_route.path, Some("/users".to_string()));
        assert_eq!(post_route.handler, Some("createUser".to_string()));
    }

    // -----------------------------------------------------------------------
    // Chi routes and groups
    // -----------------------------------------------------------------------

    #[test]
    fn test_chi_routes() {
        let source = r#"
package main

func main() {
    r := chi.NewRouter()
    r.Get("/users", getUsers)
    r.Route("/api", func(r chi.Router) {})
}
"#;
        let patterns = parse_and_extract(source);

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 1, "Expected 1 Chi Route pattern");
        assert_eq!(routes[0].http_method, Some("GET".to_string()));
        assert_eq!(routes[0].path, Some("/users".to_string()));
        assert_eq!(routes[0].framework, "chi");

        let groups: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Group)
            .collect();
        assert_eq!(groups.len(), 1, "Expected 1 Chi Group pattern");
        assert_eq!(groups[0].path, Some("/api".to_string()));
        assert_eq!(groups[0].framework, "chi");
    }

    // -----------------------------------------------------------------------
    // Middleware
    // -----------------------------------------------------------------------

    #[test]
    fn test_middleware() {
        let source = r#"
package main

func main() {
    r.Use(authMiddleware)
}
"#;
        let patterns = parse_and_extract(source);

        let middleware: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Middleware)
            .collect();
        assert_eq!(middleware.len(), 1, "Expected 1 Middleware pattern");
        assert_eq!(middleware[0].framework, "gin");
    }

    // -----------------------------------------------------------------------
    // R118 guard: generic .Get() calls must not be treated as routes
    // -----------------------------------------------------------------------

    #[test]
    fn ignores_generic_get_calls() {
        let source = r#"
package main

func main() {
    val := myMap.Get(key)
    id := params.Get("id")
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
            "Generic .Get() calls must not be treated as routes"
        );
    }

    // -----------------------------------------------------------------------
    // Additional HTTP verbs
    // -----------------------------------------------------------------------

    #[test]
    fn test_gin_all_verbs() {
        let source = r#"
package main

func main() {
    r.PUT("/users/:id", updateUser)
    r.DELETE("/users/:id", deleteUser)
    r.PATCH("/users/:id", patchUser)
    r.OPTIONS("/users", optionsHandler)
    r.HEAD("/health", healthHandler)
    r.Any("/wildcard", wildcardHandler)
}
"#;
        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(routes.len(), 6, "Expected 6 Gin route patterns");

        let methods: Vec<_> = routes
            .iter()
            .filter_map(|r| r.http_method.as_deref())
            .collect();
        assert!(methods.contains(&"PUT"));
        assert!(methods.contains(&"DELETE"));
        assert!(methods.contains(&"PATCH"));
        assert!(methods.contains(&"OPTIONS"));
        assert!(methods.contains(&"HEAD"));
        assert!(methods.contains(&"ANY"));
    }

    // -----------------------------------------------------------------------
    // Gin groups
    // -----------------------------------------------------------------------

    #[test]
    fn test_gin_group() {
        let source = r#"
package main

func main() {
    api := r.Group("/api")
    api.GET("/users", listUsers)
}
"#;
        let patterns = parse_and_extract(source);

        let groups: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Group)
            .collect();
        assert_eq!(groups.len(), 1, "Expected 1 Gin Group pattern");
        assert_eq!(groups[0].path, Some("/api".to_string()));
        assert_eq!(groups[0].framework, "gin");

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 1, "Expected 1 route inside the group");
    }

    // -----------------------------------------------------------------------
    // Chi Mount
    // -----------------------------------------------------------------------

    #[test]
    fn test_chi_mount() {
        let source = r#"
package main

func main() {
    r.Mount("/admin", adminRouter)
}
"#;
        let patterns = parse_and_extract(source);

        let groups: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Group)
            .collect();
        assert_eq!(groups.len(), 1, "Expected 1 Chi Mount pattern as Group");
        assert_eq!(groups[0].path, Some("/admin".to_string()));
        assert_eq!(groups[0].framework, "chi");
    }

    // -----------------------------------------------------------------------
    // Line numbers are 1-indexed
    // -----------------------------------------------------------------------

    #[test]
    fn test_line_numbers_are_one_indexed() {
        let source = "package main\n\nfunc main() {\n    r.GET(\"/users\", h)\n}\n";
        let patterns = parse_and_extract(source);

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();

        assert_eq!(routes.len(), 1);
        // Line 4 in the source (1-indexed): "    r.GET("/users", h)"
        assert_eq!(routes[0].line, 4, "Line number should be 1-indexed");
    }
}

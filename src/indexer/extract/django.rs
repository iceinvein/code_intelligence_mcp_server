//! Django framework pattern extraction
//!
//! Extracts URL routes, ViewSets, APIViews, and function-based views from Django
//! and Django REST Framework source files (Python AST via tree-sitter-python).
//!
//! ## Patterns detected
//!
//! **Pattern 1 — `urlpatterns` list**
//! ```python
//! urlpatterns = [
//!     path('api/users/', views.user_list),          # → Route
//!     path('api/', include('app.urls')),             # → Group
//!     re_path(r'^api/.*$', handler),                # → Route
//! ]
//! ```
//!
//! **Pattern 2 — DRF ViewSet / APIView class**
//! ```python
//! class UserViewSet(ModelViewSet):    # → Controller
//!     @action(detail=True)            # → Route (each @action method)
//!     def activate(self, request, pk=None): ...
//! ```
//!
//! **Pattern 3 — `@api_view` decorated function**
//! ```python
//! @api_view(['GET', 'POST'])
//! def user_list(request): ...        # → Route
//! ```

use tree_sitter::Node;

use super::framework_utils::{extract_string_value, text_for_node};
use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Extract Django framework patterns from a Python AST.
///
/// Walks the entire syntax tree and collects URL patterns from `urlpatterns`
/// assignments, DRF `ViewSet`/`APIView` class definitions, and `@api_view`
/// decorated functions.
///
/// # Arguments
///
/// * `root` – the root [`Node`] of a tree-sitter parse tree for a Python file.
/// * `source` – the original UTF-8 source text used to build the tree.
///
/// # Examples
///
/// ```ignore
/// use crate::indexer::parser::{parser_for_id, LanguageId};
/// use crate::indexer::extract::django::extract_django_patterns;
///
/// let mut parser = parser_for_id(LanguageId::Python).unwrap();
/// let tree = parser.parse(source, None).unwrap();
/// let patterns = extract_django_patterns(tree.root_node(), source);
/// ```
pub fn extract_django_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    walk_for_django(root, source, &mut patterns);
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

// ---------------------------------------------------------------------------
// Top-level walker
// ---------------------------------------------------------------------------

/// Recursive pre-order walk of the Python AST.
///
/// Dispatches on node kinds that are relevant to Django pattern detection and
/// recurses into their children when appropriate.  Nodes that can never contain
/// Django patterns are skipped transparently.
fn walk_for_django(node: Node, source: &str, patterns: &mut Vec<ExtractedFrameworkPattern>) {
    match node.kind() {
        // urlpatterns = [...]
        "expression_statement" => {
            try_extract_urlpatterns(node, source, patterns);
        }

        // class Foo(ModelViewSet): ...
        "class_definition" => {
            try_extract_class_pattern(node, source, patterns);
            // Do NOT recurse further: class body methods are handled inside
            // `try_extract_class_pattern` when the class is a ViewSet/APIView.
            return;
        }

        // @api_view(['GET'])
        // def user_list(request): ...
        "decorated_definition" => {
            try_extract_api_view_decorator(node, source, patterns);
            return;
        }

        _ => {}
    }

    recurse_children(node, source, patterns);
}

/// Recurse into all children of `node`.
fn recurse_children(node: Node, source: &str, patterns: &mut Vec<ExtractedFrameworkPattern>) {
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_for_django(cursor.node(), source, patterns);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pattern 1: urlpatterns assignment
// ---------------------------------------------------------------------------

/// Try to extract URL patterns from an `expression_statement` that assigns to
/// `urlpatterns`.
///
/// Expected AST shape:
/// ```text
/// (expression_statement
///   (assignment
///     left: (identifier)   ← must be "urlpatterns"
///     right: (list
///       (call ...)          ← each path() / re_path() call
///       ...)))
/// ```
fn try_extract_urlpatterns(
    expr_stmt: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    // The expression_statement wraps an assignment node.
    let assignment = match find_named_child_of_kind(expr_stmt, "assignment") {
        Some(n) => n,
        None => return,
    };

    // Left side must be the identifier `urlpatterns`.
    let left = match assignment.child_by_field_name("left") {
        Some(n) if n.kind() == "identifier" => n,
        _ => return,
    };
    if text_for_node(left, source) != "urlpatterns" {
        return;
    }

    // Right side must be a list literal.
    let right = match assignment.child_by_field_name("right") {
        Some(n) if n.kind() == "list" => n,
        _ => return,
    };

    // Walk every element in the list.
    let mut cursor = right.walk();
    for element in right.children(&mut cursor) {
        if !element.is_named() {
            continue;
        }
        if let Some(p) = try_extract_path_call(element, source) {
            patterns.push(p);
        }
    }
}

/// Try to build a framework pattern from a single `path()` or `re_path()` call
/// inside a `urlpatterns` list.
///
/// Accepts either:
/// - A `call` node directly (the path/re_path call itself).
/// - Silently returns `None` for anything else.
fn try_extract_path_call(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    if node.kind() != "call" {
        return None;
    }

    // The function identifier: `path`, `re_path`, or attribute like `urls.path`.
    let func_node = node.child_by_field_name("function")?;
    let func_name = leaf_name(func_node, source);
    if func_name != "path" && func_name != "re_path" {
        return None;
    }

    let args_node = node.child_by_field_name("arguments")?;
    let named_args = named_children(args_node);

    // First argument must be a string literal (the URL path).
    let first_arg = named_args.first()?;
    let raw_path = extract_string_literal(*first_arg, source)?;
    let url_path = normalize_django_path(&raw_path);

    let pos = node.start_position();
    let line = pos.row as u32 + 1;
    let column = pos.column as u32;

    // Second argument determines kind: include(...) → Group, otherwise Route.
    let (kind, handler) = if let Some(second_arg) = named_args.get(1) {
        if is_include_call(*second_arg, source) {
            (FrameworkPatternKind::Group, None)
        } else {
            let h = extract_django_handler_name(*second_arg, source);
            (FrameworkPatternKind::Route, h)
        }
    } else {
        (FrameworkPatternKind::Route, None)
    };

    Some(ExtractedFrameworkPattern {
        line,
        column,
        framework: "django".to_string(),
        kind,
        http_method: None,
        path: Some(url_path),
        name: None,
        handler,
        arguments: None,
        parent_chain: None,
    })
}

/// Return `true` when `node` is a `call` whose function name is `include`.
fn is_include_call(node: Node, source: &str) -> bool {
    if node.kind() != "call" {
        return false;
    }
    if let Some(func) = node.child_by_field_name("function") {
        return leaf_name(func, source) == "include";
    }
    false
}

/// Extract the handler name from a Django URL pattern's second argument.
///
/// Handles:
/// - `views.user_list`  → `"user_list"` (attribute access)
/// - `UserListView.as_view()` → `"UserListView"` (call on attribute)
/// - `user_list` → `"user_list"` (bare identifier)
fn extract_django_handler_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        // `views.user_list` — attribute access, take the attribute part.
        "attribute" => node
            .child_by_field_name("attribute")
            .map(|n| text_for_node(n, source)),

        // `UserListView.as_view()` — call expression.
        "call" => {
            let func = node.child_by_field_name("function")?;
            match func.kind() {
                // `as_view()` called on `UserListView` or `UserListView.as_view`.
                "attribute" => func
                    .child_by_field_name("object")
                    .map(|obj| leaf_name(obj, source)),
                "identifier" => Some(text_for_node(func, source)),
                _ => None,
            }
        }

        // Plain identifier: `user_list`.
        "identifier" => Some(text_for_node(node, source)),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Pattern 2: ViewSet / APIView class definitions
// ---------------------------------------------------------------------------

/// Try to extract a Controller pattern from a `class_definition` node, plus any
/// `@action`-decorated methods inside it.
///
/// A class qualifies when at least one of its base classes (the `superclasses`
/// / `argument_list` field) ends with `"ViewSet"` or equals `"APIView"`.
fn try_extract_class_pattern(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    // `superclasses` is the argument_list "(ModelViewSet, ...)" after the class name.
    let superclasses = match node.child_by_field_name("superclasses") {
        Some(n) => n,
        None => {
            // No base classes — still recurse the body for nested classes.
            recurse_class_body(node, source, patterns);
            return;
        }
    };

    let is_drf_class = named_children(superclasses).iter().any(|&base| {
        let name = leaf_name(base, source);
        name.ends_with("ViewSet") || name.ends_with("APIView")
    });

    if is_drf_class {
        let pos = node.start_position();
        patterns.push(ExtractedFrameworkPattern {
            line: pos.row as u32 + 1,
            column: pos.column as u32,
            framework: "django".to_string(),
            kind: FrameworkPatternKind::Controller,
            http_method: None,
            path: None,
            name: node
                .child_by_field_name("name")
                .map(|n| text_for_node(n, source)),
            handler: None,
            arguments: None,
            parent_chain: None,
        });

        // Extract @action-decorated methods from the class body.
        if let Some(body) = node.child_by_field_name("body") {
            extract_action_routes(body, source, patterns);
        }
    } else {
        // Not a DRF class, but there may be nested classes or decorated defs.
        recurse_class_body(node, source, patterns);
    }
}

/// Walk a class body and emit a `Route` pattern for each `@action`-decorated
/// method found inside it.
///
/// Tree-sitter Python represents a decorated method as a `decorated_definition`
/// node inside the class body `block`.
fn extract_action_routes(body: Node, source: &str, patterns: &mut Vec<ExtractedFrameworkPattern>) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() == "decorated_definition"
            && has_action_decorator(child, source)
        {
            // The last child of a `decorated_definition` is the actual
            // `function_definition`.
            if let Some(fn_def) = find_named_child_of_kind(child, "function_definition") {
                let fn_name = fn_def
                    .child_by_field_name("name")
                    .map(|n| text_for_node(n, source));

                let pos = child.start_position();
                patterns.push(ExtractedFrameworkPattern {
                    line: pos.row as u32 + 1,
                    column: pos.column as u32,
                    framework: "django".to_string(),
                    kind: FrameworkPatternKind::Route,
                    http_method: None,
                    path: None,
                    name: fn_name.clone(),
                    handler: fn_name,
                    arguments: None,
                    parent_chain: None,
                });
            }
        }
    }
}

/// Return `true` when a `decorated_definition` node has at least one `@action`
/// decorator applied to it.
///
/// In tree-sitter Python, a `decorated_definition` looks like:
/// ```text
/// (decorated_definition
///   (decorator (call function: (identifier "action") ...))
///   (function_definition ...))
/// ```
fn has_action_decorator(decorated_def: Node, source: &str) -> bool {
    let mut cursor = decorated_def.walk();
    for child in decorated_def.children(&mut cursor) {
        if child.kind() == "decorator" {
            // The decorator body is the first named child that is not "@".
            let mut dc = child.walk();
            for dec_child in child.children(&mut dc) {
                if !dec_child.is_named() {
                    continue;
                }
                let decorator_name = match dec_child.kind() {
                    "identifier" => text_for_node(dec_child, source),
                    "call" => {
                        if let Some(func) = dec_child.child_by_field_name("function") {
                            text_for_node(func, source)
                        } else {
                            continue;
                        }
                    }
                    _ => continue,
                };
                if decorator_name == "action" {
                    return true;
                }
            }
        }
    }
    false
}

/// Recurse into the body block of a class that does NOT qualify as a DRF view.
/// Allows detecting nested classes and decorated definitions within arbitrary
/// class bodies.
fn recurse_class_body(node: Node, source: &str, patterns: &mut Vec<ExtractedFrameworkPattern>) {
    if let Some(body) = node.child_by_field_name("body") {
        recurse_children(body, source, patterns);
    }
}

// ---------------------------------------------------------------------------
// Pattern 3: @api_view decorated functions
// ---------------------------------------------------------------------------

/// Try to extract a `Route` pattern from a `decorated_definition` that carries
/// an `@api_view(...)` decorator.
///
/// Expected AST shape:
/// ```text
/// (decorated_definition
///   (decorator
///     (call
///       function: (identifier "api_view")
///       arguments: (argument_list (list ...))))
///   (function_definition name: (identifier) ...))
/// ```
fn try_extract_api_view_decorator(
    decorated_def: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    let http_methods = extract_api_view_methods(decorated_def, source);
    if http_methods.is_none() {
        // Not an @api_view decorator — still recurse into children so nested
        // decorated definitions are processed.
        recurse_children(decorated_def, source, patterns);
        return;
    }

    // The function definition is the last named child of decorated_definition.
    let fn_def = match find_named_child_of_kind(decorated_def, "function_definition") {
        Some(n) => n,
        None => return,
    };

    let fn_name = fn_def
        .child_by_field_name("name")
        .map(|n| text_for_node(n, source));

    let pos = decorated_def.start_position();
    patterns.push(ExtractedFrameworkPattern {
        line: pos.row as u32 + 1,
        column: pos.column as u32,
        framework: "django".to_string(),
        kind: FrameworkPatternKind::Route,
        // Django's @api_view routes are method-agnostic at the URL level;
        // the allowed methods are enforced inside the view.  We record the
        // first listed method as the canonical http_method for indexing.
        http_method: http_methods,
        path: None,
        name: fn_name.clone(),
        handler: fn_name,
        arguments: None,
        parent_chain: None,
    });
}

/// Scan the decorators on a `decorated_definition` for an `@api_view` call and
/// return the first HTTP method listed in its argument array, or `None` if no
/// `@api_view` decorator is present.
///
/// Example: `@api_view(['GET', 'POST'])` → `Some("GET")`
fn extract_api_view_methods(decorated_def: Node, source: &str) -> Option<String> {
    let mut cursor = decorated_def.walk();
    for child in decorated_def.children(&mut cursor) {
        if child.kind() != "decorator" {
            continue;
        }

        // The decorator body is the first named non-"@" child.
        let mut dc = child.walk();
        for dec_child in child.children(&mut dc) {
            if !dec_child.is_named() {
                continue;
            }

            if dec_child.kind() != "call" {
                continue;
            }

            let func = match dec_child.child_by_field_name("function") {
                Some(n) => n,
                None => continue,
            };
            if text_for_node(func, source) != "api_view" {
                continue;
            }

            // Found @api_view(...). Extract the first string from the list argument.
            let args = match dec_child.child_by_field_name("arguments") {
                Some(n) => n,
                None => return Some(String::new()),
            };

            // The arguments node contains the list: `(['GET', 'POST'])`.
            // Walk into the list to find the first string literal.
            if let Some(method) = first_string_in_subtree(args, source) {
                return Some(method);
            }
            return Some(String::new());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Helper utilities
// ---------------------------------------------------------------------------

/// Return all named children of `node` as a `Vec<Node>`.
fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|n| n.is_named())
        .collect()
}

/// Find the first direct child of `node` with the given `kind`.
fn find_named_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let children: Vec<Node<'a>> = node.children(&mut cursor).collect();
    drop(cursor);
    children.into_iter().find(|n| n.kind() == kind)
}

/// Return the "leaf" name of a node: for an `identifier` the text itself; for
/// an `attribute` like `views.user_list` the rightmost part (`"user_list"`);
/// for anything else fall back to the full text.
fn leaf_name(node: Node, source: &str) -> String {
    match node.kind() {
        "identifier" => text_for_node(node, source),
        "attribute" => node
            .child_by_field_name("attribute")
            .map(|n| text_for_node(n, source))
            .unwrap_or_else(|| text_for_node(node, source)),
        _ => text_for_node(node, source),
    }
}

/// Try to extract an unquoted string value from a node that is a Python string
/// literal (`string` in tree-sitter-python grammar).
///
/// In tree-sitter-python v0.25 a string literal is:
/// ```text
/// (string (string_start "'" | "\"") (string_content "...") (string_end))
/// ```
/// or the legacy simple `string` node where the full text includes the quotes.
///
/// We handle both by using `extract_string_value` (strips outer quote chars)
/// and also by walking into `string_content` children for the newer grammar.
fn extract_string_literal(node: Node, source: &str) -> Option<String> {
    if node.kind() != "string" && node.kind() != "concatenated_string" {
        return None;
    }

    // Try to find a `string_content` child (tree-sitter-python v0.25 grammar).
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            let content = text_for_node(child, source);
            if !content.is_empty() {
                return Some(content);
            }
        }
    }

    // Fallback: strip outer quote characters from the raw token.
    let raw = text_for_node(node, source);
    // Strip leading r/b/f prefixes (raw strings, byte strings, f-strings).
    let stripped = raw.trim_start_matches(['r', 'b', 'f', 'R', 'B', 'F']);
    let value = extract_string_value(node, source);
    // If nothing was stripped or the result equals the raw text, it means
    // `extract_string_value` already handled it correctly.
    if value != raw && value != stripped {
        return Some(value);
    }
    // Last resort: remove the outermost pair of quote characters manually.
    let content = stripped
        .trim_matches('"')
        .trim_matches('\'');
    if content.is_empty() && !stripped.is_empty() {
        // The quotes were the whole token — return empty path.
        Some(String::new())
    } else {
        Some(content.to_string())
    }
}

/// Walk a subtree depth-first and return the content of the first `string`
/// node found (unquoted), or `None` if there is no string in the subtree.
fn first_string_in_subtree(node: Node, source: &str) -> Option<String> {
    if node.kind() == "string" {
        return extract_string_literal(node, source);
    }
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            if let Some(s) = first_string_in_subtree(cursor.node(), source) {
                return Some(s);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    None
}

/// Normalize a raw URL path string extracted from `path(...)`:
/// - Prepend `"/"` if the path does not already start with one.
/// - Leave the path otherwise unchanged (do not strip trailing slashes — Django
///   uses trailing slashes by convention).
fn normalize_django_path(raw: &str) -> String {
    if raw.starts_with('/') {
        raw.to_string()
    } else {
        format!("/{raw}")
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
        let mut parser = parser_for_id(LanguageId::Python).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_django_patterns(tree.root_node(), source)
    }

    // -----------------------------------------------------------------------
    // Pattern 1: urlpatterns
    // -----------------------------------------------------------------------

    #[test]
    fn test_urlpatterns() {
        let source = r#"
from django.urls import path, include
from . import views

urlpatterns = [
    path('api/users/', views.user_list),
    path('api/', include('app.urls')),
]
"#;
        let patterns = parse_and_extract(source);

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        let groups: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Group)
            .collect();

        assert_eq!(routes.len(), 1, "Expected 1 Route, got: {routes:?}");
        assert_eq!(groups.len(), 1, "Expected 1 Group, got: {groups:?}");

        let route = routes[0];
        assert_eq!(route.framework, "django");
        assert_eq!(route.path, Some("/api/users/".to_string()));
        assert_eq!(route.handler, Some("user_list".to_string()));

        let group = groups[0];
        assert_eq!(group.path, Some("/api/".to_string()));
    }

    #[test]
    fn test_urlpatterns_path_normalization() {
        // Django paths without leading slash must get one prepended.
        let source = r#"
urlpatterns = [
    path('users/', views.user_list),
]
"#;
        let patterns = parse_and_extract(source);
        let route = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::Route)
            .expect("Expected a route pattern");
        assert!(
            route.path.as_deref().unwrap_or("").starts_with('/'),
            "Path must start with '/', got: {:?}",
            route.path
        );
    }

    #[test]
    fn test_urlpatterns_re_path() {
        let source = r#"
from django.urls import re_path
from . import views

urlpatterns = [
    re_path(r'^api/items/$', views.item_list),
]
"#;
        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 1, "Expected 1 Route from re_path");
        assert_eq!(routes[0].framework, "django");
    }

    #[test]
    fn test_urlpatterns_already_has_slash() {
        let source = r#"
urlpatterns = [
    path('/api/items/', views.item_list),
]
"#;
        let patterns = parse_and_extract(source);
        let route = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::Route)
            .expect("Expected a route");
        // Should not double the slash.
        assert_eq!(
            route.path.as_deref().unwrap_or(""),
            "/api/items/",
            "Path with leading slash must not be doubled"
        );
    }

    // -----------------------------------------------------------------------
    // Pattern 2: ViewSet / APIView class detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_viewset_detection() {
        let source = r#"
from rest_framework.viewsets import ModelViewSet

class UserViewSet(ModelViewSet):
    pass
"#;
        let patterns = parse_and_extract(source);

        let controllers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Controller)
            .collect();

        assert_eq!(
            controllers.len(),
            1,
            "Expected 1 Controller pattern, got: {controllers:?}"
        );
        assert_eq!(controllers[0].framework, "django");
        assert_eq!(
            controllers[0].name,
            Some("UserViewSet".to_string()),
            "Expected class name in pattern name"
        );
    }

    #[test]
    fn test_apiview_detection() {
        let source = r#"
from rest_framework.views import APIView

class UserAPIView(APIView):
    pass
"#;
        let patterns = parse_and_extract(source);
        let controllers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Controller)
            .collect();
        assert_eq!(controllers.len(), 1, "Expected 1 Controller for APIView subclass");
        assert_eq!(controllers[0].name, Some("UserAPIView".to_string()));
    }

    #[test]
    fn test_viewset_with_action_decorator() {
        let source = r#"
from rest_framework.viewsets import ViewSet
from rest_framework.decorators import action

class ArticleViewSet(ViewSet):
    @action(detail=True, methods=['post'])
    def publish(self, request, pk=None):
        pass

    def list(self, request):
        pass
"#;
        let patterns = parse_and_extract(source);

        let controllers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Controller)
            .collect();
        assert_eq!(controllers.len(), 1, "Expected 1 Controller");

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 1, "Expected 1 Route from @action decorator");
        assert_eq!(routes[0].handler, Some("publish".to_string()));
    }

    #[test]
    fn test_non_drf_class_ignored() {
        let source = r#"
class RegularModel(models.Model):
    name = models.CharField(max_length=100)
"#;
        let patterns = parse_and_extract(source);
        let controllers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Controller)
            .collect();
        assert!(
            controllers.is_empty(),
            "Non-DRF class must not produce a Controller pattern"
        );
    }

    // -----------------------------------------------------------------------
    // Pattern 3: @api_view decorator
    // -----------------------------------------------------------------------

    #[test]
    fn test_api_view_decorator() {
        let source = r#"
from rest_framework.decorators import api_view

@api_view(['GET'])
def user_list(request):
    pass
"#;
        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 1, "Expected 1 Route from @api_view");
        let route = routes[0];
        assert_eq!(route.framework, "django");
        assert_eq!(route.handler, Some("user_list".to_string()));
        assert_eq!(route.http_method, Some("GET".to_string()));
    }

    #[test]
    fn test_api_view_multi_method() {
        let source = r#"
@api_view(['POST', 'PUT'])
def create_or_update(request):
    pass
"#;
        let patterns = parse_and_extract(source);
        let route = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::Route)
            .expect("Expected a route");
        // First method in the list should be recorded.
        assert_eq!(route.http_method, Some("POST".to_string()));
        assert_eq!(route.handler, Some("create_or_update".to_string()));
    }

    // -----------------------------------------------------------------------
    // Combined: urlpatterns + ViewSet in the same file
    // -----------------------------------------------------------------------

    #[test]
    fn test_combined_patterns() {
        let source = r#"
from django.urls import path, include
from rest_framework.viewsets import ModelViewSet
from . import views

class UserViewSet(ModelViewSet):
    pass

urlpatterns = [
    path('api/users/', views.user_list),
    path('api/', include('app.urls')),
]
"#;
        let patterns = parse_and_extract(source);

        assert!(
            patterns.iter().any(|p| p.kind == FrameworkPatternKind::Controller),
            "Expected Controller pattern"
        );
        assert!(
            patterns.iter().any(|p| p.kind == FrameworkPatternKind::Route),
            "Expected Route pattern"
        );
        assert!(
            patterns.iter().any(|p| p.kind == FrameworkPatternKind::Group),
            "Expected Group pattern"
        );
    }

    // -----------------------------------------------------------------------
    // Line number validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_line_numbers_are_1_indexed() {
        let source = "urlpatterns = [path('/', views.home)]\n";
        let patterns = parse_and_extract(source);
        let route = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::Route)
            .expect("Expected a route");
        // The call to path() is on line 1.
        assert_eq!(route.line, 1, "Line numbers must be 1-indexed");
    }
}

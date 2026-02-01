//! Elysia framework pattern extraction
//!
//! Extracts routes, macros, plugins, state, and other patterns from Elysia fluent API chains.

use tree_sitter::Node;

use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

/// HTTP methods recognized as Elysia routes
const ROUTE_METHODS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "options", "head", "all",
];

/// Extract Elysia framework patterns from a TypeScript AST
pub fn extract_elysia_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    extract_patterns_recursive(root, source, &mut patterns);
    // Sort by (line, column) to ensure consistent ordering regardless of AST traversal order
    // This is important for chained method calls on the same line
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

fn extract_patterns_recursive(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    // Check if this is a call expression that might be an Elysia method
    if node.kind() == "call_expression" {
        if let Some(pattern) = try_extract_elysia_call(node, source) {
            patterns.push(pattern);
        }
    }

    // Recurse into children
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            extract_patterns_recursive(cursor.node(), source, patterns);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Try to extract an Elysia pattern from a call expression
fn try_extract_elysia_call(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    // call_expression has: function (member_expression or identifier) and arguments
    let func_node = node.child_by_field_name("function")?;

    // We want member_expression like: app.get, elysia.post, etc.
    if func_node.kind() != "member_expression" {
        return None;
    }

    let property = func_node.child_by_field_name("property")?;
    let method_name = text_for_node(property, source);

    // Check what kind of Elysia method this is
    let (kind, http_method) = classify_elysia_method(&method_name)?;

    let args_node = node.child_by_field_name("arguments")?;
    // Use the property's position (the method name like .get/.post) for ordering
    // This gives unique positions even for chained calls
    let pos = property.start_position();
    let line = pos.row as u32 + 1;
    let column = pos.column as u32;

    // Extract pattern details based on kind
    let (path, name, handler, arguments) = extract_pattern_details(kind.clone(), args_node, source);

    // Try to find the chain root (variable name like 'app' or 'elysia')
    let parent_chain = find_chain_root(func_node, source);

    Some(ExtractedFrameworkPattern {
        line,
        column,
        framework: "elysia".to_string(),
        kind,
        http_method,
        path,
        name,
        handler,
        arguments,
        parent_chain,
    })
}

/// Classify an Elysia method name into pattern kind
fn classify_elysia_method(method: &str) -> Option<(FrameworkPatternKind, Option<String>)> {
    let lower = method.to_lowercase();

    // HTTP route methods
    if ROUTE_METHODS.contains(&lower.as_str()) {
        return Some((FrameworkPatternKind::Route, Some(lower.to_uppercase())));
    }

    // Other Elysia methods
    match lower.as_str() {
        "ws" => Some((FrameworkPatternKind::WebSocket, None)),
        "macro" => Some((FrameworkPatternKind::Macro, None)),
        "use" => Some((FrameworkPatternKind::Plugin, None)),
        "state" => Some((FrameworkPatternKind::State, None)),
        "decorate" => Some((FrameworkPatternKind::Decorate, None)),
        "derive" => Some((FrameworkPatternKind::Derive, None)),
        "resolve" => Some((FrameworkPatternKind::Resolve, None)),
        "guard" => Some((FrameworkPatternKind::Guard, None)),
        "group" => Some((FrameworkPatternKind::Group, None)),
        "listen" => Some((FrameworkPatternKind::Listen, None)),
        _ => None,
    }
}

/// Extract pattern-specific details from arguments
fn extract_pattern_details(
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

    match kind {
        FrameworkPatternKind::Route | FrameworkPatternKind::WebSocket | FrameworkPatternKind::Group => {
            // First arg is path (string), second is handler
            if let Some(first) = children.iter().find(|n| n.is_named()) {
                if first.kind() == "string" || first.kind() == "template_string" {
                    path = Some(extract_string_value(*first, source));
                }
            }
            // Find second named child for handler
            let named: Vec<_> = children.iter().filter(|n| n.is_named()).collect();
            if let Some(second) = named.get(1) {
                handler = extract_handler_name(**second, source);
            }
        }
        FrameworkPatternKind::State | FrameworkPatternKind::Decorate => {
            // First arg is key name (string), second is value
            if let Some(first) = children.iter().find(|n| n.is_named()) {
                if first.kind() == "string" || first.kind() == "template_string" {
                    name = Some(extract_string_value(*first, source));
                }
            }
        }
        FrameworkPatternKind::Macro => {
            // Argument is an object with macro definitions
            if let Some(first) = children.iter().find(|n| n.is_named()) {
                if first.kind() == "object" {
                    name = extract_object_keys(*first, source);
                    arguments = Some(text_for_node(*first, source));
                }
            }
        }
        FrameworkPatternKind::Plugin => {
            // Argument is the plugin (identifier or call)
            if let Some(first) = children.iter().find(|n| n.is_named()) {
                name = extract_plugin_name(*first, source);
            }
        }
        FrameworkPatternKind::Listen => {
            // First arg is port
            if let Some(first) = children.iter().find(|n| n.is_named()) {
                name = Some(text_for_node(*first, source));
            }
        }
        FrameworkPatternKind::Derive | FrameworkPatternKind::Resolve | FrameworkPatternKind::Guard => {
            // These take function/object arguments
            if let Some(first) = children.iter().find(|n| n.is_named()) {
                arguments = Some(truncate_text(&text_for_node(*first, source), 200));
            }
        }
    }

    (path, name, handler, arguments)
}

/// Extract string content without quotes
fn extract_string_value(node: Node, source: &str) -> String {
    let text = text_for_node(node, source);
    text.trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string()
}

/// Try to extract handler function name
fn extract_handler_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_for_node(node, source)),
        "arrow_function" | "function_expression" => Some("<anonymous>".to_string()),
        "call_expression" => {
            // Could be a wrapper like `handler(fn)`
            if let Some(func) = node.child_by_field_name("function") {
                return Some(text_for_node(func, source));
            }
            None
        }
        _ => None,
    }
}

/// Extract object keys as comma-separated string
fn extract_object_keys(node: Node, source: &str) -> Option<String> {
    let mut keys = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "pair" {
            if let Some(key_node) = child.child_by_field_name("key") {
                keys.push(text_for_node(key_node, source));
            }
        }
        // Shorthand property
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

/// Extract plugin name from various node types
fn extract_plugin_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_for_node(node, source)),
        "call_expression" => {
            // plugin() call - get the function name
            if let Some(func) = node.child_by_field_name("function") {
                return Some(text_for_node(func, source));
            }
            None
        }
        _ => None,
    }
}

/// Find the root variable of a method chain
fn find_chain_root(member_expr: Node, source: &str) -> Option<String> {
    let object = member_expr.child_by_field_name("object")?;

    match object.kind() {
        "identifier" => Some(text_for_node(object, source)),
        "call_expression" => {
            // Keep traversing up the chain
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
        "new_expression" => {
            // new Elysia()
            object
                .child_by_field_name("constructor")
                .map(|constructor| text_for_node(constructor, source))
        }
        _ => None,
    }
}

fn text_for_node(node: Node, source: &str) -> String {
    source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or("")
        .to_string()
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...", &text[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parser::{parser_for_id, LanguageId};

    fn parse_and_extract(source: &str) -> Vec<ExtractedFrameworkPattern> {
        let mut parser = parser_for_id(LanguageId::Typescript).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_elysia_patterns(tree.root_node(), source)
    }

    #[test]
    fn extracts_basic_routes() {
        let source = r#"
const app = new Elysia()
    .get('/users', () => users)
    .post('/users', createUser)
    .delete('/users/:id', deleteUser)
"#;
        let patterns = parse_and_extract(source);

        assert_eq!(patterns.len(), 3);

        assert_eq!(patterns[0].kind, FrameworkPatternKind::Route);
        assert_eq!(patterns[0].http_method, Some("GET".to_string()));
        assert_eq!(patterns[0].path, Some("/users".to_string()));

        assert_eq!(patterns[1].http_method, Some("POST".to_string()));
        assert_eq!(patterns[2].http_method, Some("DELETE".to_string()));
        assert_eq!(patterns[2].path, Some("/users/:id".to_string()));
    }

    #[test]
    fn extracts_plugins() {
        let source = r#"
const app = new Elysia()
    .use(swagger())
    .use(cors)
"#;
        let patterns = parse_and_extract(source);

        assert_eq!(patterns.len(), 2);
        assert_eq!(patterns[0].kind, FrameworkPatternKind::Plugin);
        assert_eq!(patterns[0].name, Some("swagger".to_string()));
        assert_eq!(patterns[1].name, Some("cors".to_string()));
    }

    #[test]
    fn extracts_state_and_decorate() {
        let source = r#"
const app = new Elysia()
    .state('counter', 0)
    .decorate('db', database)
"#;
        let patterns = parse_and_extract(source);

        assert_eq!(patterns.len(), 2);
        assert_eq!(patterns[0].kind, FrameworkPatternKind::State);
        assert_eq!(patterns[0].name, Some("counter".to_string()));
        assert_eq!(patterns[1].kind, FrameworkPatternKind::Decorate);
        assert_eq!(patterns[1].name, Some("db".to_string()));
    }

    #[test]
    fn extracts_macros() {
        let source = r#"
const app = new Elysia()
    .macro({
        auth: (enabled) => ({
            beforeHandle: authMiddleware
        })
    })
"#;
        let patterns = parse_and_extract(source);

        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].kind, FrameworkPatternKind::Macro);
        assert_eq!(patterns[0].name, Some("auth".to_string()));
    }

    #[test]
    fn extracts_websocket() {
        let source = r#"
const app = new Elysia()
    .ws('/chat', { message: handleMessage })
"#;
        let patterns = parse_and_extract(source);

        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].kind, FrameworkPatternKind::WebSocket);
        assert_eq!(patterns[0].path, Some("/chat".to_string()));
    }

    #[test]
    fn extracts_listen() {
        let source = r#"
const app = new Elysia()
    .get('/', () => 'hi')
    .listen(3000)
"#;
        let patterns = parse_and_extract(source);

        assert!(patterns.iter().any(|p| p.kind == FrameworkPatternKind::Listen));
    }
}

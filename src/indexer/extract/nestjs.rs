//! NestJS framework pattern extraction
//!
//! Extracts controller, route, injectable, module, guard, interceptor, and pipe
//! patterns from NestJS TypeScript decorator syntax.
//!
//! NestJS uses TypeScript decorators to describe its structure:
//! - `@Controller('/path')` on a class → `Controller` kind
//! - `@Get('/sub')`, `@Post(...)`, etc. on methods → `Route` kind
//! - `@Injectable()` on a class → `Injectable` kind
//! - `@Module({...})` on a class → `Module` kind
//! - `@UseGuards(AuthGuard)` on a class or method → `Guard` kind
//! - `@UseInterceptors(LoggingInterceptor)` → `Interceptor` kind
//! - `@UsePipes(ValidationPipe)` → `Pipe` kind
//!
//! In the tree-sitter TypeScript grammar, decorators appear as `decorator`
//! nodes that are:
//! - direct children of `export_statement` (class-level, field "decorator")
//! - direct children of `class_body` (method-level, siblings to method_definition)
//!
//! Method-level decorators are NOT children of `method_definition`; they sit
//! as siblings immediately before the `method_definition` inside `class_body`.

use tree_sitter::Node;

use super::framework_utils::{
    extract_object_keys, extract_string_value, text_for_node, truncate_text,
};
use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

/// HTTP method decorators that NestJS maps to route definitions.
const NESTJS_ROUTE_DECORATORS: &[(&str, &str)] = &[
    ("Get", "GET"),
    ("Post", "POST"),
    ("Put", "PUT"),
    ("Delete", "DELETE"),
    ("Patch", "PATCH"),
    ("Options", "OPTIONS"),
    ("Head", "HEAD"),
    ("All", "ALL"),
];

/// Extract NestJS framework patterns from a TypeScript AST.
///
/// Walks the entire AST looking for `decorator` nodes attached to classes and
/// methods, then classifies them into NestJS-specific pattern kinds.
pub fn extract_nestjs_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    collect_nestjs_patterns(root, source, &mut patterns);
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

/// Recursively walk every node, collecting all `decorator` nodes that appear
/// at class scope or inside `class_body` nodes.
fn collect_nestjs_patterns(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    match node.kind() {
        // Class-level decorators are direct children of `export_statement`
        // (the export_statement acts as the container when `export class ...`).
        "export_statement" => {
            collect_export_statement_decorators(node, source, patterns);
            // Don't recurse further into export_statement — we handle everything here.
            return;
        }
        // Bare `class Foo {}` not wrapped in an export_statement.
        "class_declaration" => {
            collect_class_declaration_decorators(node, source, patterns);
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            collect_nestjs_patterns(cursor.node(), source, patterns);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Handle an `export_statement` node.
///
/// In tree-sitter's TypeScript grammar, `export class Foo {}` is represented as:
/// ```text
/// (export_statement
///   (decorator "@Controller('/users')")   ← class-level decorator(s), field="decorator"
///   "export"
///   (class_declaration
///     (class_body
///       (decorator "@Get()")              ← method-level decorator(s)
///       (method_definition ...)
///       ...)))
/// ```
fn collect_export_statement_decorators(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "decorator" {
            if let Some(p) = parse_decorator(child, source) {
                patterns.push(p);
            }
        }
        if child.kind() == "class_declaration" {
            collect_class_declaration_decorators(child, source, patterns);
        }
    }
}

/// Handle a `class_declaration` node — scan its own decorator children (for
/// bare `class` declarations not wrapped in `export_statement`) and then scan
/// its `class_body` for method-level decorators.
fn collect_class_declaration_decorators(
    node: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "decorator" {
            if let Some(p) = parse_decorator(child, source) {
                patterns.push(p);
            }
        }
        if child.kind() == "class_body" {
            collect_class_body_decorators(child, source, patterns);
        }
    }
}

/// Scan a `class_body` for `decorator` nodes.
///
/// In tree-sitter's TypeScript grammar, method-level decorators are **direct
/// children of `class_body`**, NOT children of `method_definition`.  They sit
/// as siblings immediately before the `method_definition` they annotate.
fn collect_class_body_decorators(
    class_body: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    let mut cursor = class_body.walk();
    for child in class_body.children(&mut cursor) {
        if child.kind() == "decorator" {
            if let Some(p) = parse_decorator(child, source) {
                patterns.push(p);
            }
        }
    }
}

/// Parse a single `decorator` node into an `ExtractedFrameworkPattern`.
///
/// The grammar for a decorator is:
/// ```text
/// (decorator "@"
///   (call_expression
///     function: (identifier)
///     arguments: (arguments ...))
///   | (identifier))
/// ```
fn parse_decorator(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    let pos = node.start_position();
    let line = pos.row as u32 + 1;
    let column = pos.column as u32;

    // The decorator body is the non-"@" child — either a call_expression or
    // a bare identifier.
    let mut cursor = node.walk();
    let body = node
        .children(&mut cursor)
        .find(|n| n.kind() != "@" && n.is_named())?;

    let (decorator_name, args_node) = match body.kind() {
        "call_expression" => {
            let func = body.child_by_field_name("function")?;
            let name = text_for_node(func, source);
            let args = body.child_by_field_name("arguments");
            (name, args)
        }
        "identifier" => (text_for_node(body, source), None),
        // member_expression like `@nestjs/common.Controller` (rare but valid)
        "member_expression" => {
            let prop = body.child_by_field_name("property")?;
            (text_for_node(prop, source), None)
        }
        _ => return None,
    };

    classify_nestjs_decorator(&decorator_name, args_node, source, line, column)
}

/// Map a decorator name to its `FrameworkPatternKind` and build the pattern.
fn classify_nestjs_decorator(
    name: &str,
    args_node: Option<Node>,
    source: &str,
    line: u32,
    column: u32,
) -> Option<ExtractedFrameworkPattern> {
    // HTTP method route decorators: @Get, @Post, @Put, @Delete, …
    if let Some(&(_decorator, http_method)) = NESTJS_ROUTE_DECORATORS
        .iter()
        .find(|&&(d, _)| d.eq_ignore_ascii_case(name))
    {
        let path = args_node.and_then(|a| extract_first_string_arg(a, source));
        return Some(ExtractedFrameworkPattern {
            line,
            column,
            framework: "nestjs".to_string(),
            kind: FrameworkPatternKind::Route,
            http_method: Some(http_method.to_string()),
            path,
            name: None,
            handler: None,
            arguments: None,
            parent_chain: None,
        });
    }

    match name {
        "Controller" => {
            let path = args_node.and_then(|a| extract_first_string_arg(a, source));
            Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "nestjs".to_string(),
                kind: FrameworkPatternKind::Controller,
                http_method: None,
                path,
                name: None,
                handler: None,
                arguments: None,
                parent_chain: None,
            })
        }

        "Injectable" => Some(ExtractedFrameworkPattern {
            line,
            column,
            framework: "nestjs".to_string(),
            kind: FrameworkPatternKind::Injectable,
            http_method: None,
            path: None,
            name: None,
            handler: None,
            arguments: None,
            parent_chain: None,
        }),

        "Module" => {
            // @Module({ imports: [...], controllers: [...], providers: [...] })
            // Surface the object keys as the name to give a useful summary.
            let module_name = args_node.and_then(|args| {
                let mut cursor = args.walk();
                let children: Vec<_> = args.children(&mut cursor).collect();
                drop(cursor);
                children
                    .into_iter()
                    .find(|n| n.kind() == "object")
                    .and_then(|obj| extract_object_keys(obj, source))
            });
            Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "nestjs".to_string(),
                kind: FrameworkPatternKind::Module,
                http_method: None,
                path: None,
                name: module_name,
                handler: None,
                arguments: None,
                parent_chain: None,
            })
        }

        "UseGuards" => {
            let guard_name = args_node.and_then(|a| extract_first_identifier_arg(a, source));
            Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "nestjs".to_string(),
                kind: FrameworkPatternKind::Guard,
                http_method: None,
                path: None,
                name: guard_name,
                handler: None,
                arguments: None,
                parent_chain: None,
            })
        }

        "UseInterceptors" => {
            let interceptor_name = args_node.and_then(|a| extract_first_identifier_arg(a, source));
            Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "nestjs".to_string(),
                kind: FrameworkPatternKind::Interceptor,
                http_method: None,
                path: None,
                name: interceptor_name,
                handler: None,
                arguments: None,
                parent_chain: None,
            })
        }

        "UsePipes" => {
            let pipe_name = args_node.and_then(|a| extract_first_identifier_arg(a, source));
            Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "nestjs".to_string(),
                kind: FrameworkPatternKind::Pipe,
                http_method: None,
                path: None,
                name: pipe_name,
                handler: None,
                arguments: args_node.map(|a| truncate_text(&text_for_node(a, source), 200)),
                parent_chain: None,
            })
        }

        _ => None,
    }
}

/// Return the unquoted value of the first string literal in an `arguments` node.
fn extract_first_string_arg(args_node: Node, source: &str) -> Option<String> {
    let mut cursor = args_node.walk();
    let children: Vec<_> = args_node.children(&mut cursor).collect();
    drop(cursor);
    children
        .into_iter()
        .filter(|n| n.is_named())
        .find(|n| n.kind() == "string" || n.kind() == "template_string")
        .map(|n| extract_string_value(n, source))
}

/// Return the text of the first identifier (or `new_expression` constructor) in
/// an `arguments` node — used to name guards, interceptors, and pipes.
fn extract_first_identifier_arg(args_node: Node, source: &str) -> Option<String> {
    let mut cursor = args_node.walk();
    let children: Vec<_> = args_node.children(&mut cursor).collect();
    drop(cursor);
    children
        .into_iter()
        .filter(|n| n.is_named())
        .find_map(|n| match n.kind() {
            "identifier" => Some(text_for_node(n, source)),
            "new_expression" => n
                .child_by_field_name("constructor")
                .map(|c| text_for_node(c, source)),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parser::{parser_for_id, LanguageId};

    fn parse_and_extract(source: &str) -> Vec<ExtractedFrameworkPattern> {
        let mut parser = parser_for_id(LanguageId::Typescript).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_nestjs_patterns(tree.root_node(), source)
    }

    #[test]
    fn extracts_controller_and_routes() {
        let source = r#"
@Controller('/users')
export class UsersController {
  @Get()
  findAll() {}

  @Post()
  create() {}

  @Get(':id')
  findOne() {}
}
"#;
        let patterns = parse_and_extract(source);

        let controller = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::Controller);
        assert!(controller.is_some(), "Expected a Controller pattern");
        let ctrl = controller.unwrap();
        assert_eq!(ctrl.path, Some("/users".to_string()));
        assert_eq!(ctrl.framework, "nestjs");

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 3, "Expected 3 route patterns");

        let get_routes: Vec<_> = routes
            .iter()
            .filter(|r| r.http_method == Some("GET".to_string()))
            .collect();
        assert_eq!(get_routes.len(), 2, "Expected 2 GET routes");

        let post_routes: Vec<_> = routes
            .iter()
            .filter(|r| r.http_method == Some("POST".to_string()))
            .collect();
        assert_eq!(post_routes.len(), 1, "Expected 1 POST route");

        // The @Get(':id') route should carry its sub-path.
        let id_route = routes.iter().find(|r| r.path.is_some());
        assert!(id_route.is_some(), "Expected a route with a sub-path");
        assert_eq!(id_route.unwrap().path, Some(":id".to_string()));
    }

    #[test]
    fn extracts_injectable() {
        let source = r#"
@Injectable()
export class AuthService {
  constructor(private readonly usersService: UsersService) {}
}
"#;
        let patterns = parse_and_extract(source);

        let injectable = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::Injectable);
        assert!(injectable.is_some(), "Expected an Injectable pattern");
        assert_eq!(injectable.unwrap().framework, "nestjs");
    }

    #[test]
    fn extracts_module() {
        let source = r#"
@Module({
  imports: [DatabaseModule],
  controllers: [UsersController],
  providers: [UsersService],
})
export class UsersModule {}
"#;
        let patterns = parse_and_extract(source);

        let module = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::Module);
        assert!(module.is_some(), "Expected a Module pattern");
        let m = module.unwrap();
        assert_eq!(m.framework, "nestjs");
        // Object keys should be captured as the module summary.
        let name = m.name.as_deref().unwrap_or("");
        assert!(
            name.contains("imports"),
            "Expected object keys in module name, got: {name}"
        );
        assert!(
            name.contains("controllers"),
            "Expected 'controllers' key in module name, got: {name}"
        );
    }

    #[test]
    fn extracts_guards_interceptors_pipes() {
        let source = r#"
@Controller('/admin')
@UseGuards(AuthGuard)
@UseInterceptors(LoggingInterceptor)
@UsePipes(ValidationPipe)
export class AdminController {
  @Get()
  @UseGuards(RolesGuard)
  findAll() {}
}
"#;
        let patterns = parse_and_extract(source);

        let guards: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Guard)
            .collect();
        assert_eq!(
            guards.len(),
            2,
            "Expected 2 Guard patterns (class + method)"
        );
        assert_eq!(guards[0].name, Some("AuthGuard".to_string()));

        let interceptors: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Interceptor)
            .collect();
        assert_eq!(interceptors.len(), 1, "Expected 1 Interceptor pattern");
        assert_eq!(interceptors[0].name, Some("LoggingInterceptor".to_string()));

        let pipes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Pipe)
            .collect();
        assert_eq!(pipes.len(), 1, "Expected 1 Pipe pattern");
        assert_eq!(pipes[0].name, Some("ValidationPipe".to_string()));
    }
}

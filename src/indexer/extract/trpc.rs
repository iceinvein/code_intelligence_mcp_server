//! tRPC framework pattern extraction
//!
//! Extracts procedure definitions, router compositions, and middleware from tRPC
//! call-builder chains.
//!
//! tRPC constructs its API through fluent builder chains:
//! - `publicProcedure.input(schema).query(handler)` → `Procedure` (name="query")
//! - `publicProcedure.mutation(handler)` → `Procedure` (name="mutation")
//! - `publicProcedure.subscription(handler)` → `Procedure` (name="subscription")
//! - `router({...})`, `createTRPCRouter({...})`, `t.router({...})` → `Router`
//! - `middleware(handler)`, `t.middleware(handler)` → `Middleware`
//!
//! False-positive guards:
//! - Procedure chains must include the word "procedure" somewhere in the
//!   full chain text (case-insensitive).
//! - Router calls must have an object literal as their first argument.

use tree_sitter::Node;

use super::framework_utils::{extract_object_keys, text_for_node, walk_call_expressions};
use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

/// Terminal procedure method names that end a tRPC procedure builder chain.
const PROCEDURE_TERMINALS: &[&str] = &["query", "mutation", "subscription"];

/// Router factory function names (bare calls and `t.router` / `createTRPCRouter`).
const ROUTER_NAMES: &[&str] = &["router", "createTRPCRouter"];

/// Middleware factory function names.
const MIDDLEWARE_NAMES: &[&str] = &["middleware", "createMiddleware"];

/// Extract tRPC framework patterns from a TypeScript AST.
pub fn extract_trpc_patterns(root: Node, source: &str) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();
    walk_call_expressions(root, source, &mut patterns, &try_extract_trpc_call);
    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

/// Try to extract a tRPC pattern from a single `call_expression` node.
fn try_extract_trpc_call(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    let func_node = node.child_by_field_name("function")?;

    match func_node.kind() {
        // `something.query(...)` / `something.mutation(...)` / `t.router(...)` /
        // `t.middleware(...)` / `publicProcedure.input(...).query(...)`
        "member_expression" => try_member_call(node, func_node, source),

        // Bare `router({...})` / `createTRPCRouter({...})` / `middleware(fn)`
        "identifier" => try_bare_call(node, func_node, source),

        _ => None,
    }
}

/// Handle `obj.method(...)` style calls.
fn try_member_call(
    call_node: Node,
    func_node: Node,
    source: &str,
) -> Option<ExtractedFrameworkPattern> {
    let property = func_node.child_by_field_name("property")?;
    let method_name = text_for_node(property, source);
    let method_lower = method_name.to_lowercase();

    let pos = property.start_position();
    let line = pos.row as u32 + 1;
    let column = pos.column as u32;

    let args_node = call_node.child_by_field_name("arguments")?;

    // --- Procedure terminal: `.query(...)`, `.mutation(...)`, `.subscription(...)` ---
    if PROCEDURE_TERMINALS.contains(&method_lower.as_str()) {
        // Guard: the full call text must contain "procedure" to distinguish real
        // tRPC chains from unrelated `.query()` / `.mutation()` usages.
        let full_text = text_for_node(call_node, source).to_lowercase();
        if !full_text.contains("procedure") {
            return None;
        }

        return Some(ExtractedFrameworkPattern {
            line,
            column,
            framework: "trpc".to_string(),
            kind: FrameworkPatternKind::Procedure,
            http_method: None,
            path: None,
            name: Some(method_lower),
            handler: None,
            arguments: None,
            parent_chain: None,
        });
    }

    // --- Router: `t.router({...})` ---
    if method_lower == "router" {
        if let Some(obj_keys) = extract_first_object_keys(args_node, source) {
            return Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "trpc".to_string(),
                kind: FrameworkPatternKind::Router,
                http_method: None,
                path: None,
                name: Some(obj_keys),
                handler: None,
                arguments: None,
                parent_chain: None,
            });
        }
        // router() without an object literal arg — skip (false positive)
        return None;
    }

    // --- Middleware: `t.middleware(fn)` ---
    if method_lower == "middleware" || method_lower == "createmiddleware" {
        let handler_text = extract_first_arg_text(args_node, source);
        return Some(ExtractedFrameworkPattern {
            line,
            column,
            framework: "trpc".to_string(),
            kind: FrameworkPatternKind::Middleware,
            http_method: None,
            path: None,
            name: None,
            handler: handler_text,
            arguments: None,
            parent_chain: None,
        });
    }

    None
}

/// Handle bare `functionName(...)` calls (no object prefix).
fn try_bare_call(
    call_node: Node,
    func_node: Node,
    source: &str,
) -> Option<ExtractedFrameworkPattern> {
    let func_name = text_for_node(func_node, source);

    let pos = func_node.start_position();
    let line = pos.row as u32 + 1;
    let column = pos.column as u32;

    let args_node = call_node.child_by_field_name("arguments")?;

    // `router({...})` or `createTRPCRouter({...})`
    if ROUTER_NAMES.iter().any(|&n| n.eq_ignore_ascii_case(&func_name)) {
        if let Some(obj_keys) = extract_first_object_keys(args_node, source) {
            return Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "trpc".to_string(),
                kind: FrameworkPatternKind::Router,
                http_method: None,
                path: None,
                name: Some(obj_keys),
                handler: None,
                arguments: None,
                parent_chain: None,
            });
        }
        // Must have an object literal first arg; otherwise skip.
        return None;
    }

    // `middleware(fn)` or `createMiddleware(fn)`
    if MIDDLEWARE_NAMES.iter().any(|&n| n.eq_ignore_ascii_case(&func_name)) {
        let handler_text = extract_first_arg_text(args_node, source);
        return Some(ExtractedFrameworkPattern {
            line,
            column,
            framework: "trpc".to_string(),
            kind: FrameworkPatternKind::Middleware,
            http_method: None,
            path: None,
            name: None,
            handler: handler_text,
            arguments: None,
            parent_chain: None,
        });
    }

    None
}

/// Return the keys of the first `object` child inside an `arguments` node, or
/// `None` if no object literal is present as the first argument.
fn extract_first_object_keys(args_node: Node, source: &str) -> Option<String> {
    let mut cursor = args_node.walk();
    let children: Vec<_> = args_node.children(&mut cursor).collect();
    drop(cursor);
    children
        .into_iter()
        .filter(|n| n.is_named())
        .find(|n| n.kind() == "object")
        .and_then(|obj| extract_object_keys(obj, source))
}

/// Return the text of the first named argument in an `arguments` node, or `None`
/// if the node has no named children.
fn extract_first_arg_text(args_node: Node, source: &str) -> Option<String> {
    let mut cursor = args_node.walk();
    let children: Vec<_> = args_node.children(&mut cursor).collect();
    drop(cursor);
    children.into_iter().find(|n| n.is_named()).map(|n| {
        // For identifiers give just the name; for complex expressions truncate.
        if n.kind() == "identifier" {
            text_for_node(n, source)
        } else {
            let t = text_for_node(n, source);
            if t.len() > 120 {
                format!("{}...", &t[..120])
            } else {
                t
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parser::{parser_for_id, LanguageId};

    fn parse_and_extract(source: &str) -> Vec<ExtractedFrameworkPattern> {
        let mut parser = parser_for_id(LanguageId::Typescript).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_trpc_patterns(tree.root_node(), source)
    }

    #[test]
    fn extracts_procedures() {
        let source = r#"
export const appRouter = createTRPCRouter({
  getUser: publicProcedure
    .input(z.object({ id: z.string() }))
    .query(async ({ input }) => {
      return db.user.findUnique({ where: { id: input.id } })
    }),

  createUser: publicProcedure
    .input(z.object({ name: z.string() }))
    .mutation(async ({ input }) => {
      return db.user.create({ data: input })
    }),
})
"#;
        let patterns = parse_and_extract(source);

        let procedures: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Procedure)
            .collect();
        assert_eq!(procedures.len(), 2, "Expected 2 procedure patterns");

        let query = procedures
            .iter()
            .find(|p| p.name == Some("query".to_string()));
        assert!(query.is_some(), "Expected a query procedure");

        let mutation = procedures
            .iter()
            .find(|p| p.name == Some("mutation".to_string()));
        assert!(mutation.is_some(), "Expected a mutation procedure");

        // All procedures must be tagged as tRPC.
        assert!(procedures.iter().all(|p| p.framework == "trpc"));
    }

    #[test]
    fn extracts_router() {
        let source = r#"
const appRouter = createTRPCRouter({
  users: usersRouter,
  posts: postsRouter,
  auth: authRouter,
})
"#;
        let patterns = parse_and_extract(source);

        let routers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Router)
            .collect();
        assert_eq!(routers.len(), 1, "Expected 1 router pattern");
        let router = &routers[0];
        assert_eq!(router.framework, "trpc");
        let name = router.name.as_deref().unwrap_or("");
        assert!(
            name.contains("users"),
            "Expected router keys in name, got: {name}"
        );
        assert!(
            name.contains("posts"),
            "Expected 'posts' key in router name, got: {name}"
        );
    }

    #[test]
    fn extracts_middleware() {
        let source = r#"
const isAuthed = t.middleware(({ ctx, next }) => {
  if (!ctx.session) throw new TRPCError({ code: 'UNAUTHORIZED' })
  return next({ ctx: { ...ctx, session: ctx.session } })
})
"#;
        let patterns = parse_and_extract(source);

        let middleware: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Middleware)
            .collect();
        assert_eq!(middleware.len(), 1, "Expected 1 middleware pattern");
        assert_eq!(middleware[0].framework, "trpc");
    }

    #[test]
    fn ignores_unrelated_router_calls() {
        // These should NOT be extracted as tRPC patterns:
        // - `router.get(key)` (Express-style member call, method is "get" not "router")
        // - `createRouter()` without an object literal argument
        // - `.query(searchString)` without "procedure" in the chain
        let source = r#"
const result = someMap.get(key)
const r = createRouter()
const found = db.query('SELECT * FROM users')
"#;
        let patterns = parse_and_extract(source);

        assert_eq!(
            patterns.len(),
            0,
            "Unrelated calls should not produce tRPC patterns, got: {patterns:?}"
        );
    }
}

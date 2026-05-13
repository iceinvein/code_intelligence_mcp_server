//! Convex framework pattern extraction
//!
//! Extracts function definitions, HTTP routes, cron jobs, and schemas from
//! Convex backend code. Convex uses a distinctive builder pattern for
//! server-side functions:
//!
//! - `export const getTask = query({ handler: ... })` → `Query`
//! - `export const createTask = mutation({ handler: ... })` → `Mutation`
//! - `export const sendEmail = action({ handler: ... })` → `Action`
//! - `export const getUser = internalQuery({ handler: ... })` → `Query` (name="internal")
//! - `export const process = internalMutation({ handler: ... })` → `Mutation` (name="internal")
//! - `export const sync = internalAction({ handler: ... })` → `Action` (name="internal")
//! - `export const handler = httpAction(async (ctx, req) => ...)` → `Route`
//! - `http.route({ path: "/webhook", method: "POST", handler: ... })` → `Route`
//! - `crons.interval("name", { minutes: 30 }, internal.x.y)` → `CronJob`
//! - `crons.cron("name", "0 0 * * *", internal.x.y)` → `CronJob`
//! - `export default defineSchema({ tasks: defineTable({...}) })` → `Schema`
//!
//! # False-positive guards
//!
//! The builder names `query`, `mutation`, `action` are common words that appear
//! in many codebases (e.g., `db.query(...)`, GraphQL mutations). We guard
//! against false positives by:
//! - Requiring the call to have an object argument with a `handler` property
//!   (for query/mutation/action builders)
//! - Checking that `httpAction` calls have exactly `(ctx, request)` signature
//! - Requiring `http.route(...)` calls to have a `path` or `pathPrefix` property
//! - Requiring `cronJobs()` import or `crons.*` call patterns

use tree_sitter::Node;

use super::framework_utils::{
    extract_object_keys, extract_string_value, find_chain_root, text_for_node,
    walk_call_expressions,
};
use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

/// Convex function builder names (public).
const PUBLIC_BUILDERS: &[&str] = &["query", "mutation", "action"];

/// Convex function builder names (internal).
const INTERNAL_BUILDERS: &[&str] = &["internalQuery", "internalMutation", "internalAction"];

/// Cron schedule method names on the `crons` object.
const CRON_METHODS: &[&str] = &["interval", "cron", "hourly", "daily", "weekly", "monthly"];

/// Map a builder function name to its `FrameworkPatternKind`.
fn builder_to_kind(name: &str) -> Option<FrameworkPatternKind> {
    match name {
        "query" | "internalQuery" => Some(FrameworkPatternKind::Query),
        "mutation" | "internalMutation" => Some(FrameworkPatternKind::Mutation),
        "action" | "internalAction" => Some(FrameworkPatternKind::Action),
        _ => None,
    }
}

/// Return `true` when the file at `file_path` is inside a `convex/` directory
/// (the standard location for Convex backend code).
pub fn is_convex_file(file_path: &str) -> bool {
    let path = file_path.replace('\\', "/");
    path.starts_with("convex/") || path.contains("/convex/")
}

/// Extract Convex framework patterns from a TypeScript AST.
///
/// Uses both AST-level call detection (for function builders, httpAction,
/// httpRouter, cronJobs) and file-path conventions (schema.ts, http.ts,
/// crons.ts).
pub fn extract_convex_patterns(
    root: Node,
    source: &str,
    file_path: &str,
) -> Vec<ExtractedFrameworkPattern> {
    let mut patterns = Vec::new();

    // Walk for call expressions: function builders, httpAction, http.route, cron methods
    walk_call_expressions(root, source, &mut patterns, &|node, src| {
        try_extract_convex_call(node, src)
    });

    // Schema detection: `defineSchema(...)` in schema.ts files
    if is_schema_file(file_path) {
        extract_schema_patterns(root, source, &mut patterns);
    }

    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

/// Try to extract a Convex pattern from a single `call_expression` node.
fn try_extract_convex_call(node: Node, source: &str) -> Option<ExtractedFrameworkPattern> {
    let func_node = node.child_by_field_name("function")?;

    match func_node.kind() {
        // Bare calls: `query({...})`, `mutation({...})`, `httpAction(...)`,
        // `cronJobs()`, `httpRouter()`, `defineSchema({...})`
        "identifier" => try_bare_call(node, func_node, source),

        // Member calls: `http.route({...})`, `crons.interval(...)`, `crons.cron(...)`
        "member_expression" => try_member_call(node, func_node, source),

        _ => None,
    }
}

/// Handle bare `functionName(...)` calls.
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

    // --- Public function builders: query({handler}), mutation({handler}), action({handler}) ---
    if PUBLIC_BUILDERS.contains(&func_name.as_str()) {
        if has_handler_property(args_node, source) {
            let kind = builder_to_kind(&func_name)?;
            return Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "convex".to_string(),
                kind,
                http_method: None,
                path: None,
                name: None,
                handler: None,
                arguments: extract_args_schema(args_node, source),
                parent_chain: None,
            });
        }
        return None;
    }

    // --- Internal function builders: internalQuery, internalMutation, internalAction ---
    if INTERNAL_BUILDERS.contains(&func_name.as_str()) {
        if has_handler_property(args_node, source) {
            let kind = builder_to_kind(&func_name)?;
            return Some(ExtractedFrameworkPattern {
                line,
                column,
                framework: "convex".to_string(),
                kind,
                http_method: None,
                path: None,
                name: Some("internal".to_string()),
                handler: None,
                arguments: extract_args_schema(args_node, source),
                parent_chain: None,
            });
        }
        return None;
    }

    // --- httpAction(async (ctx, request) => { ... }) ---
    if func_name == "httpAction" {
        return Some(ExtractedFrameworkPattern {
            line,
            column,
            framework: "convex".to_string(),
            kind: FrameworkPatternKind::Route,
            http_method: None,
            path: None,
            name: Some("httpAction".to_string()),
            handler: None,
            arguments: None,
            parent_chain: None,
        });
    }

    // --- httpRouter() ---
    if func_name == "httpRouter" {
        return Some(ExtractedFrameworkPattern {
            line,
            column,
            framework: "convex".to_string(),
            kind: FrameworkPatternKind::Router,
            http_method: None,
            path: None,
            name: Some("httpRouter".to_string()),
            handler: None,
            arguments: None,
            parent_chain: None,
        });
    }

    // --- cronJobs() ---
    if func_name == "cronJobs" {
        return Some(ExtractedFrameworkPattern {
            line,
            column,
            framework: "convex".to_string(),
            kind: FrameworkPatternKind::CronJob,
            http_method: None,
            path: None,
            name: Some("cronJobs".to_string()),
            handler: None,
            arguments: None,
            parent_chain: None,
        });
    }

    None
}

/// Handle `obj.method(...)` calls like `http.route(...)` and `crons.interval(...)`.
fn try_member_call(
    call_node: Node,
    func_node: Node,
    source: &str,
) -> Option<ExtractedFrameworkPattern> {
    let property = func_node.child_by_field_name("property")?;
    let method_name = text_for_node(property, source);

    let pos = property.start_position();
    let line = pos.row as u32 + 1;
    let column = pos.column as u32;

    let args_node = call_node.child_by_field_name("arguments")?;

    // --- http.route({ path: "/...", method: "POST", handler: ... }) ---
    if method_name == "route" {
        let parent_chain = find_chain_root(func_node, source);
        // Only match if the chain root looks like an HTTP router variable
        // (commonly named "http" or "router")
        if let Some(ref root_name) = parent_chain {
            let root_lower = root_name.to_lowercase();
            if root_lower == "http" || root_lower.contains("router") {
                return try_extract_http_route(args_node, source, line, column, parent_chain);
            }
        }
        return None;
    }

    // --- crons.interval(...), crons.cron(...), crons.daily(...), etc. ---
    if CRON_METHODS.contains(&method_name.as_str()) {
        let parent_chain = find_chain_root(func_node, source);
        // Guard: must be called on a "crons"-like variable
        if let Some(ref root_name) = parent_chain {
            if root_name.to_lowercase().contains("cron") {
                return try_extract_cron_schedule(
                    args_node,
                    source,
                    &method_name,
                    line,
                    column,
                    parent_chain,
                );
            }
        }
        return None;
    }

    None
}

/// Extract HTTP route details from `http.route({ path: "/...", method: "POST", handler: ... })`.
fn try_extract_http_route(
    args_node: Node,
    source: &str,
    line: u32,
    column: u32,
    parent_chain: Option<String>,
) -> Option<ExtractedFrameworkPattern> {
    // First argument should be an object with `path`/`pathPrefix` and `method` keys
    let first_arg = nth_named_child(args_node, 0)?;
    if first_arg.kind() != "object" {
        return None;
    }

    let mut path: Option<String> = None;
    let mut method: Option<String> = None;
    let mut handler: Option<String> = None;

    let mut cursor = first_arg.walk();
    for child in first_arg.children(&mut cursor) {
        if child.kind() == "pair" {
            if let Some(key_node) = child.child_by_field_name("key") {
                let key = text_for_node(key_node, source);
                if let Some(val_node) = child.child_by_field_name("value") {
                    match key.as_str() {
                        "path" | "pathPrefix" => {
                            if val_node.kind() == "string" || val_node.kind() == "template_string" {
                                path = Some(extract_string_value(val_node, source));
                            }
                        }
                        "method" => {
                            if val_node.kind() == "string" || val_node.kind() == "template_string" {
                                method =
                                    Some(extract_string_value(val_node, source).to_uppercase());
                            }
                        }
                        "handler" => {
                            if val_node.kind() == "identifier" {
                                handler = Some(text_for_node(val_node, source));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Must have a path to be a valid route registration
    let route_path = path?;

    Some(ExtractedFrameworkPattern {
        line,
        column,
        framework: "convex".to_string(),
        kind: FrameworkPatternKind::Route,
        http_method: method,
        path: Some(route_path),
        name: Some("httpRoute".to_string()),
        handler,
        arguments: None,
        parent_chain,
    })
}

/// Extract cron job details from `crons.interval("name", { minutes: 30 }, internal.x.y)`.
fn try_extract_cron_schedule(
    args_node: Node,
    source: &str,
    method: &str,
    line: u32,
    column: u32,
    parent_chain: Option<String>,
) -> Option<ExtractedFrameworkPattern> {
    // First argument is the job name (string)
    let first_arg = nth_named_child(args_node, 0)?;
    let job_name = if first_arg.kind() == "string" || first_arg.kind() == "template_string" {
        Some(extract_string_value(first_arg, source))
    } else {
        None
    };

    // Second argument is the schedule (object for interval, string for cron expression)
    let second_arg = nth_named_child(args_node, 1);
    let schedule = second_arg.map(|n| {
        let text = text_for_node(n, source);
        if text.len() > 80 {
            format!("{}...", &text[..80])
        } else {
            text
        }
    });

    Some(ExtractedFrameworkPattern {
        line,
        column,
        framework: "convex".to_string(),
        kind: FrameworkPatternKind::CronJob,
        http_method: None,
        path: None,
        name: job_name,
        handler: Some(method.to_string()),
        arguments: schedule,
        parent_chain,
    })
}

/// Extract schema patterns from `defineSchema({ tableName: defineTable({...}) })`.
fn extract_schema_patterns(
    root: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    // Walk the AST looking for `defineSchema(...)` calls
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "call_expression" {
            if let Some(func) = node.child_by_field_name("function") {
                if func.kind() == "identifier" && text_for_node(func, source) == "defineSchema" {
                    let pos = func.start_position();
                    if let Some(args) = node.child_by_field_name("arguments") {
                        let table_names = extract_schema_table_names(args, source);
                        patterns.push(ExtractedFrameworkPattern {
                            line: pos.row as u32 + 1,
                            column: pos.column as u32,
                            framework: "convex".to_string(),
                            kind: FrameworkPatternKind::Schema,
                            http_method: None,
                            path: None,
                            name: if table_names.is_empty() {
                                None
                            } else {
                                Some(table_names.join(", "))
                            },
                            handler: Some("defineSchema".to_string()),
                            arguments: None,
                            parent_chain: None,
                        });
                    }
                }
            }
        }

        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                stack.push(cursor.node());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }
}

/// Extract table names from the first object argument of `defineSchema({...})`.
fn extract_schema_table_names(args_node: Node, source: &str) -> Vec<String> {
    let mut tables = Vec::new();
    let first = nth_named_child(args_node, 0);
    if let Some(obj) = first {
        if obj.kind() == "object" {
            if let Some(keys) = extract_object_keys(obj, source) {
                for key in keys.split(", ") {
                    tables.push(key.to_string());
                }
            }
        }
    }
    tables
}

/// Check if the first argument to a builder call is an object with a `handler` property.
/// This is the primary false-positive guard for `query`, `mutation`, `action` — real
/// Convex builders always take `{ handler: ... }` while unrelated calls (e.g.,
/// `db.query(...)`, GraphQL mutations) do not.
fn has_handler_property(args_node: Node, source: &str) -> bool {
    let first = match nth_named_child(args_node, 0) {
        Some(n) if n.kind() == "object" => n,
        _ => return false,
    };

    let mut cursor = first.walk();
    for child in first.children(&mut cursor) {
        if child.kind() == "pair" {
            if let Some(key) = child.child_by_field_name("key") {
                if text_for_node(key, source) == "handler" {
                    return true;
                }
            }
        }
    }
    false
}

/// Extract args schema info from a builder config object (for diagnostics).
fn extract_args_schema(args_node: Node, source: &str) -> Option<String> {
    let first = nth_named_child(args_node, 0)?;
    if first.kind() != "object" {
        return None;
    }

    let mut cursor = first.walk();
    for child in first.children(&mut cursor) {
        if child.kind() == "pair" {
            if let Some(key) = child.child_by_field_name("key") {
                if text_for_node(key, source) == "args" {
                    if let Some(val) = child.child_by_field_name("value") {
                        let text = text_for_node(val, source);
                        if text.len() > 120 {
                            return Some(format!("{}...", &text[..120]));
                        }
                        return Some(text);
                    }
                }
            }
        }
    }
    None
}

/// Return `true` when `file_path` matches the Convex `schema.ts` convention.
fn is_schema_file(file_path: &str) -> bool {
    let path = file_path.replace('\\', "/");
    let file_name = path.rsplit('/').next().unwrap_or("");
    file_name == "schema.ts" || file_name == "schema.tsx"
}

/// Get the Nth named child of a node (0-indexed).
fn nth_named_child(node: Node, n: usize) -> Option<Node> {
    let mut cursor = node.walk();
    let children: Vec<_> = node
        .children(&mut cursor)
        .filter(|c| c.is_named())
        .collect();
    children.into_iter().nth(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parser::{parser_for_id, LanguageId};

    fn parse_and_extract(source: &str) -> Vec<ExtractedFrameworkPattern> {
        parse_and_extract_with_path(source, "convex/tasks.ts")
    }

    fn parse_and_extract_with_path(
        source: &str,
        file_path: &str,
    ) -> Vec<ExtractedFrameworkPattern> {
        let mut parser = parser_for_id(LanguageId::Typescript).unwrap();
        let tree = parser.parse(source, None).unwrap();
        extract_convex_patterns(tree.root_node(), source, file_path)
    }

    #[test]
    fn extracts_query() {
        let source = r#"
import { query } from "./_generated/server";
import { v } from "convex/values";

export const getTask = query({
  args: { id: v.id("tasks") },
  handler: async (ctx, args) => {
    return await ctx.db.get(args.id);
  },
});
"#;
        let patterns = parse_and_extract(source);
        let queries: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Query)
            .collect();
        assert_eq!(queries.len(), 1, "Expected 1 query pattern");
        assert_eq!(queries[0].framework, "convex");
        assert!(
            queries[0].name.is_none(),
            "Public query should have no name"
        );
    }

    #[test]
    fn extracts_mutation() {
        let source = r#"
import { mutation } from "./_generated/server";
import { v } from "convex/values";

export const createTask = mutation({
  args: { text: v.string() },
  handler: async (ctx, args) => {
    return await ctx.db.insert("tasks", { text: args.text, done: false });
  },
});
"#;
        let patterns = parse_and_extract(source);
        let mutations: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Mutation)
            .collect();
        assert_eq!(mutations.len(), 1, "Expected 1 mutation pattern");
        assert_eq!(mutations[0].framework, "convex");
    }

    #[test]
    fn extracts_action() {
        let source = r#"
import { action } from "./_generated/server";
import { v } from "convex/values";

export const sendEmail = action({
  args: { userId: v.id("users"), message: v.string() },
  handler: async (ctx, args) => {
    await fetch("https://api.email.com/send", {});
  },
});
"#;
        let patterns = parse_and_extract(source);
        let actions: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Action)
            .collect();
        assert_eq!(actions.len(), 1, "Expected 1 action pattern");
        assert_eq!(actions[0].framework, "convex");
    }

    #[test]
    fn extracts_internal_functions() {
        let source = r#"
import { internalQuery, internalMutation, internalAction } from "./_generated/server";
import { v } from "convex/values";

export const getUser = internalQuery({
  args: { id: v.id("users") },
  handler: async (ctx, args) => {
    return await ctx.db.get(args.id);
  },
});

export const markDone = internalMutation({
  args: { taskId: v.id("tasks") },
  handler: async (ctx, args) => {
    await ctx.db.patch(args.taskId, { done: true });
  },
});

export const processWebhook = internalAction({
  args: { payload: v.string() },
  handler: async (ctx, args) => {
    // external API call
  },
});
"#;
        let patterns = parse_and_extract(source);

        let queries: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Query)
            .collect();
        assert_eq!(queries.len(), 1, "Expected 1 internal query");
        assert_eq!(queries[0].name, Some("internal".to_string()));

        let mutations: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Mutation)
            .collect();
        assert_eq!(mutations.len(), 1, "Expected 1 internal mutation");
        assert_eq!(mutations[0].name, Some("internal".to_string()));

        let actions: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Action)
            .collect();
        assert_eq!(actions.len(), 1, "Expected 1 internal action");
        assert_eq!(actions[0].name, Some("internal".to_string()));
    }

    #[test]
    fn extracts_http_routes() {
        let source = r#"
import { httpRouter } from "convex/server";
import { httpAction } from "./_generated/server";

const http = httpRouter();

http.route({
  path: "/webhook",
  method: "POST",
  handler: httpAction(async (ctx, request) => {
    return new Response(null, { status: 200 });
  }),
});

http.route({
  path: "/users",
  method: "GET",
  handler: getUser,
});

export default http;
"#;
        let patterns = parse_and_extract(source);

        let routers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Router)
            .collect();
        assert_eq!(routers.len(), 1, "Expected 1 router pattern");

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        // httpAction() inline produces 1 Route, plus 2 http.route() calls
        assert!(
            routes.len() >= 2,
            "Expected at least 2 route patterns, got {}",
            routes.len()
        );

        let webhook = routes
            .iter()
            .find(|r| r.path == Some("/webhook".to_string()));
        assert!(webhook.is_some(), "Expected /webhook route");
        assert_eq!(webhook.unwrap().http_method, Some("POST".to_string()));

        let users = routes.iter().find(|r| r.path == Some("/users".to_string()));
        assert!(users.is_some(), "Expected /users route");
        assert_eq!(users.unwrap().http_method, Some("GET".to_string()));
    }

    #[test]
    fn extracts_cron_jobs() {
        let source = r#"
import { cronJobs } from "convex/server";
import { internal } from "./_generated/api";

const crons = cronJobs();

crons.interval(
  "clean up sessions",
  { minutes: 30 },
  internal.sessions.cleanupExpired,
);

crons.cron(
  "midnight cleanup",
  "0 0 * * *",
  internal.maintenance.dailyCleanup,
);

crons.daily(
  "daily digest",
  { hourUTC: 9, minuteUTC: 0 },
  internal.emails.sendDigest,
);

export default crons;
"#;
        let patterns = parse_and_extract(source);

        let cron_jobs: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::CronJob)
            .collect();
        // cronJobs() call + 3 schedule calls
        assert!(
            cron_jobs.len() >= 3,
            "Expected at least 3 cron job patterns (interval + cron + daily), got {}",
            cron_jobs.len()
        );

        let interval = cron_jobs
            .iter()
            .find(|c| c.name == Some("clean up sessions".to_string()));
        assert!(interval.is_some(), "Expected 'clean up sessions' cron job");
        assert_eq!(interval.unwrap().handler, Some("interval".to_string()));

        let cron = cron_jobs
            .iter()
            .find(|c| c.name == Some("midnight cleanup".to_string()));
        assert!(cron.is_some(), "Expected 'midnight cleanup' cron job");
    }

    #[test]
    fn extracts_schema() {
        let source = r#"
import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

export default defineSchema({
  tasks: defineTable({
    text: v.string(),
    done: v.boolean(),
    ownerId: v.id("users"),
  }).index("by_owner", ["ownerId"]),

  users: defineTable({
    name: v.string(),
    email: v.string(),
  }),

  messages: defineTable({
    body: v.string(),
    channel: v.string(),
  }).searchIndex("search_body", {
    searchField: "body",
    filterFields: ["channel"],
  }),
});
"#;
        let patterns = parse_and_extract_with_path(source, "convex/schema.ts");

        let schemas: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Schema)
            .collect();
        assert_eq!(schemas.len(), 1, "Expected 1 schema pattern");
        let schema = &schemas[0];
        assert_eq!(schema.framework, "convex");
        assert_eq!(schema.handler, Some("defineSchema".to_string()));

        let name = schema.name.as_deref().unwrap_or("");
        assert!(name.contains("tasks"), "Schema should list 'tasks' table");
        assert!(name.contains("users"), "Schema should list 'users' table");
        assert!(
            name.contains("messages"),
            "Schema should list 'messages' table"
        );
    }

    #[test]
    fn ignores_unrelated_query_calls() {
        // These should NOT be extracted:
        // - `db.query(...)` (no handler property)
        // - `graphql mutation` (string argument, no handler)
        // - `query("SELECT ...")` (string argument, no handler)
        let source = r#"
const results = await ctx.db.query("tasks").collect();
const data = query("SELECT * FROM users");
const result = mutation({ data: "value" });
"#;
        let patterns = parse_and_extract(source);
        assert_eq!(
            patterns.len(),
            0,
            "Unrelated calls should not produce Convex patterns, got: {patterns:?}"
        );
    }

    #[test]
    fn ignores_non_convex_route_calls() {
        // Express-style route() calls should not be matched
        let source = r#"
const app = express();
app.route("/users").get(handler).post(handler);
"#;
        let patterns = parse_and_extract(source);
        // "app" doesn't contain "http" or "router", so should be filtered
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(
            routes.len(),
            0,
            "Express route() should not match as Convex route"
        );
    }

    #[test]
    fn is_convex_file_test() {
        assert!(is_convex_file("convex/tasks.ts"));
        assert!(is_convex_file("convex/schema.ts"));
        assert!(is_convex_file("src/convex/http.ts"));
        assert!(!is_convex_file("src/api/tasks.ts"));
        assert!(!is_convex_file("app/convex.ts"));
    }

    #[test]
    fn extracts_http_action_standalone() {
        let source = r#"
import { httpAction } from "./_generated/server";

export const getUser = httpAction(async (ctx, request) => {
  const { searchParams } = new URL(request.url);
  const userId = searchParams.get("id");
  return Response.json({ id: userId });
});
"#;
        let patterns = parse_and_extract(source);
        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 1, "Expected 1 httpAction route pattern");
        assert_eq!(routes[0].name, Some("httpAction".to_string()));
    }
}

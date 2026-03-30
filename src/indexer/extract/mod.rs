pub mod actix;
pub mod axum;
pub mod c;
pub mod comments;
pub mod convex;
pub mod cpp;
pub mod csharp;
pub mod django;
pub mod elysia;
pub mod express;
pub mod fastapi;
pub mod fastify;
pub mod framework_utils;
pub mod go;
pub mod go_frameworks;
pub mod hono;
pub mod java;
pub mod javascript;
pub mod kotlin;
pub mod nestjs;
pub mod nextjs;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod spring;
pub mod swift;
pub mod symbol;
pub mod trpc;
pub mod typescript;

/// Returns `true` when `node` sits inside a function body (i.e. is NOT at
/// module / namespace scope).  Used to skip extracting local variables as
/// top-level symbols.
pub fn is_inside_function_scope(node: tree_sitter::Node) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            // Reached the top — we're at module scope
            "program" => return false,

            // Function-like boundaries — we're inside a function
            "function_declaration"
            | "generator_function_declaration"
            | "arrow_function"
            | "method_definition"
            | "function_expression"
            | "generator_function" => return true,

            // Everything else (statement_block, if_statement, class_body,
            // internal_module, export_statement, …) is transparent — keep walking.
            _ => {}
        }
        current = parent;
    }
    false
}

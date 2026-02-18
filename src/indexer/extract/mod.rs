pub mod c;
pub mod cpp;
pub mod elysia;
pub mod go;
pub mod java;
pub mod javascript;
pub mod python;
pub mod rust;
pub mod symbol;
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

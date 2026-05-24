use crate::storage::sqlite::{FrameworkPatternRow, SqliteStore, SymbolRow};
use anyhow::Result;
use serde_json::{json, Value};

pub(crate) fn route_exposures_for_symbol(
    sqlite: &SqliteStore,
    symbol: &SymbolRow,
    limit: usize,
) -> Result<Vec<Value>> {
    let patterns = sqlite.search_framework_patterns(
        None,
        None,
        None,
        None,
        None,
        Some(&symbol.file_path),
        limit,
    )?;
    Ok(patterns
        .iter()
        .filter(|pattern| route_matches_symbol(pattern, symbol))
        .map(|pattern| route_value(pattern, Some(&symbol.id)))
        .collect())
}

pub(crate) fn route_value(route: &FrameworkPatternRow, handler_symbol_id: Option<&str>) -> Value {
    json!({
        "id": route.id,
        "framework": route.framework,
        "kind": route.kind,
        "http_method": route.http_method,
        "path": route.path,
        "handler": route.handler,
        "handler_symbol_id": handler_symbol_id,
        "line": route.line,
        "parent_chain": route.parent_chain,
    })
}

pub(crate) fn is_route_pattern(pattern: &FrameworkPatternRow) -> bool {
    matches!(pattern.kind.as_str(), "route" | "file_route")
}

fn route_matches_symbol(pattern: &FrameworkPatternRow, symbol: &SymbolRow) -> bool {
    if !is_route_pattern(pattern) {
        return false;
    }
    if pattern.handler.as_deref() == Some(symbol.name.as_str()) {
        return true;
    }
    pattern.line >= symbol.start_line && pattern.line <= symbol.end_line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(kind: &str, line: u32, handler: Option<&str>) -> FrameworkPatternRow {
        FrameworkPatternRow {
            id: "route:1".to_string(),
            file_path: "src/routes.rs".to_string(),
            line,
            framework: "axum".to_string(),
            kind: kind.to_string(),
            http_method: Some("GET".to_string()),
            path: Some("/users".to_string()),
            name: None,
            handler: handler.map(ToString::to_string),
            arguments: None,
            parent_chain: None,
            updated_at: 0,
        }
    }

    fn symbol() -> SymbolRow {
        SymbolRow {
            id: "sym_list".to_string(),
            file_path: "src/routes.rs".to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: "list_users".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 10,
            start_line: 10,
            end_line: 20,
            text: "pub fn list_users() {}".to_string(),
        }
    }

    #[test]
    fn route_matches_symbol_by_handler_name_or_line_range() {
        let symbol = symbol();

        assert!(route_matches_symbol(
            &pattern("route", 3, Some("list_users")),
            &symbol
        ));
        assert!(route_matches_symbol(
            &pattern("file_route", 12, None),
            &symbol
        ));
        assert!(!route_matches_symbol(
            &pattern("middleware", 12, None),
            &symbol
        ));
        assert!(!route_matches_symbol(
            &pattern("route", 30, Some("other")),
            &symbol
        ));
    }

    #[test]
    fn route_value_includes_handler_symbol_link() {
        let value = route_value(&pattern("route", 3, Some("list_users")), Some("sym_list"));

        assert_eq!(value["framework"], "axum");
        assert_eq!(value["path"], "/users");
        assert_eq!(value["handler"], "list_users");
        assert_eq!(value["handler_symbol_id"], "sym_list");
    }
}

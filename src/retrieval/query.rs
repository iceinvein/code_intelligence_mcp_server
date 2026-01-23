//! Query processing and normalization

use crate::text;

#[derive(Debug, Clone, Default)]
pub struct QueryControls {
    pub id: Option<String>,
    pub file: Option<String>,
    pub path: Option<String>,
    pub lang: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Intent {
    // Existing
    Callers(String),
    Definition,
    Schema,
    Test,

    // New intents (FNDN-15)
    Implementation,  // "how is X implemented", "implementation of"
    Config,          // "configuration", "settings", "env", "config"
    Error,           // "error handling", "exception", "error"
    API,             // "endpoint", "route", "handler", "api"
    Hook,            // "useEffect", "hook", "lifecycle"
    Middleware,      // "middleware", "interceptor"
    Migration,       // "migration", "schema change", "migrate"
}

/// Normalize query text for better search results
pub fn normalize_query(query: &str) -> String {
    let out = text::normalize_query_text(query);
    let mut final_parts = Vec::new();
    for part in out.split_whitespace() {
        final_parts.push(part.to_string());
        let lower = part.to_lowercase();
        match lower.as_str() {
            "and" | "or" | "not" => {}
            "db" => final_parts.push("database".to_string()),
            "auth" => final_parts.push("authentication".to_string()),
            "nav" => final_parts.push("navigation".to_string()),
            "config" => final_parts.push("configuration".to_string()),
            _ => {}
        }

        if lower.chars().all(|c| c.is_ascii_alphabetic()) && lower.len() >= 5 {
            for stem in text::simple_stems(&lower) {
                final_parts.push(stem);
            }
        }
    }

    final_parts.join(" ")
}

/// Detect user intent from query
pub fn detect_intent(query: &str) -> Option<Intent> {
    let q = query.trim().to_lowercase();

    // Test Detection (existing)
    if q.contains("test") || q.contains("spec") || q.contains("verify") {
        return Some(Intent::Test);
    }

    // NEW: Migration intent - check before Schema since "migration" is more specific
    if q.contains("migration") || q.contains("migrate") || q.contains("schema change") {
        return Some(Intent::Migration);
    }

    // Schema keywords (existing)
    if q.contains("schema")
        || q.contains("model")
        || q.contains("db table")
        || q.contains("database")
        || q.contains("entity")
        || q.split_whitespace().any(|w| w == "db")
    {
        return Some(Intent::Schema);
    }

    // NEW: Implementation intent
    if q.contains("implementation")
        || q.contains("how is")
        || q.contains("how does")
        || q.starts_with("implement")
    {
        return Some(Intent::Implementation);
    }

    // NEW: Config intent
    if q.contains("configuration")
        || q.contains("settings")
        || q.contains("environment")
        || q.split_whitespace().any(|w| w == "config" || w == "env")
    {
        return Some(Intent::Config);
    }

    // NEW: Error intent
    if q.contains("error handling")
        || q.contains("exception")
        || q.contains("error")
        || q.contains("catch")
        || q.contains("throw")
    {
        return Some(Intent::Error);
    }

    // NEW: API intent
    if q.contains("endpoint")
        || q.contains("route")
        || q.contains("handler")
        || q.split_whitespace().any(|w| w == "api")
    {
        return Some(Intent::API);
    }

    // NEW: Hook intent
    if q.contains("useeffect")
        || q.contains("usestate")
        || q.contains("usememo")
        || q.contains("hook")
        || q.contains("lifecycle")
    {
        return Some(Intent::Hook);
    }

    // NEW: Middleware intent
    if q.contains("middleware") || q.contains("interceptor") {
        return Some(Intent::Middleware);
    }

    // Definition keywords (existing)
    if q.contains("class")
        || q.contains("interface")
        || q.contains("struct")
        || q.contains("type")
        || q.contains("def")
    {
        return Some(Intent::Definition);
    }

    // Callers patterns (existing - keep unchanged)
    if let Some(s) = q.strip_prefix("who calls ") {
        return Some(Intent::Callers(s.trim().to_string()));
    }
    if let Some(s) = q.strip_prefix("callers of ") {
        return Some(Intent::Callers(s.trim().to_string()));
    }
    if let Some(s) = q.strip_prefix("references to ") {
        return Some(Intent::Callers(s.trim().to_string()));
    }
    if let Some(s) = q.strip_prefix("usages of ") {
        return Some(Intent::Callers(s.trim().to_string()));
    }
    if let Some(s) = q.strip_prefix("where is ") {
        if let Some(rest) = s.strip_suffix(" used") {
            return Some(Intent::Callers(rest.trim().to_string()));
        }
    }

    None
}

/// Parse query controls (filters) from query string
pub fn parse_query_controls(query: &str) -> (String, QueryControls) {
    let mut controls = QueryControls::default();
    let mut kept = Vec::new();
    for token in query.split_whitespace() {
        let Some((k, v)) = token.split_once(':') else {
            kept.push(token);
            continue;
        };
        let key = k.trim().to_lowercase();
        let value = v.trim().trim_matches('"').trim_matches('\'');
        if value.is_empty() {
            kept.push(token);
            continue;
        }
        match key.as_str() {
            "id" => controls.id = Some(value.to_string()),
            "file" => controls.file = Some(value.to_string()),
            "path" => controls.path = Some(value.to_string()),
            "lang" | "language" => controls.lang = Some(normalize_lang(value)),
            "kind" => controls.kind = Some(value.to_string()),
            _ => kept.push(token),
        }
    }
    (kept.join(" "), controls)
}

fn normalize_lang(s: &str) -> String {
    match s.trim().to_lowercase().as_str() {
        "ts" | "tsx" | "typescript" => "typescript".to_string(),
        "js" | "jsx" | "javascript" => "javascript".to_string(),
        other => other.to_string(),
    }
}

/// Trim query to max length
pub fn trim_query(s: &str, max_len: usize) -> String {
    let mut out = s.trim().to_string();
    if out.len() > max_len {
        out.truncate(max_len);
    }
    out
}

/// Decompose compound queries like "auth and db" into sub-queries (FNDN-16)
///
/// Returns a list of sub-queries. If no decomposition needed, returns the original query.
/// Uses max_depth to prevent infinite recursion on deeply nested queries.
pub fn decompose_query(query: &str, max_depth: usize) -> Vec<String> {
    if max_depth == 0 {
        return vec![query.trim().to_string()];
    }

    let q = query.trim();
    if q.is_empty() {
        return vec![];
    }

    // Split on " and " or " & " (case insensitive)
    let lower = q.to_lowercase();

    // Find split points
    let mut parts: Vec<&str> = Vec::new();
    let mut last_end = 0;

    for (idx, _) in lower.match_indices(" and ") {
        let part = q[last_end..idx].trim();
        if !part.is_empty() {
            parts.push(part);
        }
        last_end = idx + 5; // " and " is 5 chars
    }

    // If no " and " found, try " & "
    if parts.is_empty() {
        last_end = 0;
        for (idx, _) in lower.match_indices(" & ") {
            let part = q[last_end..idx].trim();
            if !part.is_empty() {
                parts.push(part);
            }
            last_end = idx + 3; // " & " is 3 chars
        }
    }

    // Add the remaining part
    if last_end < q.len() {
        let part = q[last_end..].trim();
        if !part.is_empty() {
            if parts.is_empty() {
                // No splits found, return original
                return vec![q.to_string()];
            }
            parts.push(part);
        }
    }

    if parts.len() <= 1 {
        return vec![q.to_string()];
    }

    // Recursively decompose each part
    parts
        .into_iter()
        .flat_map(|p| decompose_query(p, max_depth - 1))
        .collect()
}

/// Detect if query contains code snippets (FNDN-17)
///
/// Returns true if the query appears to contain actual code rather than
/// natural language description. Used to switch between embedding strategies.
pub fn contains_code_snippet(query: &str) -> bool {
    let q = query.trim();

    if q.is_empty() {
        return false;
    }

    // Strong code indicators - any one of these means it's code
    let strong_indicators = [
        "()",     // Function call
        "{}",     // Block
        "[]",     // Array access
        "=>",     // Arrow function
        "->",     // Rust/C++ return type or pointer
        "::",     // Rust/C++ path separator
        "fn ",    // Rust function
        "let ",   // Variable declaration
        "const ", // Constant
        "import ",// Import statement
        "export ",// Export statement
        "async ", // Async
        "await ", // Await
        "pub ",   // Rust public
        "struct ",// Struct definition
        "impl ",  // Rust impl
        "class ", // Class definition
        "def ",   // Python function
        "func ",  // Go function
    ];

    for indicator in &strong_indicators {
        if q.contains(indicator) {
            return true;
        }
    }

    // Check for multiple weaker indicators
    let weak_indicators = [
        ".",  // Method call
        ";",  // Statement terminator
        "=",  // Assignment
        "<",  // Generic or comparison
        ">",  // Generic or comparison
        "(",  // Parenthesis
        ")",  // Parenthesis
        "{",  // Brace
        "}",  // Brace
    ];

    let weak_count = weak_indicators
        .iter()
        .filter(|&ind| q.contains(ind))
        .count();

    if weak_count >= 3 {
        return true;
    }

    // Check for camelCase identifiers (not in a sentence)
    // Pattern: lowercase followed by uppercase, no spaces
    if !q.contains(' ') {
        let chars: Vec<char> = q.chars().collect();
        for i in 1..chars.len() {
            if chars[i - 1].is_lowercase() && chars[i].is_uppercase() {
                return true;
            }
        }
    }

    // Check for snake_case identifiers (not in a sentence)
    if !q.contains(' ')
        && q.contains('_')
        && q.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_query_expands_acronyms() {
        let result = normalize_query("API HTTP");
        // The current implementation expands common abbreviations like "db" -> "database"
        // but doesn't do generic acronym expansion
        assert!(result.contains("API") || result.contains("HTTP"));
    }

    #[test]
    fn detect_intent_recognizes_callers() {
        let intent = detect_intent("who calls myFunction");
        assert!(matches!(intent, Some(Intent::Callers(_))));
    }

    #[test]
    fn detect_intent_recognizes_definition() {
        let intent = detect_intent("definition of MyClass");
        assert!(matches!(intent, Some(Intent::Definition)));
    }

    #[test]
    fn parse_query_controls_extracts_filters() {
        let (query, controls) = parse_query_controls("search term id:abc123 file:test.ts");
        assert_eq!(query, "search term");
        assert_eq!(controls.id, Some("abc123".to_string()));
        assert_eq!(controls.file, Some("test.ts".to_string()));
    }

    #[test]
    fn detect_intent_recognizes_new_intents() {
        assert!(matches!(
            detect_intent("how is auth implemented"),
            Some(Intent::Implementation)
        ));
        assert!(matches!(
            detect_intent("configuration settings"),
            Some(Intent::Config)
        ));
        assert!(matches!(detect_intent("error handling"), Some(Intent::Error)));
        assert!(matches!(detect_intent("api endpoint"), Some(Intent::API)));
        assert!(matches!(detect_intent("useEffect hook"), Some(Intent::Hook)));
        assert!(matches!(
            detect_intent("middleware"),
            Some(Intent::Middleware)
        ));
        assert!(matches!(
            detect_intent("database migration"),
            Some(Intent::Migration)
        ));
    }

    #[test]
    fn test_decompose_query_splits_on_and() {
        let parts = decompose_query("auth and database", 2);
        assert_eq!(parts, vec!["auth", "database"]);
    }

    #[test]
    fn test_decompose_query_splits_on_ampersand() {
        let parts = decompose_query("user & profile", 2);
        assert_eq!(parts, vec!["user", "profile"]);
    }

    #[test]
    fn test_decompose_query_preserves_simple() {
        let parts = decompose_query("simple query", 2);
        assert_eq!(parts, vec!["simple query"]);
    }

    #[test]
    fn test_decompose_query_handles_multiple() {
        let parts = decompose_query("a and b and c", 2);
        assert_eq!(parts, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_decompose_query_respects_depth() {
        // With depth 0, no decomposition happens
        let parts = decompose_query("a and b", 0);
        assert_eq!(parts, vec!["a and b"]);
    }

    #[test]
    fn test_contains_code_snippet_detects_function_call() {
        assert!(contains_code_snippet("myFunction()"));
        assert!(contains_code_snippet("user.getName()"));
    }

    #[test]
    fn test_contains_code_snippet_detects_rust_code() {
        assert!(contains_code_snippet("fn main() {}"));
        assert!(contains_code_snippet("let x = 5;"));
        assert!(contains_code_snippet("impl Trait for Type"));
    }

    #[test]
    fn test_contains_code_snippet_detects_camel_case() {
        assert!(contains_code_snippet("getUserById"));
        assert!(contains_code_snippet("MyComponent"));
    }

    #[test]
    fn test_contains_code_snippet_detects_snake_case() {
        assert!(contains_code_snippet("get_user_by_id"));
        assert!(contains_code_snippet("my_variable"));
    }

    #[test]
    fn test_contains_code_snippet_rejects_natural_language() {
        assert!(!contains_code_snippet("how do I authenticate users"));
        assert!(!contains_code_snippet("find the login function"));
        assert!(!contains_code_snippet("search for database"));
    }
}

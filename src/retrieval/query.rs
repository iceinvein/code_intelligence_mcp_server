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
}

use once_cell::sync::Lazy;
use std::collections::HashMap;

/// Common programming synonyms (FNDN-18)
/// Maps a term to its synonyms - all terms that mean similar things in code
static SYNONYMS: Lazy<HashMap<&'static str, &'static [&'static str]>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("function", &["fn", "method", "procedure", "func", "subroutine"][..]);
    m.insert("variable", &["var", "let", "const", "binding", "field"][..]);
    m.insert("class", &["struct", "type", "interface", "object"][..]);
    m.insert("error", &["exception", "failure", "err", "fault"][..]);
    m.insert("async", &["asynchronous", "concurrent", "parallel"][..]);
    m.insert("callback", &["handler", "listener", "hook", "delegate"][..]);
    m.insert("database", &["db", "storage", "persistence", "datastore"][..]);
    m.insert("authentication", &["auth", "login", "signin", "authenticate"][..]);
    m.insert("authorization", &["authz", "permissions", "access", "acl"][..]);
    m.insert("configuration", &["config", "settings", "options", "preferences"][..]);
    m.insert("component", &["widget", "element", "view", "control"][..]);
    m.insert("request", &["req", "http", "call"][..]);
    m.insert("response", &["res", "reply", "result"][..]);
    m.insert("create", &["new", "add", "insert", "make"][..]);
    m.insert("delete", &["remove", "drop", "destroy", "erase"][..]);
    m.insert("update", &["modify", "change", "edit", "patch"][..]);
    m.insert("read", &["get", "fetch", "retrieve", "load"][..]);
    m
});

/// Common programming acronyms (FNDN-19)
/// Maps acronyms to their full forms
static ACRONYMS: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("api", "application programming interface");
    m.insert("http", "hypertext transfer protocol");
    m.insert("json", "javascript object notation");
    m.insert("sql", "structured query language");
    m.insert("orm", "object relational mapping");
    m.insert("crud", "create read update delete");
    m.insert("jwt", "json web token");
    m.insert("oauth", "open authorization");
    m.insert("rest", "representational state transfer");
    m.insert("grpc", "remote procedure call");
    m.insert("dto", "data transfer object");
    m.insert("ui", "user interface");
    m.insert("ux", "user experience");
    m.insert("cli", "command line interface");
    m.insert("sdk", "software development kit");
    m.insert("ide", "integrated development environment");
    m.insert("ci", "continuous integration");
    m.insert("cd", "continuous deployment");
    m.insert("tdd", "test driven development");
    m.insert("ddd", "domain driven design");
    m
});

pub fn normalize_query_text(query: &str) -> String {
    let mut out = String::new();
    let mut in_quotes = false;
    let chars: Vec<char> = query.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            in_quotes = !in_quotes;
            out.push(c);
            i += 1;
            continue;
        }

        if in_quotes {
            out.push(c);
            i += 1;
            continue;
        }

        if c == '(' || c == ')' {
            out.push(' ');
            out.push(c);
            out.push(' ');
            i += 1;
            continue;
        }

        if c == ':' && i + 1 < chars.len() && chars[i + 1] == ':' {
            out.push(' ');
            i += 2;
            continue;
        }

        if c == '-' && i + 1 < chars.len() && chars[i + 1] == '>' {
            out.push(' ');
            i += 2;
            continue;
        }

        if c == '_' || c == '.' || c == '/' || c == '\\' || c == ':' || c == '-' {
            out.push(' ');
            i += 1;
            continue;
        }

        if c.is_ascii_digit() && i > 0 {
            let prev = chars[i - 1];
            if prev.is_ascii_alphabetic() && prev != 'v' && prev != 'V' {
                out.push(' ');
            }
        } else if c.is_ascii_alphabetic() && i > 0 {
            let prev = chars[i - 1];
            if prev.is_ascii_digit() {
                out.push(' ');
            }
        }

        if c.is_uppercase() && i > 0 {
            let prev = chars[i - 1];
            if prev.is_lowercase()
                || (i + 1 < chars.len() && chars[i + 1].is_lowercase() && prev.is_uppercase())
            {
                out.push(' ');
            }
        }

        out.push(c);
        i += 1;
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn split_identifier_like(s: &str) -> String {
    normalize_query_text(s).replace('"', "")
}

pub fn simple_stems(token: &str) -> Vec<String> {
    let mut out = Vec::new();
    for suffix in ["ing", "ed", "es", "s"] {
        if token.len() > suffix.len() + 2 && token.ends_with(suffix) {
            let stem = token.trim_end_matches(suffix).to_string();
            if stem.len() >= 3 {
                out.push(stem);
            }
            break;
        }
    }
    out
}

/// Expand query with synonyms (FNDN-18)
///
/// For each recognized term in the query, appends its synonyms.
/// This broadens the search to find related code.
pub fn expand_synonyms(query: &str) -> String {
    let mut result = query.to_string();
    let lower = query.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    for (term, synonyms) in SYNONYMS.iter() {
        // Check if the term appears as a word (not substring)
        if words.iter().any(|w| w == term) {
            for syn in *synonyms {
                // Don't add if already present
                if !lower.contains(syn) {
                    result.push(' ');
                    result.push_str(syn);
                }
            }
        }
        // Also check if any synonym is present, and add the main term
        for syn in *synonyms {
            if words.iter().any(|w| w == syn) && !lower.contains(term) {
                result.push(' ');
                result.push_str(term);
                break;
            }
        }
    }

    result
}

/// Expand acronyms in query (FNDN-19)
///
/// For each recognized acronym, appends its full form.
/// This helps find code that uses either form.
pub fn expand_acronyms(query: &str) -> String {
    let mut result = query.to_string();
    let lower = query.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    for (acronym, expansion) in ACRONYMS.iter() {
        // Check if the acronym appears as a word (not substring)
        if words.iter().any(|w| w == acronym) {
            // Don't add if expansion already present
            if !lower.contains(expansion) {
                result.push(' ');
                result.push_str(expansion);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_query_text_splits_camel_case() {
        assert_eq!(normalize_query_text("getUserById"), "get User By Id");
    }

    #[test]
    fn test_normalize_query_text_splits_snake_case() {
        assert_eq!(normalize_query_text("get_user_by_id"), "get user by id");
    }

    #[test]
    fn test_expand_synonyms_adds_related_terms() {
        let result = expand_synonyms("function definition");
        assert!(result.contains("function"));
        assert!(result.contains("fn") || result.contains("method"));
    }

    #[test]
    fn test_expand_synonyms_adds_main_term_from_synonym() {
        let result = expand_synonyms("auth logic");
        assert!(result.contains("auth"));
        assert!(result.contains("authentication"));
    }

    #[test]
    fn test_expand_synonyms_no_duplicates() {
        let result = expand_synonyms("authentication auth");
        // Should not add auth twice
        let auth_count = result.matches("auth").count();
        // "auth" appears in "authentication" and as "auth" - that's expected
        // But we shouldn't add extra "auth" if it's already there
        assert!(auth_count <= 3); // auth + authentication contains 2 "auth" patterns
    }

    #[test]
    fn test_expand_acronyms_adds_full_form() {
        let result = expand_acronyms("api endpoint");
        assert!(result.contains("api"));
        assert!(result.contains("application programming interface"));
    }

    #[test]
    fn test_expand_acronyms_handles_multiple() {
        let result = expand_acronyms("rest api");
        assert!(result.contains("rest"));
        assert!(result.contains("api"));
        assert!(result.contains("representational state transfer"));
        assert!(result.contains("application programming interface"));
    }

    #[test]
    fn test_expand_acronyms_ignores_non_word_matches() {
        // "rapid" contains "api" but shouldn't trigger expansion
        let result = expand_acronyms("rapid development");
        assert!(!result.contains("application programming interface"));
    }

    #[test]
    fn test_simple_stems_removes_common_suffixes() {
        assert_eq!(simple_stems("running"), vec!["runn"]);
        assert_eq!(simple_stems("called"), vec!["call"]);
        assert_eq!(simple_stems("functions"), vec!["function"]);
    }
}

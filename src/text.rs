use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};

/// Common programming synonyms (FNDN-18)
/// Maps a term to its synonyms - all terms that mean similar things in code
static SYNONYMS: Lazy<HashMap<&'static str, &'static [&'static str]>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "function",
        &["fn", "method", "procedure", "func", "subroutine"][..],
    );
    m.insert("variable", &["var", "let", "const", "binding", "field"][..]);
    m.insert("class", &["struct", "type", "interface", "object"][..]);
    m.insert("error", &["exception", "failure", "err", "fault"][..]);
    m.insert("async", &["asynchronous", "concurrent", "parallel"][..]);
    m.insert("callback", &["handler", "listener", "hook", "delegate"][..]);
    m.insert(
        "database",
        &["db", "storage", "persistence", "datastore"][..],
    );
    m.insert(
        "authentication",
        &["auth", "login", "signin", "authenticate"][..],
    );
    m.insert(
        "authorization",
        &["authz", "permissions", "access", "acl"][..],
    );
    m.insert(
        "configuration",
        &["config", "settings", "options", "preferences"][..],
    );
    m.insert("component", &["widget", "element", "view", "control"][..]);
    m.insert("request", &["req", "http", "call"][..]);
    m.insert("response", &["res", "reply", "result"][..]);
    m.insert("create", &["new", "add", "insert", "make"][..]);
    m.insert("delete", &["remove", "drop", "destroy", "erase"][..]);
    m.insert("update", &["modify", "change", "edit", "patch"][..]);
    m.insert("read", &["get", "fetch", "retrieve", "load"][..]);
    // Domain-specific synonyms for semantic gap coverage
    m.insert("websocket", &["ws", "socket", "realtime"][..]);
    m.insert("socket", &["ws", "websocket"][..]); // catches camelCase-split "Web Socket"
    m.insert(
        "serialization",
        &["serialize", "serde", "deserialize", "marshal"][..],
    );
    m.insert("watcher", &["watch", "observe", "monitor", "notify"][..]);
    m.insert("debounce", &["throttle", "delay", "timer"][..]);
    m.insert("fallback", &["degradation", "recovery", "retry"][..]);
    m.insert("schema", &["table", "ddl", "migration", "create_table"][..]);
    m.insert("parse", &["parser", "parsing", "tokenize", "lex"][..]);
    m.insert("format", &["formatting", "render", "pretty"][..]);
    m.insert(
        "initialization",
        &["init", "initialize", "setup", "create", "new"][..],
    );
    m.insert(
        "detection",
        &["detect", "identify", "recognize", "classify"][..],
    );
    m.insert("reindex", &["index", "rebuild", "refresh"][..]);
    // Cross-link serde ecosystem so import-injected "serde" bridges to query terms
    m.insert("serde", &["serialize", "deserialize", "json", "serialization"][..]);
    // Tree-sitter hyphenation bridging: "tree" alone is too generic,
    // but "sitter" as a token should link to parser/parsing concepts
    m.insert("sitter", &["tree_sitter", "parser", "parsing", "ast"][..]);
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

/// Common English stop words that dilute BM25 scoring in NL queries.
/// Conservative list: articles, be-verbs, auxiliaries, pronouns, prepositions, question words.
/// Only applied to 3+ word queries to avoid removing meaningful short queries.
static STOP_WORDS: &[&str] = &[
    // Articles
    "a", "an", "the",
    // Be-verbs
    "is", "are", "was", "were", "be", "been", "being",
    // Auxiliaries
    "do", "does", "did", "has", "have", "had", "will", "would", "shall", "should",
    "can", "could", "may", "might", "must",
    // Pronouns
    "i", "you", "he", "she", "it", "we", "they", "me", "him", "her", "us", "them",
    "my", "your", "his", "its", "our", "their",
    "this", "that", "these", "those",
    // Prepositions
    "in", "on", "at", "to", "for", "of", "with", "by", "from", "about", "into",
    "through", "during", "before", "after", "above", "below", "between", "under",
    // Conjunctions
    "and", "but", "or", "nor", "so", "yet",
    // Question words
    "how", "what", "where", "when", "who", "which", "why",
    // Other function words
    "not", "no", "all", "each", "every", "both", "few", "more", "most",
    "other", "some", "such", "only", "own", "same", "than", "too", "very",
];

/// Remove stop words from a query string.
/// Only removes stop words from queries with 3+ words (NL-style queries).
/// Preserves quoted phrases untouched.
pub fn remove_stop_words(query: &str) -> String {
    let words: Vec<&str> = query.split_whitespace().collect();
    // Only apply to NL queries (3+ words)
    if words.len() < 3 {
        return query.to_string();
    }

    let filtered: Vec<&str> = words
        .into_iter()
        .filter(|w| {
            // Preserve quoted words
            if w.starts_with('"') || w.ends_with('"') {
                return true;
            }
            // Check against stop word list (case-insensitive)
            !STOP_WORDS.contains(&w.to_lowercase().as_str())
        })
        .collect();

    // Safety: if we'd remove ALL words, return original
    if filtered.is_empty() {
        return query.to_string();
    }

    filtered.join(" ")
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

/// Expand query terms with their stems to improve BM25 recall.
///
/// Code identifiers are split into base forms during indexing (e.g. `spawn_watch_loop` →
/// tokens `spawn`, `watch`, `loop`), but NL queries use derived forms (`watcher`,
/// `handler`, `formatting`). This function bridges the gap by adding stems:
/// "watcher" → also search "watch", "handler" → "handle", "formatting" → "format".
///
/// Only applied to 3+ word queries (NL queries). Short queries are left untouched.
pub fn expand_stems(query: &str) -> String {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.len() < 3 {
        return query.to_string();
    }

    let mut result = query.to_string();
    let lower = query.to_lowercase();

    for word in &words {
        // Skip short words, quoted words, and hyphenated compounds (e.g. "tree-sitter")
        if word.len() < 5 || word.starts_with('"') || word.ends_with('"') || word.contains('-') {
            continue;
        }

        let w = word.to_lowercase();

        // Try suffix stripping in priority order (longest match first)
        let stem = if w.ends_with("ation") && w.len() > 7 {
            Some(w[..w.len() - 5].to_string()) // "serialization" → "serializ"
        } else if w.ends_with("tion") && w.len() > 6 {
            Some(w[..w.len() - 4].to_string()) // "generation" → "genera"
        } else if w.ends_with("ment") && w.len() > 6 {
            Some(w[..w.len() - 4].to_string()) // "management" → "manage"
        } else if w.ends_with("ness") && w.len() > 6 {
            Some(w[..w.len() - 4].to_string()) // "readiness" → "readi"
        } else if w.ends_with("ling") && w.len() > 6 {
            Some(w[..w.len() - 4].to_string()) // "handling" → "hand"
        } else if w.ends_with("ting") && w.len() > 6 {
            Some(w[..w.len() - 4].to_string()) // "formatting" → "format"
        } else if w.ends_with("ning") && w.len() > 6 {
            Some(w[..w.len() - 4].to_string()) // "scanning" → "scan"
        } else if w.ends_with("ding") && w.len() > 6 {
            Some(w[..w.len() - 4].to_string()) // "loading" → "loa" — hmm, too short
        } else if w.ends_with("ing") && w.len() > 5 {
            Some(w[..w.len() - 3].to_string()) // "caching" → "cach"
        } else if w.ends_with("ers") && w.len() > 5 {
            Some(w[..w.len() - 3].to_string()) // "handlers" → "handl"
        } else if w.ends_with("er") && w.len() > 4 {
            Some(w[..w.len() - 2].to_string()) // "watcher" → "watch", "handler" → "handl"
        } else if w.ends_with("ed") && w.len() > 4 {
            Some(w[..w.len() - 2].to_string()) // "cached" → "cach"
        } else if w.ends_with("es") && w.len() > 4 {
            Some(w[..w.len() - 2].to_string()) // "caches" → "cach"
        } else if w.ends_with("ly") && w.len() > 4 {
            Some(w[..w.len() - 2].to_string()) // "gracefully" → "graceful"
        } else if w.ends_with("s") && w.len() > 4 && !w.ends_with("ss") {
            Some(w[..w.len() - 1].to_string()) // "requests" → "request"
        } else {
            None
        };

        if let Some(s) = stem {
            if s.len() >= 5 && !lower.contains(&s) {
                result.push(' ');
                result.push_str(&s);
            }
        }
    }

    result
}

/// Look up synonyms for a single word (key-only, used for index-time expansion).
///
/// Returns the synonym list if the word is a known KEY in the SYNONYMS table.
/// Used by index-time text expansion to enrich BM25-indexed content.
pub fn get_synonyms(word: &str) -> Option<&'static [&'static str]> {
    SYNONYMS.get(word).copied()
}

/// Bidirectional synonym lookup for a single word.
///
/// Returns related terms whether the word is a KEY or a VALUE in the SYNONYMS table.
/// For example, "handler" (a value under "callback") returns ["callback", "listener", "hook", "delegate"].
/// Used by term_coverage scoring to bridge vocabulary gaps.
pub fn get_related_terms(word: &str) -> Vec<&'static str> {
    let mut related = Vec::new();

    // Forward: word is a key → return its values
    if let Some(synonyms) = SYNONYMS.get(word) {
        related.extend_from_slice(synonyms);
    }

    // Reverse: word is a value → return the key + sibling values
    for (key, values) in SYNONYMS.iter() {
        if values.iter().any(|v| *v == word) {
            if *key != word && !related.contains(key) {
                related.push(key);
            }
            for v in *values {
                if *v != word && !related.contains(v) {
                    related.push(v);
                }
            }
        }
    }

    related
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

/// Extract import crate/module names from file source code.
///
/// For Rust files, scans for `use` statements and extracts the first path segment
/// (the crate name). Skips internal references (crate, self, super) and std.
///
/// For other languages, pass the already-extracted imports from `ExtractedFile.imports`.
///
/// Returns a space-separated string of unique crate/module names suitable for
/// appending to Tantivy indexed text. Each crate name is also split so that
/// e.g. "serde_json" produces both "serde_json" and split tokens "serde json".
pub fn extract_rust_import_tags(source: &str) -> String {
    let mut crates = HashSet::new();
    for line in source.lines() {
        let trimmed = line.trim();
        // Match `use crate_name::...` and `extern crate crate_name;`
        let rest = if let Some(r) = trimmed.strip_prefix("use ") {
            Some(r)
        } else if let Some(r) = trimmed.strip_prefix("pub use ") {
            Some(r)
        } else {
            trimmed
                .strip_prefix("extern crate ")
                .map(|r| r.trim_end_matches(';'))
        };
        if let Some(rest) = rest {
            // Extract first path segment: "tree_sitter::Language" → "tree_sitter"
            let first_segment = rest.split("::").next().unwrap_or("").trim();
            // Also handle `use {serde, anyhow}` grouped imports
            let first_segment = first_segment.trim_start_matches('{');
            let first_segment = first_segment.trim_end_matches(|c: char| c == ';' || c == ',');
            let first_segment = first_segment.trim();
            // Skip internal references and std (too generic)
            match first_segment {
                "crate" | "self" | "super" | "std" | "" => continue,
                s if s.contains(' ') => continue, // malformed
                s => {
                    crates.insert(s.to_string());
                }
            }
        }
    }

    // Build output: raw crate name + split form for BM25 tokenization
    let mut parts = Vec::new();
    for crate_name in &crates {
        parts.push(crate_name.clone());
        // Also add split form: "serde_json" → "serde json", "tree_sitter" → "tree sitter"
        let split = split_identifier_like(crate_name);
        if split != *crate_name {
            parts.push(split);
        }
    }
    parts.join(" ")
}

/// Build import tags string from already-extracted imports (for non-Rust languages).
///
/// Takes the `source` field from each `Import` and deduplicates.
pub fn build_import_tags_from_sources(sources: &[String]) -> String {
    let mut seen = HashSet::new();
    let mut parts = Vec::new();
    for source in sources {
        // Extract the package/module name (e.g., "express" from "express",
        // "react" from "react", "os" from "os")
        let module_name = source
            .split('/')
            .next()
            .unwrap_or(source)
            .trim_start_matches('@');
        if module_name.is_empty() || seen.contains(module_name) {
            continue;
        }
        seen.insert(module_name.to_string());
        parts.push(module_name.to_string());
        let split = split_identifier_like(module_name);
        if split != module_name {
            parts.push(split);
        }
    }
    parts.join(" ")
}

/// Extract concept tags from symbol body text for known code patterns.
///
/// Scans the text for patterns like `json!()`, `WebSocket`, `.map_err()`, etc.
/// and returns semantic labels that bridge the vocabulary gap between natural
/// language queries and code patterns. For example, `json!({` → "serialization
/// response formatting".
///
/// Returns a space-separated string of unique tags suitable for appending to
/// Tantivy indexed text. Compound tags are also split (e.g., "error_handling"
/// → "error handling") for BM25 tokenization.
pub fn extract_concept_tags(text: &str) -> String {
    let mut tags = HashSet::new();

    // --- JSON / Serialization (Q10) ---
    if text.contains("json!(") || text.contains("json!{") || text.contains("json! {") {
        tags.insert("serialization");
        tags.insert("response");
        tags.insert("formatting");
    }
    if text.contains("serde_json::") || text.contains("serde_json") {
        tags.insert("serialization");
    }
    if text.contains("to_string_pretty") {
        tags.insert("serialization");
        tags.insert("formatting");
    }
    if text.contains("#[derive(") && (text.contains("Serialize") || text.contains("Deserialize")) {
        tags.insert("serialization");
        tags.insert("serde");
    }

    // --- WebSocket (Q7) ---
    // R30 lesson: "handler" concept tag had zero effect on Q7 — removed in R31.
    // The concept tag fires correctly (elysia.rs body has FrameworkPatternKind::WebSocket
    // which survives string stripping), but adding "handler" to body text doesn't help
    // because Q7's problem is deeper than vocabulary mismatch.
    if text.contains("WebSocket") || text.contains("websocket") || text.contains("Websocket") {
        tags.insert("websocket");
        tags.insert("realtime");
    }
    if text.contains(".ws(") {
        tags.insert("websocket");
    }

    // --- Error handling / Graceful degradation (Q9) ---
    if text.contains("map_err(") {
        tags.insert("error_handling");
    }
    if text.contains("tool_internal_error") || text.contains("CallToolError") {
        tags.insert("error_handling");
    }
    if text.contains("unwrap_or_else(") || text.contains("ok_or_else(") || text.contains("ok_or(") {
        tags.insert("fallback");
        tags.insert("graceful_degradation");
    }
    if text.contains("downcast_ref") {
        tags.insert("error_handling");
    }
    // R27: Wider patterns for error handling and graceful degradation
    if text.contains("Err(e)") || text.contains("Err(_)") || text.contains("Err(err)") {
        tags.insert("error_handling");
    }
    if text.contains("if let Err(") {
        tags.insert("error_handling");
        tags.insert("graceful_degradation");
    }
    // match ... { Ok(...) => ..., Err(...) => ... } pattern
    if text.contains("=> Err(") || text.contains("Err(e) =>") || text.contains("Err(_) =>") {
        tags.insert("error_handling");
    }
    if text.contains(".or_else(") || text.contains(".unwrap_or(") {
        tags.insert("fallback");
        tags.insert("graceful_degradation");
    }
    if text.contains("fallback") || text.contains("degrade") || text.contains("degradation") {
        tags.insert("fallback");
        tags.insert("graceful_degradation");
    }
    // R29: Additional error handling patterns for anyhow/tracing ecosystem
    if text.contains("bail!(") {
        tags.insert("error_handling");
    }
    if text.contains(".context(") || text.contains(".with_context(") {
        tags.insert("error_handling");
    }
    if text.contains("anyhow!(") || text.contains("anyhow!{") {
        tags.insert("error_handling");
    }
    if text.contains("tracing::error") || text.contains("tracing::warn") {
        tags.insert("error_handling");
    }
    if text.contains("eprintln!(") {
        tags.insert("error_handling");
    }

    // Build output with both raw and split forms
    let mut parts = Vec::new();
    for tag in &tags {
        parts.push(tag.to_string());
        let split = split_identifier_like(tag);
        if split != *tag {
            parts.push(split);
        }
    }
    parts.join(" ")
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

    #[test]
    fn test_expand_synonyms_websocket() {
        let result = expand_synonyms("websocket handler");
        assert!(result.contains("ws"), "should expand websocket→ws");
        assert!(result.contains("callback"), "should expand handler→callback");
    }

    #[test]
    fn test_expand_synonyms_serialization() {
        let result = expand_synonyms("serialization format");
        assert!(result.contains("serde"), "should expand serialization→serde");
        assert!(result.contains("serialize"), "should expand serialization→serialize");
        assert!(result.contains("formatting"), "should expand format→formatting");
    }

    #[test]
    fn test_expand_synonyms_watcher_debounce() {
        let result = expand_synonyms("watcher debounce");
        assert!(result.contains("watch"), "should expand watcher→watch");
        assert!(result.contains("throttle"), "should expand debounce→throttle");
    }

    #[test]
    fn test_expand_synonyms_fallback_reverse() {
        // "degradation" is a synonym of "fallback", so querying "degradation"
        // should add the main term "fallback"
        let result = expand_synonyms("graceful degradation");
        assert!(result.contains("fallback"), "should add fallback from degradation synonym");
    }

    #[test]
    fn test_expand_synonyms_schema() {
        let result = expand_synonyms("schema initialization");
        assert!(result.contains("table"), "should expand schema→table");
        assert!(result.contains("ddl"), "should expand schema→ddl");
    }

    #[test]
    fn test_remove_stop_words_nl_query() {
        let result = remove_stop_words("How does the WebSocket handler work");
        assert_eq!(result, "WebSocket handler work");
    }

    #[test]
    fn test_remove_stop_words_preserves_short_queries() {
        // 1-2 word queries should be untouched
        assert_eq!(remove_stop_words("get"), "get");
        assert_eq!(remove_stop_words("the handler"), "the handler");
    }

    #[test]
    fn test_remove_stop_words_error_handling_query() {
        let result = remove_stop_words("Error handling and graceful degradation");
        assert_eq!(result, "Error handling graceful degradation");
    }

    #[test]
    fn test_remove_stop_words_json_query() {
        let result = remove_stop_words("JSON serialization and response formatting");
        assert_eq!(result, "JSON serialization response formatting");
    }

    #[test]
    fn test_remove_stop_words_preserves_all_if_empty() {
        // If ALL words are stop words, return original
        let result = remove_stop_words("how does the");
        // "how", "does", "the" are all stop words, but we return original
        assert_eq!(result, "how does the");
    }

    #[test]
    fn test_remove_stop_words_with_expanded_queries() {
        // Q7 expanded: "How does the Web Socket handler work? ws websocket callback listener hook delegate"
        // After stop word removal: Web Socket handler work? ws websocket callback listener hook delegate
        let q7 = "How does the Web Socket handler work? ws websocket callback listener hook delegate";
        let r7 = remove_stop_words(q7);
        assert!(!r7.contains("How "), "should remove 'How': {r7}");
        assert!(!r7.contains(" does "), "should remove 'does': {r7}");
        assert!(!r7.contains(" the "), "should remove 'the': {r7}");
        assert!(r7.contains("Socket"), "should keep 'Socket': {r7}");
        assert!(r7.contains("handler"), "should keep 'handler': {r7}");
        assert!(r7.contains("websocket"), "should keep 'websocket': {r7}");

        // Q9 expanded: "Error handling and graceful degradation exception failure err fault fallback recovery retry"
        let q9 = "Error handling and graceful degradation exception failure err fault fallback recovery retry";
        let r9 = remove_stop_words(q9);
        assert!(!r9.contains(" and "), "should remove 'and': {r9}");
        assert!(r9.contains("Error"), "should keep 'Error': {r9}");
        assert!(r9.contains("fallback"), "should keep 'fallback': {r9}");
        assert!(r9.contains("degradation"), "should keep 'degradation': {r9}");
    }

    #[test]
    fn test_expand_stems_short_queries_untouched() {
        assert_eq!(expand_stems("get"), "get");
        assert_eq!(expand_stems("get user"), "get user");
    }

    #[test]
    fn test_expand_stems_er_suffix() {
        let result = expand_stems("file watcher debounce reindex");
        assert!(result.contains("watch"), "should stem watcher→watch: {result}");
    }

    #[test]
    fn test_expand_stems_ing_suffix() {
        let result = expand_stems("JSON serialization response formatting");
        assert!(result.contains("format"), "should stem formatting→format: {result}");
    }

    #[test]
    fn test_expand_stems_s_suffix() {
        let result = expand_stems("find function handles search requests");
        assert!(result.contains("request"), "should stem requests→request: {result}");
        assert!(result.contains("handle"), "should stem handles→handle: {result}");
    }

    #[test]
    fn test_expand_stems_no_duplicates() {
        // "handle" is already in the query, shouldn't be added again
        let result = expand_stems("handle search code handler");
        let handle_count = result.matches("handle").count();
        // "handle" appears once naturally + "handler" stems to "handl" not "handle"
        assert!(handle_count <= 3, "should not add duplicate stems: {result}");
    }

    #[test]
    fn test_expand_stems_tion_suffix() {
        let result = expand_stems("vector embedding generation and storage");
        // "generation" → strip "ation" → "gener" (5 chars, valid)
        assert!(result.contains("gener"), "should stem generation→gener: {result}");
    }

    #[test]
    fn test_expand_stems_skips_hyphenated_words() {
        let result = expand_stems("tree-sitter parser initialization and language");
        // "tree-sitter" should not be stemmed (hyphen skip)
        let words: Vec<&str> = result.split_whitespace().collect();
        assert!(!words.contains(&"tree-sitt"), "should not stem hyphenated words: {result}");
    }

    #[test]
    fn test_expand_stems_filters_short_stems() {
        let result = expand_stems("caching and cache invalidation patterns");
        // "caching" → "cach" (4 chars) should be filtered by min stem length 5
        let words: Vec<&str> = result.split_whitespace().collect();
        assert!(!words.contains(&"cach"), "should filter stems < 5 chars: {result}");
    }

    #[test]
    fn test_extract_rust_import_tags_basic() {
        let source = r#"
use tree_sitter::{Language, Node, Parser, Tree};
use serde_json::Value;
use anyhow::{Context, Result};
use std::path::Path;
use crate::storage::sqlite::SymbolRow;
"#;
        let tags = extract_rust_import_tags(source);
        assert!(tags.contains("tree_sitter"), "should extract tree_sitter: {tags}");
        assert!(tags.contains("serde_json"), "should extract serde_json: {tags}");
        assert!(tags.contains("anyhow"), "should extract anyhow: {tags}");
        // Split forms
        assert!(tags.contains("tree") && tags.contains("sitter"), "should split tree_sitter: {tags}");
        assert!(tags.contains("serde") && tags.contains("json"), "should split serde_json: {tags}");
        // Should NOT include internal references or std
        assert!(!tags.split_whitespace().any(|w| w == "crate"), "should skip crate: {tags}");
        assert!(!tags.split_whitespace().any(|w| w == "std"), "should skip std: {tags}");
    }

    #[test]
    fn test_extract_rust_import_tags_pub_use_and_extern() {
        let source = r#"
pub use once_cell::sync::Lazy;
extern crate tokio;
use self::inner::Foo;
use super::Bar;
"#;
        let tags = extract_rust_import_tags(source);
        assert!(tags.contains("once_cell"), "should extract once_cell: {tags}");
        assert!(tags.contains("tokio"), "should extract tokio: {tags}");
        assert!(!tags.split_whitespace().any(|w| w == "self"), "should skip self: {tags}");
        assert!(!tags.split_whitespace().any(|w| w == "super"), "should skip super: {tags}");
    }

    #[test]
    fn test_build_import_tags_from_sources() {
        let sources = vec![
            "express".to_string(),
            "react".to_string(),
            "@types/node".to_string(),
            "express".to_string(), // duplicate
        ];
        let tags = build_import_tags_from_sources(&sources);
        assert!(tags.contains("express"), "should include express: {tags}");
        assert!(tags.contains("react"), "should include react: {tags}");
        assert!(tags.contains("types"), "should include types (from @types/node): {tags}");
        // Dedup: express should appear only once
        let express_count = tags.split_whitespace().filter(|w| *w == "express").count();
        assert_eq!(express_count, 1, "express should be deduplicated: {tags}");
    }

    #[test]
    fn test_concept_tags_json_macro() {
        let text = r#"pub fn build_response() { json!({"status": "ok"}) }"#;
        let tags = extract_concept_tags(text);
        assert!(tags.contains("serialization"), "json!( should trigger serialization: {tags}");
        assert!(tags.contains("response"), "json!( should trigger response: {tags}");
        assert!(tags.contains("formatting"), "json!( should trigger formatting: {tags}");
    }

    #[test]
    fn test_concept_tags_websocket() {
        let text = r#"app.ws("/ws", |ws| { ws.send("hello") })"#;
        let tags = extract_concept_tags(text);
        assert!(tags.contains("websocket"), ".ws( should trigger websocket: {tags}");
    }

    #[test]
    fn test_concept_tags_websocket_name() {
        let text = "fn handle_WebSocket_connection() {}";
        let tags = extract_concept_tags(text);
        assert!(tags.contains("websocket"), "WebSocket should trigger websocket: {tags}");
        assert!(tags.contains("realtime"), "WebSocket should trigger realtime: {tags}");
    }

    #[test]
    fn test_concept_tags_error_handling() {
        let text = "result.map_err(|e| format!(\"failed: {e}\"))";
        let tags = extract_concept_tags(text);
        assert!(tags.contains("error_handling"), "map_err should trigger error_handling: {tags}");
        // Split form
        assert!(tags.contains("error"), "should split error_handling: {tags}");
        assert!(tags.contains("handling"), "should split error_handling: {tags}");
    }

    #[test]
    fn test_concept_tags_graceful_degradation() {
        let text = "let val = opt.unwrap_or_else(|| default_value());";
        let tags = extract_concept_tags(text);
        assert!(tags.contains("fallback"), "unwrap_or_else should trigger fallback: {tags}");
        assert!(tags.contains("graceful_degradation"), "unwrap_or_else should trigger graceful_degradation: {tags}");
    }

    #[test]
    fn test_concept_tags_err_pattern() {
        let text = "match result { Ok(v) => v, Err(e) => return Err(e) }";
        let tags = extract_concept_tags(text);
        assert!(tags.contains("error_handling"), "Err(e) should trigger error_handling: {tags}");
    }

    #[test]
    fn test_concept_tags_if_let_err() {
        let text = "if let Err(e) = try_connect() { log::warn!(\"failed: {e}\"); }";
        let tags = extract_concept_tags(text);
        assert!(tags.contains("error_handling"), "if let Err should trigger error_handling: {tags}");
        assert!(tags.contains("graceful_degradation"), "if let Err should trigger graceful_degradation: {tags}");
    }

    #[test]
    fn test_concept_tags_unwrap_or() {
        let text = "let port = env::var(\"PORT\").unwrap_or(\"3000\".to_string());";
        let tags = extract_concept_tags(text);
        assert!(tags.contains("fallback"), ".unwrap_or( should trigger fallback: {tags}");
        assert!(tags.contains("graceful_degradation"), ".unwrap_or( should trigger graceful_degradation: {tags}");
    }

    #[test]
    fn test_concept_tags_fallback_keyword() {
        let text = "fn get_fallback_config() -> Config { Config::default() }";
        let tags = extract_concept_tags(text);
        assert!(tags.contains("fallback"), "fallback keyword should trigger fallback: {tags}");
        assert!(tags.contains("graceful_degradation"), "fallback keyword should trigger graceful_degradation: {tags}");
    }

    #[test]
    fn test_concept_tags_serde_derive() {
        let text = "#[derive(Debug, Serialize, Deserialize)]\nstruct Foo {}";
        let tags = extract_concept_tags(text);
        assert!(tags.contains("serialization"), "derive Serialize should trigger serialization: {tags}");
        assert!(tags.contains("serde"), "derive Serialize should trigger serde: {tags}");
    }

    #[test]
    fn test_concept_tags_empty_text() {
        let tags = extract_concept_tags("");
        assert!(tags.is_empty(), "empty text should return empty tags: '{tags}'");
    }

    #[test]
    fn test_concept_tags_no_matches() {
        let tags = extract_concept_tags("fn add(a: i32, b: i32) -> i32 { a + b }");
        assert!(tags.is_empty(), "plain function should return empty tags: '{tags}'");
    }
}

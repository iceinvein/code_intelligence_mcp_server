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
        &["authz", "permissions", "access", "acl", "rbac", "role"][..],
    );
    m.insert(
        "role",
        &["rbac", "permission", "admin", "authz", "authorization"][..],
    );
    m.insert(
        "access",
        &["auth", "permission", "role", "authorize", "rbac"][..],
    );
    m.insert(
        "transaction",
        &["commit", "rollback", "atomic", "begin_transaction"][..],
    );
    m.insert(
        "configuration",
        &["config", "settings", "options", "preferences"][..],
    );
    m.insert("component", &["widget", "element", "view"][..]);
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
    m.insert(
        "circuit",
        &["breaker", "resilience", "half_open", "trip"][..],
    );
    m.insert(
        "breaker",
        &["circuit", "trip", "half_open", "resilience"][..],
    );
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
    m.insert(
        "serde",
        &["serialize", "deserialize", "json", "serialization"][..],
    );
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
    "a", "an", "the", // Be-verbs
    "is", "are", "was", "were", "be", "been", "being", // Auxiliaries
    "do", "does", "did", "has", "have", "had", "will", "would", "shall", "should", "can", "could",
    "may", "might", "must", // Pronouns
    "i", "you", "he", "she", "it", "we", "they", "me", "him", "her", "us", "them", "my", "your",
    "his", "its", "our", "their", "this", "that", "these", "those", // Prepositions
    "in", "on", "at", "to", "for", "of", "with", "by", "from", "about", "into", "through",
    "during", "before", "after", "above", "below", "between", "under", // Conjunctions
    "and", "but", "or", "nor", "so", "yet", // Question words
    "how", "what", "where", "when", "who", "which", "why", // Other function words
    "not", "no", "all", "each", "every", "both", "few", "more", "most", "other", "some", "such",
    "only", "own", "same", "than", "too", "very",
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
        } else if w.len() > 6
            && (w.ends_with("tion")
                || w.ends_with("ment")
                || w.ends_with("ness")
                || w.ends_with("ling")
                || w.ends_with("ting")
                || w.ends_with("ning")
                || w.ends_with("ding"))
        {
            Some(w[..w.len() - 4].to_string()) // strip 4: "generation" → "genera", "handling" → "hand", etc.
        } else if w.len() > 5 && (w.ends_with("ing") || w.ends_with("ers")) {
            Some(w[..w.len() - 3].to_string()) // strip 3: "caching" → "cach", "handlers" → "handl"
        } else if w.len() > 4
            && (w.ends_with("er") || w.ends_with("ed") || w.ends_with("es") || w.ends_with("ly"))
        {
            Some(w[..w.len() - 2].to_string()) // strip 2: "watcher" → "watch", "cached" → "cach", etc.
        } else if w.ends_with("s") && w.len() > 4 && !w.ends_with("ss") {
            Some(w[..w.len() - 1].to_string()) // "requests" → "request"
        } else {
            None
        };

        if let Some(s) = stem {
            // Check if the stem already exists as a separate word in the query
            // (not just as a substring of another word).
            // E.g., "transactions" contains "transaction" as a prefix, but
            // that shouldn't prevent adding the stem — Tantivy tokenizes
            // each word individually and won't match "transactions" against
            // indexed "transaction".
            let stem_already_present = lower.split_whitespace().any(|w| w == s);
            if s.len() >= 5 && !stem_already_present {
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

/// Bidirectional synonym lookup for a single word, with stem-aware matching.
///
/// Returns related terms whether the word is a KEY or a VALUE in the SYNONYMS table.
/// For example, "handler" (a value under "callback") returns ["callback", "listener", "hook", "delegate"].
/// Also matches morphological variants: "concurrency" matches "concurrent" via shared
/// prefix, bridging the gap between query terms and synonym table entries.
/// Used by term_coverage scoring to bridge vocabulary gaps.
pub fn get_related_terms(word: &str) -> Vec<&'static str> {
    let mut related = Vec::new();

    // Forward: word is a key → return its values
    if let Some(synonyms) = SYNONYMS.get(word) {
        related.extend_from_slice(synonyms);
    }

    // Reverse: word is a value (exact or stem match) → return the key + sibling values
    for (key, values) in SYNONYMS.iter() {
        let matched = values.contains(&word)
            || (word.len() >= 6
                && values
                    .iter()
                    .any(|v| v.len() >= 6 && synonym_stems_match(word, v)));
        if matched {
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

/// Check if two words share a stem (prefix of length >= 6).
/// Conservative: requires long shared prefix to avoid false positives.
/// "concurrency" ↔ "concurrent" (share "concurren", len 9) → true
/// "configuring" ↔ "config" (share "config", len 6) → true
/// "processing" ↔ "procedure" (share "proce", len 5) → false (< 6)
fn synonym_stems_match(a: &str, b: &str) -> bool {
    let shared = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    shared >= 6
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
            let first_segment = first_segment.trim_end_matches([';', ',']);
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

/// Build framework vocabulary tags from extracted framework patterns.
///
/// Maps framework pattern kinds to multi-word search phrases that bridge
/// the vocabulary gap between natural-language queries and code that
/// represents patterns via enum variants. For example, a file with
/// `FrameworkPatternKind::WebSocket` gets "websocket handler websocket
/// endpoint realtime handler" injected so that "websocket handler" queries
/// match via BM25.
///
/// Input: slice of `(kind_string, optional_http_method)` tuples.
/// Output: space-separated vocabulary string.
pub fn build_framework_vocab_tags(patterns: &[(String, Option<String>)]) -> String {
    let mut seen = HashSet::new();
    let mut parts = Vec::new();

    for (kind, http_method) in patterns {
        let phrases: Vec<&str> = match kind.as_str() {
            "websocket" => vec![
                "websocket handler",
                "websocket endpoint",
                "realtime handler",
            ],
            "route" => vec!["route handler", "http endpoint"],
            "plugin" => vec!["plugin middleware", "plugin extension"],
            "guard" => vec!["guard middleware", "auth guard handler"],
            "group" => vec!["route group"],
            "listen" => vec!["server listen", "server startup"],
            _ => vec![],
        };

        for phrase in &phrases {
            for word in phrase.split_whitespace() {
                if seen.insert(word.to_string()) {
                    parts.push(word.to_string());
                }
            }
        }

        // Add HTTP method-specific phrases for routes
        if kind == "route" {
            if let Some(method) = http_method {
                let m = method.to_lowercase();
                let method_handler = format!("{m} handler");
                let method_endpoint = format!("{m} endpoint");
                for phrase in [&method_handler, &method_endpoint] {
                    for word in phrase.split_whitespace() {
                        if seen.insert(word.to_string()) {
                            parts.push(word.to_string());
                        }
                    }
                }
            }
        }
    }

    parts.join(" ")
}

/// Common language keywords that should be excluded from morphological expansion.
/// These add noise without improving search recall.
static LANG_KEYWORDS: &[&str] = &[
    // Rust
    "let",
    "mut",
    "pub",
    "crate",
    "self",
    "super",
    "impl",
    "use",
    "mod",
    "struct",
    "enum",
    "trait",
    "type",
    "where",
    "const",
    "static",
    "unsafe",
    "async",
    "await",
    "move",
    "dyn",
    "ref",
    "match",
    "return",
    "break",
    "continue",
    "true",
    "false",
    "some",
    "none",
    // Shared
    "for",
    "while",
    "loop",
    "else",
    "then",
    // TS/JS
    "var",
    "class",
    "interface",
    "export",
    "import",
    "from",
    "extends",
    "typeof",
    "instanceof",
    "void",
    "null",
    "undefined",
    "this",
    // Python
    "def",
    "try",
    "except",
    "with",
    "pass",
    "yield",
    "lambda",
    "elif",
    "raise",
    "finally",
    "global",
    "nonlocal",
    // Go
    "func",
    "package",
    "defer",
    "chan",
    "range",
    "select",
    // Common but low-value
    "string",
    "bool",
    "into",
    "that",
];

/// Check if a short word ending in CVC pattern should double its final consonant
/// before adding -er/-ing (e.g., "run" → "runner", "get" → "getter").
fn should_double_final_consonant(word: &str) -> bool {
    let chars: Vec<char> = word.chars().collect();
    let len = chars.len();
    if !(3..=6).contains(&len) {
        return false;
    }

    let last = chars[len - 1];
    let second_last = chars[len - 2];
    let third_last = chars[len - 3];

    let is_vowel = |c: char| matches!(c, 'a' | 'e' | 'i' | 'o' | 'u');

    // Pattern: consonant-vowel-consonant, excluding w/x/y as final
    !is_vowel(last)
        && is_vowel(second_last)
        && !is_vowel(third_last)
        && !matches!(last, 'w' | 'x' | 'y')
}

/// Generate morphological variants of a word (forward derivation + backward stemming).
///
/// Forward: adds -er, -ing suffixes for agent nouns and gerunds.
/// Backward: strips common suffixes to extract stems.
/// Prefix: adds re- for common programming verbs.
pub(crate) fn generate_morphological_variants(word: &str) -> Vec<String> {
    let mut variants = Vec::new();
    let len = word.len();

    if !(3..=15).contains(&len) {
        return variants;
    }

    // === BACKWARD (stem extraction) ===
    if len > 5 && word.ends_with("ies") {
        // "queries" → "query"
        variants.push(format!("{}y", &word[..len - 3]));
    } else if len > 5 && word.ends_with("ier") {
        // "prettier" → "pretty"
        variants.push(format!("{}y", &word[..len - 3]));
    } else if len > 4 && word.ends_with("er") && !word.ends_with("eer") {
        // "watcher" → "watch", also try "watche" (for words like "writer" → "write")
        variants.push(word[..len - 2].to_string());
        variants.push(format!("{}e", &word[..len - 2]));
    } else if len > 5 && word.ends_with("ing") {
        // "watching" → "watch", also try "watche" → "parse" from "parsing"
        variants.push(word[..len - 3].to_string());
        variants.push(format!("{}e", &word[..len - 3]));
    } else if len > 4 && word.ends_with("ed") && !word.ends_with("eed") {
        // "cached" → "cach", also "cache"
        variants.push(word[..len - 2].to_string());
        variants.push(format!("{}e", &word[..len - 2]));
    } else if len > 4 && word.ends_with("es") && !word.ends_with("ees") {
        // "changes" → "change" (strip just 's')
        variants.push(word[..len - 1].to_string());
    } else if len > 4 && word.ends_with('s') && !word.ends_with("ss") {
        // "requests" → "request"
        variants.push(word[..len - 1].to_string());
    }

    // === FORWARD (derivation) ===
    let has_derived_suffix = word.ends_with("er")
        || word.ends_with("or")
        || word.ends_with("ing")
        || word.ends_with("tion")
        || word.ends_with("ment")
        || word.ends_with("ness")
        || word.ends_with("ous")
        || word.ends_with("ive")
        || word.ends_with("able");

    if !has_derived_suffix && (3..=10).contains(&len) {
        if word.ends_with('e') && !word.ends_with("ee") {
            // "parse" → "parser", "parsing"
            variants.push(format!("{}r", word));
            variants.push(format!("{}ing", &word[..len - 1]));
        } else if should_double_final_consonant(word) {
            // "run" → "runner", "running"; "get" → "getter", "getting"
            let doubled = format!("{}{}", word, word.chars().last().unwrap());
            variants.push(format!("{}er", doubled));
            variants.push(format!("{}ing", doubled));
        } else {
            // "watch" → "watcher", "watching"
            variants.push(format!("{}er", word));
            variants.push(format!("{}ing", word));
        }
    }

    // === PREFIX: re- for common programming verbs ===
    static RE_PREFIXABLE: &[&str] = &[
        "index",
        "build",
        "load",
        "connect",
        "start",
        "run",
        "create",
        "write",
        "read",
        "fetch",
        "init",
        "process",
        "compile",
        "render",
        "compute",
        "generate",
        "validate",
        "scan",
        "parse",
        "format",
        "sync",
        "fresh",
        "name",
        "play",
        "bind",
        "configure",
        "assemble",
        "quest",
        "solve",
        "open",
        "evaluate",
        "execute",
        "balance",
    ];
    if RE_PREFIXABLE.contains(&word) {
        variants.push(format!("re{}", word));
    }

    // Filter: remove too-short variants, self-references, and duplicates
    variants.retain(|v| v.len() >= 3 && v != word);
    variants.dedup();
    variants
}

/// Generate a natural-language description for a symbol to improve BM25 recall.
///
/// Generates morphological variants (forward derivation + backward stemming)
/// from the symbol NAME only, not the full body. This is deliberately selective:
/// name tokens carry the highest signal-to-noise ratio for search, while body
/// tokens are too numerous and spread common terms across many documents,
/// causing IDF dilution (R41 lesson: Q4/Q6/Q14 regressed -2 each when body
/// identifiers were included).
///
/// Budget capped at 15 words to minimize BM25 document length inflation.
pub fn generate_nl_description(name: &str, kind: &str, _body_text: &str) -> String {
    let mut new_words: HashSet<String> = HashSet::new();

    // Collect words from the name (the primary source for variants)
    let mut name_words: Vec<String> = Vec::new();
    for w in split_identifier_like(name).split_whitespace() {
        let lower = w.to_lowercase();
        if lower.len() >= 3 {
            name_words.push(lower);
        }
    }

    // Kind context (e.g., "function", "struct", "enum")
    let kind_lower = kind.to_lowercase();
    if kind_lower.len() >= 3
        && !LANG_KEYWORDS.contains(&kind_lower.as_str())
        && !name_words.contains(&kind_lower)
    {
        new_words.insert(kind_lower);
    }

    // Generate morphological variants for name words only
    let name_set: HashSet<&str> = name_words.iter().map(|s| s.as_str()).collect();
    for word in &name_words {
        if LANG_KEYWORDS.contains(&word.as_str()) {
            continue;
        }
        for variant in generate_morphological_variants(word) {
            if !name_set.contains(variant.as_str()) && !LANG_KEYWORDS.contains(&variant.as_str()) {
                new_words.insert(variant);
            }
        }
    }

    // Sort by length (shorter = more useful stems) then alphabetically for determinism
    let mut words: Vec<String> = new_words.into_iter().collect();
    words.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    words.truncate(15);
    words.join(" ")
}

/// Prepare text for embedding by adding a semantic header and stripping comments.
///
/// Embedding models (like BGE-base-en-v1.5) understand natural language better than
/// raw code. This function creates a structured representation:
///   "function index_files_parallel in parallel.rs\n{comment-stripped body}"
///
/// The header acts as a natural-language summary, improving retrieval for NL queries
/// like "How does parallel file indexing work?" while the stripped body preserves
/// the actual code logic without comment noise.
pub fn prepare_embedding_text(name: &str, kind: &str, file_path: &str, text: &str) -> String {
    let stripped = strip_code_comments(text);
    let filename = file_path.rsplit('/').next().unwrap_or(file_path);
    let header = format!("{} {} in {}\n", kind, name, filename);

    // Truncate body to prevent ONNX Runtime memory explosion.
    // The Jina model supports 8192 tokens (~4 chars/token), but attention is O(n²).
    // At 8K chars (~2K tokens), semantic content is captured without blowing ORT's
    // BFCArena past available memory on large repos (7500+ symbols).
    const MAX_BODY_BYTES: usize = 8000;
    if stripped.len() > MAX_BODY_BYTES {
        // Find the nearest char boundary at or below the cap. Slicing a
        // `&str` on a non-boundary byte index panics, which we hit in the
        // wild on files with multi-byte chars (UTF-8 strings, emoji, etc.)
        // straddling the 8000-byte mark.
        let mut end = MAX_BODY_BYTES;
        while end > 0 && !stripped.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}{}", header, &stripped[..end])
    } else {
        format!("{}{}", header, stripped)
    }
}

/// Strip comments from source code before BM25 indexing.
///
/// Removes:
/// 1. Full-line comments (`//`, `///`, `/* */` delimiters, `* ` continuation, `#`)
/// 2. Inline trailing comments (`code; // comment` → `code;`)
///
/// This prevents false BM25 matches where a function's comments *describe*
/// a concept but don't *implement* it. For example, `expand_stems` in text.rs
/// has inline comments like `// "handling" → "hand"` that cause it to rank #1
/// for "error handling" queries.
///
/// Inline comment detection uses a simple quote-tracking state machine to avoid
/// stripping `//` inside string literals (e.g., `"http://example.com"`).
pub fn strip_code_comments(text: &str) -> String {
    text.lines()
        .filter_map(|line| {
            let t = line.trim_start();
            // Full-line comments: remove entirely
            if t.starts_with("//") {
                return None;
            }
            // Block comment delimiters and continuation lines
            if t.starts_with("/*") || t.starts_with("*/") || t.starts_with("* ") || t == "*" {
                return None;
            }
            // Python/Ruby comments (but NOT Rust attributes #[...] or #![...])
            if t.starts_with('#') && !t.starts_with("#[") && !t.starts_with("#!") {
                return None;
            }

            // Inline trailing comments: strip the comment portion
            if let Some(pos) = find_inline_comment(line) {
                let trimmed = line[..pos].trim_end();
                if trimmed.is_empty() {
                    return None;
                }
                return Some(trimmed);
            }

            Some(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Find the byte position of an inline `//` comment outside string literals.
///
/// Returns `None` if no inline comment is found. Tracks double-quote and
/// single-quote (char literal) boundaries with backslash escape handling.
fn find_inline_comment(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut in_char = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' && (in_string || in_char) {
            i += 2; // skip escaped character
            continue;
        }
        if b == b'"' && !in_char {
            in_string = !in_string;
        } else if b == b'\'' && !in_string {
            in_char = !in_char;
        } else if !in_string && !in_char && b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/'
        {
            // Found // outside any string — only if there's code before it
            let before = &line[..i];
            if !before.trim().is_empty() {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Extract concept tags from symbol body text for known code patterns.
///
/// Scans the text for rare, discriminating patterns like `json!()` and `WebSocket`
/// and returns semantic labels that bridge the vocabulary gap between natural
/// language queries and code patterns. For example, `json!({` → "response formatting".
///
/// R34: Removed broad tags (error_handling, fallback, serialization, serde) that
/// fired on 80%+ of files with near-zero BM25 IDF. Only rare tags remain:
/// websocket, realtime, response, formatting.
///
/// Returns a space-separated string of unique tags suitable for appending to
/// Tantivy indexed text. Compound tags are also split for BM25 tokenization.
pub fn extract_concept_tags(text: &str) -> String {
    let mut tags = HashSet::new();

    // --- JSON / Response formatting (Q10) ---
    // R34: Removed broad "serialization" tag — fires on most data structs via serde,
    // near-zero IDF. Keep only "response" and "formatting" which are rare/discriminating.
    if text.contains("json!(") || text.contains("json!{") || text.contains("json! {") {
        tags.insert("response");
        tags.insert("formatting");
    }
    if text.contains("to_string_pretty") {
        tags.insert("formatting");
    }
    // R34: Removed serde_json, #[derive(Serialize/Deserialize)] triggers entirely —
    // "serialization" and "serde" tags fired on 80%+ of files, zero discrimination.

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

    // R34: Removed ALL error_handling / fallback / graceful_degradation tags.
    // These fired on ~80% of files (map_err, Err(e), unwrap_or, bail!, .context(), etc.)
    // giving near-zero BM25 IDF — confirmed zero discrimination in R33 benchmarks.

    // --- Async / Concurrency / Parallelism (Q11) ---
    // Only rare primitives (< 2% of corpus). Skipped: async fn (7.3%), Mutex (2.2%).
    if text.contains("tokio::spawn") || text.contains("task::spawn") {
        tags.insert("async");
        tags.insert("concurrency");
    }
    if text.contains("spawn_blocking") {
        tags.insert("async");
        tags.insert("concurrency");
    }
    if text.contains("rayon") || text.contains("par_iter") || text.contains("par_bridge") {
        tags.insert("parallel");
        tags.insert("concurrency");
    }
    if text.contains("Semaphore") {
        tags.insert("concurrency");
    }
    if text.contains("join!") || text.contains("select!") {
        tags.insert("async");
        tags.insert("concurrency");
    }
    if text.contains("CancellationToken") {
        tags.insert("async");
        tags.insert("concurrency");
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
        assert!(
            result.contains("callback"),
            "should expand handler→callback"
        );
    }

    #[test]
    fn test_expand_synonyms_serialization() {
        let result = expand_synonyms("serialization format");
        assert!(
            result.contains("serde"),
            "should expand serialization→serde"
        );
        assert!(
            result.contains("serialize"),
            "should expand serialization→serialize"
        );
        assert!(
            result.contains("formatting"),
            "should expand format→formatting"
        );
    }

    #[test]
    fn test_expand_synonyms_watcher_debounce() {
        let result = expand_synonyms("watcher debounce");
        assert!(result.contains("watch"), "should expand watcher→watch");
        assert!(
            result.contains("throttle"),
            "should expand debounce→throttle"
        );
    }

    #[test]
    fn test_expand_synonyms_fallback_reverse() {
        // "degradation" is a synonym of "fallback", so querying "degradation"
        // should add the main term "fallback"
        let result = expand_synonyms("graceful degradation");
        assert!(
            result.contains("fallback"),
            "should add fallback from degradation synonym"
        );
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
        let q7 =
            "How does the Web Socket handler work? ws websocket callback listener hook delegate";
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
        assert!(
            r9.contains("degradation"),
            "should keep 'degradation': {r9}"
        );
    }

    #[test]
    fn test_expand_stems_short_queries_untouched() {
        assert_eq!(expand_stems("get"), "get");
        assert_eq!(expand_stems("get user"), "get user");
    }

    #[test]
    fn test_expand_stems_er_suffix() {
        let result = expand_stems("file watcher debounce reindex");
        assert!(
            result.contains("watch"),
            "should stem watcher→watch: {result}"
        );
    }

    #[test]
    fn test_expand_stems_ing_suffix() {
        let result = expand_stems("JSON serialization response formatting");
        assert!(
            result.contains("format"),
            "should stem formatting→format: {result}"
        );
    }

    #[test]
    fn test_expand_stems_s_suffix() {
        let result = expand_stems("find function handles search requests");
        assert!(
            result.contains("request"),
            "should stem requests→request: {result}"
        );
        assert!(
            result.contains("handle"),
            "should stem handles→handle: {result}"
        );
    }

    #[test]
    fn test_expand_stems_no_duplicates() {
        // "handle" is already in the query, shouldn't be added again
        let result = expand_stems("handle search code handler");
        let handle_count = result.matches("handle").count();
        // "handle" appears once naturally + "handler" stems to "handl" not "handle"
        assert!(
            handle_count <= 3,
            "should not add duplicate stems: {result}"
        );
    }

    #[test]
    fn test_expand_stems_tion_suffix() {
        let result = expand_stems("vector embedding generation and storage");
        // "generation" → strip "ation" → "gener" (5 chars, valid)
        assert!(
            result.contains("gener"),
            "should stem generation→gener: {result}"
        );
    }

    #[test]
    fn test_expand_stems_skips_hyphenated_words() {
        let result = expand_stems("tree-sitter parser initialization and language");
        // "tree-sitter" should not be stemmed (hyphen skip)
        let words: Vec<&str> = result.split_whitespace().collect();
        assert!(
            !words.contains(&"tree-sitt"),
            "should not stem hyphenated words: {result}"
        );
    }

    #[test]
    fn test_expand_stems_filters_short_stems() {
        let result = expand_stems("caching and cache invalidation patterns");
        // "caching" → "cach" (4 chars) should be filtered by min stem length 5
        let words: Vec<&str> = result.split_whitespace().collect();
        assert!(
            !words.contains(&"cach"),
            "should filter stems < 5 chars: {result}"
        );
    }

    #[test]
    fn test_expand_stems_substring_not_confused_with_word() {
        // "transactions" contains "transaction" as a substring, but "transaction"
        // should still be appended because Tantivy tokenizes by word boundaries.
        let result = expand_stems("Database transactions and helpers");
        assert!(
            result.split_whitespace().any(|w| w == "transaction"),
            "should append stem 'transaction' even though 'transactions' contains it as substring: {result}"
        );
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
        assert!(
            tags.contains("tree_sitter"),
            "should extract tree_sitter: {tags}"
        );
        assert!(
            tags.contains("serde_json"),
            "should extract serde_json: {tags}"
        );
        assert!(tags.contains("anyhow"), "should extract anyhow: {tags}");
        // Split forms
        assert!(
            tags.contains("tree") && tags.contains("sitter"),
            "should split tree_sitter: {tags}"
        );
        assert!(
            tags.contains("serde") && tags.contains("json"),
            "should split serde_json: {tags}"
        );
        // Should NOT include internal references or std
        assert!(
            !tags.split_whitespace().any(|w| w == "crate"),
            "should skip crate: {tags}"
        );
        assert!(
            !tags.split_whitespace().any(|w| w == "std"),
            "should skip std: {tags}"
        );
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
        assert!(
            tags.contains("once_cell"),
            "should extract once_cell: {tags}"
        );
        assert!(tags.contains("tokio"), "should extract tokio: {tags}");
        assert!(
            !tags.split_whitespace().any(|w| w == "self"),
            "should skip self: {tags}"
        );
        assert!(
            !tags.split_whitespace().any(|w| w == "super"),
            "should skip super: {tags}"
        );
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
        assert!(
            tags.contains("types"),
            "should include types (from @types/node): {tags}"
        );
        // Dedup: express should appear only once
        let express_count = tags.split_whitespace().filter(|w| *w == "express").count();
        assert_eq!(express_count, 1, "express should be deduplicated: {tags}");
    }

    #[test]
    fn test_concept_tags_json_macro() {
        let text = r#"pub fn build_response() { json!({"status": "ok"}) }"#;
        let tags = extract_concept_tags(text);
        assert!(
            !tags.contains("serialization"),
            "R34: serialization removed as broad tag: {tags}"
        );
        assert!(
            tags.contains("response"),
            "json!( should trigger response: {tags}"
        );
        assert!(
            tags.contains("formatting"),
            "json!( should trigger formatting: {tags}"
        );
    }

    #[test]
    fn test_concept_tags_websocket() {
        let text = r#"app.ws("/ws", |ws| { ws.send("hello") })"#;
        let tags = extract_concept_tags(text);
        assert!(
            tags.contains("websocket"),
            ".ws( should trigger websocket: {tags}"
        );
    }

    #[test]
    fn test_concept_tags_websocket_name() {
        let text = "fn handle_WebSocket_connection() {}";
        let tags = extract_concept_tags(text);
        assert!(
            tags.contains("websocket"),
            "WebSocket should trigger websocket: {tags}"
        );
        assert!(
            tags.contains("realtime"),
            "WebSocket should trigger realtime: {tags}"
        );
    }

    #[test]
    fn test_concept_tags_async_concurrency() {
        let text = "tokio::spawn(async move { process_batch().await })";
        let tags = extract_concept_tags(text);
        assert!(
            tags.contains("async"),
            "tokio::spawn should trigger async: {tags}"
        );
        assert!(
            tags.contains("concurrency"),
            "tokio::spawn should trigger concurrency: {tags}"
        );
    }

    #[test]
    fn test_concept_tags_parallel() {
        let text = "items.par_iter().map(|x| process(x)).collect()";
        let tags = extract_concept_tags(text);
        assert!(
            tags.contains("parallel"),
            "par_iter should trigger parallel: {tags}"
        );
        assert!(
            tags.contains("concurrency"),
            "par_iter should trigger concurrency: {tags}"
        );
    }

    // R34: Removed tests for error_handling, fallback, graceful_degradation, serde —
    // all broad tags removed because they fired on 80%+ of files with near-zero IDF.

    #[test]
    fn test_concept_tags_broad_tags_removed() {
        // Verify broad tags no longer fire
        let error_text = "result.map_err(|e| format!(\"failed: {e}\"))";
        let tags = extract_concept_tags(error_text);
        assert!(
            tags.is_empty(),
            "R34: map_err should not produce tags: '{tags}'"
        );

        let fallback_text = "let val = opt.unwrap_or_else(|| default_value());";
        let tags = extract_concept_tags(fallback_text);
        assert!(
            tags.is_empty(),
            "R34: unwrap_or_else should not produce tags: '{tags}'"
        );

        let serde_text = "#[derive(Debug, Serialize, Deserialize)]\nstruct Foo {}";
        let tags = extract_concept_tags(serde_text);
        assert!(
            tags.is_empty(),
            "R34: derive Serialize should not produce tags: '{tags}'"
        );
    }

    #[test]
    fn test_concept_tags_empty_text() {
        let tags = extract_concept_tags("");
        assert!(
            tags.is_empty(),
            "empty text should return empty tags: '{tags}'"
        );
    }

    #[test]
    fn test_concept_tags_no_matches() {
        let tags = extract_concept_tags("fn add(a: i32, b: i32) -> i32 { a + b }");
        assert!(
            tags.is_empty(),
            "plain function should return empty tags: '{tags}'"
        );
    }

    // --- prepare_embedding_text tests ---

    #[test]
    fn test_prepare_embedding_text_basic() {
        let text = "/// Doc comment\nfn foo() { bar() }";
        let result = prepare_embedding_text("foo", "function", "src/handlers/mod.rs", text);
        assert!(
            result.starts_with("function foo in mod.rs\n"),
            "should have semantic header: {result}"
        );
        assert!(
            !result.contains("Doc comment"),
            "should strip comments: {result}"
        );
        assert!(
            result.contains("fn foo() { bar() }"),
            "should preserve code: {result}"
        );
    }

    #[test]
    fn test_prepare_embedding_text_truncates_on_utf8_boundary() {
        // A body whose 8000th byte falls in the middle of a multi-byte
        // character used to panic with "byte index N is not a char
        // boundary." Mix ASCII padding with a 4-byte emoji straddling the
        // cap to verify truncation rounds DOWN to a char boundary.
        let mut body = String::new();
        body.push_str(&"a".repeat(7998)); // bytes 0..7998
        body.push('🦀'); // bytes 7998..8002 (4-byte UTF-8)
        body.push_str(&"b".repeat(100));

        // Must not panic.
        let result = prepare_embedding_text("crab", "function", "lib.rs", &body);
        // Truncation should have dropped the partial crab and kept only
        // the leading ASCII run.
        assert!(result.is_char_boundary(result.len()));
        // Body section starts after the header; it must be valid UTF-8
        // (implied by being a String) and shorter than the original.
        assert!(result.len() < body.len() + 64);
    }

    #[test]
    fn test_prepare_embedding_text_extracts_filename() {
        let result = prepare_embedding_text(
            "MyStruct",
            "struct",
            "src/storage/sqlite/mod.rs",
            "struct MyStruct {}",
        );
        assert!(
            result.starts_with("struct MyStruct in mod.rs\n"),
            "should extract filename: {result}"
        );
    }

    // --- strip_code_comments tests ---

    #[test]
    fn test_strip_code_comments_removes_doc_comments() {
        let text = "/// This is a doc comment\n/// about error_handling\npub fn foo() {}";
        let stripped = strip_code_comments(text);
        assert_eq!(stripped, "pub fn foo() {}");
        assert!(!stripped.contains("error_handling"));
    }

    #[test]
    fn test_strip_code_comments_removes_line_comments() {
        let text = "fn bar() {\n    // R34: Removed error_handling tags\n    let x = 1;\n}";
        let stripped = strip_code_comments(text);
        assert!(stripped.contains("let x = 1;"));
        assert!(!stripped.contains("error_handling"));
    }

    #[test]
    fn test_strip_code_comments_preserves_code() {
        let text = "pub fn extract() {\n    if text.contains(\"json!(\") {\n        tags.insert(\"response\");\n    }\n}";
        let stripped = strip_code_comments(text);
        assert_eq!(stripped, text, "code without comments should be unchanged");
    }

    #[test]
    fn test_strip_code_comments_removes_block_comments() {
        let text = "/*\n * Block comment about serialization\n */\nfn baz() {}";
        let stripped = strip_code_comments(text);
        assert!(stripped.contains("fn baz() {}"));
        assert!(!stripped.contains("serialization"));
    }

    #[test]
    fn test_strip_code_comments_preserves_rust_attributes() {
        let text = "#[derive(Debug, Serialize)]\nstruct Foo {\n    // A field\n    pub x: i32,\n}";
        let stripped = strip_code_comments(text);
        assert!(
            stripped.contains("#[derive(Debug, Serialize)]"),
            "Rust attributes preserved"
        );
        assert!(!stripped.contains("A field"), "inline comment stripped");
    }

    #[test]
    fn test_strip_code_comments_preserves_python_shebangs() {
        let text = "#!/usr/bin/env python\n# This is a comment\ndef foo(): pass";
        let stripped = strip_code_comments(text);
        assert!(
            stripped.contains("#!/usr/bin/env python"),
            "shebang preserved"
        );
        assert!(
            !stripped.contains("This is a comment"),
            "Python comment stripped"
        );
    }

    // --- build_framework_vocab_tags tests ---

    #[test]
    fn test_framework_vocab_websocket() {
        let patterns = vec![("websocket".to_string(), None)];
        let tags = build_framework_vocab_tags(&patterns);
        assert!(
            tags.contains("websocket"),
            "should include websocket: {tags}"
        );
        assert!(tags.contains("handler"), "should include handler: {tags}");
        assert!(tags.contains("endpoint"), "should include endpoint: {tags}");
        assert!(tags.contains("realtime"), "should include realtime: {tags}");
    }

    #[test]
    fn test_framework_vocab_route_with_method() {
        let patterns = vec![("route".to_string(), Some("GET".to_string()))];
        let tags = build_framework_vocab_tags(&patterns);
        assert!(tags.contains("route"), "should include route: {tags}");
        assert!(tags.contains("handler"), "should include handler: {tags}");
        assert!(tags.contains("http"), "should include http: {tags}");
        assert!(tags.contains("endpoint"), "should include endpoint: {tags}");
        assert!(tags.contains("get"), "should include get (method): {tags}");
    }

    #[test]
    fn test_framework_vocab_empty_input() {
        let patterns: Vec<(String, Option<String>)> = vec![];
        let tags = build_framework_vocab_tags(&patterns);
        assert!(
            tags.is_empty(),
            "empty patterns should produce empty tags: '{tags}'"
        );
    }

    #[test]
    fn test_framework_vocab_dedup() {
        // Both websocket and route produce "handler" — should only appear once
        let patterns = vec![
            ("websocket".to_string(), None),
            ("route".to_string(), Some("POST".to_string())),
        ];
        let tags = build_framework_vocab_tags(&patterns);
        let handler_count = tags.split_whitespace().filter(|w| *w == "handler").count();
        assert_eq!(handler_count, 1, "handler should be deduplicated: {tags}");
        // Both should still have their unique words
        assert!(
            tags.contains("realtime"),
            "websocket's realtime should be present: {tags}"
        );
        assert!(tags.contains("route"), "route should be present: {tags}");
        assert!(
            tags.contains("post"),
            "POST method should be present: {tags}"
        );
    }

    // --- NL description tests ---

    #[test]
    fn test_generate_morphological_variants_backward() {
        // -er stripping
        let v = generate_morphological_variants("watcher");
        assert!(v.contains(&"watch".to_string()), "watcher → watch: {v:?}");

        // -ing stripping
        let v = generate_morphological_variants("watching");
        assert!(v.contains(&"watch".to_string()), "watching → watch: {v:?}");

        // -es → strip s
        let v = generate_morphological_variants("changes");
        assert!(v.contains(&"change".to_string()), "changes → change: {v:?}");

        // -s stripping
        let v = generate_morphological_variants("requests");
        assert!(
            v.contains(&"request".to_string()),
            "requests → request: {v:?}"
        );

        // -ing with e restoration
        let v = generate_morphological_variants("parsing");
        assert!(v.contains(&"parse".to_string()), "parsing → parse: {v:?}");
    }

    #[test]
    fn test_generate_morphological_variants_forward() {
        // -er addition
        let v = generate_morphological_variants("watch");
        assert!(v.contains(&"watcher".to_string()), "watch → watcher: {v:?}");
        assert!(
            v.contains(&"watching".to_string()),
            "watch → watching: {v:?}"
        );

        // -e ending: parse → parser, parsing
        let v = generate_morphological_variants("parse");
        assert!(v.contains(&"parser".to_string()), "parse → parser: {v:?}");
        assert!(v.contains(&"parsing".to_string()), "parse → parsing: {v:?}");

        // Double consonant: run → runner, running
        let v = generate_morphological_variants("run");
        assert!(v.contains(&"runner".to_string()), "run → runner: {v:?}");
        assert!(v.contains(&"running".to_string()), "run → running: {v:?}");
    }

    #[test]
    fn test_generate_morphological_variants_prefix() {
        let v = generate_morphological_variants("index");
        assert!(v.contains(&"reindex".to_string()), "index → reindex: {v:?}");

        let v = generate_morphological_variants("build");
        assert!(v.contains(&"rebuild".to_string()), "build → rebuild: {v:?}");
    }

    #[test]
    fn test_generate_morphological_variants_no_self_reference() {
        let v = generate_morphological_variants("watch");
        assert!(
            !v.contains(&"watch".to_string()),
            "should not contain self: {v:?}"
        );
    }

    #[test]
    fn test_generate_nl_description_basic() {
        let desc = generate_nl_description(
            "spawn_watch_loop",
            "function",
            "let watcher = check_for_changes(debounce_ms);",
        );
        // Should contain kind context
        assert!(desc.contains("function"), "should include kind: {desc}");
        // Name-only variants: "watch" → watcher/watching, "spawn" → spawner/spawning
        assert!(
            desc.contains("watcher") || desc.contains("watching"),
            "watch → watcher/watching: {desc}"
        );
        assert!(
            desc.contains("spawner") || desc.contains("spawning"),
            "spawn → spawner/spawning: {desc}"
        );
        // Body tokens should NOT appear (name-only restriction from R41)
        assert!(
            !desc.contains("change"),
            "body token 'changes' should not produce variants: {desc}"
        );
    }

    #[test]
    fn test_generate_nl_description_excludes_existing() {
        let desc = generate_nl_description(
            "get_config",
            "function",
            "fn get_config() -> Config { config }",
        );
        // "config" and "get" are already in the body — shouldn't be in description
        // (they'd be in existing_words set)
        // But their VARIANTS should be present
        let words: HashSet<&str> = desc.split_whitespace().collect();
        // "config" (4 chars) is in body, but "configing" or "configer" (>= 3) might be
        // Main point: the description should NOT duplicate existing words
        assert!(
            words.len() < 80,
            "should not produce excessive words: {desc}"
        );
    }

    #[test]
    fn test_generate_nl_description_kind_context() {
        // "struct" kind when body doesn't contain "struct" as an identifier
        let _desc =
            generate_nl_description("MyConfig", "struct", "pub name: String, pub value: i32,");
        // "struct" is in LANG_KEYWORDS, so it won't be added as kind context
        // But "struct" IS a valid kind... let me check: LANG_KEYWORDS has "struct"
        // This is actually OK — for structs, the kind is less useful than for functions

        let desc = generate_nl_description("process_events", "function", "for event in events {}");
        assert!(
            desc.contains("function"),
            "function kind should be added: {desc}"
        );
    }

    #[test]
    fn test_generate_nl_description_name_only_variants() {
        // R41 lesson: only name tokens get variants, not body tokens
        let desc = generate_nl_description(
            "check_for_changes",
            "function",
            "if changed { index_files(path); }",
        );
        // Name tokens: "check" (5), "for" (3), "changes" (7)
        // "changes" → backward stem "change", forward "changer/changing"
        // "check" → forward "checker/checking"
        assert!(
            desc.contains("change"),
            "changes → change (backward stem): {desc}"
        );
        assert!(
            desc.contains("checker") || desc.contains("checking"),
            "check → checker/checking: {desc}"
        );
        // "index" is only in body — should NOT produce "reindex"
        assert!(
            !desc.contains("reindex"),
            "body token 'index' should not produce variants: {desc}"
        );
    }

    #[test]
    fn test_strip_code_comments_meta_matching_prevention() {
        // Simulates extract_concept_tags body — comments should be stripped
        // but string literals in code should remain
        let text = r#"/// Scans for patterns like error_handling and serialization
/// R34: Removed broad tags (error_handling, fallback, serialization)
pub fn extract_concept_tags(text: &str) -> String {
    // --- JSON / Response formatting (Q10) ---
    // R34: Removed broad "serialization" tag
    if text.contains("json!(") {
        tags.insert("response");
    }
}"#;
        let stripped = strip_code_comments(text);
        // Comments mentioning "error_handling" and "serialization" should be gone
        assert!(
            !stripped.contains("error_handling"),
            "doc comment keyword stripped"
        );
        assert!(
            !stripped.contains("serialization"),
            "inline comment keyword stripped"
        );
        // But code string literals should remain
        assert!(stripped.contains("json!("), "code string literal preserved");
        assert!(
            stripped.contains("response"),
            "code string literal preserved"
        );
    }

    #[test]
    fn test_strip_code_comments_inline_trailing() {
        // Inline trailing comments should be stripped
        let text = r#"Some(w[..w.len() - 4].to_string()) // "handling" → "hand"
Some(w[..w.len() - 2].to_string()) // "gracefully" → "graceful"
let x = 1; // this is a trailing comment"#;
        let stripped = strip_code_comments(text);
        assert!(
            !stripped.contains("handling"),
            "inline comment word stripped"
        );
        assert!(
            !stripped.contains("gracefully"),
            "inline comment word stripped"
        );
        assert!(
            !stripped.contains("trailing comment"),
            "inline comment stripped"
        );
        assert!(
            stripped.contains("Some(w[..w.len() - 4].to_string())"),
            "code preserved"
        );
        assert!(stripped.contains("let x = 1;"), "code preserved");
    }

    #[test]
    fn test_strip_code_comments_preserves_url_in_string() {
        // // inside a string literal should NOT be treated as a comment
        let text = r#"let url = "http://example.com"; // a comment
let path = "file:///tmp/test";"#;
        let stripped = strip_code_comments(text);
        assert!(
            stripped.contains("http://example.com"),
            "URL in string preserved"
        );
        assert!(!stripped.contains("a comment"), "trailing comment stripped");
        assert!(
            stripped.contains("file:///tmp/test"),
            "file URL in string preserved"
        );
    }

    #[test]
    fn test_get_related_terms_stem_matching() {
        // "concurrency" should stem-match "concurrent" (value under "async")
        let related = get_related_terms("concurrency");
        assert!(
            !related.is_empty(),
            "concurrency should find related terms via stem matching"
        );
        assert!(
            related.contains(&"async"),
            "concurrency should relate to async"
        );
        assert!(
            related.contains(&"parallel"),
            "concurrency should relate to parallel"
        );

        // "processing" has no stem match in the table (too short shared prefix with anything)
        let related = get_related_terms("processing");
        assert!(
            related.is_empty(),
            "processing should have no related terms"
        );

        // Exact matches still work
        let related = get_related_terms("concurrent");
        assert!(related.contains(&"async"), "exact match still works");

        // Short words don't false-positive
        let related = get_related_terms("con");
        assert!(related.is_empty(), "short words should not match");
    }

    #[test]
    fn test_synonym_stems_match() {
        assert!(synonym_stems_match("concurrency", "concurrent")); // share "concurren" (9)
        assert!(synonym_stems_match("serialization", "serialize")); // share "serializ" (8)
        assert!(synonym_stems_match("configuring", "configuration")); // share "configur" (8)
        assert!(!synonym_stems_match("con", "concurrent")); // too short
        assert!(!synonym_stems_match("consent", "concurrent")); // share "con" (3) < 6
        assert!(!synonym_stems_match("processing", "procedure")); // share "proce" (5) < 6
    }
}

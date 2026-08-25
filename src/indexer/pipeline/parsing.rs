use crate::indexer::extract::symbol::SymbolKind;
use std::collections::HashSet;

pub fn symbol_kind_to_string(kind: SymbolKind) -> String {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Class => "class",
        SymbolKind::Interface => "interface",
        SymbolKind::TypeAlias => "type_alias",
        SymbolKind::Enum => "enum",
        SymbolKind::Const => "const",
        SymbolKind::Struct => "struct",
        SymbolKind::Trait => "trait",
        SymbolKind::Impl => "impl",
        SymbolKind::Module => "module",
        SymbolKind::Property => "property",
        SymbolKind::Document => "document",
    }
    .to_string()
}

pub fn extract_callee_names(text: &str) -> Vec<String> {
    extract_calls(text).into_iter().map(|c| c.method).collect()
}

/// One detected call expression in source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    /// Method/function name (the identifier immediately before `(`).
    pub method: String,
    /// The receiver identifier when the call is `receiver.method()`. None
    /// for bare `method()` calls. Only the immediate left-hand identifier
    /// is captured; chained accesses like `a.b.method()` yield `b`.
    pub receiver: Option<String>,
}

/// Extract every call expression in `text` along with its immediate
/// receiver (when present). Used by the edge builder to resolve
/// `imported_instance.method()` to the method's defining file even when
/// the method name isn't directly imported into the current file.
pub fn extract_calls(text: &str) -> Vec<CallSite> {
    let stopwords: HashSet<&'static str> = [
        "if", "for", "while", "switch", "catch", "function", "return", "new", "await", "match",
    ]
    .into_iter()
    .collect();

    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    // Remember the most recently scanned identifier and whether the
    // character immediately before it was `.`. When we then see that
    // identifier followed by `(`, we know whether it was a method call
    // on a receiver and what the receiver name was.
    let mut last_ident: Option<(String, usize, usize)> = None; // (text, start_byte, end_byte)
    let mut last_was_dot_chain: bool = false;
    let mut prev_ident_text: Option<String> = None;
    let mut prev_ident_end: Option<usize> = None;
    while i < bytes.len() {
        let b = bytes[i];
        let is_ident_start = b.is_ascii_alphabetic() || b == b'_' || b == b'$';
        if !is_ident_start {
            i += 1;
            continue;
        }

        let start = i;
        i += 1;
        while i < bytes.len() {
            let b = bytes[i];
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                i += 1;
            } else {
                break;
            }
        }
        let ident_text = text[start..i].to_string();
        let ident_end = i;

        // Was the byte immediately before `start` a `.`? If so this
        // identifier is the right-hand side of a member-expression.
        let preceded_by_dot = start > 0 && bytes[start - 1] == b'.';

        // Look ahead past whitespace to see if `(` follows.
        let mut j = ident_end;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        let is_call =
            j < bytes.len() && bytes[j] == b'(' && !stopwords.contains(ident_text.as_str());

        if is_call {
            let receiver = if preceded_by_dot {
                // Receiver is the most recent identifier ending exactly
                // at `start - 1` (with the dot at start-1). Use
                // last_ident's end if it matches.
                prev_ident_text.clone()
            } else {
                None
            };
            out.push(CallSite {
                method: ident_text.clone(),
                receiver,
            });
        }

        prev_ident_text = Some(ident_text.clone());
        prev_ident_end = Some(ident_end);
        last_ident = Some((ident_text, start, ident_end));
        last_was_dot_chain = preceded_by_dot;
    }
    // Silence unused warnings.
    let _ = (last_ident, last_was_dot_chain, prev_ident_end);
    out
}

pub fn extract_identifiers(text: &str) -> Vec<String> {
    let stopwords: HashSet<&'static str> = [
        "if", "for", "while", "switch", "catch", "function", "return", "new", "await", "match",
        "let", "const", "var", "pub", "impl", "trait", "struct", "enum", "mod", "use",
    ]
    .into_iter()
    .collect();

    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < bytes.len() {
        let b = bytes[i];
        let is_ident_start = b.is_ascii_alphabetic() || b == b'_' || b == b'$';
        if !is_ident_start {
            i += 1;
            continue;
        }

        let start = i;
        i += 1;
        while i < bytes.len() {
            let b = bytes[i];
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                i += 1;
            } else {
                break;
            }
        }
        let ident = &text[start..i];
        if !stopwords.contains(ident) {
            out.push(ident.to_string());
        }
    }
    out
}

pub fn identifier_evidence(
    text: &str,
    target: &str,
    start_line: u32,
) -> (u32, u32, Vec<(u32, u32)>) {
    if target.is_empty() {
        return (1, start_line, Vec::new());
    }

    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut line = start_line;
    let mut first_line = None::<u32>;
    let mut total = 0u32;
    let mut counts = std::collections::HashMap::<u32, u32>::new();

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\n' {
            line = line.saturating_add(1);
            i += 1;
            continue;
        }

        let is_ident_start = b.is_ascii_alphabetic() || b == b'_' || b == b'$';
        if !is_ident_start {
            i += 1;
            continue;
        }

        let start = i;
        i += 1;
        while i < bytes.len() {
            let b = bytes[i];
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                i += 1;
            } else {
                break;
            }
        }
        let ident = &text[start..i];
        if ident == target {
            total = total.saturating_add(1);
            first_line.get_or_insert(line);
            *counts.entry(line).or_insert(0) += 1;
        }
    }

    if total == 0 {
        return (1, start_line, Vec::new());
    }

    let mut per_line = counts.into_iter().collect::<Vec<_>>();
    per_line.sort_by(|(a_line, a_count), (b_line, b_count)| {
        b_count.cmp(a_count).then_with(|| a_line.cmp(b_line))
    });
    if per_line.len() > 5 {
        per_line.truncate(5);
    }

    (total.max(1), first_line.unwrap_or(start_line), per_line)
}

pub fn parse_type_relations(text: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut extends = Vec::new();
    let mut implements = Vec::new();
    let mut aliases = Vec::new();

    let mut rest = text;
    while let Some(pos) = rest.find("extends") {
        rest = &rest[pos + "extends".len()..];
        if let Some(name) = parse_next_identifier(rest) {
            extends.push(name);
        }
    }

    let mut rest = text;
    while let Some(pos) = rest.find("implements") {
        rest = &rest[pos + "implements".len()..];
        if let Some(name) = parse_next_identifier(rest) {
            implements.push(name);
        }
    }

    if let Some(eq_pos) = text.find('=') {
        let rhs = &text[eq_pos + 1..];
        if let Some(name) = parse_next_identifier(rhs) {
            aliases.push(name);
        }
    }

    (extends, implements, aliases)
}

pub fn parse_next_identifier(s: &str) -> Option<String> {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.peek().copied() {
        if c.is_alphabetic() || c == '_' || c == '$' {
            break;
        }
        chars.next();
    }
    let mut out = String::new();
    while let Some(c) = chars.peek().copied() {
        if c.is_alphanumeric() || c == '_' || c == '$' {
            out.push(c);
            chars.next();
        } else {
            break;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

pub fn extract_usage_line(text: &str, needle: &str) -> Option<String> {
    for line in text.lines() {
        if line.contains(needle) {
            return Some(trim_snippet(line, 200));
        }
    }
    None
}

pub fn trim_snippet(s: &str, max_len: usize) -> String {
    let mut out = s.trim().to_string();
    if out.len() > max_len {
        // Find the last char boundary at or before max_len to avoid panic
        let mut end = max_len;
        while end > 0 && !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

#[cfg(test)]
mod call_extraction_tests {
    use super::{extract_calls, CallSite};

    fn site(method: &str, receiver: Option<&str>) -> CallSite {
        CallSite {
            method: method.to_string(),
            receiver: receiver.map(str::to_string),
        }
    }

    #[test]
    fn bare_calls_have_no_receiver() {
        let calls = extract_calls("foo(); bar(1, 2);");
        assert_eq!(calls, vec![site("foo", None), site("bar", None)]);
    }

    #[test]
    fn member_expression_call_captures_receiver() {
        let calls = extract_calls("sessionManager.createSession(args);");
        assert_eq!(calls, vec![site("createSession", Some("sessionManager"))]);
    }

    #[test]
    fn chained_member_call_captures_immediate_receiver() {
        let calls = extract_calls("obj.inner.run();");
        assert_eq!(calls, vec![site("run", Some("inner"))]);
    }

    #[test]
    fn keywords_followed_by_paren_are_skipped() {
        let calls = extract_calls("if (x) { return; } new Foo(); await bar();");
        assert_eq!(calls, vec![site("Foo", None), site("bar", None)]);
    }

    #[test]
    fn nested_calls_each_emit() {
        let calls = extract_calls("outer(inner(state.value));");
        assert_eq!(
            calls,
            vec![
                site("outer", None),
                site("inner", None),
                // `state.value` is a property access, not a call - no entry
            ]
        );
    }
}

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
    }
    .to_string()
}

pub fn extract_callee_names(text: &str) -> Vec<String> {
    let stopwords: HashSet<&'static str> = [
        "if", "for", "while", "switch", "catch", "function", "return", "new", "await", "match",
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
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'(' && !stopwords.contains(ident) {
            out.push(ident.to_string());
        }
    }
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
        out.truncate(max_len);
    }
    out
}

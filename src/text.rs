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

use super::evidence_pack::PackLocation;
use std::collections::HashSet;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonCallgraphShape {
    Pipeline,
    Callsite,
}

#[allow(dead_code)]
pub fn extract_non_callgraph_candidates(
    target: &str,
    locations: &[PackLocation],
    shape: NonCallgraphShape,
) -> Vec<PackLocation> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();

    for location in locations {
        let Some(body) = location.body.as_deref() else {
            continue;
        };

        let base_line = location.start_line.unwrap_or(1);

        for (offset, line) in body.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let line_number = base_line.saturating_add(offset as u32);
            let kind = match shape {
                NonCallgraphShape::Pipeline => pipeline_candidate_kind(target, trimmed),
                NonCallgraphShape::Callsite => callsite_candidate_kind(target, trimmed),
            };

            let Some(kind) = kind else {
                continue;
            };

            let file_path = location.file_path.clone();
            let dedupe_key = format!(
                "{}:{line_number}:{kind}",
                file_path.as_deref().unwrap_or_default()
            );
            if !seen.insert(dedupe_key) {
                continue;
            }

            candidates.push(PackLocation {
                symbol_id: Some(format!("{}:{kind}:{line_number}", source_id(location))),
                symbol_name: location.symbol_name.clone(),
                file_path,
                kind: Some(kind.to_string()),
                start_line: Some(line_number),
                end_line: Some(line_number),
                via: Some("non_callgraph_edges".to_string()),
                body: Some(trimmed.to_string()),
            });
        }
    }

    candidates
}

fn pipeline_candidate_kind(target: &str, line: &str) -> Option<&'static str> {
    if !matches_target(target, line) {
        return None;
    }

    if is_event_emitter(line) {
        return Some("event_emitter");
    }
    if is_event_subscriber(line) {
        return Some("event_subscriber");
    }
    if is_callback_producer(line) {
        return Some("callback_producer");
    }
    None
}

fn matches_target(target: &str, line: &str) -> bool {
    let target_lower = target.to_ascii_lowercase();
    let line_lower = line.to_ascii_lowercase();
    if target_lower.is_empty() {
        return false;
    }
    if line_lower.contains(&target_lower) {
        return true;
    }

    let target_alnum = alphanumeric_lowercase(target);
    if target_alnum.is_empty() {
        return false;
    }
    let line_alnum = alphanumeric_lowercase(line);
    if line_alnum.contains(&target_alnum) {
        return true;
    }

    let target_tokens = split_tokens(target);
    if target_tokens.is_empty() {
        return false;
    }

    let line_tokens = split_tokens(line);
    contains_token_sequence(&line_tokens, &target_tokens)
        || target_tokens
            .last()
            .is_some_and(|tail| tail.len() >= 4 && line_tokens.iter().any(|token| token == tail))
}

fn callsite_candidate_kind(target: &str, line: &str) -> Option<&'static str> {
    if !target.is_empty() && !line.contains(target) {
        return None;
    }
    if has_callback_name(line) && has_assignment_or_function_syntax(line) {
        return Some("config_hook");
    }
    None
}

fn is_callback_producer(line: &str) -> bool {
    has_callback_name(line) && has_assignment_or_function_syntax(line)
}

fn has_callback_name(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();
    CALLBACK_NAMES.iter().any(|name| normalized.contains(name))
}

fn has_assignment_or_function_syntax(line: &str) -> bool {
    line.contains(':')
        || line.contains('=')
        || line.contains("=>")
        || line.contains("function")
        || line.contains("async ")
}

fn is_event_emitter(line: &str) -> bool {
    line.contains("webContents.send")
        || line.contains(".emit(")
        || line.contains(".send(")
        || line.contains("postMessage(")
}

fn is_event_subscriber(line: &str) -> bool {
    line.contains("addEventListener(")
        || line.contains("ipcRenderer.on(")
        || line.contains(".on(")
        || line.contains(".once(")
}

fn source_id(location: &PackLocation) -> String {
    location
        .symbol_id
        .as_deref()
        .or(location.file_path.as_deref())
        .unwrap_or("unknown")
        .to_string()
}

const CALLBACK_NAMES: &[&str] = &[
    "onbefore", "onafter", "before", "after", "handler", "callback", "listener",
];

fn alphanumeric_lowercase(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn split_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_was_lowercase = false;

    for ch in value.chars() {
        if !ch.is_ascii_alphanumeric() {
            push_token(&mut tokens, &mut current);
            previous_was_lowercase = false;
            continue;
        }

        if ch.is_ascii_uppercase() && previous_was_lowercase {
            push_token(&mut tokens, &mut current);
        }

        previous_was_lowercase = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        current.push(ch.to_ascii_lowercase());
    }

    push_token(&mut tokens, &mut current);
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        tokens.push(std::mem::take(current));
    }
}

fn contains_token_sequence(line_tokens: &[String], target_tokens: &[String]) -> bool {
    target_tokens.len() <= line_tokens.len()
        && line_tokens
            .windows(target_tokens.len())
            .any(|window| window == target_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(body: &str) -> PackLocation {
        PackLocation {
            symbol_id: Some("source-symbol".to_string()),
            symbol_name: Some("source".to_string()),
            file_path: Some("src/config.ts".to_string()),
            kind: Some("function".to_string()),
            start_line: Some(10),
            end_line: Some(10),
            via: Some("search_code".to_string()),
            body: Some(body.to_string()),
        }
    }

    #[test]
    fn extracts_callback_producer_candidate() {
        let candidates = extract_non_callgraph_candidates(
            "toolUse",
            &[location(
                "const config = {\n  onBeforeToolUse: async () => dispatchToolUse(),\n};",
            )],
            NonCallgraphShape::Pipeline,
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].kind.as_deref(), Some("callback_producer"));
        assert_eq!(candidates[0].via.as_deref(), Some("non_callgraph_edges"));
        assert_eq!(candidates[0].file_path.as_deref(), Some("src/config.ts"));
        assert_eq!(candidates[0].start_line, Some(11));
        assert_eq!(candidates[0].end_line, Some(11));
        assert_eq!(
            candidates[0].body.as_deref(),
            Some("onBeforeToolUse: async () => dispatchToolUse(),")
        );
        assert_eq!(
            candidates[0].symbol_id.as_deref(),
            Some("source-symbol:callback_producer:11")
        );
    }

    #[test]
    fn extracts_event_emitter_and_subscriber_candidates() {
        let candidates = extract_non_callgraph_candidates(
            "tool-use",
            &[location(
                "webContents.send('tool-use', payload);\nipcRenderer.on('tool-use', handler);",
            )],
            NonCallgraphShape::Pipeline,
        );

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].kind.as_deref(), Some("event_emitter"));
        assert_eq!(candidates[0].start_line, Some(10));
        assert_eq!(
            candidates[0].body.as_deref(),
            Some("webContents.send('tool-use', payload);")
        );
        assert_eq!(candidates[1].kind.as_deref(), Some("event_subscriber"));
        assert_eq!(candidates[1].start_line, Some(11));
        assert_eq!(
            candidates[1].body.as_deref(),
            Some("ipcRenderer.on('tool-use', handler);")
        );
    }

    #[test]
    fn callsite_shape_extracts_config_hook_candidate() {
        let candidates = extract_non_callgraph_candidates(
            "createSession",
            &[location(
                "const options = {\n  sessionHandler: () => createSession(),\n};",
            )],
            NonCallgraphShape::Callsite,
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].kind.as_deref(), Some("config_hook"));
        assert_eq!(candidates[0].via.as_deref(), Some("non_callgraph_edges"));
        assert_eq!(candidates[0].start_line, Some(11));
        assert_eq!(
            candidates[0].body.as_deref(),
            Some("sessionHandler: () => createSession(),")
        );
        assert_eq!(
            candidates[0].symbol_id.as_deref(),
            Some("source-symbol:config_hook:11")
        );
    }

    #[test]
    fn pipeline_ignores_unrelated_response_send() {
        let candidates = extract_non_callgraph_candidates(
            "tool-use",
            &[location("response.send('health', payload);")],
            NonCallgraphShape::Pipeline,
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn pipeline_ignores_unrelated_button_subscriber() {
        let candidates = extract_non_callgraph_candidates(
            "tool-use",
            &[location("button.on('click', handler);")],
            NonCallgraphShape::Pipeline,
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn pipeline_ignores_unrelated_before_after_words() {
        let candidates = extract_non_callgraph_candidates(
            "tool-use",
            &[location("const afterDelay = 100;")],
            NonCallgraphShape::Pipeline,
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn pipeline_ignores_unrelated_ipc_subscriber_channel() {
        let candidates = extract_non_callgraph_candidates(
            "tool-use",
            &[location("ipcRenderer.on('session-message', handler);")],
            NonCallgraphShape::Pipeline,
        );

        assert!(candidates.is_empty());
    }
}

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
                NonCallgraphShape::Pipeline => pipeline_candidate_kind(trimmed),
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

fn pipeline_candidate_kind(line: &str) -> Option<&'static str> {
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
}

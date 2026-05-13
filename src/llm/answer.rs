//! Answer synthesis prompt + citation validator for `ask_code`.
//!
//! Pure-logic module. No I/O. The `ask_code` handler composes:
//!   1. `build_answer_prompt(question, evidence)` -> Qwen2.5 chat-template prompt.
//!   2. `LlmGenerator::generate(prompt, max_tokens)` -> raw answer text.
//!   3. `parse_citations(answer)` -> structured citation refs.
//!   4. `validate_citations(parsed, evidence)` -> resolved citations + dropped count.
//!   5. `compute_confidence(...)` -> `high` / `medium` / `low` for the response.
//!
//! The validator drops any citation the LLM emits that does not resolve to an
//! entry in `evidence`. This is the structural defence against the
//! hallucinated-file-paths failure mode (R006 q12).
//!
//! Citation forms accepted from the LLM:
//!   - `[file/path.rs:42]`   - file + line, bracketed
//!   - `(file/path.rs:42)`   - file + line, parenthesised
//!   - `[#symbol_id]`        - explicit symbol_id, bracketed
//!   - `(#symbol_id)`        - explicit symbol_id, parenthesised
//!
//! Looser forms ("at foo.rs line 42", inline prose mentions) are not parsed.
//! The prompt explicitly instructs the LLM to use the bracket form so this is
//! a defensible constraint.

use serde::{Deserialize, Serialize};

/// One verified piece of evidence the LLM is allowed to cite. Mirrors
/// `investigation::VerifiedLocation` but is decoupled so this module has no
/// dependency on the handler module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceItem {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file_path: String,
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    pub body: String,
}

/// One citation extracted from the LLM's answer text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCitation {
    /// The raw citation as it appeared in the answer (e.g. `[src/foo.rs:42]`).
    pub raw: String,
    /// Byte offset of the citation's opening delimiter in the answer string.
    pub start: usize,
    /// Byte offset one past the closing delimiter.
    pub end: usize,
    pub form: CitationForm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CitationForm {
    FileLine { file: String, line: u32 },
    SymbolId(String),
}

/// A citation that resolved successfully to an evidence entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSpan {
    pub symbol_id: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// Outcome of validating a batch of parsed citations against evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    pub kept: Vec<(ParsedCitation, ResolvedSpan)>,
    pub dropped: Vec<ParsedCitation>,
}

impl ValidationResult {
    pub fn kept_count(&self) -> usize {
        self.kept.len()
    }
    pub fn dropped_count(&self) -> usize {
        self.dropped.len()
    }
    pub fn total_count(&self) -> usize {
        self.kept.len() + self.dropped.len()
    }
}

/// Server-facing confidence label for the answer as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// Build a Qwen2.5 chat-template prompt for grounded answer synthesis.
///
/// The system prompt establishes the citation contract; the user message
/// includes the question and numbered evidence entries with bodies trimmed
/// to fit within the model's context budget.
pub fn build_answer_prompt(question: &str, evidence: &[EvidenceItem]) -> String {
    let mut user = String::new();
    user.push_str("Question: ");
    user.push_str(question.trim());
    user.push_str("\n\nEvidence:\n");

    if evidence.is_empty() {
        user.push_str("(no evidence retrieved)\n");
    } else {
        for (i, e) in evidence.iter().enumerate() {
            user.push_str(&format!(
                "\n[{idx}] {kind} `{name}` at {path}:{start}-{end} (id: {id})\n",
                idx = i + 1,
                kind = e.kind,
                name = e.symbol_name,
                path = e.file_path,
                start = e.start_line,
                end = e.end_line,
                id = e.symbol_id,
            ));
            let body = trim_body_for_prompt(&e.body, MAX_BODY_LINES_PER_ITEM);
            user.push_str("```\n");
            user.push_str(&body);
            if !body.ends_with('\n') {
                user.push('\n');
            }
            user.push_str("```\n");
        }
    }

    format!(
        "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n",
        system = ANSWER_SYSTEM_PROMPT.trim(),
        user = user,
    )
}

const ANSWER_SYSTEM_PROMPT: &str = r#"
You answer questions about a codebase using ONLY the evidence provided.

Rules:
1. Every factual claim about specific code MUST be followed by a citation in square brackets, using one of these exact forms:
     [file/path.rs:LINE]      e.g. [src/retrieval/mod.rs:142]
     [#symbol_id]              e.g. [#abc12345]
2. If a question cannot be fully answered from the evidence, say so explicitly. Do NOT invent file paths, line numbers, or symbol names.
3. Keep the answer under 400 tokens. Prefer short, direct sentences over preamble.
4. When evidence shows a multi-step flow, list the steps in order with one citation per step.
5. Do not repeat the question. Start the answer directly.
"#;

/// Maximum body lines per evidence entry in the prompt. Beyond this we
/// truncate with a `// ... N more lines` marker so the LLM sees a definite
/// bound rather than mistaking the cut for the function's end.
const MAX_BODY_LINES_PER_ITEM: usize = 80;

fn trim_body_for_prompt(body: &str, max_lines: usize) -> String {
    let total = body.lines().count();
    let kept: Vec<&str> = body.lines().take(max_lines).map(|l| l.trim_end()).collect();
    let mut out = kept.join("\n");
    if total > max_lines {
        out.push_str(&format!("\n// ... {} more lines", total - max_lines));
    }
    out
}

/// Scan `answer` for citation tokens. Returns them in source order. Only the
/// four documented forms are recognised; looser inline mentions are ignored.
///
/// Implementation note: a full regex engine is overkill. We do a single-pass
/// scan over bracket pairs and validate the contents.
pub fn parse_citations(answer: &str) -> Vec<ParsedCitation> {
    let mut out = Vec::new();
    let bytes = answer.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let (open, close) = match bytes[i] {
            b'[' => (b'[', b']'),
            b'(' => (b'(', b')'),
            _ => {
                i += 1;
                continue;
            }
        };
        // Find the matching close on the same line (citations don't span lines).
        let mut j = i + 1;
        let mut closed = None;
        while j < bytes.len() {
            if bytes[j] == b'\n' {
                break;
            }
            if bytes[j] == close {
                closed = Some(j);
                break;
            }
            j += 1;
        }
        let Some(close_idx) = closed else {
            i += 1;
            continue;
        };
        let inner = &answer[i + 1..close_idx];
        if let Some(form) = parse_citation_inner(inner) {
            out.push(ParsedCitation {
                raw: answer[i..=close_idx].to_string(),
                start: i,
                end: close_idx + 1,
                form,
            });
            // Skip past the closing delim for the next iteration.
            i = close_idx + 1;
        } else {
            i += 1;
        }
        let _ = open; // suppress unused-warn on the open delim byte
    }
    out
}

fn parse_citation_inner(s: &str) -> Option<CitationForm> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(id) = s.strip_prefix('#') {
        let id = id.trim();
        if id.is_empty() || !id_chars_ok(id) {
            return None;
        }
        return Some(CitationForm::SymbolId(id.to_string()));
    }
    // file:line — last ':' splits (file paths may contain '.', '/', '-').
    let colon = s.rfind(':')?;
    let (file_raw, line_raw) = (&s[..colon], &s[colon + 1..]);
    let file = file_raw.trim();
    let line_str = line_raw.trim();
    if file.is_empty() || line_str.is_empty() {
        return None;
    }
    // Path-shape sanity: contains a '/' or a '.rs' style suffix. This filters
    // bracket pairs that happen to contain a colon but aren't citations.
    if !file.contains('/') && !file.contains('.') {
        return None;
    }
    let line: u32 = line_str.parse().ok()?;
    Some(CitationForm::FileLine {
        file: file.to_string(),
        line,
    })
}

fn id_chars_ok(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Try to resolve a single citation against the evidence list.
///
/// FileLine match: file_path equals citation's file, and citation's line is
/// within `[start_line, end_line]` of an evidence entry.
///
/// SymbolId match: any evidence entry's `symbol_id` equals the citation.
pub fn resolve_citation(
    citation: &CitationForm,
    evidence: &[EvidenceItem],
) -> Option<ResolvedSpan> {
    match citation {
        CitationForm::SymbolId(id) => {
            evidence
                .iter()
                .find(|e| &e.symbol_id == id)
                .map(|e| ResolvedSpan {
                    symbol_id: e.symbol_id.clone(),
                    file_path: e.file_path.clone(),
                    start_line: e.start_line,
                    end_line: e.end_line,
                })
        }
        CitationForm::FileLine { file, line } => evidence
            .iter()
            .find(|e| &e.file_path == file && *line >= e.start_line && *line <= e.end_line)
            .map(|e| ResolvedSpan {
                symbol_id: e.symbol_id.clone(),
                file_path: e.file_path.clone(),
                start_line: e.start_line,
                end_line: e.end_line,
            }),
    }
}

/// Validate every parsed citation against the evidence. Returns kept (with
/// resolved spans) and dropped lists. Order preserved from input.
pub fn validate_citations(
    parsed: &[ParsedCitation],
    evidence: &[EvidenceItem],
) -> ValidationResult {
    let mut kept = Vec::with_capacity(parsed.len());
    let mut dropped = Vec::new();
    for p in parsed {
        match resolve_citation(&p.form, evidence) {
            Some(span) => kept.push((p.clone(), span)),
            None => dropped.push(p.clone()),
        }
    }
    ValidationResult { kept, dropped }
}

/// Compute the overall confidence label for the answer based on citation
/// resolution rates and whether any evidence was returned at all.
///
/// Rules:
///   - `Low` if no evidence was retrieved at all, OR if no citations resolved
///     when at least one was attempted.
///   - `Low` if more than half of attempted citations were dropped.
///   - `High` if all attempted citations resolved AND at least one was made.
///   - `Medium` otherwise (some citations resolved; some dropped).
///
/// An answer with zero parsed citations but non-empty evidence is `Medium`:
/// the LLM may have answered without explicit citations, which is suspicious
/// but not necessarily wrong (e.g. the answer references a single symbol the
/// agent can infer from context).
pub fn compute_confidence(
    parsed_count: usize,
    kept_count: usize,
    evidence_count: usize,
) -> Confidence {
    if evidence_count == 0 {
        return Confidence::Low;
    }
    if parsed_count == 0 {
        return Confidence::Medium;
    }
    if kept_count == 0 {
        return Confidence::Low;
    }
    if kept_count == parsed_count {
        return Confidence::High;
    }
    // Some kept, some dropped.
    let ratio = kept_count as f32 / parsed_count as f32;
    if ratio < 0.5 {
        Confidence::Low
    } else {
        Confidence::Medium
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, file: &str, start: u32, end: u32) -> EvidenceItem {
        EvidenceItem {
            symbol_id: id.to_string(),
            symbol_name: "fn_x".to_string(),
            file_path: file.to_string(),
            kind: "function".to_string(),
            start_line: start,
            end_line: end,
            body: "fn fn_x() { /* body */ }".to_string(),
        }
    }

    #[test]
    fn prompt_includes_question_and_evidence_entries() {
        let evidence = vec![ev("sym_a", "src/foo.rs", 10, 20)];
        let prompt = build_answer_prompt("Where is fn_x defined?", &evidence);
        assert!(prompt.contains("<|im_start|>system"));
        assert!(prompt.contains("Where is fn_x defined?"));
        assert!(prompt.contains("[1] function `fn_x` at src/foo.rs:10-20"));
        assert!(prompt.contains("(id: sym_a)"));
        assert!(prompt.contains("```"));
    }

    #[test]
    fn prompt_handles_empty_evidence() {
        let prompt = build_answer_prompt("Anything?", &[]);
        assert!(prompt.contains("(no evidence retrieved)"));
    }

    #[test]
    fn prompt_system_documents_citation_contract() {
        let prompt = build_answer_prompt("q", &[]);
        assert!(prompt.contains("[file/path.rs:LINE]"));
        assert!(prompt.contains("[#symbol_id]"));
        assert!(prompt.contains("Do NOT invent"));
    }

    #[test]
    fn prompt_truncates_long_bodies_with_marker() {
        let mut e = ev("sym", "src/x.rs", 1, 200);
        e.body = (0..120).map(|i| format!("line {}\n", i)).collect();
        let prompt = build_answer_prompt("q", &[e]);
        assert!(prompt.contains("// ... 40 more lines"));
        assert!(prompt.contains("line 0"));
        assert!(!prompt.contains("line 100"));
    }

    #[test]
    fn parse_extracts_file_line_in_brackets() {
        let cites = parse_citations("Function is defined [src/retrieval/mod.rs:142] then called.");
        assert_eq!(cites.len(), 1);
        assert_eq!(
            cites[0].form,
            CitationForm::FileLine {
                file: "src/retrieval/mod.rs".to_string(),
                line: 142
            }
        );
        assert_eq!(cites[0].raw, "[src/retrieval/mod.rs:142]");
    }

    #[test]
    fn parse_extracts_file_line_in_parens() {
        let cites = parse_citations("See (src/path/mod.rs:42) for details.");
        assert_eq!(cites.len(), 1);
        assert_eq!(
            cites[0].form,
            CitationForm::FileLine {
                file: "src/path/mod.rs".to_string(),
                line: 42
            }
        );
    }

    #[test]
    fn parse_extracts_symbol_id_form() {
        let cites = parse_citations("Defined at [#abc_123].");
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].form, CitationForm::SymbolId("abc_123".to_string()));
    }

    #[test]
    fn parse_extracts_multiple_citations_in_order() {
        let answer = "First [src/a.rs:1], then [src/b.rs:2], finally [#z9].";
        let cites = parse_citations(answer);
        assert_eq!(cites.len(), 3);
        match &cites[0].form {
            CitationForm::FileLine { file, line } => {
                assert_eq!(file, "src/a.rs");
                assert_eq!(*line, 1);
            }
            other => panic!("expected file:line, got {other:?}"),
        }
        assert_eq!(
            cites[1].form,
            CitationForm::FileLine {
                file: "src/b.rs".into(),
                line: 2
            }
        );
        assert_eq!(cites[2].form, CitationForm::SymbolId("z9".into()));
    }

    #[test]
    fn parse_ignores_non_citation_brackets() {
        let answer = "Some [text] without a colon or hash. Also [foo bar] which isn't valid.";
        let cites = parse_citations(answer);
        assert!(cites.is_empty(), "got: {:?}", cites);
    }

    #[test]
    fn parse_ignores_bracket_pairs_with_colon_but_no_path_shape() {
        // "1:00" isn't a path:line citation - no '/' or '.'.
        let cites = parse_citations("Started [1:00] in the morning.");
        assert!(cites.is_empty(), "got: {:?}", cites);
    }

    #[test]
    fn parse_does_not_span_newlines() {
        let answer = "Truncated [src/foo.rs\n:42] should not match.";
        let cites = parse_citations(answer);
        assert!(cites.is_empty(), "got: {:?}", cites);
    }

    #[test]
    fn resolve_symbol_id_exact_match() {
        let evidence = vec![
            ev("sym_a", "src/foo.rs", 10, 20),
            ev("sym_b", "src/bar.rs", 1, 5),
        ];
        let cite = CitationForm::SymbolId("sym_b".into());
        let resolved = resolve_citation(&cite, &evidence).expect("Some");
        assert_eq!(resolved.symbol_id, "sym_b");
        assert_eq!(resolved.file_path, "src/bar.rs");
    }

    #[test]
    fn resolve_file_line_within_range() {
        let evidence = vec![ev("sym_a", "src/foo.rs", 10, 20)];
        let cite = CitationForm::FileLine {
            file: "src/foo.rs".into(),
            line: 15,
        };
        let resolved = resolve_citation(&cite, &evidence).expect("Some");
        assert_eq!(resolved.symbol_id, "sym_a");
    }

    #[test]
    fn resolve_file_line_boundary_lines_inclusive() {
        let evidence = vec![ev("sym_a", "src/foo.rs", 10, 20)];
        // start_line and end_line are both inclusive.
        assert!(resolve_citation(
            &CitationForm::FileLine {
                file: "src/foo.rs".into(),
                line: 10
            },
            &evidence
        )
        .is_some());
        assert!(resolve_citation(
            &CitationForm::FileLine {
                file: "src/foo.rs".into(),
                line: 20
            },
            &evidence
        )
        .is_some());
        assert!(resolve_citation(
            &CitationForm::FileLine {
                file: "src/foo.rs".into(),
                line: 9
            },
            &evidence
        )
        .is_none());
        assert!(resolve_citation(
            &CitationForm::FileLine {
                file: "src/foo.rs".into(),
                line: 21
            },
            &evidence
        )
        .is_none());
    }

    #[test]
    fn resolve_returns_none_for_unknown_file() {
        let evidence = vec![ev("sym_a", "src/foo.rs", 10, 20)];
        let cite = CitationForm::FileLine {
            file: "src/other.rs".into(),
            line: 15,
        };
        assert!(resolve_citation(&cite, &evidence).is_none());
    }

    #[test]
    fn resolve_returns_none_for_unknown_symbol_id() {
        let evidence = vec![ev("sym_a", "src/foo.rs", 10, 20)];
        let cite = CitationForm::SymbolId("sym_zzz".into());
        assert!(resolve_citation(&cite, &evidence).is_none());
    }

    #[test]
    fn validate_keeps_resolved_drops_unresolved() {
        let evidence = vec![ev("sym_a", "src/foo.rs", 10, 20)];
        let parsed = vec![
            ParsedCitation {
                raw: "[src/foo.rs:15]".into(),
                start: 0,
                end: 16,
                form: CitationForm::FileLine {
                    file: "src/foo.rs".into(),
                    line: 15,
                },
            },
            ParsedCitation {
                raw: "[#sym_ghost]".into(),
                start: 17,
                end: 29,
                form: CitationForm::SymbolId("sym_ghost".into()),
            },
        ];
        let result = validate_citations(&parsed, &evidence);
        assert_eq!(result.kept_count(), 1);
        assert_eq!(result.dropped_count(), 1);
        assert_eq!(result.kept[0].1.symbol_id, "sym_a");
        assert_eq!(result.dropped[0].raw, "[#sym_ghost]");
    }

    #[test]
    fn confidence_low_when_no_evidence() {
        assert_eq!(compute_confidence(0, 0, 0), Confidence::Low);
        assert_eq!(compute_confidence(5, 5, 0), Confidence::Low);
    }

    #[test]
    fn confidence_medium_when_no_citations_attempted_but_evidence_present() {
        assert_eq!(compute_confidence(0, 0, 3), Confidence::Medium);
    }

    #[test]
    fn confidence_high_when_all_citations_resolved() {
        assert_eq!(compute_confidence(3, 3, 5), Confidence::High);
        assert_eq!(compute_confidence(1, 1, 1), Confidence::High);
    }

    #[test]
    fn confidence_low_when_all_citations_dropped() {
        assert_eq!(compute_confidence(3, 0, 3), Confidence::Low);
    }

    #[test]
    fn confidence_low_when_below_half_resolved() {
        assert_eq!(compute_confidence(4, 1, 4), Confidence::Low);
    }

    #[test]
    fn confidence_medium_when_at_least_half_resolved() {
        assert_eq!(compute_confidence(4, 2, 4), Confidence::Medium);
        assert_eq!(compute_confidence(4, 3, 4), Confidence::Medium);
    }

    #[test]
    fn confidence_string_repr_matches_response_contract() {
        assert_eq!(Confidence::High.as_str(), "high");
        assert_eq!(Confidence::Medium.as_str(), "medium");
        assert_eq!(Confidence::Low.as_str(), "low");
    }
}

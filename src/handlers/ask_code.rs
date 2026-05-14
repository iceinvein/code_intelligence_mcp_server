//! `ask_code` composite handler.
//!
//! Pipeline:
//!   1. Run `handle_investigate` to retrieve evidence.
//!   2. Adapt verified_locations into `EvidenceItem`s for the answer prompt.
//!   3. Lazy-load the answer LLM (Phase 3 wires the actual loader; for now
//!      the handler falls back gracefully when no generator is available).
//!   4. Generate answer; parse and validate citations against the evidence.
//!   5. Compute confidence and emit a structured response.
//!
//! The agent's job after this call is to present the `answer` text to the
//! user. The `citations[]` list is server-validated, so any `file:line` or
//! `symbol_id` referenced there is guaranteed to exist in the index.

use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};

use crate::llm::answer::{
    build_answer_prompt, compute_confidence, parse_citations, validate_citations, Confidence,
    EvidenceItem, ResolvedSpan,
};
use crate::llm::create_llm_generator_with_ctx;
use crate::llm::LlmGenerator;
use crate::tools::{AskCodeTool, InvestigateTool};

use super::ask_code_cache::AskCodeCacheKey;
use super::investigation::handle_investigate;
use super::AppState;

/// Default evidence budget for the answer prompt. Clamped 1..=15.
const DEFAULT_MAX_EVIDENCE: u32 = 8;
const MAX_EVIDENCE_HARD_CAP: u32 = 15;

/// Token budget for the LLM's completion. Should fit the 400-token answer
/// guidance in the system prompt with headroom for citations.
const ANSWER_MAX_TOKENS_BALANCED: u32 = 512;
const ANSWER_MAX_TOKENS_FAST: u32 = 256;

/// Wall-clock parameter used in the response so callers can see what quality
/// tier ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerQuality {
    Fast,
    Balanced,
}

impl AnswerQuality {
    fn from_str(s: Option<&str>) -> Self {
        match s.map(str::to_ascii_lowercase).as_deref() {
            Some("fast") => Self::Fast,
            // Default + any unknown value -> balanced.
            _ => Self::Balanced,
        }
    }
    fn max_tokens(self) -> u32 {
        match self {
            Self::Fast => ANSWER_MAX_TOKENS_FAST,
            Self::Balanced => ANSWER_MAX_TOKENS_BALANCED,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
        }
    }
}

pub async fn handle_ask_code(state: &AppState, tool: AskCodeTool) -> Result<Value> {
    let question = tool.question.trim().to_string();
    if question.is_empty() {
        anyhow::bail!("question is required");
    }
    let max_evidence = tool
        .max_evidence
        .unwrap_or(DEFAULT_MAX_EVIDENCE)
        .clamp(1, MAX_EVIDENCE_HARD_CAP);
    let quality = AnswerQuality::from_str(tool.quality.as_deref());

    // ---- Cache lookup (keyed on question + index version + quality) ----
    let repo_index_version = state
        .sqlite
        .latest_index_run()
        .ok()
        .flatten()
        .map(|run| run.started_at_unix_s)
        .unwrap_or(0);
    let cache_key = AskCodeCacheKey::new(&question, repo_index_version, quality);
    if let Some(mut cached) = state.ask_code_cache.get(&cache_key) {
        if let Some(obj) = cached.as_object_mut() {
            obj.insert("cached".to_string(), json!(true));
        }
        return Ok(cached);
    }

    // ---- Step 1: retrieval via investigate -----------------------------
    let investigate_tool = InvestigateTool {
        question: question.clone(),
        target: tool.target.clone(),
        file_path: tool.file_path.clone(),
        mode: tool.mode.clone(),
        max_hops: None,
    };
    let investigate_response = handle_investigate(state, investigate_tool).await?;

    let mut evidence = extract_evidence_from_investigate(&investigate_response, max_evidence);
    let evidence_count = evidence.len();

    // ---- Path 2 (default): evidence-only response ----------------------
    // Local-LLM synthesis routinely emits hallucinated or shallow prose that
    // the agent then anchors on, producing worse final answers than if it
    // had read the structured evidence directly. We default to skipping the
    // local synthesis step entirely; set `ASK_CODE_LLM_SYNTHESIS=true` to
    // restore the LLM path (e.g. for experiments with a stronger model).
    if !llm_synthesis_enabled() {
        evidence.truncate(max_evidence as usize);
        let response = build_evidence_only_response(
            &question,
            quality,
            &investigate_response,
            &evidence,
            evidence_count,
        );
        state.ask_code_cache.put(cache_key, response.clone());
        return Ok(response);
    }

    // ---- Step 2: try to obtain an answer LLM ---------------------------
    let generator = get_or_init_answer_generator(state);

    let Some(generator) = generator else {
        // `llm_unavailable` responses are not cacheable (see is_cacheable):
        // the load may succeed on a later call. Return without caching.
        return Ok(build_unavailable_response(
            &question,
            quality,
            &investigate_response,
            &evidence,
        ));
    };

    // ---- Step 3: prompt + generate -------------------------------------
    let prompt = build_answer_prompt(&question, &evidence);
    let raw_answer = generator.generate(&prompt, quality.max_tokens())?;
    let answer_text = raw_answer.trim().to_string();

    // ---- Step 4: parse + validate citations ---------------------------
    let parsed = parse_citations(&answer_text);
    let validation = validate_citations(&parsed, &evidence);
    let confidence = compute_confidence(parsed.len(), validation.kept_count(), evidence_count);

    let stop_reason = if evidence_count == 0 {
        "no_evidence"
    } else if matches!(confidence, Confidence::Low) {
        "low_confidence"
    } else {
        "answered"
    };

    let follow_up = follow_up_hint(
        confidence,
        evidence_count,
        parsed.len(),
        validation.dropped_count(),
    );

    // Trim evidence we return to the agent to the slice that actually
    // backed the answer; cap stays at max_evidence so the response stays
    // bounded. Identical to the slice fed to the prompt.
    evidence.truncate(max_evidence as usize);

    let citations_json: Vec<Value> = validation
        .kept
        .iter()
        .enumerate()
        .map(|(idx, (parsed_cite, span))| citation_to_json(idx, parsed_cite.raw.as_str(), span))
        .collect();

    let response = json!({
        "question": question,
        "answer": answer_text,
        "citations": citations_json,
        "evidence": evidence_to_json(&evidence),
        "confidence": confidence.as_str(),
        "mode_used": investigate_response.get("mode_used").cloned().unwrap_or(json!(null)),
        "stop_reason": stop_reason,
        "quality": quality.as_str(),
        "follow_up": follow_up,
        "dropped_citation_count": validation.dropped_count(),
        "evidence_count": evidence_count,
        "cached": false,
    });
    // is_cacheable() drops llm_unavailable; everything else is deterministic
    // given (question, index_version, quality) so it's safe to memoise.
    state.ask_code_cache.put(cache_key, response.clone());
    Ok(response)
}

/// Extract `EvidenceItem`s from an `investigate` response's
/// `verified_locations` array, capped at `max_evidence`.
fn extract_evidence_from_investigate(response: &Value, max_evidence: u32) -> Vec<EvidenceItem> {
    let Some(arr) = response
        .get("verified_locations")
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    arr.iter()
        .take(max_evidence as usize)
        .filter_map(|v| {
            let obj = v.as_object()?;
            Some(EvidenceItem {
                symbol_id: obj.get("symbol_id")?.as_str()?.to_string(),
                symbol_name: obj
                    .get("symbol_name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                file_path: obj
                    .get("file_path")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                kind: obj
                    .get("kind")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                start_line: obj.get("start_line").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                end_line: obj.get("end_line").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                body: obj
                    .get("body")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

fn evidence_to_json(evidence: &[EvidenceItem]) -> Vec<Value> {
    evidence
        .iter()
        .map(|e| {
            json!({
                "symbol_id": e.symbol_id,
                "symbol_name": e.symbol_name,
                "file_path": e.file_path,
                "kind": e.kind,
                "start_line": e.start_line,
                "end_line": e.end_line,
                "body": e.body,
            })
        })
        .collect()
}

fn citation_to_json(claim_index: usize, raw: &str, span: &ResolvedSpan) -> Value {
    json!({
        "claim_index": claim_index,
        "raw": raw,
        "symbol_id": span.symbol_id,
        "file_path": span.file_path,
        "start_line": span.start_line,
        "end_line": span.end_line,
    })
}

/// Lazy-load (or return cached) answer LLM. On miss, attempts to construct
/// one via `create_llm_generator_with_ctx`. The answer LLM uses
/// `config.answer_llm_n_ctx` (default 16384), not the 512 used by the
/// description pipeline — `build_answer_prompt` embeds retrieved code
/// evidence so prompts routinely exceed several thousand tokens. If
/// construction fails or LLM is disabled, caches `None` so subsequent calls
/// short-circuit immediately.
fn get_or_init_answer_generator(state: &AppState) -> Option<std::sync::Arc<dyn LlmGenerator>> {
    let cell = &state.answer_generator;
    let n_ctx = state.config.answer_llm_n_ctx;
    let slot = cell.get_or_init(|| match create_llm_generator_with_ctx(&state.config, n_ctx) {
        Ok(maybe) => maybe,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "ask_code: failed to construct answer LLM; falling back to evidence-only response"
            );
            None
        }
    });
    slot.clone()
}

/// Response shape used when the LLM is unavailable. Returns the evidence
/// the agent would have got anyway, plus a clear `stop_reason` so the
/// caller knows to fall back to specialist tools or `investigate` directly.
fn build_unavailable_response(
    question: &str,
    quality: AnswerQuality,
    investigate_response: &Value,
    evidence: &[EvidenceItem],
) -> Value {
    json!({
        "question": question,
        "answer": "",
        "citations": [],
        "evidence": evidence_to_json(evidence),
        "confidence": Confidence::Low.as_str(),
        "mode_used": investigate_response.get("mode_used").cloned().unwrap_or(json!(null)),
        "stop_reason": "llm_unavailable",
        "quality": quality.as_str(),
        "follow_up": "ask_code's answer LLM is not available (LLM_ENABLED=false, missing model, or load failure). Use the evidence[] array below or call `investigate` / specialist tools directly.",
        "dropped_citation_count": 0,
        "evidence_count": evidence.len(),
    })
}

/// Has local-LLM prose synthesis been re-enabled? Default is `false`: ask_code
/// returns retrieved + verified evidence and lets the agent synthesise the
/// final answer. The local Qwen 1.5B / 3B variants produced hallucinated prose
/// during v3.3 evaluation that the agent then anchored on, worsening final
/// answer quality. Set `ASK_CODE_LLM_SYNTHESIS=true` (or `1`) to restore the
/// LLM path -- intended for experiments with stronger models.
fn llm_synthesis_enabled() -> bool {
    parse_synthesis_flag(std::env::var("ASK_CODE_LLM_SYNTHESIS").ok().as_deref())
}

/// Parse the truthy-string contract used by `ASK_CODE_LLM_SYNTHESIS`. Factored
/// out so unit tests can exercise the parsing without touching process env.
fn parse_synthesis_flag(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// Evidence-only response (Path 2). No prose is generated; the agent receives
/// the question, the retrieved + verified evidence, and a clear instruction
/// to synthesise the user-facing answer itself. Confidence is derived from
/// evidence count alone since no LLM citations exist to validate.
fn build_evidence_only_response(
    question: &str,
    quality: AnswerQuality,
    investigate_response: &Value,
    evidence: &[EvidenceItem],
    evidence_count: usize,
) -> Value {
    let confidence = if evidence_count >= 3 {
        Confidence::Medium
    } else if evidence_count > 0 {
        Confidence::Low
    } else {
        Confidence::Low
    };
    let stop_reason = if evidence_count == 0 {
        "no_evidence"
    } else {
        "evidence_only"
    };
    json!({
        "question": question,
        "answer": "",
        "citations": [],
        "evidence": evidence_to_json(evidence),
        "confidence": confidence.as_str(),
        "mode_used": investigate_response.get("mode_used").cloned().unwrap_or(json!(null)),
        "stop_reason": stop_reason,
        "quality": quality.as_str(),
        "follow_up": "ask_code returned verified evidence without LLM prose (Path 2 default). \
            Synthesise the final answer yourself from the `evidence[]` array below: each item carries \
            symbol_name, file_path, line range, and the actual code body. Use these as the source of \
            truth -- they were already retrieved and shape-classified by `investigate`. If you need \
            more context call specialist tools (find_references, get_call_hierarchy, get_definition) \
            with the symbol_ids from evidence.",
        "dropped_citation_count": 0,
        "evidence_count": evidence_count,
        "cached": false,
    })
}

fn follow_up_hint(
    confidence: Confidence,
    evidence_count: usize,
    parsed_count: usize,
    dropped_count: usize,
) -> Option<&'static str> {
    if evidence_count == 0 {
        return Some(
            "No evidence retrieved. Rephrase the question with a more specific target, or call \
             investigate / search_code directly to inspect what the index has.",
        );
    }
    if matches!(confidence, Confidence::Low) && parsed_count > 0 && dropped_count > 0 {
        return Some(
            "More than half of the LLM's citations did not resolve to retrieved evidence. The \
             answer may be partially hallucinated. Call `investigate` for raw evidence or \
             specialist tools (find_references / get_call_hierarchy) for the symbols you care \
             about.",
        );
    }
    if matches!(confidence, Confidence::Medium) && parsed_count == 0 {
        return Some(
            "The LLM did not cite any specific evidence. Treat the answer as a high-level \
             summary; call `get_definition` or `find_references` on the symbols you need to \
             verify.",
        );
    }
    None
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
            body: "body".to_string(),
        }
    }

    #[test]
    fn quality_default_is_balanced() {
        assert_eq!(AnswerQuality::from_str(None), AnswerQuality::Balanced);
        assert_eq!(
            AnswerQuality::from_str(Some("balanced")),
            AnswerQuality::Balanced
        );
        assert_eq!(
            AnswerQuality::from_str(Some("garbage")),
            AnswerQuality::Balanced
        );
    }

    #[test]
    fn quality_fast_routed_explicitly() {
        assert_eq!(AnswerQuality::from_str(Some("fast")), AnswerQuality::Fast);
        assert_eq!(AnswerQuality::from_str(Some("FAST")), AnswerQuality::Fast);
    }

    #[test]
    fn quality_max_tokens_distinct_per_tier() {
        assert!(AnswerQuality::Balanced.max_tokens() > AnswerQuality::Fast.max_tokens());
    }

    #[test]
    fn extract_evidence_pulls_locations_and_caps() {
        let payload = json!({
            "verified_locations": [
                {"symbol_id": "a", "symbol_name": "fa", "file_path": "src/a.rs",
                 "kind": "function", "start_line": 1, "end_line": 10, "body": "fn fa() {}"},
                {"symbol_id": "b", "symbol_name": "fb", "file_path": "src/b.rs",
                 "kind": "function", "start_line": 20, "end_line": 40, "body": "fn fb() {}"},
                {"symbol_id": "c", "symbol_name": "fc", "file_path": "src/c.rs",
                 "kind": "function", "start_line": 5, "end_line": 9, "body": "fn fc() {}"},
            ]
        });
        let ev = extract_evidence_from_investigate(&payload, 2);
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].symbol_id, "a");
        assert_eq!(ev[1].symbol_id, "b");
        // Confirm field plumbing.
        assert_eq!(ev[0].file_path, "src/a.rs");
        assert_eq!(ev[0].start_line, 1);
        assert_eq!(ev[0].end_line, 10);
        assert_eq!(ev[0].kind, "function");
        assert_eq!(ev[1].symbol_name, "fb");
    }

    #[test]
    fn extract_evidence_returns_empty_when_locations_missing() {
        let payload = json!({"unrelated": "field"});
        assert!(extract_evidence_from_investigate(&payload, 5).is_empty());
    }

    #[test]
    fn extract_evidence_returns_empty_when_locations_not_array() {
        let payload = json!({"verified_locations": "not an array"});
        assert!(extract_evidence_from_investigate(&payload, 5).is_empty());
    }

    #[test]
    fn extract_evidence_skips_malformed_entries() {
        let payload = json!({
            "verified_locations": [
                {"symbol_id": "ok", "symbol_name": "fn", "file_path": "src/a.rs",
                 "kind": "function", "start_line": 1, "end_line": 2, "body": ""},
                {"missing_symbol_id": true},
                {"symbol_id": 12345}, // wrong type for symbol_id
            ]
        });
        let ev = extract_evidence_from_investigate(&payload, 5);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].symbol_id, "ok");
    }

    #[test]
    fn citation_to_json_carries_claim_index_and_resolved_span() {
        let span = ResolvedSpan {
            symbol_id: "sym".into(),
            file_path: "src/x.rs".into(),
            start_line: 5,
            end_line: 10,
        };
        let v = citation_to_json(3, "[src/x.rs:7]", &span);
        assert_eq!(v["claim_index"], 3);
        assert_eq!(v["raw"], "[src/x.rs:7]");
        assert_eq!(v["symbol_id"], "sym");
        assert_eq!(v["file_path"], "src/x.rs");
        assert_eq!(v["start_line"], 5);
        assert_eq!(v["end_line"], 10);
    }

    #[test]
    fn unavailable_response_carries_evidence_and_stop_reason() {
        let evidence = vec![ev("sym_a", "src/a.rs", 1, 5)];
        let investigate_response = json!({"mode_used": "discover"});
        let response = build_unavailable_response(
            "what is fn_x?",
            AnswerQuality::Balanced,
            &investigate_response,
            &evidence,
        );
        assert_eq!(response["stop_reason"], "llm_unavailable");
        assert_eq!(response["confidence"], "low");
        assert_eq!(response["mode_used"], "discover");
        assert_eq!(response["evidence_count"], 1);
        let evj = response["evidence"].as_array().unwrap();
        assert_eq!(evj.len(), 1);
        assert_eq!(evj[0]["symbol_id"], "sym_a");
        // Answer is empty so the caller doesn't relay a stub message.
        assert_eq!(response["answer"], "");
    }

    #[test]
    fn follow_up_hint_no_evidence_prompts_rephrase() {
        assert!(follow_up_hint(Confidence::Low, 0, 0, 0)
            .unwrap()
            .contains("No evidence"));
    }

    #[test]
    fn follow_up_hint_partial_citations_warns_hallucination() {
        let hint = follow_up_hint(Confidence::Low, 1, 3, 3).expect("Some");
        assert!(hint.to_lowercase().contains("hallucinated"));
    }

    #[test]
    fn follow_up_hint_medium_no_citations_flags_summary_only() {
        let hint = follow_up_hint(Confidence::Medium, 5, 0, 0).expect("Some");
        assert!(hint.to_lowercase().contains("high-level"));
    }

    #[test]
    fn follow_up_hint_high_returns_none() {
        assert!(follow_up_hint(Confidence::High, 5, 3, 0).is_none());
    }

    #[test]
    fn evidence_only_response_carries_evidence_and_stop_reason() {
        let evidence = vec![
            ev("sym_a", "src/a.rs", 1, 5),
            ev("sym_b", "src/b.rs", 10, 20),
            ev("sym_c", "src/c.rs", 30, 40),
        ];
        let investigate_response = json!({"mode_used": "discover"});
        let response = build_evidence_only_response(
            "where is fn_x defined?",
            AnswerQuality::Balanced,
            &investigate_response,
            &evidence,
            evidence.len(),
        );
        assert_eq!(response["stop_reason"], "evidence_only");
        assert_eq!(response["answer"], "");
        assert_eq!(response["dropped_citation_count"], 0);
        assert_eq!(response["evidence_count"], 3);
        assert_eq!(response["mode_used"], "discover");
        assert_eq!(
            response["citations"].as_array().map(|a| a.len()),
            Some(0),
            "evidence-only path never emits citations",
        );
        let evj = response["evidence"].as_array().unwrap();
        assert_eq!(evj.len(), 3);
        assert_eq!(evj[0]["symbol_id"], "sym_a");
        let follow_up = response["follow_up"].as_str().unwrap();
        assert!(
            follow_up.contains("evidence"),
            "follow_up must direct the agent at evidence[], got: {follow_up}",
        );
    }

    #[test]
    fn evidence_only_confidence_scales_with_count() {
        let investigate_response = json!({});
        // 0 items -> low + no_evidence stop reason
        let r0 = build_evidence_only_response(
            "q",
            AnswerQuality::Balanced,
            &investigate_response,
            &[],
            0,
        );
        assert_eq!(r0["confidence"], "low");
        assert_eq!(r0["stop_reason"], "no_evidence");

        // 1 item -> low + evidence_only
        let one = vec![ev("a", "src/a.rs", 1, 2)];
        let r1 = build_evidence_only_response(
            "q",
            AnswerQuality::Balanced,
            &investigate_response,
            &one,
            1,
        );
        assert_eq!(r1["confidence"], "low");
        assert_eq!(r1["stop_reason"], "evidence_only");

        // 3 items -> medium
        let three = vec![
            ev("a", "src/a.rs", 1, 2),
            ev("b", "src/b.rs", 1, 2),
            ev("c", "src/c.rs", 1, 2),
        ];
        let r3 = build_evidence_only_response(
            "q",
            AnswerQuality::Balanced,
            &investigate_response,
            &three,
            3,
        );
        assert_eq!(r3["confidence"], "medium");
        assert_eq!(r3["stop_reason"], "evidence_only");
    }

    #[test]
    fn parse_synthesis_flag_accepts_truthy_values() {
        for raw in ["1", "true", "yes", "on", "TRUE", "  YES  ", "On"] {
            assert!(
                parse_synthesis_flag(Some(raw)),
                "{raw:?} should enable synthesis",
            );
        }
    }

    #[test]
    fn parse_synthesis_flag_rejects_falsy_and_unset() {
        for raw in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("off"),
            Some("maybe"),
        ] {
            assert!(
                !parse_synthesis_flag(raw),
                "{raw:?} should keep evidence-only default",
            );
        }
    }
}

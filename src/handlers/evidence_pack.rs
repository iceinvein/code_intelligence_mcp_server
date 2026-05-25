use serde::Serialize;
use serde_json::Value;

use crate::handlers::investigation::InvestigationShape;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidencePackKind {
    CallsiteEnumeration,
    PipelineTrace,
    DataFlow,
    ImpactRadius,
    DependencyMap,
    SymbolLookup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Complete,
    Partial,
    NoHits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Coverage {
    pub status: CoverageStatus,
    pub basis: String,
    pub missing: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceRow {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enclosing_symbol: Option<String>,
    pub evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceEdge {
    pub from_ordinal: u32,
    pub to_ordinal: u32,
    pub relationship: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidencePack {
    pub kind: EvidencePackKind,
    pub target: String,
    pub coverage: Coverage,
    pub rows: Vec<EvidenceRow>,
    pub edges: Vec<EvidenceEdge>,
    pub answer_guidance: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackLocation {
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    pub file_path: Option<String>,
    pub kind: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub via: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidencePackInput {
    pub question: String,
    pub target: String,
    pub shape: InvestigationShape,
    pub primary: Vec<PackLocation>,
    pub secondary: Vec<PackLocation>,
    pub secondary_via: Option<String>,
}

pub fn build_evidence_pack(input: EvidencePackInput) -> EvidencePack {
    let kind = pack_kind(&input.question, input.shape);
    let mut rows = Vec::with_capacity(input.primary.len() + input.secondary.len());

    for location in input.primary {
        rows.push(row_from_location(
            location,
            &input.target,
            kind,
            "primary",
            rows.len() as u32 + 1,
        ));
    }

    let secondary_role = input.secondary_via.as_deref().unwrap_or("secondary");
    for location in input.secondary {
        rows.push(row_from_location(
            location,
            &input.target,
            kind,
            secondary_role,
            rows.len() as u32 + 1,
        ));
    }

    if matches!(kind, EvidencePackKind::PipelineTrace) {
        rows.sort_by_key(|row| row.ordinal.unwrap_or(u32::MAX));
    }

    let coverage = coverage_for(kind, &rows);

    EvidencePack {
        kind,
        target: input.target,
        coverage,
        rows,
        edges: Vec::new(),
        answer_guidance: answer_guidance(kind),
    }
}

pub fn pack_to_value(pack: &EvidencePack) -> Value {
    serde_json::to_value(pack).expect("evidence pack should serialize")
}

fn pack_kind(question: &str, shape: InvestigationShape) -> EvidencePackKind {
    if is_callsite_question(question) {
        return EvidencePackKind::CallsiteEnumeration;
    }

    match shape {
        InvestigationShape::CallTrace => EvidencePackKind::PipelineTrace,
        InvestigationShape::DataTrace => EvidencePackKind::DataFlow,
        InvestigationShape::ImpactRadius => EvidencePackKind::ImpactRadius,
        InvestigationShape::DependencyWalk => EvidencePackKind::DependencyMap,
        InvestigationShape::Discover | InvestigationShape::ModuleSurvey => {
            EvidencePackKind::SymbolLookup
        }
    }
}

fn is_callsite_question(question: &str) -> bool {
    let question = question.to_ascii_lowercase();
    ["who calls", "callsites", "invokes", "who references"]
        .iter()
        .any(|needle| question.contains(needle))
        || question.contains("references to")
        || (question.contains("where is")
            && (question.contains(" called")
                || question.contains(" call")
                || question.contains(" referenced")))
}

fn row_from_location(
    location: PackLocation,
    target: &str,
    kind: EvidencePackKind,
    fallback_role: &str,
    fallback_ordinal: u32,
) -> EvidenceRow {
    let body = location.body.as_deref();
    let selected = select_evidence_line(body, target);
    let evidence = selected.text;
    let line = match (location.start_line, selected.offset) {
        (Some(start_line), Some(offset)) => Some(start_line + offset),
        (Some(start_line), None) => Some(start_line),
        (None, _) => None,
    };
    let role = match kind {
        EvidencePackKind::CallsiteEnumeration => "callsite".to_string(),
        EvidencePackKind::PipelineTrace => infer_pipeline_role(body.unwrap_or(&evidence)),
        _ => location.via.as_deref().unwrap_or(fallback_role).to_string(),
    };
    let ordinal = match kind {
        EvidencePackKind::PipelineTrace => pipeline_ordinal(&role),
        _ => Some(fallback_ordinal),
    };

    EvidenceRow {
        role,
        ordinal,
        symbol_id: location.symbol_id,
        symbol_name: location.symbol_name,
        file_path: location.file_path,
        line,
        end_line: location.end_line,
        enclosing_symbol: None,
        evidence,
        reason: location.kind,
        risk: None,
    }
}

struct SelectedEvidenceLine {
    text: String,
    offset: Option<u32>,
}

fn select_evidence_line(body: Option<&str>, target: &str) -> SelectedEvidenceLine {
    let Some(body) = body else {
        return SelectedEvidenceLine {
            text: String::new(),
            offset: None,
        };
    };
    let final_segment = target
        .split(['.', ':', '#'])
        .filter(|segment| !segment.is_empty())
        .next_back()
        .unwrap_or(target);

    let selected = body
        .lines()
        .enumerate()
        .map(|(offset, line)| (offset as u32, line.trim()))
        .find(|(_, line)| line.contains(target) || line.contains(final_segment))
        .or_else(|| {
            body.lines()
                .enumerate()
                .map(|(offset, line)| (offset as u32, line.trim()))
                .find(|(_, line)| !line.is_empty())
        });

    match selected {
        Some((offset, line)) => SelectedEvidenceLine {
            text: line.to_string(),
            offset: Some(offset),
        },
        None => SelectedEvidenceLine {
            text: String::new(),
            offset: None,
        },
    }
}

fn infer_pipeline_role(evidence: &str) -> String {
    if evidence.contains("onBeforeToolUse") || evidence.contains("beforeToolUse") {
        "producer"
    } else if evidence.contains("webContents.send")
        || evidence.contains("IPC.SESSION_MESSAGE")
        || evidence.contains("this.send")
    {
        "bridge"
    } else if evidence.contains("session:message") || evidence.contains("SESSION_MESSAGE") {
        "channel"
    } else if evidence.contains("ipcRenderer.on") || evidence.contains("onSessionMessage") {
        "subscriber"
    } else {
        "dispatcher"
    }
    .to_string()
}

fn pipeline_ordinal(role: &str) -> Option<u32> {
    match role {
        "producer" => Some(1),
        "normalizer" => Some(2),
        "dispatcher" => Some(3),
        "bridge" => Some(4),
        "channel" => Some(5),
        "subscriber" => Some(6),
        "consumer" => Some(7),
        _ => None,
    }
}

fn coverage_for(kind: EvidencePackKind, rows: &[EvidenceRow]) -> Coverage {
    if rows.is_empty() {
        return Coverage {
            status: CoverageStatus::NoHits,
            basis: "no evidence rows were found".to_string(),
            missing: "all".to_string(),
        };
    }

    if matches!(kind, EvidencePackKind::PipelineTrace) {
        let required = ["producer", "bridge", "subscriber"];
        let missing = required
            .iter()
            .filter(|role| !rows.iter().any(|row| row.role == **role))
            .copied()
            .collect::<Vec<_>>();

        if !missing.is_empty() {
            return Coverage {
                status: CoverageStatus::Partial,
                basis: "pipeline evidence is missing required roles".to_string(),
                missing: missing.join(","),
            };
        }
    }

    Coverage {
        status: CoverageStatus::Complete,
        basis: "evidence rows cover the requested shape".to_string(),
        missing: String::new(),
    }
}

fn answer_guidance(kind: EvidencePackKind) -> Vec<String> {
    match kind {
        EvidencePackKind::CallsiteEnumeration => vec![
            "List each callsite separately and cite file and line.".to_string(),
            "Do not merge distinct lines in the same file.".to_string(),
        ],
        EvidencePackKind::PipelineTrace => vec![
            "Explain the pipeline in ordinal order.".to_string(),
            "Call out missing required roles before drawing conclusions.".to_string(),
        ],
        EvidencePackKind::DataFlow => {
            vec!["Separate reads, writes, and transfers when evidence supports it.".to_string()]
        }
        EvidencePackKind::ImpactRadius => {
            vec!["Group impacted code by likely blast radius and cite evidence.".to_string()]
        }
        EvidencePackKind::DependencyMap => {
            vec!["Describe dependency direction and mention unresolved links.".to_string()]
        }
        EvidencePackKind::SymbolLookup => vec![
            "Answer from the provided symbol evidence and avoid unsupported claims.".to_string(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::investigation::InvestigationShape;

    fn location(start_line: u32, body: &str) -> PackLocation {
        PackLocation {
            symbol_id: Some("sym-1".to_string()),
            symbol_name: Some("handler".to_string()),
            file_path: Some("src/app.rs".to_string()),
            kind: Some("function".to_string()),
            start_line: Some(start_line),
            end_line: Some(start_line + 1),
            via: None,
            body: Some(body.to_string()),
        }
    }

    #[test]
    fn callsite_pack_keeps_two_distinct_lines() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "who calls target_fn".to_string(),
            target: "target_fn".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![
                location(10, "target_fn();\ncaller_one();"),
                location(24, "target_fn();\ncaller_two();"),
            ],
            secondary: vec![],
            secondary_via: None,
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "callsite_enumeration");
        assert_eq!(value["rows"].as_array().unwrap().len(), 2);
        assert_eq!(value["rows"][0]["line"], 10);
        assert_eq!(value["rows"][1]["line"], 24);
    }

    #[test]
    fn evidence_line_uses_body_offset_for_citation_line() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "who calls target_fn".to_string(),
            target: "target_fn".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location(10, "setup();\nmore_setup();\ntarget_fn();")],
            secondary: vec![],
            secondary_via: None,
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["rows"][0]["line"], 12);
        assert_eq!(value["rows"][0]["evidence"], "target_fn();");
    }

    #[test]
    fn where_is_defined_stays_symbol_lookup() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "where is target_fn defined?".to_string(),
            target: "target_fn".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location(10, "fn target_fn() {}")],
            secondary: vec![],
            secondary_via: None,
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "symbol_lookup");
    }

    #[test]
    fn where_is_reference_named_symbol_defined_stays_symbol_lookup() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "where is reference_count defined?".to_string(),
            target: "reference_count".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location(10, "let reference_count = 0;")],
            secondary: vec![],
            secondary_via: None,
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "symbol_lookup");
    }

    #[test]
    fn where_is_called_stays_callsite_enumeration() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "where is target_fn called?".to_string(),
            target: "target_fn".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location(10, "target_fn();")],
            secondary: vec![],
            secondary_via: None,
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "callsite_enumeration");
    }

    #[test]
    fn where_is_referenced_stays_callsite_enumeration() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "where is createSession referenced?".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location(10, "createSession();")],
            secondary: vec![],
            secondary_via: None,
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "callsite_enumeration");
    }

    #[test]
    fn pipeline_pack_orders_roles_and_marks_missing_subscriber() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "trace the message pipeline".to_string(),
            target: "session.message".to_string(),
            shape: InvestigationShape::CallTrace,
            primary: vec![location(30, "onBeforeToolUse(() => dispatch());")],
            secondary: vec![location(
                40,
                "webContents.send('session:message', payload);",
            )],
            secondary_via: None,
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "pipeline_trace");
        assert_eq!(value["rows"][0]["role"], "producer");
        assert_eq!(value["rows"][0]["ordinal"], 1);
        assert_eq!(value["rows"][1]["role"], "bridge");
        assert_eq!(value["rows"][1]["ordinal"], 4);
        assert_eq!(value["coverage"]["status"], "partial");
        assert_eq!(value["coverage"]["missing"], "subscriber");
    }

    #[test]
    fn empty_pack_reports_no_hits() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "what is target_fn".to_string(),
            target: "target_fn".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![],
            secondary: vec![],
            secondary_via: None,
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "symbol_lookup");
        assert_eq!(value["coverage"]["status"], "no_hits");
        assert_eq!(value["rows"].as_array().unwrap().len(), 0);
    }
}

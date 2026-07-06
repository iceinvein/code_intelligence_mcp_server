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
    pub extra_candidates: Vec<PackLocation>,
}

pub fn build_evidence_pack(input: EvidencePackInput) -> EvidencePack {
    let kind = pack_kind(&input.question, input.shape);
    let mut rows = Vec::with_capacity(
        input.primary.len() + input.secondary.len() + input.extra_candidates.len(),
    );

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

    for location in input.extra_candidates {
        rows.push(row_from_location(
            location,
            &input.target,
            kind,
            "candidate",
            rows.len() as u32 + 1,
        ));
    }

    rows = verify_rows(kind, &input.target, rows);

    if matches!(kind, EvidencePackKind::PipelineTrace) {
        rows.sort_by_key(|row| row.ordinal.unwrap_or(u32::MAX));
    }

    let coverage = coverage_for(kind, &rows, &input.target);

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
    if matches!(shape, InvestigationShape::Discover) && is_callsite_question(question) {
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
        EvidencePackKind::CallsiteEnumeration => callsite_role(location.via.as_deref()),
        EvidencePackKind::PipelineTrace => infer_pipeline_role(body.unwrap_or(&evidence)),
        EvidencePackKind::ImpactRadius => impact_role(
            location.file_path.as_deref(),
            location.kind.as_deref(),
            fallback_role,
        ),
        _ => location.via.as_deref().unwrap_or(fallback_role).to_string(),
    };
    let ordinal = match kind {
        EvidencePackKind::PipelineTrace => pipeline_ordinal(&role),
        _ => Some(fallback_ordinal),
    };
    let risk = if matches!(kind, EvidencePackKind::ImpactRadius) {
        impact_risk(&role).map(str::to_string)
    } else {
        None
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
        risk,
    }
}

fn verify_rows(kind: EvidencePackKind, target: &str, rows: Vec<EvidenceRow>) -> Vec<EvidenceRow> {
    let rows = rows
        .into_iter()
        .map(|mut row| {
            if matches!(kind, EvidencePackKind::CallsiteEnumeration)
                && (row.role == "callsite" || row.role == "caller")
                && !evidence_mentions_target(&row.evidence, target)
            {
                row.role = "candidate".to_string();
            }

            row
        })
        .collect();

    match kind {
        EvidencePackKind::CallsiteEnumeration => dedup_rows_by_file_line(rows, target),
        _ => rows,
    }
}

fn dedup_rows_by_file_line(rows: Vec<EvidenceRow>, target: &str) -> Vec<EvidenceRow> {
    let mut deduped = Vec::with_capacity(rows.len());

    for row in rows {
        let Some(key) = row.file_path.clone().zip(row.line) else {
            deduped.push(row);
            continue;
        };

        if let Some(existing_index) = deduped.iter().position(|existing: &EvidenceRow| {
            existing.file_path.as_ref().zip(existing.line) == Some((&key.0, key.1))
        }) {
            if row_confidence(&row, target) > row_confidence(&deduped[existing_index], target) {
                deduped[existing_index] = row;
            }
        } else {
            deduped.push(row);
        }
    }

    deduped
}

fn row_confidence(row: &EvidenceRow, target: &str) -> u8 {
    let verified = matches!(row.role.as_str(), "callsite" | "caller");
    let mentions_target = evidence_mentions_target(&row.evidence, target);

    match (verified, mentions_target) {
        (true, true) => 3,
        (true, false) => 2,
        (false, true) => 1,
        (false, false) => 0,
    }
}

fn evidence_mentions_target(evidence: &str, target: &str) -> bool {
    if evidence.contains(target) {
        return true;
    }

    let final_segment = target
        .split(['.', ':', '#'])
        .rfind(|segment| !segment.is_empty())
        .unwrap_or(target);

    evidence.contains(final_segment)
}

fn is_candidate_reason(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some("callback_producer" | "event_emitter" | "event_subscriber" | "config_hook")
    )
}

fn impact_role(file_path: Option<&str>, reason: Option<&str>, fallback_role: &str) -> String {
    let file_path = file_path.unwrap_or_default();
    let file_path_lower = file_path.to_ascii_lowercase();
    let reason = reason.unwrap_or_default().to_ascii_lowercase();

    if reason.contains("test") || crate::classify::is_test_file(file_path) {
        "affected_test"
    } else if reason.contains("cochange")
        || reason.contains("co-change")
        || reason.contains("co_change")
    {
        "cochange"
    } else if reason.contains("dependency")
        || reason.contains("import")
        || file_path_lower.ends_with("cargo.toml")
        || file_path_lower.ends_with("package.json")
        || file_path_lower.ends_with("package-lock.json")
    {
        "dependency"
    } else if reason.contains("config")
        || file_path_lower.contains("config")
        || file_path_lower.ends_with(".toml")
        || file_path_lower.ends_with(".yaml")
        || file_path_lower.ends_with(".yml")
        || file_path_lower.ends_with(".json")
    {
        "config"
    } else if !file_path_lower.is_empty() {
        "affected_production"
    } else {
        fallback_role
    }
    .to_string()
}

fn impact_risk(role: &str) -> Option<&'static str> {
    match role {
        "affected_production" | "dependency" => Some("high"),
        "cochange" | "config" => Some("medium"),
        "affected_test" => Some("low"),
        _ => None,
    }
}

fn callsite_role(via: Option<&str>) -> String {
    if via.is_some_and(is_verified_callsite_source) {
        "callsite"
    } else {
        "candidate"
    }
    .to_string()
}

fn is_verified_callsite_source(via: &str) -> bool {
    let via = via.to_ascii_lowercase();
    via == "find_references"
        || via.contains("reference")
        || via.contains("call_hierarchy")
        || via.contains("call")
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
        .rfind(|segment| !segment.is_empty())
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

fn coverage_for(kind: EvidencePackKind, rows: &[EvidenceRow], target: &str) -> Coverage {
    if rows.is_empty() {
        return Coverage {
            status: CoverageStatus::NoHits,
            basis: "no evidence rows were found".to_string(),
            missing: "all".to_string(),
        };
    }

    match kind {
        EvidencePackKind::CallsiteEnumeration => {
            if rows
                .iter()
                .any(|row| row.role == "callsite" || row.role == "caller")
            {
                return Coverage {
                    status: CoverageStatus::Complete,
                    basis: "evidence rows cover the requested shape".to_string(),
                    missing: String::new(),
                };
            }

            return Coverage {
                status: CoverageStatus::Partial,
                basis: "callsite evidence is limited to search candidates".to_string(),
                missing: format!("verified callsite evidence for {target}"),
            };
        }
        EvidencePackKind::PipelineTrace => {
            let required = ["producer", "bridge", "subscriber"];
            let missing = required
                .iter()
                .filter(|role| {
                    !rows.iter().any(|row| {
                        row.role == **role && !is_candidate_reason(row.reason.as_deref())
                    })
                })
                .copied()
                .collect::<Vec<_>>();

            if !missing.is_empty() {
                return Coverage {
                    status: CoverageStatus::Partial,
                    basis: "pipeline evidence is missing verified required roles".to_string(),
                    missing: format!("verified pipeline roles: {}", missing.join(",")),
                };
            }
        }
        _ => {}
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
        location_via(start_line, body, None)
    }

    fn location_via(start_line: u32, body: &str, via: Option<&str>) -> PackLocation {
        PackLocation {
            symbol_id: Some("sym-1".to_string()),
            symbol_name: Some("handler".to_string()),
            file_path: Some("src/app.rs".to_string()),
            kind: Some("function".to_string()),
            start_line: Some(start_line),
            end_line: Some(start_line + 1),
            via: via.map(str::to_string),
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
            extra_candidates: Vec::new(),
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
            extra_candidates: Vec::new(),
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
            extra_candidates: Vec::new(),
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
            extra_candidates: Vec::new(),
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
            extra_candidates: Vec::new(),
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
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "callsite_enumeration");
    }

    #[test]
    fn pack_kind_maps_callsite_question_to_callsite_enumeration() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "list callsites for createSession".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![PackLocation {
                symbol_id: Some("a".to_string()),
                symbol_name: Some("run".to_string()),
                file_path: Some("src/a.ts".to_string()),
                kind: Some("search_code".to_string()),
                start_line: Some(1),
                end_line: Some(1),
                via: None,
                body: Some("createSession()\n".to_string()),
            }],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        assert_eq!(pack.kind, EvidencePackKind::CallsiteEnumeration);
    }

    #[test]
    fn search_code_callsite_pack_marks_rows_as_candidates_and_coverage_partial() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "who calls createSession".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location_via(10, "createSession();", Some("search_code"))],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "callsite_enumeration");
        assert_eq!(value["rows"][0]["role"], "candidate");
        assert_eq!(value["coverage"]["status"], "partial");
        assert_eq!(
            value["coverage"]["missing"],
            "verified callsite evidence for createSession"
        );
    }

    #[test]
    fn verified_callsite_pack_marks_rows_as_callsites_and_coverage_complete() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "who calls createSession".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location_via(
                10,
                "createSession();",
                Some("find_references"),
            )],
            secondary: vec![location_via(
                20,
                "createSession();",
                Some("get_call_hierarchy"),
            )],
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "callsite_enumeration");
        assert_eq!(value["rows"][0]["role"], "callsite");
        assert_eq!(value["rows"][1]["role"], "callsite");
        assert_eq!(value["coverage"]["status"], "complete");
    }

    #[test]
    fn verified_callsite_without_target_evidence_is_downgraded_to_candidate() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "who calls createSession".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location_via(
                10,
                "const session = await makeSession();",
                Some("find_references"),
            )],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["rows"][0]["role"], "candidate");
        assert_eq!(value["coverage"]["status"], "partial");
        assert_eq!(
            value["coverage"]["missing"],
            "verified callsite evidence for createSession"
        );
    }

    #[test]
    fn callsite_pack_deduplicates_same_file_and_line() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "who calls createSession".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![
                location_via(10, "createSession();", Some("find_references")),
                location_via(10, "createSession();", Some("get_call_hierarchy")),
            ],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["rows"].as_array().unwrap().len(), 1);
        assert_eq!(value["coverage"]["status"], "complete");
    }

    #[test]
    fn callsite_pack_prefers_verified_duplicate_over_candidate() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "who calls createSession".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![
                location_via(10, "createSession();", Some("search_code")),
                location_via(10, "createSession();", Some("find_references")),
            ],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["rows"].as_array().unwrap().len(), 1);
        assert_eq!(value["rows"][0]["line"], 10);
        assert_eq!(value["rows"][0]["role"], "callsite");
        assert_eq!(value["coverage"]["status"], "complete");
    }

    #[test]
    fn pipeline_pack_includes_extra_non_callgraph_candidates() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "trace the tool-use pipeline".to_string(),
            target: "toolUse".to_string(),
            shape: InvestigationShape::CallTrace,
            primary: vec![location_via(
                10,
                "dispatchToolUse(payload);",
                Some("search_code"),
            )],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: vec![PackLocation {
                symbol_id: Some("hook".to_string()),
                symbol_name: Some("onBeforeToolUse".to_string()),
                file_path: Some("src/config.ts".to_string()),
                kind: Some("callback_producer".to_string()),
                start_line: Some(30),
                end_line: Some(30),
                via: Some("non_callgraph_edges".to_string()),
                body: Some(
                    "onBeforeToolUse: async payload => dispatchToolUse(payload)".to_string(),
                ),
            }],
        });

        let value = pack_to_value(&pack);

        assert!(value["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["reason"] == "callback_producer"));
    }

    #[test]
    fn candidate_only_pipeline_pack_is_partial_even_with_required_roles() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "trace the tool-use pipeline".to_string(),
            target: "toolUse".to_string(),
            shape: InvestigationShape::CallTrace,
            primary: vec![],
            secondary: vec![],
            secondary_via: None,
            extra_candidates: vec![
                PackLocation {
                    symbol_id: Some("producer".to_string()),
                    symbol_name: Some("config.onBeforeToolUse".to_string()),
                    file_path: Some("src/config.ts".to_string()),
                    kind: Some("callback_producer".to_string()),
                    start_line: Some(12),
                    end_line: Some(12),
                    via: Some("non_callgraph_edges".to_string()),
                    body: Some("onBeforeToolUse: async () => dispatch()".to_string()),
                },
                PackLocation {
                    symbol_id: Some("bridge".to_string()),
                    symbol_name: Some("sendToolUse".to_string()),
                    file_path: Some("src/main.ts".to_string()),
                    kind: Some("event_emitter".to_string()),
                    start_line: Some(24),
                    end_line: Some(24),
                    via: Some("non_callgraph_edges".to_string()),
                    body: Some("webContents.send('tool-use', payload);".to_string()),
                },
                PackLocation {
                    symbol_id: Some("subscriber".to_string()),
                    symbol_name: Some("onToolUse".to_string()),
                    file_path: Some("src/renderer.ts".to_string()),
                    kind: Some("event_subscriber".to_string()),
                    start_line: Some(36),
                    end_line: Some(36),
                    via: Some("non_callgraph_edges".to_string()),
                    body: Some("ipcRenderer.on('tool-use', onToolUse);".to_string()),
                },
            ],
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["coverage"]["status"], "partial");
        assert_eq!(
            value["coverage"]["missing"],
            "verified pipeline roles: producer,bridge,subscriber"
        );
    }

    #[test]
    fn mixed_verified_dispatcher_and_candidate_pipeline_roles_stays_partial() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "trace the tool-use pipeline".to_string(),
            target: "toolUse".to_string(),
            shape: InvestigationShape::CallTrace,
            primary: vec![location_via(
                10,
                "dispatchToolUse(payload);",
                Some("search_code"),
            )],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: vec![
                PackLocation {
                    symbol_id: Some("producer".to_string()),
                    symbol_name: Some("config.onBeforeToolUse".to_string()),
                    file_path: Some("src/config.ts".to_string()),
                    kind: Some("callback_producer".to_string()),
                    start_line: Some(12),
                    end_line: Some(12),
                    via: Some("non_callgraph_edges".to_string()),
                    body: Some("onBeforeToolUse: async () => dispatchToolUse()".to_string()),
                },
                PackLocation {
                    symbol_id: Some("bridge".to_string()),
                    symbol_name: Some("sendToolUse".to_string()),
                    file_path: Some("src/main.ts".to_string()),
                    kind: Some("event_emitter".to_string()),
                    start_line: Some(24),
                    end_line: Some(24),
                    via: Some("non_callgraph_edges".to_string()),
                    body: Some("webContents.send('tool-use', payload);".to_string()),
                },
                PackLocation {
                    symbol_id: Some("subscriber".to_string()),
                    symbol_name: Some("onToolUse".to_string()),
                    file_path: Some("src/renderer.ts".to_string()),
                    kind: Some("event_subscriber".to_string()),
                    start_line: Some(36),
                    end_line: Some(36),
                    via: Some("non_callgraph_edges".to_string()),
                    body: Some("ipcRenderer.on('tool-use', onToolUse);".to_string()),
                },
            ],
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["coverage"]["status"], "partial");
        assert_eq!(
            value["coverage"]["missing"],
            "verified pipeline roles: producer,bridge,subscriber"
        );
    }

    #[test]
    fn callsite_words_do_not_override_explicit_impact_radius_shape() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "who calls createSession and what breaks if it changes?".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::ImpactRadius,
            primary: vec![location(10, "createSession();")],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        assert_eq!(pack.kind, EvidencePackKind::ImpactRadius);
    }

    #[test]
    fn impact_pack_does_not_emit_unknown_risk_for_ordinary_rows() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "what breaks if createSession changes?".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::ImpactRadius,
            primary: vec![location(10, "createSession();")],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_ne!(value["rows"][0]["risk"], "unknown");
    }

    #[test]
    fn impact_pack_uses_test_file_classifier_for_affected_tests() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "what breaks if createSession changes?".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::ImpactRadius,
            primary: vec![
                PackLocation {
                    symbol_id: Some("test-rs".to_string()),
                    symbol_name: Some("test_create_session".to_string()),
                    file_path: Some("tests/foo.rs".to_string()),
                    kind: Some("function".to_string()),
                    start_line: Some(10),
                    end_line: Some(10),
                    via: None,
                    body: Some("createSession();".to_string()),
                },
                PackLocation {
                    symbol_id: Some("spec-ts".to_string()),
                    symbol_name: Some("creates session".to_string()),
                    file_path: Some("src/foo.spec.ts".to_string()),
                    kind: Some("function".to_string()),
                    start_line: Some(20),
                    end_line: Some(20),
                    via: None,
                    body: Some("createSession();".to_string()),
                },
            ],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["rows"][0]["role"], "affected_test");
        assert_eq!(value["rows"][0]["risk"], "low");
        assert_eq!(value["rows"][1]["role"], "affected_test");
        assert_eq!(value["rows"][1]["risk"], "low");
    }

    #[test]
    fn callsite_words_do_not_override_explicit_call_trace_shape() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "who calls createSession through the session pipeline?".to_string(),
            target: "createSession".to_string(),
            shape: InvestigationShape::CallTrace,
            primary: vec![location(10, "createSession();")],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        assert_eq!(pack.kind, EvidencePackKind::PipelineTrace);
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
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "pipeline_trace");
        assert_eq!(value["rows"][0]["role"], "producer");
        assert_eq!(value["rows"][0]["ordinal"], 1);
        assert_eq!(value["rows"][1]["role"], "bridge");
        assert_eq!(value["rows"][1]["ordinal"], 4);
        assert_eq!(value["coverage"]["status"], "partial");
        assert_eq!(
            value["coverage"]["missing"],
            "verified pipeline roles: subscriber"
        );
    }

    mod golden_failures {
        use super::*;

        fn pack_location(
            file_path: &str,
            start_line: u32,
            body: &str,
            via: Option<&str>,
            kind: Option<&str>,
        ) -> PackLocation {
            PackLocation {
                symbol_id: Some(format!("sym-{file_path}-{start_line}")),
                symbol_name: Some("handler".to_string()),
                file_path: Some(file_path.to_string()),
                kind: kind.map(str::to_string),
                start_line: Some(start_line),
                end_line: Some(start_line),
                via: via.map(str::to_string),
                body: Some(body.to_string()),
            }
        }

        #[test]
        fn distinct_create_session_callsites_are_not_merged() {
            let pack = build_evidence_pack(EvidencePackInput {
                question: "list callsites for createSession".to_string(),
                target: "createSession".to_string(),
                shape: InvestigationShape::Discover,
                primary: vec![
                    pack_location(
                        "src/review.ts",
                        42,
                        "const session = createSession(request);",
                        Some("find_references"),
                        Some("function"),
                    ),
                    pack_location(
                        "src/review.ts",
                        91,
                        "return createSession(fallbackRequest);",
                        Some("find_references"),
                        Some("function"),
                    ),
                ],
                secondary: Vec::new(),
                secondary_via: None,
                extra_candidates: Vec::new(),
            });

            assert_eq!(pack.kind, EvidencePackKind::CallsiteEnumeration);
            assert_eq!(pack.rows.len(), 2);
            assert_eq!(pack.rows[0].file_path.as_deref(), Some("src/review.ts"));
            assert_eq!(pack.rows[0].line, Some(42));
            assert_eq!(pack.rows[0].role, "callsite");
            assert_eq!(pack.rows[1].file_path.as_deref(), Some("src/review.ts"));
            assert_eq!(pack.rows[1].line, Some(91));
            assert_eq!(pack.rows[1].role, "callsite");
            assert_eq!(pack.coverage.status, CoverageStatus::Complete);
        }

        #[test]
        fn callback_producer_candidate_downgrades_incomplete_pipeline() {
            let pack = build_evidence_pack(EvidencePackInput {
                question: "trace how tool-use flows from provider to renderer".to_string(),
                target: "toolUse".to_string(),
                shape: InvestigationShape::CallTrace,
                primary: Vec::new(),
                secondary: Vec::new(),
                secondary_via: None,
                extra_candidates: vec![pack_location(
                    "src/provider.ts",
                    17,
                    "onBeforeToolUse: async toolUse => provider.enqueue(toolUse)",
                    Some("non_callgraph_edges"),
                    Some("callback_producer"),
                )],
            });

            assert_eq!(pack.kind, EvidencePackKind::PipelineTrace);
            assert!(pack.rows.iter().any(|row| row.role == "producer"));
            assert_eq!(pack.coverage.status, CoverageStatus::Partial);
            assert_eq!(
                pack.coverage.missing,
                "verified pipeline roles: producer,bridge,subscriber"
            );
        }

        #[test]
        fn impact_pack_assigns_test_config_and_production_roles() {
            let pack = build_evidence_pack(EvidencePackInput {
                question: "what breaks if createSession changes?".to_string(),
                target: "createSession".to_string(),
                shape: InvestigationShape::ImpactRadius,
                primary: vec![
                    pack_location(
                        "src/review.ts",
                        10,
                        "const session = createSession(request);",
                        Some("find_references"),
                        Some("function"),
                    ),
                    pack_location(
                        "src/review.test.ts",
                        22,
                        "expect(createSession(request)).toBeDefined();",
                        Some("find_references"),
                        Some("function"),
                    ),
                    pack_location(
                        "src/session-config.ts",
                        5,
                        "export const sessionFactory = createSession;",
                        Some("find_references"),
                        Some("function"),
                    ),
                ],
                secondary: Vec::new(),
                secondary_via: None,
                extra_candidates: Vec::new(),
            });

            let production = pack
                .rows
                .iter()
                .find(|row| row.file_path.as_deref() == Some("src/review.ts"))
                .expect("production row should be present");
            let test = pack
                .rows
                .iter()
                .find(|row| row.file_path.as_deref() == Some("src/review.test.ts"))
                .expect("test row should be present");
            let config = pack
                .rows
                .iter()
                .find(|row| row.file_path.as_deref() == Some("src/session-config.ts"))
                .expect("config row should be present");

            assert_eq!(production.role, "affected_production");
            assert_eq!(test.role, "affected_test");
            assert_eq!(config.role, "config");
            assert_ne!(production.role, test.role);
            assert_ne!(production.role, config.role);
            assert_ne!(test.role, config.role);
        }
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
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "symbol_lookup");
        assert_eq!(value["coverage"]["status"], "no_hits");
        assert_eq!(value["rows"].as_array().unwrap().len(), 0);
    }
}

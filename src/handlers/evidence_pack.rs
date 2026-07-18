use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};

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

/// Shape-independent semantic role used by the coverage contract. The
/// existing `EvidenceRow::role` remains the presentation role (for example
/// `affected_test` or `dispatcher`) so current clients keep their ordering
/// and prose hints, while this enum lets every question shape describe the
/// evidence it requires in a stable vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageRole {
    CanonicalDefinition,
    Implementation,
    PublicExposure,
    DirectCaller,
    WrapperAlias,
    StateMechanism,
    CounterEvidence,
    Producer,
    Bridge,
    Channel,
    Subscriber,
    Consumer,
    AffectedCode,
    Dependency,
    Test,
    Config,
    ModuleContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Verified,
    Candidate,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocationClaimCoverage {
    pub policy: String,
    pub source_backed_rows: usize,
    pub unsupported_rows: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Coverage {
    pub status: CoverageStatus,
    pub basis: String,
    pub missing: String,
    pub required_roles: Vec<CoverageRole>,
    pub optional_roles: Vec<CoverageRole>,
    pub resolved_roles: Vec<CoverageRole>,
    pub missing_roles: Vec<CoverageRole>,
    pub ambiguous_roles: Vec<CoverageRole>,
    pub candidate_roles: Vec<CoverageRole>,
    pub location_claims: LocationClaimCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceRow {
    pub role: String,
    pub coverage_role: CoverageRole,
    pub verification: VerificationStatus,
    /// True only when file, line, and non-empty source evidence all came from
    /// a returned indexed location. Exact path:line citations are emitted
    /// only for rows with this flag.
    pub source_backed: bool,
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
    /// Copy-paste citation form: the shortest '/'-boundary path suffix that is
    /// unique across the indexed file list, plus the row's line range.
    /// Agents shorten long paths in prose; when two files share a basename the
    /// shortened cite is ambiguous to a reader. This field gives them a short
    /// form that stays unambiguous (R028).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cite: Option<String>,
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
    let secondary_role = input.secondary_via.as_deref().unwrap_or("secondary");

    let mut channeled = Vec::with_capacity(
        input.primary.len() + input.secondary.len() + input.extra_candidates.len(),
    );
    channeled.extend(input.primary.into_iter().map(|l| (l, "primary")));
    channeled.extend(input.secondary.into_iter().map(|l| (l, secondary_role)));
    channeled.extend(input.extra_candidates.into_iter().map(|l| (l, "candidate")));
    let channeled = dedup_pack_locations(channeled);

    let mut rows = Vec::with_capacity(channeled.len());
    for (location, fallback_role) in channeled {
        rows.push(row_from_location(
            location,
            &input.target,
            kind,
            fallback_role,
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

/// Select evidence rows under a hard response budget. Required semantic
/// roles ride first, then the selector prefers new roles over duplicates.
/// Within each tier, verified/source-backed rows beat candidates and shorter
/// evidence wins when two rows carry the same value. Returned rows retain
/// their original order so pipeline ordinals and callsite ordering stay
/// stable.
pub fn select_pack_rows_for_budget(rows: &[Value], coverage: &Value, limit: usize) -> Vec<Value> {
    if rows.len() <= limit {
        return rows.to_vec();
    }
    if limit == 0 {
        return Vec::new();
    }

    let required = coverage
        .get("required_roles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let optional = coverage
        .get("optional_roles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    let required_set = required.iter().copied().collect::<HashSet<_>>();
    let mut selected = HashSet::new();
    let mut represented = HashSet::<String>::new();

    let score = |row: &Value| -> i64 {
        let coverage_role = row
            .get("coverage_role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let semantic = if required_set.contains(coverage_role) {
            1_000
        } else if optional.contains(coverage_role) {
            250
        } else {
            100
        };
        let verification = match row.get("verification").and_then(Value::as_str) {
            Some("verified") => 120,
            Some("ambiguous") => 20,
            _ => 0,
        };
        let source = if row
            .get("source_backed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            80
        } else {
            0
        };
        let injected = match row.get("role").and_then(Value::as_str) {
            Some(
                "supporting_definition"
                | "route_endpoint"
                | "sibling_route"
                | "handler_dependency"
                | "module_breadth"
                | "breadth_dependency"
                | "hub_type",
            ) => 60,
            _ => 0,
        };
        let evidence_cost = row
            .get("evidence")
            .and_then(Value::as_str)
            .map(|text| (text.len() / 128).min(50) as i64)
            .unwrap_or(0);
        semantic + verification + source + injected - evidence_cost
    };
    let best_unselected_for_role = |role: &str, selected: &HashSet<usize>| {
        rows.iter()
            .enumerate()
            .filter(|(index, row)| {
                !selected.contains(index)
                    && row.get("coverage_role").and_then(Value::as_str) == Some(role)
            })
            .max_by_key(|(index, row)| (score(row), std::cmp::Reverse(*index)))
            .map(|(index, _)| index)
    };

    // Preserve the contract's required-role order when the limit is smaller
    // than the number of required roles.
    for role in required {
        if selected.len() >= limit {
            break;
        }
        if let Some(index) = best_unselected_for_role(role, &selected) {
            selected.insert(index);
            represented.insert(role.to_string());
        }
    }

    let mut ranked = (0..rows.len()).collect::<Vec<_>>();
    ranked.sort_by_key(|index| (std::cmp::Reverse(score(&rows[*index])), *index));

    // Novel roles have higher information density than another row for a
    // role already represented in the retained set.
    for index in ranked.iter().copied() {
        if selected.len() >= limit {
            break;
        }
        let role = rows[index]
            .get("coverage_role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !selected.contains(&index) && !represented.contains(role) {
            selected.insert(index);
            represented.insert(role.to_string());
        }
    }
    for index in ranked {
        if selected.len() >= limit {
            break;
        }
        selected.insert(index);
    }

    let mut indices = selected.into_iter().collect::<Vec<_>>();
    indices.sort_unstable();
    indices
        .into_iter()
        .map(|index| rows[index].clone())
        .collect()
}

/// Recompute the serialized role contract after response-budget truncation.
/// This prevents a compacted pack from claiming a required role that no
/// longer rides in the response.
pub fn refresh_coverage_after_budget(coverage: &mut Value, rows: &[Value], omitted_count: usize) {
    let required = coverage
        .get("required_roles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let required_names = required
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let mut resolved = BTreeSet::new();
    let mut ambiguous = BTreeSet::new();
    let mut candidate = BTreeSet::new();
    for row in rows {
        let Some(role) = row.get("coverage_role").and_then(Value::as_str) else {
            continue;
        };
        match row.get("verification").and_then(Value::as_str) {
            Some("verified") => {
                resolved.insert(role.to_string());
            }
            Some("ambiguous") => {
                ambiguous.insert(role.to_string());
            }
            _ => {
                candidate.insert(role.to_string());
            }
        }
    }
    ambiguous.retain(|role| !resolved.contains(role));
    candidate.retain(|role| !resolved.contains(role) && !ambiguous.contains(role));
    let present = resolved
        .iter()
        .chain(ambiguous.iter())
        .chain(candidate.iter())
        .cloned()
        .collect::<HashSet<_>>();
    let missing = required_names
        .into_iter()
        .filter(|role| !present.contains(*role))
        .collect::<Vec<_>>();
    let source_backed_rows = rows
        .iter()
        .filter(|row| {
            row.get("source_backed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .count();

    coverage["status"] = json!("partial");
    coverage["basis"] = json!("evidence rows were truncated for response budget");
    coverage["missing"] = json!(format!(
        "{omitted_count} rows omitted by response budget; returned rows are not exhaustive"
    ));
    coverage["resolved_roles"] = json!(resolved);
    coverage["missing_roles"] = json!(missing);
    coverage["ambiguous_roles"] = json!(ambiguous);
    coverage["candidate_roles"] = json!(candidate);
    coverage["location_claims"] = json!({
        "policy": "exact_path_line_requires_source_backed_row",
        "source_backed_rows": source_backed_rows,
        "unsupported_rows": rows.len().saturating_sub(source_backed_rows),
    });
}

/// Drop redundant pack rows before role assignment. Two rules, both from the
/// django-arch-03 pack audit (R029-R033):
/// - one row per symbol_id: the pivot rode as the primary search hit AND the
///   secondary call-hierarchy root. The first copy keeps its position; a
///   later copy with a verified via (find_references / call hierarchy)
///   donates its via so callsite packs still mark the row verified.
/// - a single-line anchor of a symbol whose full row already shows that line
///   (same file, same symbol, line within the UNTRUNCATED body) repeats
///   visible evidence. Anchors past a truncated body still carry unseen
///   lines and ride.
fn dedup_pack_locations(channeled: Vec<(PackLocation, &str)>) -> Vec<(PackLocation, &str)> {
    let mut kept: Vec<(PackLocation, &str)> = Vec::with_capacity(channeled.len());

    for (location, role) in channeled {
        if location.symbol_id.is_some() {
            if let Some((existing, _)) = kept
                .iter_mut()
                .find(|(k, _)| k.symbol_id == location.symbol_id)
            {
                let existing_verified = existing
                    .via
                    .as_deref()
                    .is_some_and(is_verified_callsite_source);
                let new_verified = location
                    .via
                    .as_deref()
                    .is_some_and(is_verified_callsite_source);
                if !existing_verified && new_verified {
                    existing.via = location.via;
                }
                continue;
            }
        }

        let single_line = location.start_line.is_some() && location.start_line == location.end_line;
        if single_line {
            let line = location.start_line.expect("checked above");
            let covered = kept.iter().any(|(k, _)| {
                k.file_path == location.file_path
                    && k.symbol_name == location.symbol_name
                    && visible_body_contains(k, line)
            });
            if covered {
                continue;
            }
        }

        kept.push((location, role));
    }

    kept
}

/// Whether `line` falls inside the portion of `location`'s body that actually
/// rides in the pack. `body_with_cap` truncation appends a "// ... N more
/// lines" marker; lines past the kept window are NOT visible even though the
/// symbol's line range covers them.
fn visible_body_contains(location: &PackLocation, line: u32) -> bool {
    let (Some(start), Some(body)) = (location.start_line, location.body.as_deref()) else {
        return false;
    };
    let visible_lines = body
        .lines()
        .filter(|l| !l.trim_start().starts_with("// ..."))
        .count() as u32;
    if visible_lines == 0 {
        return false;
    }
    line >= start && line < start + visible_lines
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
    // Injected rows keep their via as the role in every pack kind: the via
    // is the provenance the agent needs for citation, kind-specific role
    // inference would relabel them as pipeline stages or callsites, and the
    // budget stages key their truncation exemption on these role names.
    let injected_via = location
        .via
        .as_deref()
        .filter(|v| crate::handlers::investigation::is_injected_via_str(v));
    let role = if let Some(via) = injected_via {
        via.to_string()
    } else {
        match kind {
            EvidencePackKind::CallsiteEnumeration => callsite_role(location.via.as_deref()),
            EvidencePackKind::PipelineTrace => infer_pipeline_role(body.unwrap_or(&evidence)),
            EvidencePackKind::ImpactRadius => impact_role(
                location.file_path.as_deref(),
                location.kind.as_deref(),
                fallback_role,
            ),
            _ => location.via.as_deref().unwrap_or(fallback_role).to_string(),
        }
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
    let source_backed =
        location.file_path.is_some() && line.is_some() && !evidence.trim().is_empty();
    let coverage_role = coverage_role_for(kind, &role, location.kind.as_deref(), fallback_role);
    let verification = verification_for(&role, location.kind.as_deref(), source_backed);

    EvidenceRow {
        role,
        coverage_role,
        verification,
        source_backed,
        ordinal,
        symbol_id: location.symbol_id,
        symbol_name: location.symbol_name,
        file_path: location.file_path,
        line,
        end_line: location.end_line,
        enclosing_symbol: None,
        cite: None,
        evidence,
        reason: location.kind,
        risk,
    }
}

/// Shortest '/'-boundary suffix (at least two segments) that is unique across
/// `files`, per file. Two segments minimum so the form still reads as a path;
/// single-segment paths keep the full path.
///
/// Among unique suffixes, prefer the shortest whose LEADING segment is not a
/// generic directory name: R030 showed agents copy "backend-worker/src/x.rs"
/// verbatim but strip the head off "src/x.rs" (it reads as noise), landing
/// back on an ambiguous basename. A package-anchored form also degrades
/// gracefully: dropping its head still leaves a unique "src/x.rs".
pub fn short_cite_forms(files: &[String]) -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;

    let mut counts: HashMap<&str, u32> = HashMap::new();
    for file in files {
        for suffix in multi_segment_suffixes(file) {
            *counts.entry(suffix).or_insert(0) += 1;
        }
    }

    files
        .iter()
        .map(|file| {
            let unique = |s: &&str| counts.get(*s) == Some(&1);
            let form = multi_segment_suffixes(file)
                .filter(unique)
                .find(|s| !has_generic_lead(s))
                .or_else(|| multi_segment_suffixes(file).find(unique))
                .unwrap_or(file.as_str());
            (file.clone(), form.to_string())
        })
        .collect()
}

/// Directory names too common to anchor a citation: an agent reading
/// "src/aggregation.rs" sees no information in "src/" and drops it.
fn has_generic_lead(suffix: &str) -> bool {
    let lead = suffix.split('/').next().unwrap_or(suffix);
    matches!(
        lead,
        "src"
            | "lib"
            | "source"
            | "app"
            | "core"
            | "base"
            | "common"
            | "internal"
            | "pkg"
            | "mod"
            | "utils"
            | "util"
            | "helpers"
            | "test"
            | "tests"
            | "dist"
            | "build"
            | "bin"
    )
}

/// Suffixes of `path` on '/' boundaries with >= 2 segments, shortest first,
/// ending with the full path. A single-segment path yields only itself.
fn multi_segment_suffixes(path: &str) -> impl Iterator<Item = &str> {
    let mut starts: Vec<usize> = path.match_indices('/').map(|(i, _)| i + 1).collect();
    // Drop the last-segment start: a bare basename is not a path form.
    starts.pop();
    starts.reverse(); // largest start = shortest suffix first
    starts.push(0); // full path last
    starts.into_iter().map(move |s| &path[s..])
}

/// Fill each row's `cite` from the short-form map, appending the line range.
/// Files missing from the map (e.g. synthetic locations) fall back to the
/// full path, which is always a valid citation.
pub fn apply_cite_forms(
    pack: &mut EvidencePack,
    forms: &std::collections::HashMap<String, String>,
) {
    let mut applied = false;
    for row in &mut pack.rows {
        if !row.source_backed {
            row.cite = None;
            continue;
        }
        let Some(file) = row.file_path.as_deref() else {
            continue;
        };
        let short = forms.get(file).map(String::as_str).unwrap_or(file);
        row.cite = Some(match (row.line, row.end_line) {
            (Some(line), Some(end)) if end > line => format!("{short}:{line}-{end}"),
            (Some(line), _) => format!("{short}:{line}"),
            (None, _) => short.to_string(),
        });
        applied = true;
    }
    if applied {
        pack.answer_guidance.insert(
            0,
            "Cite locations by copying each row's `cite` value verbatim; it is \
             the shortest path form that stays unambiguous in this repo."
                .to_string(),
        );
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
                row.verification = VerificationStatus::Candidate;
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

fn coverage_role_for(
    kind: EvidencePackKind,
    role: &str,
    reason: Option<&str>,
    channel: &str,
) -> CoverageRole {
    let reason = reason.unwrap_or_default().to_ascii_lowercase();
    match role {
        "public_exposure" | "route_endpoint" | "sibling_route" => CoverageRole::PublicExposure,
        "wrapper" | "alias" => CoverageRole::WrapperAlias,
        "supporting_definition" => CoverageRole::StateMechanism,
        "callsite" | "caller" => CoverageRole::DirectCaller,
        "producer" => CoverageRole::Producer,
        "bridge" => CoverageRole::Bridge,
        "channel" => CoverageRole::Channel,
        "subscriber" => CoverageRole::Subscriber,
        "consumer" => CoverageRole::Consumer,
        "affected_test" => CoverageRole::Test,
        "config" => CoverageRole::Config,
        "dependency" | "breadth_dependency" => CoverageRole::Dependency,
        "module_breadth" | "hub_type" => CoverageRole::ModuleContext,
        "handler_dependency" => CoverageRole::Implementation,
        "counter_evidence" => CoverageRole::CounterEvidence,
        _ if reason.contains("public_exposure") || reason.contains("public exposure") => {
            CoverageRole::PublicExposure
        }
        _ if reason.contains("wrapper") || reason.contains("alias") => CoverageRole::WrapperAlias,
        _ if reason.contains("counter") || reason.contains("negative") => {
            CoverageRole::CounterEvidence
        }
        _ => match (kind, channel) {
            (EvidencePackKind::CallsiteEnumeration, _) => CoverageRole::DirectCaller,
            (EvidencePackKind::PipelineTrace, _) => CoverageRole::Implementation,
            (EvidencePackKind::DataFlow, "primary") => CoverageRole::CanonicalDefinition,
            (EvidencePackKind::DataFlow, _) => CoverageRole::StateMechanism,
            (EvidencePackKind::ImpactRadius, "primary") => CoverageRole::Implementation,
            (EvidencePackKind::ImpactRadius, _) => CoverageRole::AffectedCode,
            (EvidencePackKind::DependencyMap, "primary") => CoverageRole::CanonicalDefinition,
            (EvidencePackKind::DependencyMap, _) => CoverageRole::Dependency,
            (EvidencePackKind::SymbolLookup, "primary") => CoverageRole::CanonicalDefinition,
            (EvidencePackKind::SymbolLookup, _) => CoverageRole::Implementation,
        },
    }
}

fn verification_for(role: &str, reason: Option<&str>, source_backed: bool) -> VerificationStatus {
    let reason = reason.unwrap_or_default().to_ascii_lowercase();
    if reason.contains("ambiguous") || reason.contains("unresolved") || reason.contains("conflict")
    {
        VerificationStatus::Ambiguous
    } else if role == "candidate" || is_candidate_reason(Some(&reason)) || !source_backed {
        VerificationStatus::Candidate
    } else {
        VerificationStatus::Verified
    }
}

fn impact_role(file_path: Option<&str>, reason: Option<&str>, fallback_role: &str) -> String {
    let file_path = file_path.unwrap_or_default();
    let file_path_lower = file_path.to_ascii_lowercase();
    let reason = reason.unwrap_or_default().to_ascii_lowercase();

    if reason.contains("public_exposure") || reason.contains("public exposure") {
        "public_exposure"
    } else if reason.contains("wrapper") || reason.contains("alias") {
        "wrapper"
    } else if reason.contains("test") || crate::classify::is_test_file(file_path) {
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

struct RoleContract {
    required: &'static [CoverageRole],
    optional: &'static [CoverageRole],
}

fn role_contract(kind: EvidencePackKind) -> RoleContract {
    use CoverageRole::*;
    match kind {
        EvidencePackKind::CallsiteEnumeration => RoleContract {
            required: &[DirectCaller],
            optional: &[CanonicalDefinition, CounterEvidence],
        },
        EvidencePackKind::PipelineTrace => RoleContract {
            required: &[Producer, Bridge, Subscriber],
            optional: &[
                Implementation,
                StateMechanism,
                Channel,
                Consumer,
                CounterEvidence,
            ],
        },
        EvidencePackKind::DataFlow => RoleContract {
            required: &[CanonicalDefinition, StateMechanism],
            optional: &[DirectCaller, CounterEvidence],
        },
        EvidencePackKind::ImpactRadius => RoleContract {
            required: &[Implementation, AffectedCode, PublicExposure],
            optional: &[WrapperAlias, Test, Config, CounterEvidence],
        },
        EvidencePackKind::DependencyMap => RoleContract {
            required: &[CanonicalDefinition, Dependency],
            optional: &[PublicExposure, WrapperAlias, CounterEvidence],
        },
        EvidencePackKind::SymbolLookup => RoleContract {
            required: &[CanonicalDefinition],
            optional: &[
                StateMechanism,
                PublicExposure,
                CounterEvidence,
                ModuleContext,
            ],
        },
    }
}

fn coverage_role_name(role: CoverageRole) -> &'static str {
    use CoverageRole::*;
    match role {
        CanonicalDefinition => "canonical_definition",
        Implementation => "implementation",
        PublicExposure => "public_exposure",
        DirectCaller => "direct_caller",
        WrapperAlias => "wrapper_alias",
        StateMechanism => "state_mechanism",
        CounterEvidence => "counter_evidence",
        Producer => "producer",
        Bridge => "bridge",
        Channel => "channel",
        Subscriber => "subscriber",
        Consumer => "consumer",
        AffectedCode => "affected_code",
        Dependency => "dependency",
        Test => "test",
        Config => "config",
        ModuleContext => "module_context",
    }
}

fn coverage_for(kind: EvidencePackKind, rows: &[EvidenceRow], target: &str) -> Coverage {
    let contract = role_contract(kind);
    let required_roles = contract.required.to_vec();
    let optional_roles = contract.optional.to_vec();
    let location_claims = LocationClaimCoverage {
        policy: "exact_path_line_requires_source_backed_row".to_string(),
        source_backed_rows: rows.iter().filter(|row| row.source_backed).count(),
        unsupported_rows: rows.iter().filter(|row| !row.source_backed).count(),
    };

    if rows.is_empty() {
        return Coverage {
            status: CoverageStatus::NoHits,
            basis: "no evidence rows were found".to_string(),
            missing: "all".to_string(),
            required_roles: required_roles.clone(),
            optional_roles,
            resolved_roles: Vec::new(),
            missing_roles: required_roles,
            ambiguous_roles: Vec::new(),
            candidate_roles: Vec::new(),
            location_claims,
        };
    }

    let mut resolved = BTreeSet::new();
    let mut ambiguous = BTreeSet::new();
    let mut candidate = BTreeSet::new();
    for row in rows {
        match row.verification {
            VerificationStatus::Verified => {
                resolved.insert(row.coverage_role);
            }
            VerificationStatus::Ambiguous => {
                ambiguous.insert(row.coverage_role);
            }
            VerificationStatus::Candidate => {
                candidate.insert(row.coverage_role);
            }
        }
    }
    // A verified row wins over weaker evidence for the same semantic role.
    ambiguous.retain(|role| !resolved.contains(role));
    candidate.retain(|role| !resolved.contains(role) && !ambiguous.contains(role));

    let present = resolved
        .iter()
        .chain(ambiguous.iter())
        .chain(candidate.iter())
        .copied()
        .collect::<BTreeSet<_>>();
    let missing_roles = contract
        .required
        .iter()
        .filter(|role| !present.contains(role))
        .copied()
        .collect::<Vec<_>>();
    let unresolved_required = contract
        .required
        .iter()
        .filter(|role| !resolved.contains(role))
        .copied()
        .collect::<Vec<_>>();
    let status = if unresolved_required.is_empty() {
        CoverageStatus::Complete
    } else {
        CoverageStatus::Partial
    };
    let basis = match status {
        CoverageStatus::Complete => "all required evidence roles are source-backed and verified",
        CoverageStatus::Partial if matches!(kind, EvidencePackKind::CallsiteEnumeration) => {
            "callsite evidence is limited to search candidates"
        }
        CoverageStatus::Partial if matches!(kind, EvidencePackKind::PipelineTrace) => {
            "pipeline evidence is missing verified required roles"
        }
        CoverageStatus::Partial => "required evidence roles are missing, ambiguous, or candidates",
        CoverageStatus::NoHits => unreachable!("rows are non-empty"),
    }
    .to_string();
    let missing = if unresolved_required.is_empty() {
        String::new()
    } else if matches!(kind, EvidencePackKind::CallsiteEnumeration) {
        format!("verified callsite evidence for {target}")
    } else if matches!(kind, EvidencePackKind::PipelineTrace) {
        format!(
            "verified pipeline roles: {}",
            unresolved_required
                .iter()
                .map(|role| coverage_role_name(*role))
                .collect::<Vec<_>>()
                .join(",")
        )
    } else {
        format!(
            "verified evidence roles: {}",
            unresolved_required
                .iter()
                .map(|role| coverage_role_name(*role))
                .collect::<Vec<_>>()
                .join(",")
        )
    };

    Coverage {
        status,
        basis,
        missing,
        required_roles,
        optional_roles,
        resolved_roles: resolved.into_iter().collect(),
        missing_roles,
        ambiguous_roles: ambiguous.into_iter().collect(),
        candidate_roles: candidate.into_iter().collect(),
        location_claims,
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
            // Line-derived so fixtures model production, where symbol_id is
            // unique per symbol: the pack dedups exact symbol_id repeats.
            symbol_id: Some(format!("sym-{start_line}")),
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
    fn pipeline_pack_preserves_injected_via_as_role() {
        // R024: injected route/dependency rows entered pipeline_trace packs
        // with an inferred stage role ("dispatcher"), losing their provenance
        // and their truncation exemption; agents outlining from pack.rows
        // described the evidence but never cited the files.
        let pack = build_evidence_pack(EvidencePackInput {
            question: "trace how the desktop app receives a session token".to_string(),
            target: "login".to_string(),
            shape: InvestigationShape::CallTrace,
            primary: vec![location(10, "login();")],
            secondary: vec![
                location_via(
                    37,
                    ".post(\"/exchange\", async ({ body }) => {",
                    Some("route_endpoint"),
                ),
                location_via(
                    68,
                    "export async function withTransaction(fn) {",
                    Some("handler_dependency"),
                ),
            ],
            secondary_via: Some("get_call_hierarchy".to_string()),
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        assert_eq!(value["kind"], "pipeline_trace");
        let roles: Vec<String> = value["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["role"].as_str().unwrap().to_string())
            .collect();
        assert!(
            roles.contains(&"route_endpoint".to_string())
                && roles.contains(&"handler_dependency".to_string()),
            "injected rows must keep their via as the pack role: {roles:?}"
        );
    }

    #[test]
    fn symbol_lookup_pack_preserves_module_breadth_role() {
        // Arch questions classify as Discover, whose pack kind is
        // symbol_lookup; the breadth rows must keep their provenance role
        // there too (roles ARE the citation signal, R025).
        let pack = build_evidence_pack(EvidencePackInput {
            question: "How is anchoring split between the backend and the worker?".to_string(),
            target: "anchoring".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location(10, "sealEpoch();")],
            secondary: vec![location_via(
                55,
                "fn main() { run_loop() }",
                Some("module_breadth"),
            )],
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        let value = pack_to_value(&pack);

        let roles: Vec<String> = value["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["role"].as_str().unwrap().to_string())
            .collect();
        assert!(
            roles.contains(&"module_breadth".to_string()),
            "module_breadth rows must keep their via as the pack role: {roles:?}"
        );
    }

    #[test]
    fn pack_dedups_same_symbol_across_channels() {
        // django-arch-03: load_middleware rode once as the primary search hit
        // and again as the secondary call-hierarchy root node. Keep-first so
        // the primary copy (full body, top position) wins.
        let mut primary_loc = location(26, "def load_middleware(self):");
        primary_loc.symbol_id = Some("sym-lm".to_string());
        let mut secondary_loc = location(26, "def load_middleware(self):");
        secondary_loc.symbol_id = Some("sym-lm".to_string());
        secondary_loc.via = Some("get_call_hierarchy".to_string());

        let pack = build_evidence_pack(EvidencePackInput {
            question: "trace how middleware is assembled".to_string(),
            target: "load_middleware".to_string(),
            shape: InvestigationShape::CallTrace,
            primary: vec![primary_loc],
            secondary: vec![secondary_loc],
            secondary_via: Some("get_call_hierarchy".to_string()),
            extra_candidates: Vec::new(),
        });

        assert_eq!(pack.rows.len(), 1, "same symbol_id must ride once");
    }

    #[test]
    fn pack_drops_line_anchor_covered_by_visible_body() {
        // Four single-line "callback_producer" anchors of load_middleware
        // rode alongside its complete 76-line body row: pure redundancy
        // when the covering body is untruncated.
        let mut full = PackLocation {
            symbol_id: Some("sym-lm".to_string()),
            symbol_name: Some("load_middleware".to_string()),
            file_path: Some("src/base.py".to_string()),
            kind: Some("function".to_string()),
            start_line: Some(26),
            end_line: Some(30),
            via: None,
            body: Some("def load_middleware(self):\n  a\n  b\n  handler = x\n  done".to_string()),
        };
        full.via = Some("search_code".to_string());
        let anchor = PackLocation {
            symbol_id: Some("sym-lm:callback_producer:29".to_string()),
            symbol_name: Some("load_middleware".to_string()),
            file_path: Some("src/base.py".to_string()),
            kind: Some("callback_producer".to_string()),
            start_line: Some(29),
            end_line: Some(29),
            via: Some("non_callgraph_edges".to_string()),
            body: Some("handler = x".to_string()),
        };

        let pack = build_evidence_pack(EvidencePackInput {
            question: "trace how middleware is assembled".to_string(),
            target: "load_middleware".to_string(),
            shape: InvestigationShape::CallTrace,
            primary: vec![full],
            secondary: vec![],
            secondary_via: None,
            extra_candidates: vec![anchor],
        });

        assert_eq!(
            pack.rows.len(),
            1,
            "anchor inside a visible body must be dropped: {:?}",
            pack.rows
                .iter()
                .map(|r| (r.symbol_name.clone(), r.line))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn pack_keeps_line_anchor_beyond_truncated_body() {
        // A truncated covering body hides the anchored line, so the anchor
        // still carries unseen evidence and must ride.
        let full = PackLocation {
            symbol_id: Some("sym-big".to_string()),
            symbol_name: Some("Options".to_string()),
            file_path: Some("src/options.py".to_string()),
            kind: Some("class".to_string()),
            start_line: Some(1),
            end_line: Some(1025),
            via: Some("search_code".to_string()),
            body: Some("class Options:\n  a\n  b\n// ... 1022 more lines".to_string()),
        };
        let anchor = PackLocation {
            symbol_id: Some("sym-big:callback_producer:500".to_string()),
            symbol_name: Some("Options".to_string()),
            file_path: Some("src/options.py".to_string()),
            kind: Some("callback_producer".to_string()),
            start_line: Some(500),
            end_line: Some(500),
            via: Some("non_callgraph_edges".to_string()),
            body: Some("register(Options)".to_string()),
        };

        let pack = build_evidence_pack(EvidencePackInput {
            question: "trace how options are registered".to_string(),
            target: "Options".to_string(),
            shape: InvestigationShape::CallTrace,
            primary: vec![full],
            secondary: vec![],
            secondary_via: None,
            extra_candidates: vec![anchor],
        });

        assert_eq!(
            pack.rows.len(),
            2,
            "anchor beyond the visible body must survive"
        );
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
    fn short_cite_forms_pick_shortest_unique_suffix() {
        // R028: agents shorten long paths in prose ("aggregation.rs:23") and
        // collide when two files share a basename, halving mech via the
        // ambiguous_path flag. The cite form is the shortest '/'-boundary
        // suffix (>= 2 segments) unique across the indexed file list.
        let files = vec![
            "packages/backend-worker/src/aggregation.rs".to_string(),
            "packages/desktop/src/aggregation.rs".to_string(),
            "packages/backend/src/lib/receipt-signer.ts".to_string(),
            "src/main.rs".to_string(),
            "build.rs".to_string(),
        ];
        let forms = short_cite_forms(&files);

        // Both aggregation.rs end in "src/aggregation.rs" -> 3 segments needed.
        assert_eq!(
            forms["packages/backend-worker/src/aggregation.rs"],
            "backend-worker/src/aggregation.rs"
        );
        assert_eq!(
            forms["packages/desktop/src/aggregation.rs"],
            "desktop/src/aggregation.rs"
        );
        // Unique basename still gets >= 2 segments so it reads as a path,
        // extended past generic directories (lib/, src/) to a package anchor.
        assert_eq!(
            forms["packages/backend/src/lib/receipt-signer.ts"],
            "backend/src/lib/receipt-signer.ts"
        );
        // Generic lead is acceptable when no longer unique form improves it.
        assert_eq!(forms["src/main.rs"], "src/main.rs");
        // Single-segment path: the full path is the only form.
        assert_eq!(forms["build.rs"], "build.rs");
    }

    #[test]
    fn short_cite_forms_skip_generic_leading_segments() {
        // R030: agents copied "backend-worker/src/aggregation.rs" verbatim
        // but stripped the head off "src/aggregation.rs", landing on an
        // ambiguous bare basename. Prefer the package-anchored form even
        // when the src/-led suffix is already unique.
        let files = vec![
            "packages/backend-worker/src/aggregation.rs".to_string(),
            "packages/desktop/src-tauri/commands/report/aggregation.rs".to_string(),
        ];
        let forms = short_cite_forms(&files);
        assert_eq!(
            forms["packages/backend-worker/src/aggregation.rs"],
            "backend-worker/src/aggregation.rs"
        );
        assert_eq!(
            forms["packages/desktop/src-tauri/commands/report/aggregation.rs"],
            "report/aggregation.rs"
        );
    }

    #[test]
    fn apply_cite_forms_sets_row_cite_with_line_range() {
        let mut pack = build_evidence_pack(EvidencePackInput {
            question: "what is handler".to_string(),
            target: "handler".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location(10, "handler();")],
            secondary: vec![],
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        let forms = short_cite_forms(&["src/app.rs".to_string(), "other/app.rs".to_string()]);
        apply_cite_forms(&mut pack, &forms);

        let row = &pack.rows[0];
        // location() uses src/app.rs lines 10-11; evidence-line offset keeps
        // line at 10 here.
        assert_eq!(row.cite.as_deref(), Some("src/app.rs:10-11"));

        let value = pack_to_value(&pack);
        assert_eq!(value["rows"][0]["cite"], "src/app.rs:10-11");
    }

    #[test]
    fn apply_cite_forms_falls_back_to_full_path_for_unknown_files() {
        let mut pack = build_evidence_pack(EvidencePackInput {
            question: "what is handler".to_string(),
            target: "handler".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location(10, "handler();")],
            secondary: vec![],
            secondary_via: None,
            extra_candidates: Vec::new(),
        });

        let forms = short_cite_forms(&["unrelated/file.rs".to_string()]);
        apply_cite_forms(&mut pack, &forms);

        assert_eq!(pack.rows[0].cite.as_deref(), Some("src/app.rs:10-11"));
    }

    #[test]
    fn symbol_lookup_contract_resolves_canonical_definition() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "where is handler defined?".to_string(),
            target: "handler".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location_via(10, "fn handler() {}", Some("search_code"))],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });
        let value = pack_to_value(&pack);

        assert_eq!(value["rows"][0]["coverage_role"], "canonical_definition");
        assert_eq!(value["rows"][0]["verification"], "verified");
        assert_eq!(value["rows"][0]["source_backed"], true);
        assert_eq!(value["coverage"]["status"], "complete");
        assert_eq!(
            value["coverage"]["required_roles"],
            json!(["canonical_definition"])
        );
        assert_eq!(
            value["coverage"]["resolved_roles"],
            json!(["canonical_definition"])
        );
    }

    #[test]
    fn impact_contract_reports_missing_public_exposure() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "what breaks if handler changes?".to_string(),
            target: "handler".to_string(),
            shape: InvestigationShape::ImpactRadius,
            primary: vec![location_via(10, "fn handler() {}", Some("search_code"))],
            secondary: vec![location_via(20, "handler();", Some("find_affected_code"))],
            secondary_via: Some("find_affected_code".to_string()),
            extra_candidates: Vec::new(),
        });
        let value = pack_to_value(&pack);

        assert_eq!(value["coverage"]["status"], "partial");
        assert_eq!(
            value["coverage"]["missing_roles"],
            json!(["public_exposure"])
        );
        assert_eq!(
            value["coverage"]["resolved_roles"],
            json!(["implementation", "affected_code"])
        );
    }

    #[test]
    fn impact_contract_resolves_public_exposure_role() {
        let mut public = location_via(30, "export { handler };", Some("find_affected_code"));
        public.kind = Some("public_exposure".to_string());
        let pack = build_evidence_pack(EvidencePackInput {
            question: "what breaks if handler changes?".to_string(),
            target: "handler".to_string(),
            shape: InvestigationShape::ImpactRadius,
            primary: vec![location_via(10, "fn handler() {}", Some("search_code"))],
            secondary: vec![
                location_via(20, "handler();", Some("find_affected_code")),
                public,
            ],
            secondary_via: Some("find_affected_code".to_string()),
            extra_candidates: Vec::new(),
        });
        let value = pack_to_value(&pack);

        assert_eq!(value["coverage"]["status"], "complete");
        let public_row = value["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["coverage_role"] == "public_exposure")
            .expect("public exposure row");
        assert_eq!(public_row["role"], "public_exposure");
        assert_eq!(public_row["verification"], "verified");
    }

    #[test]
    fn supporting_definition_is_explicit_cached_state_evidence() {
        let mut cached_state = location_via(
            40,
            "static INSTANCE: OnceLock<Service> = OnceLock::new();",
            Some("supporting_definition"),
        );
        cached_state.symbol_name = Some("INSTANCE".to_string());
        cached_state.kind = Some("static".to_string());
        let pack = build_evidence_pack(EvidencePackInput {
            question: "how is Service lazily initialized and cached?".to_string(),
            target: "Service".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![location_via(10, "struct Service {}", Some("search_code"))],
            secondary: vec![cached_state],
            secondary_via: None,
            extra_candidates: Vec::new(),
        });
        let value = pack_to_value(&pack);

        let state = value["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["coverage_role"] == "state_mechanism")
            .expect("state mechanism row");
        assert_eq!(state["role"], "supporting_definition");
        assert_eq!(state["verification"], "verified");
        assert!(value["coverage"]["resolved_roles"]
            .as_array()
            .unwrap()
            .contains(&json!("state_mechanism")));
    }

    #[test]
    fn ambiguous_required_role_is_not_reported_as_resolved() {
        let mut ambiguous = location_via(10, "fn handler() {}", Some("search_code"));
        ambiguous.kind = Some("ambiguous_binding".to_string());
        let pack = build_evidence_pack(EvidencePackInput {
            question: "where is handler defined?".to_string(),
            target: "handler".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![ambiguous],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });
        let value = pack_to_value(&pack);

        assert_eq!(value["coverage"]["status"], "partial");
        assert_eq!(
            value["coverage"]["ambiguous_roles"],
            json!(["canonical_definition"])
        );
        assert_eq!(value["coverage"]["missing_roles"], json!([]));
        assert_eq!(value["coverage"]["resolved_roles"], json!([]));
    }

    #[test]
    fn exact_citation_requires_source_backed_location() {
        let mut unsupported = location_via(10, "handler", Some("search_code"));
        unsupported.start_line = None;
        unsupported.end_line = None;
        let mut pack = build_evidence_pack(EvidencePackInput {
            question: "where is handler defined?".to_string(),
            target: "handler".to_string(),
            shape: InvestigationShape::Discover,
            primary: vec![unsupported],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });
        apply_cite_forms(&mut pack, &short_cite_forms(&["src/app.rs".to_string()]));
        let value = pack_to_value(&pack);

        assert_eq!(value["rows"][0]["source_backed"], false);
        assert_eq!(value["rows"][0]["verification"], "candidate");
        assert!(value["rows"][0].get("cite").is_none());
        assert_eq!(value["coverage"]["location_claims"]["unsupported_rows"], 1);
    }

    #[test]
    fn budget_selector_retains_each_required_impact_role() {
        let mut public = location_via(90, "export { handler };", Some("find_affected_code"));
        public.kind = Some("public_exposure".to_string());
        let pack = build_evidence_pack(EvidencePackInput {
            question: "what breaks if handler changes?".to_string(),
            target: "handler".to_string(),
            shape: InvestigationShape::ImpactRadius,
            primary: vec![location_via(10, "fn handler() {}", Some("search_code"))],
            secondary: vec![
                location_via(20, "handler();", Some("find_affected_code")),
                location_via(30, "handler();", Some("find_affected_code")),
                location_via(40, "handler();", Some("find_affected_code")),
                public,
            ],
            secondary_via: Some("find_affected_code".to_string()),
            extra_candidates: Vec::new(),
        });
        let value = pack_to_value(&pack);
        let selected =
            select_pack_rows_for_budget(value["rows"].as_array().unwrap(), &value["coverage"], 3);
        let roles = selected
            .iter()
            .filter_map(|row| row["coverage_role"].as_str())
            .collect::<HashSet<_>>();

        assert_eq!(
            roles,
            HashSet::from(["implementation", "affected_code", "public_exposure"])
        );
    }

    #[test]
    fn budget_refresh_does_not_claim_omitted_required_roles() {
        let pack = build_evidence_pack(EvidencePackInput {
            question: "trace how events cross the bridge".to_string(),
            target: "event".to_string(),
            shape: InvestigationShape::CallTrace,
            primary: vec![
                location(10, "onBeforeToolUse(event);"),
                location(20, "webContents.send(event);"),
                location(30, "ipcRenderer.on(event);"),
            ],
            secondary: Vec::new(),
            secondary_via: None,
            extra_candidates: Vec::new(),
        });
        let mut value = pack_to_value(&pack);
        let selected =
            select_pack_rows_for_budget(value["rows"].as_array().unwrap(), &value["coverage"], 1);
        refresh_coverage_after_budget(&mut value["coverage"], &selected, 2);

        assert_eq!(value["coverage"]["status"], "partial");
        assert_eq!(value["coverage"]["resolved_roles"], json!(["producer"]));
        assert_eq!(
            value["coverage"]["missing_roles"],
            json!(["bridge", "subscriber"])
        );
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
        assert_eq!(
            value["coverage"]["missing_roles"],
            json!(["canonical_definition"])
        );
        assert_eq!(value["rows"].as_array().unwrap().len(), 0);
    }
}

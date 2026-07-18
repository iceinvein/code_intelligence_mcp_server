//! Typed enrichment orchestration for `investigate`.
//!
//! Passes collect immutable candidates from an evidence snapshot. A single
//! allocator owns deduplication, replacement, provenance, priority, and cost
//! decisions so individual passes cannot mutate shared vectors directly.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashSet};
use std::time::Instant;

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;

use super::investigation::{
    delivered_ids, endpoint_path_tokens, evidence_route_injections, evidence_route_tokens,
    handler_dependency_locations, injected_location, is_injected_via_str,
    is_module_breadth_question, is_system_mechanics_question, module_breadth_locations,
    path_matches_subsystem, path_segment_set, route_endpoint_locations, run_module_breadth_search,
    should_include_supporting_modules, sibling_route_locations, subsystem_candidate_tokens,
    supporting_definition_locations, token_segments_indexed, InvestigationShape, VerifiedLocation,
    BREADTH_DEPENDENCY_CAP, HANDLER_DEPENDENCY_CAP, HUB_TYPE_CAP, HUB_TYPE_MIN_FAN_IN,
    HUB_TYPE_TOKENS_CAP, MODULE_BREADTH_BODY_LINES, MODULE_BREADTH_CAP,
    MODULE_BREADTH_ROWS_PER_SUBSYSTEM, MODULE_BREADTH_SUBSYSTEM_CAP, SUPPORTING_DEFINITION_CAP,
};
use super::AppState;

const DEFAULT_ENRICHMENT_COST_BUDGET: u32 = 96;

#[derive(Debug, Clone)]
pub struct InvestigationContext {
    pub question: String,
    pub target: Option<String>,
    pub shape: InvestigationShape,
    pub supporting_definition_targets: Vec<(crate::storage::sqlite::SymbolRow, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRole {
    Primary,
    Secondary,
    SupportingDefinition,
    RouteEndpoint,
    SiblingRoute,
    HandlerDependency,
    ModuleBreadth,
    BreadthDependency,
    HubType,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CoverageState {
    pub resolved_roles: BTreeSet<EvidenceRole>,
    pub attempted_passes: BTreeSet<String>,
    pub skipped_passes: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplacementPolicy {
    KeepExisting,
    ReplaceSecondary,
    ReplaceAny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PassCost {
    SqliteLookups(u32),
    GraphWalk(u32),
    HybridSearch(u32),
}

impl PassCost {
    fn units(self) -> u32 {
        match self {
            Self::SqliteLookups(units) | Self::GraphWalk(units) | Self::HybridSearch(units) => {
                units
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct EvidenceCandidate {
    pub location: VerifiedLocation,
    pub role: EvidenceRole,
    pub provenance: &'static str,
    pub confidence: f32,
    pub priority: u16,
    pub cost: u32,
    pub replacement: ReplacementPolicy,
}

#[derive(Debug, Clone, Copy)]
pub struct PassDescriptor {
    pub id: &'static str,
    pub dependencies: &'static [&'static str],
    pub role: EvidenceRole,
    pub priority: u16,
    pub confidence: f32,
    pub cost: PassCost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applicability {
    Applicable,
    NotApplicable(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct EvidenceSnapshot<'a> {
    pub primary: &'a [VerifiedLocation],
    pub secondary: &'a [VerifiedLocation],
    pub coverage: &'a CoverageState,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnrichmentTrace {
    pub pass: &'static str,
    pub status: &'static str,
    pub dependencies: &'static [&'static str],
    pub role: EvidenceRole,
    pub confidence: f32,
    pub priority: u16,
    pub cost: PassCost,
    pub elapsed_ms: u64,
    pub collected: usize,
    pub accepted: usize,
    pub rejected: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rejection_reasons: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<EvidenceDecision>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceDecision {
    pub symbol_id: String,
    pub file_path: String,
    pub role: EvidenceRole,
    pub provenance: &'static str,
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

#[derive(Debug)]
pub struct PipelineOutput {
    pub primary: Vec<VerifiedLocation>,
    pub secondary: Vec<VerifiedLocation>,
    pub coverage: CoverageState,
    pub trace: Vec<EnrichmentTrace>,
}

#[async_trait]
pub trait EnrichmentPass: Send + Sync {
    fn descriptor(&self) -> PassDescriptor;

    fn applicability(
        &self,
        context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Applicability;

    async fn collect(
        &self,
        state: &AppState,
        context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Result<Vec<EvidenceCandidate>>;
}

struct EvidenceAllocator {
    primary: Vec<VerifiedLocation>,
    secondary: Vec<VerifiedLocation>,
    coverage: CoverageState,
    spent_cost: u32,
    max_cost: u32,
}

impl EvidenceAllocator {
    fn new(
        primary: Vec<VerifiedLocation>,
        secondary: Vec<VerifiedLocation>,
        max_cost: u32,
    ) -> Self {
        let primary_keys = primary.iter().map(location_key).collect::<HashSet<_>>();
        let mut seen_secondary = HashSet::new();
        let secondary = secondary
            .into_iter()
            .filter(|location| {
                let key = location_key(location);
                !primary_keys.contains(&key) && seen_secondary.insert(key)
            })
            .collect::<Vec<_>>();

        let mut coverage = CoverageState::default();
        if !primary.is_empty() {
            coverage.resolved_roles.insert(EvidenceRole::Primary);
        }
        if !secondary.is_empty() {
            coverage.resolved_roles.insert(EvidenceRole::Secondary);
        }
        for location in &secondary {
            if let Some(role) = evidence_role_for_via(location.via) {
                coverage.resolved_roles.insert(role);
            }
        }

        Self {
            primary,
            secondary,
            coverage,
            spent_cost: 0,
            max_cost,
        }
    }

    fn snapshot(&self) -> EvidenceSnapshot<'_> {
        EvidenceSnapshot {
            primary: &self.primary,
            secondary: &self.secondary,
            coverage: &self.coverage,
        }
    }

    fn offer(&mut self, candidate: EvidenceCandidate) -> EvidenceDecision {
        if !candidate.confidence.is_finite() || candidate.confidence < 0.5 {
            return evidence_decision(&candidate, false, Some("below_confidence_floor"));
        }
        if self.spent_cost.saturating_add(candidate.cost) > self.max_cost {
            return evidence_decision(&candidate, false, Some("cost_budget_exhausted"));
        }

        let key = location_key(&candidate.location);
        let primary_pos = self
            .primary
            .iter()
            .position(|location| location_key(location) == key);
        let secondary_pos = self
            .secondary
            .iter()
            .position(|location| location_key(location) == key);

        match candidate.replacement {
            ReplacementPolicy::KeepExisting if primary_pos.is_some() || secondary_pos.is_some() => {
                return evidence_decision(&candidate, false, Some("duplicate_kept_existing"));
            }
            ReplacementPolicy::ReplaceSecondary if primary_pos.is_some() => {
                return evidence_decision(&candidate, false, Some("primary_has_precedence"));
            }
            ReplacementPolicy::ReplaceSecondary => {
                if let Some(index) = secondary_pos {
                    self.secondary.remove(index);
                }
            }
            ReplacementPolicy::ReplaceAny => {
                if let Some(index) = primary_pos {
                    self.primary.remove(index);
                }
                if let Some(index) = secondary_pos {
                    self.secondary.remove(index);
                }
            }
            ReplacementPolicy::KeepExisting => {}
        }

        self.spent_cost = self.spent_cost.saturating_add(candidate.cost);
        self.coverage.resolved_roles.insert(candidate.role);
        let decision = evidence_decision(&candidate, true, None);
        self.secondary.push(candidate.location);
        decision
    }
}

pub struct InvestigationEnrichmentPipeline {
    passes: Vec<Box<dyn EnrichmentPass>>,
    disabled: HashSet<String>,
    max_cost: u32,
}

impl InvestigationEnrichmentPipeline {
    pub fn all_from_env() -> Self {
        let disabled = std::env::var("INVESTIGATION_DISABLED_PASSES")
            .ok()
            .into_iter()
            .flat_map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        Self::all(disabled)
    }

    fn all(disabled: HashSet<String>) -> Self {
        Self {
            passes: vec![
                Box::new(SupportingDefinitionPass),
                Box::new(QuestionRoutePass),
                Box::new(EvidenceRoutePass),
                Box::new(SiblingRoutePass),
                Box::new(HandlerDependencyPass),
                Box::new(ModuleBreadthPass),
                Box::new(BreadthDependencyPass),
                Box::new(HubTypePass),
            ],
            disabled,
            max_cost: DEFAULT_ENRICHMENT_COST_BUDGET,
        }
    }

    pub async fn run(
        &self,
        state: &AppState,
        context: &InvestigationContext,
        primary: Vec<VerifiedLocation>,
        secondary: Vec<VerifiedLocation>,
    ) -> Result<PipelineOutput> {
        self.validate_ordering()?;
        let mut allocator = EvidenceAllocator::new(primary, secondary, self.max_cost);
        let mut trace = Vec::with_capacity(self.passes.len());

        for pass in &self.passes {
            let descriptor = pass.descriptor();
            let started = Instant::now();
            allocator
                .coverage
                .attempted_passes
                .insert(descriptor.id.to_string());

            if self.disabled.contains(descriptor.id) {
                allocator
                    .coverage
                    .skipped_passes
                    .insert(descriptor.id.to_string());
                observe_pass_metrics(state, descriptor.id, started, 0);
                trace.push(trace_entry(
                    descriptor,
                    "disabled",
                    started,
                    TraceResult::default(),
                ));
                continue;
            }

            let snapshot = allocator.snapshot();
            if let Applicability::NotApplicable(reason) = pass.applicability(context, &snapshot) {
                allocator
                    .coverage
                    .skipped_passes
                    .insert(descriptor.id.to_string());
                observe_pass_metrics(state, descriptor.id, started, 0);
                trace.push(trace_entry(
                    descriptor,
                    reason,
                    started,
                    TraceResult::default(),
                ));
                continue;
            }

            let mut candidates = pass.collect(state, context, &snapshot).await?;
            candidates.sort_by_key(|candidate| Reverse(candidate.priority));
            let collected = candidates.len();
            let mut accepted = 0;
            let mut rejection_reasons = Vec::new();
            let mut decisions = Vec::with_capacity(collected);
            for candidate in candidates {
                let decision = allocator.offer(candidate);
                if decision.accepted {
                    accepted += 1;
                } else if let Some(reason) = decision.reason {
                    rejection_reasons.push(reason);
                }
                decisions.push(decision);
            }
            rejection_reasons.sort_unstable();
            rejection_reasons.dedup();
            let rejected = collected.saturating_sub(accepted);
            observe_pass_metrics(state, descriptor.id, started, collected);
            trace.push(trace_entry(
                descriptor,
                "ran",
                started,
                TraceResult {
                    collected,
                    accepted,
                    rejected,
                    rejection_reasons,
                    decisions,
                },
            ));
        }

        Ok(PipelineOutput {
            primary: allocator.primary,
            secondary: allocator.secondary,
            coverage: allocator.coverage,
            trace,
        })
    }

    fn validate_ordering(&self) -> Result<()> {
        let mut seen = HashSet::new();
        for pass in &self.passes {
            let descriptor = pass.descriptor();
            for dependency in descriptor.dependencies {
                anyhow::ensure!(
                    seen.contains(dependency),
                    "Enrichment pass '{}' must run after dependency '{}'",
                    descriptor.id,
                    dependency
                );
            }
            anyhow::ensure!(
                seen.insert(descriptor.id),
                "Duplicate enrichment pass id '{}'",
                descriptor.id
            );
        }
        Ok(())
    }
}

pub fn trace_enabled_from_env() -> bool {
    std::env::var("INVESTIGATION_TRACE")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

fn observe_pass_metrics(state: &AppState, pass: &'static str, started: Instant, collected: usize) {
    state.retriever.metrics().observe_query_stage(
        "investigate_enrichment",
        pass,
        started.elapsed(),
    );
    state
        .retriever
        .metrics()
        .observe_query_candidates("investigate_enrichment", pass, collected);
}

#[derive(Default)]
struct TraceResult {
    collected: usize,
    accepted: usize,
    rejected: usize,
    rejection_reasons: Vec<&'static str>,
    decisions: Vec<EvidenceDecision>,
}

fn trace_entry(
    descriptor: PassDescriptor,
    status: &'static str,
    started: Instant,
    result: TraceResult,
) -> EnrichmentTrace {
    EnrichmentTrace {
        pass: descriptor.id,
        status,
        dependencies: descriptor.dependencies,
        role: descriptor.role,
        confidence: descriptor.confidence,
        priority: descriptor.priority,
        cost: descriptor.cost,
        elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        collected: result.collected,
        accepted: result.accepted,
        rejected: result.rejected,
        rejection_reasons: result.rejection_reasons,
        decisions: result.decisions,
    }
}

fn evidence_decision(
    candidate: &EvidenceCandidate,
    accepted: bool,
    reason: Option<&'static str>,
) -> EvidenceDecision {
    EvidenceDecision {
        symbol_id: candidate.location.symbol_id.clone(),
        file_path: candidate.location.file_path.clone(),
        role: candidate.role,
        provenance: candidate.provenance,
        accepted,
        reason,
    }
}

fn location_key(location: &VerifiedLocation) -> String {
    if !location.symbol_id.is_empty() {
        format!("symbol:{}", location.symbol_id)
    } else {
        format!(
            "location:{}:{}:{}",
            location.file_path, location.start_line, location.end_line
        )
    }
}

fn evidence_role_for_via(via: &str) -> Option<EvidenceRole> {
    match via {
        "supporting_definition" => Some(EvidenceRole::SupportingDefinition),
        "route_endpoint" => Some(EvidenceRole::RouteEndpoint),
        "sibling_route" => Some(EvidenceRole::SiblingRoute),
        "handler_dependency" => Some(EvidenceRole::HandlerDependency),
        "module_breadth" => Some(EvidenceRole::ModuleBreadth),
        "breadth_dependency" => Some(EvidenceRole::BreadthDependency),
        "hub_type" => Some(EvidenceRole::HubType),
        _ => None,
    }
}

fn candidate(
    location: VerifiedLocation,
    descriptor: PassDescriptor,
    replacement: ReplacementPolicy,
) -> EvidenceCandidate {
    EvidenceCandidate {
        location,
        role: descriptor.role,
        provenance: descriptor.id,
        confidence: descriptor.confidence,
        priority: descriptor.priority,
        cost: descriptor.cost.units(),
        replacement,
    }
}

struct SupportingDefinitionPass;

#[async_trait]
impl EnrichmentPass for SupportingDefinitionPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor {
            id: "supporting_definition",
            dependencies: &[],
            role: EvidenceRole::SupportingDefinition,
            priority: 110,
            confidence: 1.0,
            cost: PassCost::GraphWalk(1),
        }
    }

    fn applicability(
        &self,
        context: &InvestigationContext,
        _evidence: &EvidenceSnapshot<'_>,
    ) -> Applicability {
        if context.supporting_definition_targets.is_empty() {
            Applicability::NotApplicable("no_supporting_definitions")
        } else {
            Applicability::Applicable
        }
    }

    async fn collect(
        &self,
        _state: &AppState,
        context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Result<Vec<EvidenceCandidate>> {
        let descriptor = self.descriptor();
        let present = evidence
            .primary
            .iter()
            .chain(evidence.secondary.iter())
            .map(|location| location.symbol_id.as_str())
            .collect::<HashSet<_>>();
        let targets = context
            .supporting_definition_targets
            .iter()
            .filter(|(row, _)| !present.contains(row.id.as_str()))
            .cloned()
            .collect();
        Ok(
            supporting_definition_locations(targets, SUPPORTING_DEFINITION_CAP)
                .into_iter()
                .map(|location| candidate(location, descriptor, ReplacementPolicy::KeepExisting))
                .collect(),
        )
    }
}

struct QuestionRoutePass;

#[async_trait]
impl EnrichmentPass for QuestionRoutePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor {
            id: "question_route",
            dependencies: &["supporting_definition"],
            role: EvidenceRole::RouteEndpoint,
            priority: 90,
            confidence: 1.0,
            cost: PassCost::SqliteLookups(2),
        }
    }

    fn applicability(
        &self,
        context: &InvestigationContext,
        _evidence: &EvidenceSnapshot<'_>,
    ) -> Applicability {
        if endpoint_path_tokens(&context.question).is_empty() {
            Applicability::NotApplicable("no_question_route")
        } else {
            Applicability::Applicable
        }
    }

    async fn collect(
        &self,
        state: &AppState,
        context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Result<Vec<EvidenceCandidate>> {
        let descriptor = self.descriptor();
        let primary_ids = evidence
            .primary
            .iter()
            .map(|location| location.symbol_id.as_str())
            .collect::<HashSet<_>>();
        Ok(route_endpoint_locations(&state.sqlite, &context.question)?
            .into_iter()
            .filter(|location| !primary_ids.contains(location.symbol_id.as_str()))
            .map(|location| candidate(location, descriptor, ReplacementPolicy::ReplaceSecondary))
            .collect())
    }
}

struct EvidenceRoutePass;

#[async_trait]
impl EnrichmentPass for EvidenceRoutePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor {
            id: "evidence_route",
            dependencies: &["question_route"],
            role: EvidenceRole::RouteEndpoint,
            priority: 100,
            confidence: 0.95,
            cost: PassCost::SqliteLookups(2),
        }
    }

    fn applicability(
        &self,
        context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Applicability {
        let bodies = evidence
            .primary
            .iter()
            .chain(evidence.secondary.iter())
            .map(|location| location.body.as_str())
            .collect::<Vec<_>>();
        let question_tokens = endpoint_path_tokens(&context.question);
        if evidence_route_tokens(&bodies)
            .into_iter()
            .all(|token| question_tokens.contains(&token))
        {
            Applicability::NotApplicable("no_evidence_route")
        } else {
            Applicability::Applicable
        }
    }

    async fn collect(
        &self,
        state: &AppState,
        context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Result<Vec<EvidenceCandidate>> {
        let descriptor = self.descriptor();
        Ok(evidence_route_injections(
            &state.sqlite,
            &context.question,
            evidence.primary,
            evidence.secondary,
        )?
        .into_iter()
        .map(|location| candidate(location, descriptor, ReplacementPolicy::ReplaceAny))
        .collect())
    }
}

struct SiblingRoutePass;

#[async_trait]
impl EnrichmentPass for SiblingRoutePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor {
            id: "sibling_route",
            dependencies: &["evidence_route"],
            role: EvidenceRole::SiblingRoute,
            priority: 80,
            confidence: 0.9,
            cost: PassCost::SqliteLookups(2),
        }
    }

    fn applicability(
        &self,
        _context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Applicability {
        if evidence
            .coverage
            .resolved_roles
            .contains(&EvidenceRole::RouteEndpoint)
        {
            Applicability::Applicable
        } else {
            Applicability::NotApplicable("no_route_seed")
        }
    }

    async fn collect(
        &self,
        state: &AppState,
        _context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Result<Vec<EvidenceCandidate>> {
        let descriptor = self.descriptor();
        let routes = evidence
            .secondary
            .iter()
            .filter(|location| location.via == "route_endpoint")
            .cloned()
            .collect::<Vec<_>>();
        let present = delivered_ids(evidence.primary, evidence.secondary);
        Ok(sibling_route_locations(&state.sqlite, &routes, &present)?
            .into_iter()
            .map(|location| candidate(location, descriptor, ReplacementPolicy::ReplaceAny))
            .collect())
    }
}

struct HandlerDependencyPass;

#[async_trait]
impl EnrichmentPass for HandlerDependencyPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor {
            id: "handler_dependency",
            dependencies: &["sibling_route"],
            role: EvidenceRole::HandlerDependency,
            priority: 70,
            confidence: 0.9,
            cost: PassCost::GraphWalk(3),
        }
    }

    fn applicability(
        &self,
        _context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Applicability {
        if evidence.coverage.resolved_roles.iter().any(|role| {
            matches!(
                role,
                EvidenceRole::RouteEndpoint | EvidenceRole::SiblingRoute
            )
        }) {
            Applicability::Applicable
        } else {
            Applicability::NotApplicable("no_handler_seed")
        }
    }

    async fn collect(
        &self,
        state: &AppState,
        _context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Result<Vec<EvidenceCandidate>> {
        let descriptor = self.descriptor();
        let handler_ids = evidence
            .secondary
            .iter()
            .filter(|location| matches!(location.via, "route_endpoint" | "sibling_route"))
            .map(|location| location.symbol_id.clone())
            .collect::<Vec<_>>();
        let present = evidence
            .primary
            .iter()
            .chain(evidence.secondary.iter())
            .map(|location| location.symbol_id.as_str())
            .collect::<HashSet<_>>();
        Ok(handler_dependency_locations(
            &state.sqlite,
            &handler_ids,
            &present,
            "handler_dependency",
            HANDLER_DEPENDENCY_CAP,
        )?
        .into_iter()
        .map(|location| candidate(location, descriptor, ReplacementPolicy::KeepExisting))
        .collect())
    }
}

struct ModuleBreadthPass;

#[async_trait]
impl EnrichmentPass for ModuleBreadthPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor {
            id: "module_breadth",
            dependencies: &["handler_dependency"],
            role: EvidenceRole::ModuleBreadth,
            priority: 60,
            confidence: 0.9,
            cost: PassCost::HybridSearch(4),
        }
    }

    fn applicability(
        &self,
        context: &InvestigationContext,
        _evidence: &EvidenceSnapshot<'_>,
    ) -> Applicability {
        if context.shape == InvestigationShape::ModuleSurvey
            || is_module_breadth_question(&context.question)
        {
            Applicability::Applicable
        } else {
            Applicability::NotApplicable("not_a_breadth_question")
        }
    }

    async fn collect(
        &self,
        state: &AppState,
        context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Result<Vec<EvidenceCandidate>> {
        let descriptor = self.descriptor();
        let mut covered = evidence
            .primary
            .iter()
            .take(3)
            .chain(
                evidence
                    .secondary
                    .iter()
                    .filter(|location| is_injected_via_str(location.via)),
            )
            .map(|location| location.file_path.clone())
            .collect::<HashSet<_>>();
        let mut fresh = Vec::new();

        let segments = path_segment_set(
            state
                .sqlite
                .list_indexed_files()?
                .iter()
                .map(|(path, _)| path.as_str()),
        );
        let mut scoped_subsystems = 0usize;
        for token in subsystem_candidate_tokens(&context.question) {
            if scoped_subsystems >= MODULE_BREADTH_SUBSYSTEM_CAP {
                break;
            }
            if !token_segments_indexed(&token, &segments)
                || covered
                    .iter()
                    .any(|file| path_matches_subsystem(file, &token))
            {
                continue;
            }
            let scoped = run_module_breadth_search(state, &token, None)
                .await?
                .into_iter()
                .filter(|row| path_matches_subsystem(&row.file_path, &token))
                .collect();
            let covered_refs = covered.iter().map(String::as_str).collect::<HashSet<_>>();
            let rows =
                module_breadth_locations(scoped, &covered_refs, MODULE_BREADTH_ROWS_PER_SUBSYSTEM);
            if rows.is_empty() {
                continue;
            }
            scoped_subsystems += 1;
            covered.extend(rows.iter().map(|location| location.file_path.clone()));
            fresh.extend(rows);
        }

        if fresh.len() < MODULE_BREADTH_CAP {
            let broad =
                run_module_breadth_search(state, &context.question, context.target.as_deref())
                    .await?;
            let covered_refs = covered.iter().map(String::as_str).collect::<HashSet<_>>();
            fresh.extend(module_breadth_locations(
                broad,
                &covered_refs,
                MODULE_BREADTH_CAP - fresh.len(),
            ));
        }

        Ok(fresh
            .into_iter()
            .map(|location| candidate(location, descriptor, ReplacementPolicy::ReplaceAny))
            .collect())
    }
}

struct BreadthDependencyPass;

#[async_trait]
impl EnrichmentPass for BreadthDependencyPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor {
            id: "breadth_dependency",
            dependencies: &["module_breadth"],
            role: EvidenceRole::BreadthDependency,
            priority: 50,
            confidence: 0.85,
            cost: PassCost::GraphWalk(3),
        }
    }

    fn applicability(
        &self,
        _context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Applicability {
        if evidence
            .coverage
            .resolved_roles
            .contains(&EvidenceRole::ModuleBreadth)
        {
            Applicability::Applicable
        } else {
            Applicability::NotApplicable("no_breadth_seed")
        }
    }

    async fn collect(
        &self,
        state: &AppState,
        _context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Result<Vec<EvidenceCandidate>> {
        let descriptor = self.descriptor();
        let breadth_ids = evidence
            .secondary
            .iter()
            .filter(|location| location.via == "module_breadth")
            .map(|location| location.symbol_id.clone())
            .collect::<Vec<_>>();
        let present = evidence
            .primary
            .iter()
            .chain(evidence.secondary.iter())
            .map(|location| location.symbol_id.as_str())
            .collect::<HashSet<_>>();
        Ok(handler_dependency_locations(
            &state.sqlite,
            &breadth_ids,
            &present,
            "breadth_dependency",
            BREADTH_DEPENDENCY_CAP,
        )?
        .into_iter()
        .map(|location| candidate(location, descriptor, ReplacementPolicy::KeepExisting))
        .collect())
    }
}

struct HubTypePass;

#[async_trait]
impl EnrichmentPass for HubTypePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor {
            id: "hub_type",
            dependencies: &["breadth_dependency"],
            role: EvidenceRole::HubType,
            priority: 40,
            confidence: 0.85,
            cost: PassCost::SqliteLookups(4),
        }
    }

    fn applicability(
        &self,
        context: &InvestigationContext,
        _evidence: &EvidenceSnapshot<'_>,
    ) -> Applicability {
        if is_system_mechanics_question(&context.question)
            || is_module_breadth_question(&context.question)
            || should_include_supporting_modules(&context.question, context.shape)
        {
            Applicability::Applicable
        } else {
            Applicability::NotApplicable("not_a_system_question")
        }
    }

    async fn collect(
        &self,
        state: &AppState,
        context: &InvestigationContext,
        evidence: &EvidenceSnapshot<'_>,
    ) -> Result<Vec<EvidenceCandidate>> {
        let descriptor = self.descriptor();
        let present = evidence
            .primary
            .iter()
            .chain(evidence.secondary.iter())
            .map(|location| location.symbol_id.as_str())
            .collect::<HashSet<_>>();
        let files = state.sqlite.list_indexed_files()?;
        let segments = path_segment_set(files.iter().map(|(path, _)| path.as_str()));
        let mut hubs = Vec::new();
        for token in subsystem_candidate_tokens(&context.question)
            .into_iter()
            .filter(|token| token_segments_indexed(token, &segments))
            .take(HUB_TYPE_TOKENS_CAP)
        {
            for (row, fan_in) in state.sqlite.list_hub_types_matching(
                &token,
                HUB_TYPE_MIN_FAN_IN,
                HUB_TYPE_CAP * 2,
            )? {
                if present.contains(row.id.as_str())
                    || crate::classify::is_test_file(&row.file_path)
                    || crate::classify::is_generated_output_path(&row.file_path)
                    || hubs
                        .iter()
                        .any(|(seen, _): &(crate::storage::sqlite::SymbolRow, u64)| {
                            seen.id == row.id
                        })
                {
                    continue;
                }
                hubs.push((row, fan_in));
            }
        }
        hubs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));

        Ok(hubs
            .into_iter()
            .take(HUB_TYPE_CAP)
            .map(|(row, _)| injected_location(row, "hub_type", MODULE_BREADTH_BODY_LINES))
            .map(|location| candidate(location, descriptor, ReplacementPolicy::KeepExisting))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(id: &str, via: &'static str) -> VerifiedLocation {
        VerifiedLocation {
            symbol_id: id.to_string(),
            symbol_name: id.to_string(),
            file_path: format!("src/{id}.rs"),
            kind: "function".to_string(),
            start_line: 1,
            end_line: 2,
            via,
            body: format!("fn {id}() {{}}"),
            route_exposure: Vec::new(),
        }
    }

    fn route_candidate(id: &str, replacement: ReplacementPolicy) -> EvidenceCandidate {
        EvidenceCandidate {
            location: location(id, "route_endpoint"),
            role: EvidenceRole::RouteEndpoint,
            provenance: "test_route",
            confidence: 1.0,
            priority: 100,
            cost: 1,
            replacement,
        }
    }

    #[test]
    fn allocator_characterizes_primary_first_dedup() {
        let allocator = EvidenceAllocator::new(
            vec![location("same", "search_code")],
            vec![
                location("same", "call_hierarchy"),
                location("other", "call_hierarchy"),
            ],
            10,
        );

        assert_eq!(allocator.primary.len(), 1);
        assert_eq!(allocator.secondary.len(), 1);
        assert_eq!(allocator.secondary[0].symbol_id, "other");
    }

    #[test]
    fn question_route_replaces_secondary_but_preserves_primary() {
        let mut allocator = EvidenceAllocator::new(
            vec![location("primary", "search_code")],
            vec![location("secondary", "call_hierarchy")],
            10,
        );

        let rejected = allocator.offer(route_candidate(
            "primary",
            ReplacementPolicy::ReplaceSecondary,
        ));
        assert!(!rejected.accepted);
        assert_eq!(rejected.reason, Some("primary_has_precedence"));

        let accepted = allocator.offer(route_candidate(
            "secondary",
            ReplacementPolicy::ReplaceSecondary,
        ));
        assert!(accepted.accepted);
        assert_eq!(allocator.primary[0].via, "search_code");
        assert_eq!(allocator.secondary[0].via, "route_endpoint");
    }

    #[test]
    fn evidence_route_replaces_doomed_primary_copy() {
        let mut allocator =
            EvidenceAllocator::new(vec![location("route", "search_code")], Vec::new(), 10);

        let decision = allocator.offer(route_candidate("route", ReplacementPolicy::ReplaceAny));
        assert!(decision.accepted);

        assert!(allocator.primary.is_empty());
        assert_eq!(allocator.secondary[0].via, "route_endpoint");
        assert!(allocator
            .coverage
            .resolved_roles
            .contains(&EvidenceRole::RouteEndpoint));
    }

    #[test]
    fn allocator_enforces_candidate_cost_budget() {
        let mut allocator = EvidenceAllocator::new(Vec::new(), Vec::new(), 1);
        let mut expensive = route_candidate("route", ReplacementPolicy::KeepExisting);
        expensive.cost = 2;

        let decision = allocator.offer(expensive);
        assert!(!decision.accepted);
        assert_eq!(decision.reason, Some("cost_budget_exhausted"));
        assert!(allocator.secondary.is_empty());
    }

    #[test]
    fn enrichment_passes_declare_dependencies_in_order() {
        let pipeline = InvestigationEnrichmentPipeline::all(HashSet::new());
        pipeline.validate_ordering().unwrap();
        let ids = pipeline
            .passes
            .iter()
            .map(|pass| pass.descriptor().id)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "supporting_definition",
                "question_route",
                "evidence_route",
                "sibling_route",
                "handler_dependency",
                "module_breadth",
                "breadth_dependency",
                "hub_type",
            ]
        );
    }

    #[test]
    fn passes_can_be_disabled_independently() {
        let pipeline =
            InvestigationEnrichmentPipeline::all(HashSet::from(["sibling_route".to_string()]));
        assert!(pipeline.disabled.contains("sibling_route"));
        assert!(!pipeline.disabled.contains("question_route"));
    }
}

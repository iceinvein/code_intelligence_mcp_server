use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tools::PlanCodeInvestigationTool;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationIntent {
    ImpactAnalysis,
    DataFlow,
    DependencyWalk,
    ModuleSummary,
    FileSummary,
    SymbolLookup,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecommendedStep {
    pub tool: String,
    pub why: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvestigationPlan {
    pub intent: InvestigationIntent,
    pub confidence: f32,
    pub target: Option<String>,
    pub recommended_steps: Vec<RecommendedStep>,
    pub avoid: Vec<String>,
}

pub fn plan_code_investigation(
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
    max_steps: usize,
) -> Result<InvestigationPlan> {
    let question = question.trim();
    if question.is_empty() {
        bail!("question is required");
    }

    let max_steps = max_steps.clamp(1, 6);
    let normalized = question.to_lowercase();
    let intent = classify_intent(&normalized, file_path);
    let target = target
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| derive_target(question, file_path));

    let confidence = confidence_for(&intent, target.as_deref(), file_path);
    let mut recommended_steps = steps_for(&intent, question, target.as_deref(), file_path);
    recommended_steps.truncate(max_steps);

    Ok(InvestigationPlan {
        intent,
        confidence,
        target,
        recommended_steps,
        avoid: vec![
            "Do not start with grep unless code-intelligence cannot locate the target.".to_string(),
        ],
    })
}

pub fn handle_plan_code_investigation(tool: PlanCodeInvestigationTool) -> Result<Value> {
    let plan = plan_code_investigation(
        &tool.question,
        tool.target.as_deref(),
        tool.file_path.as_deref(),
        tool.max_steps.unwrap_or(4) as usize,
    )?;
    Ok(json!(plan))
}

fn classify_intent(question: &str, file_path: Option<&str>) -> InvestigationIntent {
    if contains_any(
        question,
        &[
            "data flow",
            "where does this value come from",
            "where is this value used",
            "read and written",
            "reads and writes",
            "written",
            "writes",
        ],
    ) {
        return InvestigationIntent::DataFlow;
    }

    if contains_any(
        question,
        &[
            "what breaks",
            "impact",
            "rename",
            "change",
            "refactor",
            "blast radius",
            "affected",
            "callers",
        ],
    ) {
        return InvestigationIntent::ImpactAnalysis;
    }

    if contains_any(
        question,
        &[
            "who imports",
            "depends on",
            "dependency graph",
            "upstream",
            "downstream",
            "imports this module",
        ],
    ) {
        return InvestigationIntent::DependencyWalk;
    }

    if contains_any(
        question,
        &[
            "public api",
            "what's in this module",
            "whats in this module",
            "walk me through this module",
            "exports",
        ],
    ) {
        return InvestigationIntent::ModuleSummary;
    }

    if file_path.is_some()
        && contains_any(
            question,
            &[
                "summarize this file",
                "what's in this file",
                "whats in this file",
                "symbol-level summary",
            ],
        )
    {
        return InvestigationIntent::FileSummary;
    }

    InvestigationIntent::SymbolLookup
}

fn steps_for(
    intent: &InvestigationIntent,
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
) -> Vec<RecommendedStep> {
    match intent {
        InvestigationIntent::ImpactAnalysis => impact_steps(question, target, file_path),
        InvestigationIntent::DataFlow => dataflow_steps(question, target, file_path),
        InvestigationIntent::DependencyWalk => dependency_steps(question, target, file_path),
        InvestigationIntent::ModuleSummary => module_summary_steps(question, target, file_path),
        InvestigationIntent::FileSummary => file_summary_steps(question, target, file_path),
        InvestigationIntent::SymbolLookup => symbol_lookup_steps(question, target, file_path),
    }
}

fn impact_steps(
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
) -> Vec<RecommendedStep> {
    let query = target.unwrap_or(question);
    let mut steps = vec![search_step(query)];
    let symbol_name = target.unwrap_or(query);
    steps.push(RecommendedStep {
        tool: "find_affected_code".to_string(),
        why: "Find reverse dependencies affected by changes to the target.".to_string(),
        arguments: optional_file_args(
            json!({
                "symbol_name": symbol_name,
                "depth": 3,
                "limit": 20
            }),
            file_path,
        ),
    });
    steps.push(RecommendedStep {
        tool: "predict_impact".to_string(),
        why: "Add git co-change signal alongside the static graph.".to_string(),
        arguments: optional_file_args(
            json!({
                "symbol_name": symbol_name,
                "limit": 20
            }),
            file_path,
        ),
    });
    steps.push(hydrate_step());
    steps
}

fn dataflow_steps(
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
) -> Vec<RecommendedStep> {
    let query = target.unwrap_or(question);
    let symbol_name = target.unwrap_or(query);
    vec![
        search_step(query),
        RecommendedStep {
            tool: "trace_data_flow".to_string(),
            why: "Trace where the target is read and written across the codebase.".to_string(),
            arguments: optional_file_args(
                json!({
                    "symbol_name": symbol_name,
                    "direction": "both",
                    "depth": 3,
                    "limit": 20
                }),
                file_path,
            ),
        },
        hydrate_step(),
    ]
}

fn dependency_steps(
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
) -> Vec<RecommendedStep> {
    let query = target.unwrap_or(question);
    let symbol_name = target.unwrap_or(query);
    let direction = infer_dependency_direction(question);
    vec![
        search_step(query),
        RecommendedStep {
            tool: "explore_dependency_graph".to_string(),
            why: "Walk module-level dependency edges instead of grepping import statements."
                .to_string(),
            arguments: optional_file_args(
                json!({
                    "symbol_name": symbol_name,
                    "direction": direction,
                    "depth": 3,
                    "limit": 20
                }),
                file_path,
            ),
        },
    ]
}

fn module_summary_steps(
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
) -> Vec<RecommendedStep> {
    if let Some(path) = file_path.or(target) {
        vec![RecommendedStep {
            tool: "get_module_summary".to_string(),
            why: "Summarize the exported public API surface for the module.".to_string(),
            arguments: json!({
                "file_path": path,
                "group_by_kind": true
            }),
        }]
    } else {
        vec![search_step(question)]
    }
}

fn file_summary_steps(
    question: &str,
    _target: Option<&str>,
    file_path: Option<&str>,
) -> Vec<RecommendedStep> {
    if let Some(path) = file_path {
        vec![RecommendedStep {
            tool: "summarize_file".to_string(),
            why: "Get a symbol-level summary without reading the whole file.".to_string(),
            arguments: json!({
                "file_path": path,
                "include_signatures": true,
                "verbose": false
            }),
        }]
    } else {
        vec![search_step(question)]
    }
}

fn symbol_lookup_steps(
    question: &str,
    target: Option<&str>,
    file_path: Option<&str>,
) -> Vec<RecommendedStep> {
    let query = target.unwrap_or(question);
    let mut steps = vec![search_step(query), hydrate_step()];
    let lower = question.to_lowercase();
    if lower.contains("definition") || lower.contains("defined") {
        steps.push(RecommendedStep {
            tool: "get_definition".to_string(),
            why: "Fetch definition context once the symbol name is known.".to_string(),
            arguments: optional_file_args(
                json!({
                    "symbol_name": query,
                    "limit": 10
                }),
                file_path,
            ),
        });
    } else if lower.contains("reference") || lower.contains("references") || lower.contains("uses")
    {
        steps.push(RecommendedStep {
            tool: "find_references".to_string(),
            why: "Find direct references when the question asks for uses.".to_string(),
            arguments: optional_file_args(
                json!({
                    "symbol_name": query,
                    "reference_type": "all",
                    "limit": 200
                }),
                file_path,
            ),
        });
    }
    steps
}

fn search_step(query: &str) -> RecommendedStep {
    RecommendedStep {
        tool: "search_code".to_string(),
        why: "Locate the target symbol or module before choosing a specialist follow-up."
            .to_string(),
        arguments: json!({
            "query": query,
            "limit": 5
        }),
    }
}

fn hydrate_step() -> RecommendedStep {
    RecommendedStep {
        tool: "hydrate_symbols".to_string(),
        why: "Fetch source bodies for symbol IDs returned by earlier code-intelligence tools."
            .to_string(),
        arguments: json!({
            "ids": ["<symbol IDs from previous step>"],
            "mode": "full",
            "verbose": true
        }),
    }
}

fn optional_file_args(mut value: Value, file_path: Option<&str>) -> Value {
    if let (Value::Object(map), Some(path)) = (&mut value, file_path) {
        map.insert("file_path".to_string(), Value::String(path.to_string()));
        map.insert("file".to_string(), Value::String(path.to_string()));
    }
    value
}

fn infer_dependency_direction(question: &str) -> &'static str {
    let lower = question.to_lowercase();
    if contains_any(
        &lower,
        &[
            "who imports",
            "depends on this",
            "upstream",
            "imports this module",
        ],
    ) {
        "upstream"
    } else if contains_any(&lower, &["what does this depend on", "downstream"]) {
        "downstream"
    } else {
        "upstream"
    }
}

fn confidence_for(
    intent: &InvestigationIntent,
    target: Option<&str>,
    file_path: Option<&str>,
) -> f32 {
    match intent {
        InvestigationIntent::SymbolLookup => {
            if target.is_some() || file_path.is_some() {
                0.72
            } else {
                0.58
            }
        }
        InvestigationIntent::FileSummary | InvestigationIntent::ModuleSummary
            if file_path.is_some() =>
        {
            0.9
        }
        _ if target.is_some() || file_path.is_some() => 0.86,
        _ => 0.74,
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn derive_target(question: &str, file_path: Option<&str>) -> Option<String> {
    if let Some(path) = file_path {
        return Some(path.to_string());
    }

    for quote in ['`', '"', '\''] {
        let mut parts = question.split(quote);
        while let Some(_before) = parts.next() {
            if let Some(candidate) = parts.next() {
                let candidate = candidate.trim();
                if looks_like_target(candidate) {
                    return Some(candidate.to_string());
                }
            }
        }
    }

    question
        .split_whitespace()
        .map(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != ':'))
        .find(|word| looks_like_target(word))
        .map(ToOwned::to_owned)
}

fn looks_like_target(value: &str) -> bool {
    if value.len() < 3 || value.contains(' ') {
        return false;
    }
    value.contains("::")
        || value.contains('_')
        || value.contains('/')
        || value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_tools(plan: &InvestigationPlan) -> Vec<&str> {
        plan.recommended_steps
            .iter()
            .map(|step| step.tool.as_str())
            .collect()
    }

    #[test]
    fn impact_phrase_routes_to_impact_tools() {
        let plan = plan_code_investigation(
            "what breaks if I refactor PathNormalizer?",
            Some("PathNormalizer"),
            None,
            4,
        )
        .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::ImpactAnalysis);
        let tools = step_tools(&plan);
        assert!(tools.contains(&"search_code"), "tools were {tools:?}");
        assert!(
            tools.contains(&"find_affected_code"),
            "tools were {tools:?}"
        );
        assert!(tools.contains(&"predict_impact"), "tools were {tools:?}");
    }

    #[test]
    fn dataflow_phrase_routes_to_trace_data_flow() {
        let plan = plan_code_investigation(
            "where does this value come from and where is it written?",
            Some("repo_id"),
            None,
            4,
        )
        .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::DataFlow);
        let tools = step_tools(&plan);
        assert!(tools.contains(&"trace_data_flow"), "tools were {tools:?}");
    }

    #[test]
    fn dependency_phrase_routes_to_dependency_graph() {
        let plan =
            plan_code_investigation("who imports this module?", Some("src/storage"), None, 4)
                .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::DependencyWalk);
        let tools = step_tools(&plan);
        assert!(
            tools.contains(&"explore_dependency_graph"),
            "tools were {tools:?}"
        );
    }

    #[test]
    fn public_api_phrase_routes_to_module_summary() {
        let plan = plan_code_investigation(
            "walk me through the public API of the storage module",
            Some("src/storage"),
            Some("src/storage"),
            4,
        )
        .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::ModuleSummary);
        let tools = step_tools(&plan);
        assert!(
            tools.contains(&"get_module_summary"),
            "tools were {tools:?}"
        );
    }

    #[test]
    fn file_summary_with_path_routes_to_summarize_file() {
        let plan =
            plan_code_investigation("summarize this file", None, Some("src/server/mod.rs"), 4)
                .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::FileSummary);
        let tools = step_tools(&plan);
        assert!(tools.contains(&"summarize_file"), "tools were {tools:?}");
    }

    #[test]
    fn generic_lookup_routes_to_search_code() {
        let plan = plan_code_investigation(
            "where is PathNormalizer defined?",
            Some("PathNormalizer"),
            None,
            4,
        )
        .unwrap();

        assert_eq!(plan.intent, InvestigationIntent::SymbolLookup);
        let tools = step_tools(&plan);
        assert_eq!(tools.first(), Some(&"search_code"));
    }

    #[test]
    fn max_steps_truncates_recommendations() {
        let plan = plan_code_investigation(
            "what breaks if I rename PathNormalizer?",
            Some("PathNormalizer"),
            None,
            2,
        )
        .unwrap();

        assert_eq!(plan.recommended_steps.len(), 2);
        let tools = step_tools(&plan);
        assert_eq!(tools, vec!["search_code", "find_affected_code"]);
    }

    #[test]
    fn empty_question_returns_error() {
        let err = plan_code_investigation("   ", None, None, 4).unwrap_err();
        assert!(
            err.to_string().contains("question is required"),
            "unexpected error: {err}"
        );
    }
}

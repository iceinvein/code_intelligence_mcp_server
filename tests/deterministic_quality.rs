//! Engine-only quality gate. No answering agent, network, or model download.

mod support;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use code_intelligence_mcp_server::handlers::{handle_find_affected_code, handle_get_definition};
use code_intelligence_mcp_server::retrieval::ContextMode;
use code_intelligence_mcp_server::storage::sqlite::SymbolRow;
use code_intelligence_mcp_server::tools::{FindAffectedCodeTool, GetDefinitionTool};
use tempfile::TempDir;

use support::fixtures::{app_state_for_config, test_config_for_dir};
use support::quality::{ranking_metrics, set_metrics, RankingMetrics};

const RETRIEVAL_CASES: &[(&str, &str)] = &[
    ("rust_quality_anchor", "rust_quality.rs"),
    ("typescriptQualityAnchor", "typescript_quality.ts"),
    ("python_quality_anchor", "python_quality.py"),
    ("GoQualityAnchor", "go_quality.go"),
    (
        "JavaQualityService.javaQualityAnchor",
        "JavaQualityService.java",
    ),
    ("kotlinQualityAnchor", "kotlin_quality.kt"),
    (
        "CSharpQualityService.CSharpQualityAnchor",
        "CSharpQualityService.cs",
    ),
    ("swiftQualityAnchor", "swift_quality.swift"),
    ("c_quality_anchor", "c_quality.c"),
    ("cpp_quality_anchor", "cpp_quality.cpp"),
    ("ruby_quality_anchor", "ruby_quality.rb"),
];

const GRAPH_CASES: &[(&str, &str)] = &[
    ("rust_quality_anchor", "rust_quality_leaf"),
    ("typescriptQualityAnchor", "typescriptQualityLeaf"),
    ("python_quality_anchor", "python_quality_leaf"),
    ("GoQualityAnchor", "GoQualityLeaf"),
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deterministic_engine_quality_gate() {
    let workspace = TempDir::new().expect("quality tempdir");
    copy_polyglot_fixture(workspace.path());
    let config = test_config_for_dir(workspace.path().to_path_buf());
    let state = app_state_for_config(config).await;
    let stats = state
        .indexer
        .index_all()
        .await
        .expect("quality fixture index");
    assert!(
        stats.files_indexed >= RETRIEVAL_CASES.len(),
        "polyglot fixture was not fully indexed: {stats:?}"
    );

    let mut ranking_results = Vec::new();
    for (query, expected_file) in RETRIEVAL_CASES {
        let expected = exact_symbol(&state.sqlite, query, Some(expected_file));
        let response = state
            .retriever
            .search(query, 5, false, ContextMode::None)
            .await
            .unwrap_or_else(|error| panic!("retrieval failed for {query}: {error}"));
        let ranked = response
            .response
            .hits
            .iter()
            .map(|hit| hit.id.clone())
            .collect::<Vec<_>>();
        ranking_results.push((
            *query,
            ranking_metrics(&ranked, &HashMap::from([(expected.id, 3)]), 5),
        ));
    }
    let retrieval = mean_ranking_metrics(&ranking_results);
    println!("deterministic retrieval metrics: {retrieval:?}");
    assert!(
        retrieval.recall_at_k >= 0.95,
        "recall@5 gate failed: {retrieval:?}; cases={ranking_results:?}"
    );
    assert!(
        retrieval.reciprocal_rank >= 0.80,
        "MRR gate failed: {retrieval:?}; cases={ranking_results:?}"
    );
    assert!(
        retrieval.ndcg_at_k >= 0.85,
        "nDCG@5 gate failed: {retrieval:?}; cases={ranking_results:?}"
    );

    let mut predicted_edges = HashSet::new();
    let mut expected_edges = HashSet::new();
    for (caller_name, callee_name) in GRAPH_CASES {
        let caller = exact_symbol(&state.sqlite, caller_name, None);
        let callee = exact_symbol(&state.sqlite, callee_name, None);
        expected_edges.insert((caller.id.clone(), callee.id));
        predicted_edges.extend(
            state
                .sqlite
                .list_edges_from(&caller.id, 64)
                .expect("outgoing edges")
                .into_iter()
                .filter(|edge| matches!(edge.edge_type.as_str(), "call" | "async_call"))
                .map(|edge| (edge.from_symbol_id, edge.to_symbol_id)),
        );
    }
    let graph = set_metrics(&predicted_edges, &expected_edges);
    println!("deterministic graph metrics: {graph:?}");
    assert!(
        graph.precision >= 0.90 && graph.recall >= 0.90,
        "graph precision/recall gate failed: {graph:?}; predicted={predicted_edges:?}; expected={expected_edges:?}"
    );

    let impact = handle_find_affected_code(
        &state,
        FindAffectedCodeTool {
            symbol_name: "rust_quality_leaf".to_string(),
            file_path: Some("rust_quality.rs".to_string()),
            depth: Some(2),
            limit: Some(20),
            include_tests: Some(false),
            edge_types: Some(vec!["call".to_string(), "delegates_to".to_string()]),
            include_display: Some(false),
        },
    )
    .expect("impact response");
    let predicted_impact = affected_names(&impact);
    let expected_impact = HashSet::from(["rust_quality_anchor".to_string()]);
    let impact_metrics = set_metrics(&predicted_impact, &expected_impact);
    println!("deterministic impact metrics: {impact_metrics:?}");
    assert_eq!(
        impact_metrics.precision, 1.0,
        "impact precision gate failed: {impact_metrics:?}; response={impact}"
    );
    assert_eq!(
        impact_metrics.recall, 1.0,
        "impact recall gate failed: {impact_metrics:?}; response={impact}"
    );

    let canonical_coverage = canonical_definition_coverage(&state).await;
    println!("canonical-definition coverage: {canonical_coverage:.3}");
    assert_eq!(
        canonical_coverage, 1.0,
        "canonical-definition coverage gate failed: {canonical_coverage:.3}"
    );

    let public_impact = handle_find_affected_code(
        &state,
        FindAffectedCodeTool {
            symbol_name: "canonicalQualityTarget".to_string(),
            file_path: Some("canonical.ts".to_string()),
            depth: Some(3),
            limit: Some(30),
            include_tests: Some(false),
            edge_types: None,
            include_display: Some(false),
        },
    )
    .expect("public exposure impact response");
    let public_exposure_covered = public_impact["affected"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|entry| {
            entry["file_path"] == "barrel.ts" && entry["evidence_role"] == "public_exposure"
        });
    assert!(
        public_exposure_covered,
        "public-exposure coverage gate failed: {public_impact}"
    );

    let overload = handle_get_definition(
        &state,
        GetDefinitionTool {
            symbol_name: "qualityOverload".to_string(),
            file: Some("adversarial.ts".to_string()),
            limit: Some(10),
        },
    )
    .await
    .expect("overload definition response");
    assert_eq!(overload["resolution"], "exact");
    assert_eq!(overload["logical_count"], 1);
    assert_eq!(
        overload["definitions"]
            .as_array()
            .expect("overload definitions")
            .iter()
            .filter(|definition| definition["is_canonical"] == true)
            .count(),
        1,
        "overloads must have one canonical occurrence: {overload}"
    );

    let dynamic_dispatch = exact_symbol(&state.sqlite, "dynamic_quality_dispatch", None);
    let dynamic_leaf = exact_symbol(&state.sqlite, "dynamic_quality_leaf", None);
    assert!(
        state
            .sqlite
            .list_edges_from(&dynamic_dispatch.id, 64)
            .expect("dynamic edges")
            .iter()
            .all(|edge| edge.to_symbol_id != dynamic_leaf.id),
        "dynamic lookup must not fabricate a static call edge"
    );

    let negative = handle_get_definition(
        &state,
        GetDefinitionTool {
            symbol_name: "nonexistent_quality_phantom".to_string(),
            file: None,
            limit: Some(10),
        },
    )
    .await
    .expect("negative definition response");
    assert_eq!(negative["resolution"], "unresolved");
    assert_eq!(negative["count"], 0);
}

fn copy_polyglot_fixture(destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quality/polyglot");
    for entry in fs::read_dir(source).expect("quality fixture directory") {
        let entry = entry.expect("quality fixture entry");
        fs::copy(entry.path(), destination.join(entry.file_name())).expect("copy quality fixture");
    }
}

fn exact_symbol(
    sqlite: &code_intelligence_mcp_server::storage::sqlite::SqliteStore,
    name: &str,
    file_suffix: Option<&str>,
) -> SymbolRow {
    let mut rows = sqlite
        .search_symbols_by_exact_name(name, None, 20)
        .unwrap_or_else(|error| panic!("exact lookup failed for {name}: {error}"));
    if let Some(suffix) = file_suffix {
        rows.retain(|row| row.file_path.ends_with(suffix));
    }
    assert_eq!(
        rows.len(),
        1,
        "expected one exact symbol for {name}: {rows:?}"
    );
    rows.remove(0)
}

fn mean_ranking_metrics(cases: &[(&str, RankingMetrics)]) -> RankingMetrics {
    let count = cases.len() as f64;
    RankingMetrics {
        recall_at_k: cases
            .iter()
            .map(|(_, metrics)| metrics.recall_at_k)
            .sum::<f64>()
            / count,
        reciprocal_rank: cases
            .iter()
            .map(|(_, metrics)| metrics.reciprocal_rank)
            .sum::<f64>()
            / count,
        ndcg_at_k: cases
            .iter()
            .map(|(_, metrics)| metrics.ndcg_at_k)
            .sum::<f64>()
            / count,
    }
}

fn affected_names(response: &serde_json::Value) -> HashSet<String> {
    response["affected"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["symbol_name"].as_str().map(str::to_string))
        .collect()
}

async fn canonical_definition_coverage(
    state: &code_intelligence_mcp_server::handlers::AppState,
) -> f64 {
    let mut covered = 0usize;
    for (name, file) in RETRIEVAL_CASES {
        let response = handle_get_definition(
            state,
            GetDefinitionTool {
                symbol_name: (*name).to_string(),
                file: Some((*file).to_string()),
                limit: Some(20),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("definition failed for {name}: {error}"));
        let has_canonical = response["resolution"] == "exact"
            && response["definitions"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|definition| definition["is_canonical"] == true);
        covered += usize::from(has_canonical);
    }
    covered as f64 / RETRIEVAL_CASES.len() as f64
}

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use code_intelligence_mcp_server::config::{Config, EmbeddingsBackend, EmbeddingsDevice};
use code_intelligence_mcp_server::embeddings::{hash::HashEmbedder, SharedEmbedder};
use code_intelligence_mcp_server::external_index::artifact::read_normalized_artifact;
use code_intelligence_mcp_server::external_index::importer::import_external_index;
use code_intelligence_mcp_server::external_index::provider::{
    merged_references_to_internal_symbol, ReferenceSource,
};
use code_intelligence_mcp_server::handlers::{
    handle_find_affected_code, handle_get_call_hierarchy, handle_refresh_index, AppState,
};
use code_intelligence_mcp_server::indexer::pipeline::IndexPipeline;
use code_intelligence_mcp_server::metrics::MetricsRegistry;
use code_intelligence_mcp_server::path::{Utf8Path, Utf8PathBuf};
use code_intelligence_mcp_server::retrieval::Retriever;
use code_intelligence_mcp_server::storage::{
    sqlite::{EdgeRow, SqliteStore, SymbolRow},
    tantivy::TantivyIndex,
    vector::LanceDbStore,
};
use code_intelligence_mcp_server::tools::{
    CallHierarchyDirection, FindAffectedCodeTool, GetCallHierarchyTool, RefreshIndexTool,
};
use sha2::{Digest, Sha256};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn tier1_python_producer_artifact_imports_into_overlay() {
    let fixture_repo = tier1_fixture_repo("python");
    let artifact_file = run_tier1_producer("python", &fixture_repo);
    let artifact = read_normalized_artifact(artifact_file.path()).expect("read python artifact");

    assert_eq!(artifact.language, "python");
    assert!(artifact
        .symbols
        .iter()
        .any(|symbol| symbol.display_name == "render_user"));
    assert!(artifact
        .references
        .iter()
        .any(|reference| reference.relationship == "call"));
    assert!(
        artifact
            .references
            .iter()
            .all(|reference| reference.relationship != "calls"),
        "Tier 1 artifacts must use canonical singular relationship names"
    );
    assert_generated_artifact_imports_into_overlay(
        &fixture_repo,
        artifact_file.path(),
        artifact.symbols.len(),
        artifact.references.len(),
    );
}

#[test]
fn tier1_typescript_producer_artifact_imports_into_overlay() {
    let fixture_repo = tier1_fixture_repo("typescript");
    let artifact_file = run_tier1_producer("typescript", &fixture_repo);
    let artifact =
        read_normalized_artifact(artifact_file.path()).expect("read typescript artifact");

    assert_eq!(artifact.language, "typescript");
    assert!(artifact
        .symbols
        .iter()
        .any(|symbol| symbol.display_name == "renderUser"));
    assert!(artifact
        .references
        .iter()
        .any(|reference| reference.relationship == "call"));
    assert!(
        artifact
            .references
            .iter()
            .all(|reference| reference.relationship != "calls"),
        "Tier 1 artifacts must use canonical singular relationship names"
    );
    assert_generated_artifact_imports_into_overlay(
        &fixture_repo,
        artifact_file.path(),
        artifact.symbols.len(),
        artifact.references.len(),
    );
}

#[test]
fn external_overlay_reference_and_impact_matrix_covers_every_indexed_language() {
    let (_tmp, app_state) = test_app_state();
    let languages = [
        ("typescript", "ts"),
        ("javascript", "js"),
        ("rust", "rs"),
        ("python", "py"),
        ("go", "go"),
        ("java", "java"),
        ("kotlin", "kt"),
        ("csharp", "cs"),
        ("swift", "swift"),
        ("c", "c"),
        ("cpp", "cpp"),
        ("ruby", "rb"),
    ];

    for (language, extension) in languages {
        let target_id = format!("target_{language}_internal");
        let caller_id = format!("caller_{language}_internal");
        let target_name = format!("target_{language}");
        let caller_name = format!("caller_{language}");
        let target_file = format!("matrix/{language}/target.{extension}");
        let caller_file = format!("matrix/{language}/caller.{extension}");
        insert_symbol_for_language(
            &app_state.sqlite,
            &target_id,
            &target_file,
            &target_name,
            language,
            1,
            3,
        );
        insert_symbol_for_language(
            &app_state.sqlite,
            &caller_id,
            &caller_file,
            &caller_name,
            language,
            1,
            4,
        );

        let target_external = format!("matrix {language} target");
        let caller_external = format!("matrix {language} caller");
        let artifact = serde_json::json!({
            "source_kind": "normalized_json",
            "producer": format!("matrix-{language}"),
            "language": language,
            "root_path": "/fixture/repo",
            "symbols": [
                {
                    "external_symbol": target_external,
                    "display_name": target_name,
                    "kind": "function",
                    "file_path": target_file,
                    "start_line": 1,
                    "end_line": 3,
                    "start_byte": 0,
                    "end_byte": 42
                },
                {
                    "external_symbol": caller_external,
                    "display_name": caller_name,
                    "kind": "function",
                    "file_path": caller_file,
                    "start_line": 1,
                    "end_line": 4,
                    "start_byte": 0,
                    "end_byte": 60
                }
            ],
            "references": [
                {
                    "from_external_symbol": caller_external,
                    "to_external_symbol": target_external,
                    "relationship": "call",
                    "file_path": caller_file,
                    "line": 2,
                    "confidence": 1.0,
                    "provenance": "language-matrix"
                }
            ]
        });
        let artifact_file = write_artifact(&artifact.to_string());
        let report =
            import_external_index(&app_state.sqlite, "/fixture/repo", artifact_file.path())
                .unwrap_or_else(|err| panic!("{language} overlay import failed: {err}"));
        assert_eq!(report.symbols_mapped, 2, "{language} symbol mapping");

        let references =
            merged_references_to_internal_symbol(&app_state.sqlite, &target_id, Some("call"), 20)
                .unwrap_or_else(|err| panic!("{language} reference query failed: {err}"));
        assert_eq!(references.len(), 1, "{language} reference precision/recall");
        assert_eq!(
            references[0].source,
            ReferenceSource::External,
            "{language}"
        );
        assert_eq!(
            references[0].from_symbol_id.as_deref(),
            Some(caller_id.as_str()),
            "{language} mapped caller"
        );

        let response = handle_find_affected_code(
            &app_state,
            FindAffectedCodeTool {
                symbol_name: target_name,
                file_path: Some(target_file),
                depth: Some(1),
                limit: Some(20),
                include_tests: Some(true),
                edge_types: None,
                include_display: Some(false),
            },
        )
        .unwrap_or_else(|err| panic!("{language} impact query failed: {err}"));
        let affected = response["affected"]
            .as_array()
            .unwrap_or_else(|| panic!("{language} affected array"));
        assert_eq!(affected.len(), 1, "{language} impact precision/recall");
        assert_eq!(affected[0]["symbol_id"], caller_id, "{language}");
        assert_eq!(affected[0]["source"], "external", "{language}");
        assert_eq!(affected[0]["provenance"], "language-matrix", "{language}");
    }
}

#[test]
fn imports_normalized_artifact_and_maps_exact_range() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");

    store
        .upsert_symbol(&SymbolRow {
            id: "target_internal".to_string(),
            file_path: "src/app.ts".to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: "target".to_string(),
            exported: false,
            start_byte: 0,
            end_byte: 42,
            start_line: 1,
            end_line: 3,
            text: "function target() {}".to_string(),
        })
        .expect("insert target symbol");
    store
        .upsert_symbol(&SymbolRow {
            id: "caller_internal".to_string(),
            file_path: "src/caller.ts".to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: "caller".to_string(),
            exported: false,
            start_byte: 0,
            end_byte: 60,
            start_line: 1,
            end_line: 4,
            text: "function caller() { target(); }".to_string(),
        })
        .expect("insert caller symbol");

    let report = import_external_index(
        &store,
        "/fixture/repo",
        Path::new("tests/fixtures/external_index/typescript-normalized.json"),
    )
    .expect("import external index");

    assert_eq!(report.symbols_imported, 2);
    assert_eq!(report.references_imported, 1);
    assert_eq!(report.symbols_mapped, 2);

    let references = store
        .list_external_references_to_internal_symbol("target_internal", Some("call"), 20)
        .expect("list references");
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].file_path, "src/caller.ts");
}

#[test]
fn get_call_hierarchy_callers_includes_external_overlay() {
    let (_tmp, app_state) = test_app_state();
    insert_symbol(
        &app_state.sqlite,
        "target_internal",
        "src/app.ts",
        "target",
        1,
        3,
    );
    insert_symbol(
        &app_state.sqlite,
        "caller_internal",
        "src/caller.ts",
        "caller",
        1,
        4,
    );
    let report = import_external_index(
        &app_state.sqlite,
        "/fixture/repo",
        Path::new("tests/fixtures/external_index/typescript-normalized.json"),
    )
    .expect("import external index");

    let response = handle_get_call_hierarchy(
        &app_state,
        GetCallHierarchyTool {
            symbol_name: "target".to_string(),
            direction: Some(CallHierarchyDirection::Callers),
            depth: Some(1),
            limit: Some(20),
            file: Some("src/app.ts".to_string()),
        },
    )
    .expect("call hierarchy");

    let edges = response["edges"].as_array().expect("edges array");
    let edge = edges
        .iter()
        .find(|edge| edge["from"] == "caller_internal" && edge["to"] == "target_internal")
        .expect("external caller edge");
    assert_eq!(edge["edge_type"], "call");
    assert_eq!(edge["at_file"], "src/caller.ts");
    assert_eq!(edge["at_line"], 2);
    assert_eq!(edge["source"], "external");
    assert_eq!(edge["external_index_id"], report.index_id);
    assert_eq!(edge["provenance"], "fixture");
    assert_eq!(edge["confidence"], 1.0);

    let nodes = response["nodes"].as_array().expect("nodes array");
    assert!(
        nodes
            .iter()
            .any(|node| node["id"] == "caller_internal" && node["name"] == "caller"),
        "mapped external caller should be added as a graph node"
    );
}

#[test]
fn find_affected_code_includes_external_overlay_when_native_edge_absent() {
    let (_tmp, app_state) = test_app_state();
    insert_symbol(
        &app_state.sqlite,
        "target_internal",
        "src/app.ts",
        "target",
        1,
        3,
    );
    insert_symbol(
        &app_state.sqlite,
        "caller_internal",
        "src/caller.ts",
        "caller",
        1,
        4,
    );
    let report = import_external_index(
        &app_state.sqlite,
        "/fixture/repo",
        Path::new("tests/fixtures/external_index/typescript-normalized.json"),
    )
    .expect("import external index");

    let response = handle_find_affected_code(
        &app_state,
        FindAffectedCodeTool {
            symbol_name: "target".to_string(),
            file_path: Some("src/app.ts".to_string()),
            depth: Some(1),
            limit: Some(20),
            include_tests: Some(true),
            edge_types: None,
            include_display: Some(false),
        },
    )
    .expect("affected code");

    let affected = response["affected"].as_array().expect("affected array");
    let entry = affected
        .iter()
        .find(|entry| entry["symbol_id"] == "caller_internal")
        .expect("external caller affected entry");
    assert_eq!(entry["symbol_name"], "caller");
    assert_eq!(entry["file_path"], "src/caller.ts");
    assert_eq!(entry["source"], "external");
    assert_eq!(entry["external_index_id"], report.index_id);
    assert_eq!(entry["provenance"], "fixture");
    assert_eq!(entry["confidence"], 1.0);
    assert_eq!(entry["reference_type"], "call");
}

#[test]
fn find_affected_code_labels_transparent_wrapper_with_exact_delegation() {
    let (_tmp, app_state) = test_app_state();
    insert_symbol(
        &app_state.sqlite,
        "target_internal",
        "src/app.ts",
        "target",
        1,
        3,
    );
    insert_symbol(
        &app_state.sqlite,
        "wrapper_internal",
        "src/wrapper.ts",
        "publicWrapper",
        5,
        10,
    );
    insert_edge(
        &app_state.sqlite,
        "wrapper_internal",
        "target_internal",
        "delegates_to",
        "src/wrapper.ts",
        8,
        0.95,
    );

    let response = handle_find_affected_code(
        &app_state,
        FindAffectedCodeTool {
            symbol_name: "target".to_string(),
            file_path: Some("src/app.ts".to_string()),
            depth: Some(2),
            limit: Some(20),
            include_tests: Some(true),
            edge_types: None,
            include_display: Some(false),
        },
    )
    .expect("affected code");

    let wrapper = response["affected"]
        .as_array()
        .expect("affected array")
        .iter()
        .find(|entry| entry["symbol_id"] == "wrapper_internal")
        .expect("wrapper affected entry");
    assert_eq!(wrapper["evidence_role"], "wrapper");
    assert_eq!(wrapper["at_file"], "src/wrapper.ts");
    assert_eq!(wrapper["at_line"], 8);
    assert_eq!(wrapper["delegation"][0]["edge_type"], "delegates_to");
    let confidence = wrapper["delegation"][0]["confidence"]
        .as_f64()
        .expect("numeric confidence");
    assert!((confidence - 0.95).abs() < 1e-6);
}

#[test]
fn refresh_index_runs_external_producer_when_enabled_for_explicit_refresh() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let (_tmp, app_state) =
        test_app_state_with_external_index(true, Some("rust".to_string()), "explicit");
    let producer = app_state.config.base_dir.join("fake-rust-producer.sh");
    std::fs::write(
        producer.as_std_path(),
        r#"#!/bin/sh
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then
    shift
    out="$1"
  fi
  shift
done
cat > "$out" <<'JSON'
{
  "source_kind": "normalized_json",
  "producer": "fake-rust",
  "language": "rust",
  "root_path": "/fixture/repo",
  "symbols": [],
  "references": []
}
JSON
"#,
    )
    .expect("write fake producer");
    make_executable(producer.as_std_path());
    let _env = EnvVarGuard::set("EXTERNAL_INDEX_RUST_COMMAND", producer.as_str());

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let response = rt
        .block_on(handle_refresh_index(
            &app_state,
            RefreshIndexTool { files: None },
        ))
        .expect("refresh index");

    assert_eq!(response["ok"], true);
    assert_eq!(response["external_index"]["ok"], true);
    assert_eq!(response["external_index"]["producer"], "rust");
    let stats = app_state
        .sqlite
        .external_overlay_stats()
        .expect("external stats");
    assert_eq!(stats.index_count, 1);
}

#[test]
fn refresh_index_reports_missing_external_toolchain_without_failing_indexing() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let (_tmp, app_state) =
        test_app_state_with_external_index(true, Some("rust".to_string()), "explicit");
    let _env = EnvVarGuard::set(
        "EXTERNAL_INDEX_RUST_COMMAND",
        "__missing_refresh_external_index_toolchain__",
    );

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let response = rt
        .block_on(handle_refresh_index(
            &app_state,
            RefreshIndexTool { files: None },
        ))
        .expect("refresh index should not fail when producer is missing");

    assert_eq!(response["ok"], true);
    assert_eq!(response["external_index"]["ok"], false);
    assert_eq!(response["external_index"]["status"], "missing_bundle");
    assert_eq!(response["external_index"]["producer"], "rust");
    let stats = app_state
        .sqlite
        .external_overlay_stats()
        .expect("external stats");
    assert_eq!(stats.index_count, 0);
}

#[test]
fn merged_references_include_mapped_external_reference_provenance() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);

    let report = import_external_index(
        &store,
        "/fixture/repo",
        Path::new("tests/fixtures/external_index/typescript-normalized.json"),
    )
    .expect("import external index");

    let references =
        merged_references_to_internal_symbol(&store, "target_internal", Some("call"), 20)
            .expect("merged references");

    assert_eq!(references.len(), 1);
    let reference = &references[0];
    assert_eq!(reference.source, ReferenceSource::External);
    assert_eq!(reference.to_symbol_id, "target_internal");
    assert_eq!(reference.reference_type, "call");
    assert_eq!(reference.at_file.as_deref(), Some("src/caller.ts"));
    assert_eq!(reference.at_line, Some(2));
    assert_eq!(reference.confidence, 1.0);
    assert_eq!(
        reference.external_index_id.as_deref(),
        Some(report.index_id.as_str())
    );
    assert_eq!(reference.provenance.as_deref(), Some("fixture"));
    assert_eq!(reference.metadata_json.as_deref(), Some("{}"));
}

#[test]
fn merged_references_keep_native_fallback_when_external_refs_absent() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);
    insert_edge(
        &store,
        "caller_internal",
        "target_internal",
        "call",
        "src/caller.ts",
        2,
        0.75,
    );

    let references =
        merged_references_to_internal_symbol(&store, "target_internal", Some("call"), 20)
            .expect("merged references");

    assert_eq!(references.len(), 1);
    let reference = &references[0];
    assert_eq!(reference.source, ReferenceSource::Native);
    assert_eq!(reference.to_symbol_id, "target_internal");
    assert_eq!(reference.from_symbol_id.as_deref(), Some("caller_internal"));
    assert_eq!(reference.from_symbol_name.as_deref(), Some("caller"));
    assert_eq!(reference.from_symbol_file.as_deref(), Some("src/caller.ts"));
    assert_eq!(reference.reference_type, "call");
    assert_eq!(reference.at_file.as_deref(), Some("src/caller.ts"));
    assert_eq!(reference.at_line, Some(2));
    assert_eq!(reference.confidence, 0.75);
    assert!(reference.external_index_id.is_none());
    assert!(reference.provenance.is_none());
    assert!(reference.metadata_json.is_none());
}

#[test]
fn merged_references_dedupe_prefers_external_on_same_location_and_type() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);
    insert_edge(
        &store,
        "caller_internal",
        "target_internal",
        "call",
        "src/caller.ts",
        2,
        1.0,
    );
    import_external_index(
        &store,
        "/fixture/repo",
        Path::new("tests/fixtures/external_index/typescript-normalized.json"),
    )
    .expect("import external index");

    let references =
        merged_references_to_internal_symbol(&store, "target_internal", Some("call"), 20)
            .expect("merged references");

    assert_eq!(references.len(), 1);
    assert_eq!(references[0].source, ReferenceSource::External);
    assert_eq!(references[0].at_file.as_deref(), Some("src/caller.ts"));
    assert_eq!(references[0].at_line, Some(2));
    assert_eq!(references[0].reference_type, "call");
}

#[test]
fn merged_references_keep_higher_confidence_native_overlay_equivalent() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);
    insert_edge(
        &store,
        "caller_internal",
        "target_internal",
        "call",
        "src/caller.ts",
        2,
        0.95,
    );

    let artifact = r#"{
      "source_kind": "normalized_json",
      "producer": "manual-fixture",
      "language": "typescript",
      "root_path": "/fixture/repo",
      "symbols": [
        {
          "external_symbol": "local src/app.ts target().",
          "display_name": "target",
          "kind": "function",
          "file_path": "src/app.ts",
          "start_line": 1,
          "end_line": 3,
          "start_byte": 0,
          "end_byte": 42
        },
        {
          "external_symbol": "local src/caller.ts caller().",
          "display_name": "caller",
          "kind": "function",
          "file_path": "src/caller.ts",
          "start_line": 1,
          "end_line": 4,
          "start_byte": 0,
          "end_byte": 60
        }
      ],
      "references": [
        {
          "from_external_symbol": "local src/caller.ts caller().",
          "to_external_symbol": "local src/app.ts target().",
          "relationship": "call",
          "file_path": "src/caller.ts",
          "line": 2,
          "confidence": 0.5,
          "provenance": "fixture"
        }
      ]
    }"#;
    let artifact_path = write_artifact(artifact);
    import_external_index(&store, "/fixture/repo", artifact_path.path()).expect("import");

    let references =
        merged_references_to_internal_symbol(&store, "target_internal", Some("call"), 20)
            .expect("merged references");

    assert_eq!(references.len(), 1);
    assert_eq!(references[0].source, ReferenceSource::Native);
    assert_eq!(references[0].confidence, 0.95);
    assert_eq!(
        references[0].from_symbol_id.as_deref(),
        Some("caller_internal")
    );
    assert_eq!(references[0].at_file.as_deref(), Some("src/caller.ts"));
    assert_eq!(references[0].at_line, Some(2));
}

#[test]
fn merged_references_keep_distinct_external_refs_on_same_line() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);
    insert_symbol(
        &store,
        "other_caller_internal",
        "src/other.ts",
        "other",
        1,
        4,
    );

    let artifact = r#"{
      "source_kind": "normalized_json",
      "producer": "manual-fixture",
      "language": "typescript",
      "root_path": "/fixture/repo",
      "symbols": [
        {
          "external_symbol": "local src/app.ts target().",
          "display_name": "target",
          "kind": "function",
          "file_path": "src/app.ts",
          "start_line": 1,
          "end_line": 3,
          "start_byte": 0,
          "end_byte": 42
        },
        {
          "external_symbol": "local src/caller.ts caller().",
          "display_name": "caller",
          "kind": "function",
          "file_path": "src/caller.ts",
          "start_line": 1,
          "end_line": 4,
          "start_byte": 0,
          "end_byte": 60
        },
        {
          "external_symbol": "local src/other.ts other().",
          "display_name": "other",
          "kind": "function",
          "file_path": "src/other.ts",
          "start_line": 1,
          "end_line": 4,
          "start_byte": 0,
          "end_byte": 60
        }
      ],
      "references": [
        {
          "from_external_symbol": "local src/caller.ts caller().",
          "to_external_symbol": "local src/app.ts target().",
          "relationship": "call",
          "file_path": "src/caller.ts",
          "line": 2,
          "column": 10,
          "end_line": 2,
          "end_column": 16,
          "confidence": 1.0,
          "provenance": "fixture"
        },
        {
          "from_external_symbol": "local src/caller.ts caller().",
          "to_external_symbol": "local src/app.ts target().",
          "relationship": "call",
          "file_path": "src/caller.ts",
          "line": 2,
          "column": 20,
          "end_line": 2,
          "end_column": 26,
          "confidence": 0.99,
          "provenance": "fixture"
        },
        {
          "from_external_symbol": "local src/other.ts other().",
          "to_external_symbol": "local src/app.ts target().",
          "relationship": "call",
          "file_path": "src/caller.ts",
          "line": 2,
          "column": 10,
          "end_line": 2,
          "end_column": 16,
          "confidence": 0.98,
          "provenance": "fixture"
        }
      ]
    }"#;
    let artifact_path = write_artifact(artifact);
    import_external_index(&store, "/fixture/repo", artifact_path.path()).expect("import");

    let references =
        merged_references_to_internal_symbol(&store, "target_internal", Some("call"), 20)
            .expect("merged references");

    assert_eq!(references.len(), 3);
    assert!(references
        .iter()
        .all(|reference| reference.source == ReferenceSource::External));
    assert!(references
        .iter()
        .all(|reference| reference.from_external_symbol_id.is_some()));
    assert!(references
        .iter()
        .all(|reference| reference.from_symbol_id.is_some()));

    let spans = references
        .iter()
        .map(|reference| {
            (
                reference.from_external_symbol_id.as_deref(),
                reference.at_column,
                reference.at_end_line,
                reference.at_end_column,
            )
        })
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(spans.len(), 3);
    assert!(
        spans
            .iter()
            .filter(|(_, column, end_line, end_column)| {
                *column == Some(10) && *end_line == Some(2) && *end_column == Some(16)
            })
            .count()
            == 2
    );
    assert!(spans.iter().any(|(_, column, end_line, end_column)| {
        *column == Some(20) && *end_line == Some(2) && *end_column == Some(26)
    }));
}

#[test]
fn merged_references_dedupe_mapped_external_refs_by_logical_span() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);

    let lower_confidence_artifact = r#"{
      "source_kind": "normalized_json",
      "producer": "manual-fixture-low",
      "language": "typescript",
      "root_path": "/fixture/repo",
      "symbols": [
        {
          "external_symbol": "low src/app.ts target().",
          "display_name": "target",
          "kind": "function",
          "file_path": "src/app.ts",
          "start_line": 1,
          "end_line": 3,
          "start_byte": 0,
          "end_byte": 42
        },
        {
          "external_symbol": "low src/caller.ts caller().",
          "display_name": "caller",
          "kind": "function",
          "file_path": "src/caller.ts",
          "start_line": 1,
          "end_line": 4,
          "start_byte": 0,
          "end_byte": 60
        }
      ],
      "references": [
        {
          "from_external_symbol": "low src/caller.ts caller().",
          "to_external_symbol": "low src/app.ts target().",
          "relationship": "call",
          "file_path": "src/caller.ts",
          "line": 2,
          "column": 10,
          "end_line": 2,
          "end_column": 16,
          "confidence": 0.5,
          "provenance": "lower"
        }
      ]
    }"#;
    let higher_confidence_artifact = r#"{
      "source_kind": "normalized_json",
      "producer": "manual-fixture-high",
      "language": "typescript",
      "root_path": "/fixture/repo",
      "symbols": [
        {
          "external_symbol": "high src/app.ts target().",
          "display_name": "target",
          "kind": "function",
          "file_path": "src/app.ts",
          "start_line": 1,
          "end_line": 3,
          "start_byte": 0,
          "end_byte": 42
        },
        {
          "external_symbol": "high src/caller.ts caller().",
          "display_name": "caller",
          "kind": "function",
          "file_path": "src/caller.ts",
          "start_line": 1,
          "end_line": 4,
          "start_byte": 0,
          "end_byte": 60
        }
      ],
      "references": [
        {
          "from_external_symbol": "high src/caller.ts caller().",
          "to_external_symbol": "high src/app.ts target().",
          "relationship": "call",
          "file_path": "src/caller.ts",
          "line": 2,
          "column": 10,
          "end_line": 2,
          "end_column": 16,
          "confidence": 0.9,
          "provenance": "higher"
        }
      ]
    }"#;
    let lower_path = write_artifact(lower_confidence_artifact);
    let higher_path = write_artifact(higher_confidence_artifact);
    import_external_index(&store, "/fixture/repo", lower_path.path()).expect("import lower");
    import_external_index(&store, "/fixture/repo", higher_path.path()).expect("import higher");

    let references =
        merged_references_to_internal_symbol(&store, "target_internal", Some("call"), 20)
            .expect("merged references");

    assert_eq!(references.len(), 1);
    let reference = &references[0];
    assert_eq!(reference.source, ReferenceSource::External);
    assert_eq!(reference.from_symbol_id.as_deref(), Some("caller_internal"));
    assert_eq!(reference.at_file.as_deref(), Some("src/caller.ts"));
    assert_eq!(reference.at_line, Some(2));
    assert_eq!(reference.at_column, Some(10));
    assert_eq!(reference.at_end_line, Some(2));
    assert_eq!(reference.at_end_column, Some(16));
    assert_eq!(reference.confidence, 0.9);
    assert_eq!(reference.provenance.as_deref(), Some("higher"));
}

#[test]
fn merged_references_keep_lower_confidence_distinct_external_span_over_native_same_line() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);
    insert_edge(
        &store,
        "caller_internal",
        "target_internal",
        "call",
        "src/caller.ts",
        2,
        0.9,
    );

    let artifact = r#"{
      "source_kind": "normalized_json",
      "producer": "manual-fixture",
      "language": "typescript",
      "root_path": "/fixture/repo",
      "symbols": [
        {
          "external_symbol": "local src/app.ts target().",
          "display_name": "target",
          "kind": "function",
          "file_path": "src/app.ts",
          "start_line": 1,
          "end_line": 3,
          "start_byte": 0,
          "end_byte": 42
        },
        {
          "external_symbol": "local src/caller.ts caller().",
          "display_name": "caller",
          "kind": "function",
          "file_path": "src/caller.ts",
          "start_line": 1,
          "end_line": 4,
          "start_byte": 0,
          "end_byte": 60
        }
      ],
      "references": [
        {
          "from_external_symbol": "local src/caller.ts caller().",
          "to_external_symbol": "local src/app.ts target().",
          "relationship": "call",
          "file_path": "src/caller.ts",
          "line": 2,
          "column": 10,
          "end_line": 2,
          "end_column": 16,
          "confidence": 1.0,
          "provenance": "fixture"
        },
        {
          "from_external_symbol": "local src/caller.ts caller().",
          "to_external_symbol": "local src/app.ts target().",
          "relationship": "call",
          "file_path": "src/caller.ts",
          "line": 2,
          "column": 20,
          "end_line": 2,
          "end_column": 26,
          "confidence": 0.8,
          "provenance": "fixture"
        }
      ]
    }"#;
    let artifact_path = write_artifact(artifact);
    import_external_index(&store, "/fixture/repo", artifact_path.path()).expect("import");

    let references =
        merged_references_to_internal_symbol(&store, "target_internal", Some("call"), 20)
            .expect("merged references");

    assert_eq!(references.len(), 2);
    assert!(references
        .iter()
        .all(|reference| reference.source == ReferenceSource::External));
    assert!(references.iter().any(|reference| {
        reference.at_column == Some(10)
            && reference.at_end_line == Some(2)
            && reference.at_end_column == Some(16)
            && reference.confidence == 1.0
    }));
    assert!(references.iter().any(|reference| {
        reference.at_column == Some(20)
            && reference.at_end_line == Some(2)
            && reference.at_end_column == Some(26)
            && reference.confidence == 0.8
    }));
}

#[test]
fn merged_references_filters_native_relationship_before_limit() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);

    for index in 0..60 {
        let from_symbol_id = format!("noise_{index:02}");
        insert_symbol(
            &store,
            &from_symbol_id,
            &format!("src/noise_{index:02}.ts"),
            &from_symbol_id,
            1,
            1,
        );
        insert_edge(
            &store,
            &from_symbol_id,
            "target_internal",
            "aaa_noise",
            &format!("src/noise_{index:02}.ts"),
            1,
            0.9,
        );
    }
    insert_edge(
        &store,
        "caller_internal",
        "target_internal",
        "call",
        "src/caller.ts",
        2,
        0.8,
    );

    let references =
        merged_references_to_internal_symbol(&store, "target_internal", Some("call"), 1)
            .expect("merged references");

    assert_eq!(references.len(), 1);
    assert_eq!(references[0].source, ReferenceSource::Native);
    assert_eq!(references[0].reference_type, "call");
    assert_eq!(
        references[0].from_symbol_id.as_deref(),
        Some("caller_internal")
    );
}

#[test]
fn merged_references_filter_relationship_for_native_and_external_refs() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);
    insert_edge(
        &store,
        "caller_internal",
        "target_internal",
        "import",
        "src/caller.ts",
        1,
        0.8,
    );
    import_external_index(
        &store,
        "/fixture/repo",
        Path::new("tests/fixtures/external_index/typescript-normalized.json"),
    )
    .expect("import external index");

    let call_refs =
        merged_references_to_internal_symbol(&store, "target_internal", Some("call"), 20)
            .expect("call references");
    assert_eq!(call_refs.len(), 1);
    assert_eq!(call_refs[0].source, ReferenceSource::External);
    assert_eq!(call_refs[0].reference_type, "call");

    let import_refs =
        merged_references_to_internal_symbol(&store, "target_internal", Some("import"), 20)
            .expect("import references");
    assert_eq!(import_refs.len(), 1);
    assert_eq!(import_refs[0].source, ReferenceSource::Native);
    assert_eq!(import_refs[0].reference_type, "import");

    let all_refs = merged_references_to_internal_symbol(&store, "target_internal", Some("all"), 20)
        .expect("all references");
    assert_eq!(all_refs.len(), 2);
    assert!(all_refs.iter().any(|reference| {
        reference.source == ReferenceSource::External && reference.reference_type == "call"
    }));
    assert!(all_refs.iter().any(|reference| {
        reference.source == ReferenceSource::Native && reference.reference_type == "import"
    }));
}

#[test]
fn rejects_invalid_reference_path_without_partial_external_index() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);

    let artifact = r#"{
      "source_kind": "normalized_json",
      "producer": "manual-fixture",
      "language": "typescript",
      "root_path": "/fixture/repo",
      "symbols": [
        {
          "external_symbol": "local src/caller.ts caller().",
          "display_name": "caller",
          "kind": "function",
          "file_path": "src/caller.ts",
          "start_line": 1,
          "end_line": 4,
          "start_byte": 0,
          "end_byte": 60
        }
      ],
      "references": [
        {
          "from_external_symbol": "local src/caller.ts caller().",
          "to_external_symbol": "local src/app.ts target().",
          "relationship": "call",
          "file_path": "../escape.ts",
          "line": 2
        }
      ]
    }"#;
    let artifact_path = write_artifact(artifact);
    let index_id = external_index_id_for_json(artifact);

    let result = import_external_index(&store, "/fixture/repo", artifact_path.path());

    assert!(result.is_err());
    let stats = store
        .external_index_stats(&index_id)
        .expect("external stats after rollback");
    assert_eq!(stats.symbol_count, 0);
    assert_eq!(stats.reference_count, 0);
    assert_eq!(stats.mapping_count, 0);
}

#[test]
fn creates_placeholder_symbols_for_missing_reference_endpoints() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);

    let artifact = r#"{
      "source_kind": "normalized_json",
      "producer": "manual-fixture",
      "language": "typescript",
      "root_path": "/fixture/repo",
      "symbols": [
        {
          "external_symbol": "local src/caller.ts caller().",
          "display_name": "caller",
          "kind": "function",
          "file_path": "src/caller.ts",
          "start_line": 1,
          "end_line": 4,
          "start_byte": 0,
          "end_byte": 60
        }
      ],
      "references": [
        {
          "from_external_symbol": "local src/caller.ts caller().",
          "to_external_symbol": "local src/missing.ts missing().",
          "relationship": "call",
          "file_path": "src/caller.ts",
          "line": 2
        }
      ]
    }"#;
    let artifact_path = write_artifact(artifact);

    let report =
        import_external_index(&store, "/fixture/repo", artifact_path.path()).expect("import");

    let symbols = store
        .list_external_symbols_for_index(&report.index_id, 20)
        .expect("external symbols");
    let placeholder = symbols
        .iter()
        .find(|symbol| symbol.external_symbol == "local src/missing.ts missing().")
        .expect("placeholder symbol");
    assert_eq!(placeholder.kind, "unknown");
    assert_eq!(placeholder.metadata_json, r#"{"placeholder":true}"#);

    let conn = store.read().expect("read sqlite");
    let to_external_symbol_id: Option<String> = conn
        .query_row(
            "SELECT to_external_symbol_id FROM external_references WHERE external_index_id = ?1",
            [&report.index_id],
            |row| row.get(0),
        )
        .expect("reference target");
    assert_eq!(
        to_external_symbol_id.as_deref(),
        Some(placeholder.id.as_str())
    );
}

#[test]
fn leaves_ambiguous_same_file_name_fallback_unmapped() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "first_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "second_internal", "src/app.ts", "target", 10, 12);

    let artifact = r#"{
      "source_kind": "normalized_json",
      "producer": "manual-fixture",
      "language": "typescript",
      "root_path": "/fixture/repo",
      "symbols": [
        {
          "external_symbol": "local src/app.ts target().",
          "display_name": "target",
          "kind": "function",
          "file_path": "src/app.ts"
        }
      ],
      "references": []
    }"#;
    let artifact_path = write_artifact(artifact);

    let report =
        import_external_index(&store, "/fixture/repo", artifact_path.path()).expect("import");

    assert_eq!(report.symbols_imported, 1);
    assert_eq!(report.symbols_mapped, 0);
    assert_eq!(report.symbols_unmapped, 1);
    assert!(!store
        .has_external_mapping_for_internal_symbol("first_internal")
        .expect("first mapping check"));
    assert!(!store
        .has_external_mapping_for_internal_symbol("second_internal")
        .expect("second mapping check"));
}

#[test]
fn reimporting_same_artifact_is_idempotent() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);

    let artifact_path = Path::new("tests/fixtures/external_index/typescript-normalized.json");
    let first =
        import_external_index(&store, "/fixture/repo", artifact_path).expect("first import");
    let first_stats = store
        .external_index_stats(&first.index_id)
        .expect("first stats");
    let second =
        import_external_index(&store, "/fixture/repo", artifact_path).expect("second import");
    let second_stats = store
        .external_index_stats(&second.index_id)
        .expect("second stats");

    assert_eq!(first.index_id, second.index_id);
    assert_eq!(first.symbols_imported, second.symbols_imported);
    assert_eq!(first.references_imported, second.references_imported);
    assert_eq!(first.symbols_mapped, second.symbols_mapped);
    assert_eq!(first.symbols_unmapped, second.symbols_unmapped);
    assert_eq!(first_stats, second_stats);
    assert_eq!(second_stats.symbol_count, 2);
    assert_eq!(second_stats.reference_count, 1);
    assert_eq!(second_stats.mapping_count, 2);
}

#[test]
fn reimport_removes_stale_mapping_when_symbol_becomes_unmapped() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "target_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "caller_internal", "src/caller.ts", "caller", 1, 4);

    let artifact_path = Path::new("tests/fixtures/external_index/typescript-normalized.json");
    let first =
        import_external_index(&store, "/fixture/repo", artifact_path).expect("first import");
    assert_eq!(first.symbols_mapped, 2);
    assert_eq!(
        mapping_count_for_external_symbol(&store, &first.index_id, "local src/app.ts target()."),
        1
    );

    // Production reindexing replaces a file atomically: delete its old rows,
    // then insert declarations under the new identity contract. Mutating a
    // function into a class in-place with the same id is now deliberately
    // rejected as a collision.
    store
        .delete_symbols_by_file("src/app.ts")
        .expect("delete replaced source symbols");
    insert_symbol_with_kind(
        &store,
        "target_internal",
        "src/app.ts",
        "target",
        "class",
        20,
        22,
    );

    let second =
        import_external_index(&store, "/fixture/repo", artifact_path).expect("second import");

    assert_eq!(second.symbols_mapped, 1);
    assert_eq!(second.symbols_unmapped, 1);
    assert_eq!(
        mapping_count_for_external_symbol(&store, &second.index_id, "local src/app.ts target()."),
        0
    );
    let stats = store
        .external_index_stats(&second.index_id)
        .expect("external stats");
    assert_eq!(stats.mapping_count, 1);
}

#[test]
fn leaves_duplicate_exact_range_candidates_unmapped() {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");
    insert_symbol(&store, "first_internal", "src/app.ts", "target", 1, 3);
    insert_symbol(&store, "second_internal", "src/app.ts", "target", 1, 3);

    let artifact = r#"{
      "source_kind": "normalized_json",
      "producer": "manual-fixture",
      "language": "typescript",
      "root_path": "/fixture/repo",
      "symbols": [
        {
          "external_symbol": "local src/app.ts target().",
          "display_name": "target",
          "kind": "function",
          "file_path": "src/app.ts",
          "start_line": 1,
          "end_line": 3,
          "start_byte": 0,
          "end_byte": 42
        }
      ],
      "references": []
    }"#;
    let artifact_path = write_artifact(artifact);

    let report =
        import_external_index(&store, "/fixture/repo", artifact_path.path()).expect("import");

    assert_eq!(report.symbols_mapped, 0);
    assert_eq!(report.symbols_unmapped, 1);
    assert_eq!(
        mapping_count_for_external_symbol(&store, &report.index_id, "local src/app.ts target()."),
        0
    );
}

fn insert_symbol(
    store: &SqliteStore,
    id: &str,
    file_path: &str,
    name: &str,
    start_line: u32,
    end_line: u32,
) {
    insert_symbol_with_kind(store, id, file_path, name, "function", start_line, end_line);
}

fn insert_symbol_with_kind(
    store: &SqliteStore,
    id: &str,
    file_path: &str,
    name: &str,
    kind: &str,
    start_line: u32,
    end_line: u32,
) {
    insert_symbol_with_kind_for_language(
        store,
        id,
        file_path,
        name,
        kind,
        "typescript",
        start_line,
        end_line,
    );
}

fn insert_symbol_for_language(
    store: &SqliteStore,
    id: &str,
    file_path: &str,
    name: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
) {
    insert_symbol_with_kind_for_language(
        store, id, file_path, name, "function", language, start_line, end_line,
    );
}

#[allow(clippy::too_many_arguments)]
fn insert_symbol_with_kind_for_language(
    store: &SqliteStore,
    id: &str,
    file_path: &str,
    name: &str,
    kind: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
) {
    store
        .upsert_symbol(&SymbolRow {
            id: id.to_string(),
            file_path: file_path.to_string(),
            language: language.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            exported: false,
            start_byte: 0,
            end_byte: 60,
            start_line,
            end_line,
            text: format!("function {name}() {{}}"),
        })
        .expect("insert symbol");
}

fn insert_edge(
    store: &SqliteStore,
    from_symbol_id: &str,
    to_symbol_id: &str,
    edge_type: &str,
    at_file: &str,
    at_line: u32,
    confidence: f32,
) {
    store
        .upsert_edge(&EdgeRow {
            from_symbol_id: from_symbol_id.to_string(),
            to_symbol_id: to_symbol_id.to_string(),
            edge_type: edge_type.to_string(),
            at_file: Some(at_file.to_string()),
            at_line: Some(at_line),
            confidence,
            evidence_count: 1,
            resolution: "resolved".to_string(),
        })
        .expect("insert edge");
}

fn mapping_count_for_external_symbol(
    store: &SqliteStore,
    external_index_id: &str,
    external_symbol: &str,
) -> i64 {
    let conn = store.read().expect("read sqlite");
    conn.query_row(
        r#"
SELECT COUNT(*)
FROM symbol_mappings sm
JOIN external_symbols es ON es.id = sm.external_symbol_id
WHERE es.external_index_id = ?1 AND es.external_symbol = ?2
"#,
        [external_index_id, external_symbol],
        |row| row.get(0),
    )
    .expect("mapping count")
}

fn write_artifact(contents: &str) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().expect("temp artifact");
    std::fs::write(file.path(), contents).expect("write temp artifact");
    file
}

fn external_index_id_for_json(contents: &str) -> String {
    let hash = hex::encode(Sha256::digest(contents.as_bytes()));
    format!("external:{}", &hash[..16])
}

fn tier1_fixture_repo(language: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("producers")
        .join("tests")
        .join("fixtures")
        .join(language)
}

fn run_tier1_producer(producer: &str, fixture_repo: &Utf8Path) -> tempfile::NamedTempFile {
    let artifact_file = tempfile::NamedTempFile::new().expect("temp producer artifact");
    let wrapper = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("producers")
        .join("bin")
        .join(format!("code-intelligence-external-{producer}"));
    let output = Command::new(wrapper.as_std_path())
        .arg("index")
        .arg("--output")
        .arg(artifact_file.path())
        .current_dir(fixture_repo.as_std_path())
        .output()
        .expect("run Tier 1 producer wrapper");

    assert!(
        output.status.success(),
        "producer {producer} failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    artifact_file
}

fn assert_generated_artifact_imports_into_overlay(
    fixture_repo: &Utf8Path,
    artifact_path: &Path,
    expected_symbols: usize,
    expected_references: usize,
) {
    let store = SqliteStore::open_in_memory().expect("in-memory sqlite");
    store.init().expect("init sqlite");

    let report = import_external_index(&store, fixture_repo.as_str(), artifact_path)
        .expect("import generated Tier 1 artifact");
    assert_eq!(report.symbols_imported, expected_symbols);
    assert_eq!(report.references_imported, expected_references);

    let stats = store
        .external_index_stats(&report.index_id)
        .expect("external stats for generated Tier 1 artifact");
    assert_eq!(stats.symbol_count, expected_symbols as u64);
    assert_eq!(stats.reference_count, expected_references as u64);
}

fn test_app_state() -> (tempfile::TempDir, AppState) {
    test_app_state_with_external_index(false, None, "disabled")
}

fn test_app_state_with_external_index(
    external_index_auto: bool,
    external_index_producer: Option<String>,
    external_index_on_refresh: &str,
) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let base_dir = tmp
        .path()
        .canonicalize()
        .unwrap_or_else(|_| tmp.path().to_path_buf());
    let base_dir_utf8 = Utf8PathBuf::from_path_buf(base_dir.clone())
        .unwrap_or_else(|_| Utf8PathBuf::from(base_dir.to_string_lossy().as_ref()));
    let config = Arc::new(Config {
        db_path: base_dir_utf8.join("code-intelligence.db"),
        vector_db_path: base_dir_utf8.join("vectors"),
        tantivy_index_path: base_dir_utf8.join("tantivy-index"),
        base_dir: base_dir_utf8.clone(),
        embeddings_backend: EmbeddingsBackend::Hash,
        embeddings_model_dir: None,
        embeddings_device: EmbeddingsDevice::Cpu,
        embedding_batch_size: 32,
        hash_embedding_dim: 32,
        vector_search_limit: 20,
        vector_guaranteed_results: 3,
        hybrid_alpha: 0.7,
        rank_vector_weight: 0.7,
        rank_keyword_weight: 0.3,
        rank_exported_boost: 0.1,
        rank_index_file_boost: 0.05,
        rank_test_penalty: 0.1,
        rank_popularity_weight: 0.05,
        rank_popularity_cap: 50,
        index_patterns: vec![
            "**/*.ts".to_string(),
            "**/*.tsx".to_string(),
            "**/*.rs".to_string(),
        ],
        exclude_patterns: vec![],
        watch_mode: false,
        watch_debounce_ms: 100,
        watch_min_index_interval_ms: 50,
        max_context_bytes: 200_000,
        index_node_modules: false,
        repo_roots: vec![base_dir_utf8],
        reranker_enabled: false,
        reranker_model_path: None,
        reranker_top_k: 20,
        reranker_cache_dir: None,
        learning_enabled: false,
        learning_selection_boost: 0.1,
        learning_file_affinity_boost: 0.05,
        max_context_tokens: 8192,
        token_encoding: "o200k_base".to_string(),
        parallel_workers: 4,
        embedding_cache_enabled: true,
        pagerank_damping: 0.85,
        pagerank_iterations: 20,
        synonym_expansion_enabled: true,
        acronym_expansion_enabled: true,
        rrf_enabled: true,
        rrf_k: 60.0,
        rrf_keyword_weight: 1.0,
        rrf_vector_weight: 1.0,
        rrf_graph_weight: 0.5,
        hyde_enabled: false,
        hyde_llm_backend: "openai".to_string(),
        hyde_api_key: None,
        hyde_max_tokens: 512,
        metrics_enabled: false,
        metrics_port: 9090,
        package_detection_enabled: false,
        external_index_auto,
        external_index_producer,
        external_index_on_refresh: external_index_on_refresh.to_string(),
        external_index_min_interval_ms: 60_000,
        llm_enabled: false,
        descriptions_enabled: false,
        store_query_text: false,
        llm_device: EmbeddingsDevice::Cpu,
        llm_model_dir: None,
        llm_max_tokens: 30,
        llm_batch_commit: 10,
        answer_llm_n_ctx: 16384,
        sampling_descriptions_enabled: true,
        leader_election_enabled: false,
        leader_heartbeat_interval_ms: 10_000,
        leader_ttl_seconds: 30,
        embedding_truncate_dim: None,
        embedding_dim_override: None,
    });

    let sqlite = Arc::new(SqliteStore::open(&config.db_path).expect("sqlite open"));
    sqlite.init().expect("sqlite init");
    let tantivy_index =
        Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).expect("tantivy"));
    let hash_embedder = Arc::new(SharedEmbedder::new(Box::new(HashEmbedder::new(
        config.hash_embedding_dim,
    ))));
    let metrics = Arc::new(MetricsRegistry::new().expect("metrics"));
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let vector_store = rt.block_on(async {
        let lancedb = LanceDbStore::connect(&config.vector_db_path)
            .await
            .expect("lancedb connect");
        Arc::new(
            lancedb
                .open_or_create_table("symbols", config.hash_embedding_dim)
                .await
                .expect("lancedb table"),
        )
    });
    let indexer = IndexPipeline::new(
        config.clone(),
        sqlite.clone(),
        tantivy_index.clone(),
        vector_store.clone(),
        hash_embedder.clone(),
        metrics.clone(),
    );
    let retriever = Retriever::new(
        config.clone(),
        sqlite.clone(),
        tantivy_index,
        vector_store,
        hash_embedder,
        None,
        None,
        metrics,
    );
    let state = AppState {
        config,
        indexer,
        retriever,
        sqlite,
        mcp_runtime: Arc::new(once_cell::sync::OnceCell::new()),
        answer_generator: Arc::new(once_cell::sync::OnceCell::new()),
        ask_code_cache: Arc::new(Default::default()),
    };
    (tmp, state)
}

fn make_executable(path: &Path) {
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

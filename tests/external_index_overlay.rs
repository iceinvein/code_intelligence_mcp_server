use std::path::Path;

use code_intelligence_mcp_server::external_index::importer::import_external_index;
use code_intelligence_mcp_server::external_index::provider::{
    merged_references_to_internal_symbol, ReferenceSource,
};
use code_intelligence_mcp_server::storage::sqlite::{EdgeRow, SqliteStore, SymbolRow};
use sha2::{Digest, Sha256};

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
    store
        .upsert_symbol(&SymbolRow {
            id: id.to_string(),
            file_path: file_path.to_string(),
            language: "typescript".to_string(),
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

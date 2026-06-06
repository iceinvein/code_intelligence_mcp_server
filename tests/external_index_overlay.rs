use std::path::Path;

use code_intelligence_mcp_server::external_index::importer::import_external_index;
use code_intelligence_mcp_server::storage::sqlite::{SqliteStore, SymbolRow};
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

fn insert_symbol(
    store: &SqliteStore,
    id: &str,
    file_path: &str,
    name: &str,
    start_line: u32,
    end_line: u32,
) {
    store
        .upsert_symbol(&SymbolRow {
            id: id.to_string(),
            file_path: file_path.to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
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

fn write_artifact(contents: &str) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().expect("temp artifact");
    std::fs::write(file.path(), contents).expect("write temp artifact");
    file
}

fn external_index_id_for_json(contents: &str) -> String {
    let hash = hex::encode(Sha256::digest(contents.as_bytes()));
    format!("external:{}", &hash[..16])
}

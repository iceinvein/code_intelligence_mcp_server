use std::path::Path;

use code_intelligence_mcp_server::external_index::importer::import_external_index;
use code_intelligence_mcp_server::storage::sqlite::{SqliteStore, SymbolRow};

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

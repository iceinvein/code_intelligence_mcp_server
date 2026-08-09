use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

use crate::indexer::extract::symbol::{ModuleBinding, ModuleBindingKind};
use crate::storage::sqlite::queries;
use crate::storage::sqlite::{EdgeEvidenceRow, EdgeRow, ModuleBindingRow, SymbolRow};

use super::parse::ParsedFile;
use super::utils::{module_source_is_local, resolve_indexed_module_file, ModuleFileResolution};

pub type BindingEdges = Vec<(EdgeRow, Vec<EdgeEvidenceRow>)>;

enum Lookup<T> {
    Exact(T),
    Ambiguous,
    Missing,
}

#[derive(Debug, Clone)]
struct CatalogFile {
    language: String,
    bindings: Vec<ModuleBinding>,
}

/// Immutable syntax catalog used while resolving bindings across files in the
/// same indexing batch. Resolution remains database-backed for symbol
/// identity, while this catalog supplies public names such as `default` and
/// chained aliases that do not have their own declaration rows.
#[derive(Debug, Clone, Default)]
pub struct BindingCatalog {
    files: HashMap<String, CatalogFile>,
}

impl BindingCatalog {
    pub fn from_parsed_files(parsed_files: &[ParsedFile]) -> Self {
        let files = parsed_files
            .iter()
            .map(|parsed| {
                (
                    parsed.rel_path.clone(),
                    CatalogFile {
                        language: parsed.language.clone(),
                        bindings: parsed.module_bindings.clone(),
                    },
                )
            })
            .collect();
        Self { files }
    }

    fn file(&self, file_path: &str) -> Option<&CatalogFile> {
        self.files.get(file_path)
    }
}

pub fn resolve_for_file(
    parsed: &ParsedFile,
    conn: &Connection,
    catalog: &BindingCatalog,
) -> Result<(Vec<ModuleBindingRow>, BindingEdges)> {
    let file_root = parsed
        .symbol_rows
        .iter()
        .find(|symbol| symbol.kind == "file")
        .context("Parsed file is missing its file-root symbol")?;

    let mut rows = Vec::with_capacity(parsed.module_bindings.len());
    let mut edges = Vec::new();
    for binding in &parsed.module_bindings {
        let row = resolve_binding(
            &parsed.rel_path,
            &parsed.language,
            &parsed.module_bindings,
            binding,
            conn,
            catalog,
            &mut HashSet::new(),
        )?;
        if let Some(target_symbol_id) = row.target_symbol_id.as_deref() {
            if target_symbol_id != file_root.id {
                edges.push((binding_edge(file_root, &row, target_symbol_id), Vec::new()));
            }
        }
        rows.push(row);
    }
    Ok((rows, edges))
}

fn resolve_binding(
    file_path: &str,
    language: &str,
    file_bindings: &[ModuleBinding],
    binding: &ModuleBinding,
    conn: &Connection,
    catalog: &BindingCatalog,
    visiting: &mut HashSet<String>,
) -> Result<ModuleBindingRow> {
    let mut row = ModuleBindingRow {
        id: 0,
        file_path: file_path.to_string(),
        binding_kind: binding.kind.as_str().to_string(),
        source_module: binding.source.clone(),
        source_file: None,
        imported_name: binding.imported_name.clone(),
        local_name: binding.local_name.clone(),
        exported_name: binding.exported_name.clone(),
        target_symbol_id: None,
        at_line: binding.at_line,
        resolution: "unresolved".to_string(),
        confidence: 0.0,
    };

    let key = binding_key(file_path, binding);
    if !visiting.insert(key.clone()) {
        row.resolution = "cyclic".to_string();
        return Ok(row);
    }

    if binding.kind == ModuleBindingKind::Export {
        match lookup_symbol(conn, file_path, &binding.local_name)? {
            Lookup::Missing => {
                // `import { A as B } from "./a"; export { B as C };` has no
                // declaration row for B in this file. Resolve the local export
                // through its unique import binding while keeping this row's
                // local/exported names as written at the public boundary.
                let imports = file_bindings
                    .iter()
                    .filter(|candidate| {
                        candidate.kind == ModuleBindingKind::Import
                            && candidate.local_name == binding.local_name
                    })
                    .collect::<Vec<_>>();
                match imports.as_slice() {
                    [import] => {
                        let resolved_import = resolve_binding(
                            file_path,
                            language,
                            file_bindings,
                            import,
                            conn,
                            catalog,
                            visiting,
                        )?;
                        apply_nested_resolution(&mut row, resolved_import);
                    }
                    [] => {}
                    _ => row.resolution = "ambiguous".to_string(),
                }
            }
            lookup => apply_symbol_lookup(&mut row, lookup),
        }
        visiting.remove(&key);
        return Ok(row);
    }

    let source_file = match resolve_indexed_module_file(conn, file_path, &binding.source, language)?
    {
        ModuleFileResolution::Exact(file) => file,
        ModuleFileResolution::Ambiguous => {
            row.resolution = "ambiguous".to_string();
            visiting.remove(&key);
            return Ok(row);
        }
        ModuleFileResolution::Missing => {
            row.resolution = if module_source_is_local(language, &binding.source) {
                "unresolved"
            } else {
                "external"
            }
            .to_string();
            visiting.remove(&key);
            return Ok(row);
        }
    };
    row.source_file = Some(source_file.clone());

    let targets_module =
        binding.kind == ModuleBindingKind::ExportAll || binding.imported_name == "*";
    if targets_module {
        apply_symbol_lookup(&mut row, lookup_file_root(conn, &source_file)?);
        visiting.remove(&key);
        return Ok(row);
    }

    match lookup_exported_symbol(conn, &source_file, &binding.imported_name)? {
        Lookup::Missing => {
            let public_bindings = catalog
                .file(&source_file)
                .map(|file| {
                    file.bindings
                        .iter()
                        .filter(|candidate| {
                            candidate.kind != ModuleBindingKind::Import
                                && candidate.exported_name == binding.imported_name
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            match public_bindings.as_slice() {
                [public_binding] => {
                    let source = catalog
                        .file(&source_file)
                        .expect("binding came from catalog file");
                    let resolved_public = resolve_binding(
                        &source_file,
                        &source.language,
                        &source.bindings,
                        public_binding,
                        conn,
                        catalog,
                        visiting,
                    )?;
                    apply_nested_resolution(&mut row, resolved_public);
                }
                [] => match lookup_persisted_public_target(
                    conn,
                    &source_file,
                    &binding.imported_name,
                )? {
                    Lookup::Exact((target, resolution, confidence)) => {
                        row.target_symbol_id = Some(target);
                        row.resolution = resolution;
                        row.confidence = confidence;
                    }
                    Lookup::Ambiguous => row.resolution = "ambiguous".to_string(),
                    Lookup::Missing if binding.kind == ModuleBindingKind::ReExport => {
                        // A named re-export can legitimately target an older
                        // barrel with no resolved public-name row. Keep
                        // traversal possible at reduced confidence without
                        // inventing a same-name declaration.
                        match lookup_file_root(conn, &source_file)? {
                            Lookup::Exact(file_root) => {
                                row.target_symbol_id = Some(file_root.id);
                                row.resolution = "inferred".to_string();
                                row.confidence = 0.75;
                            }
                            Lookup::Ambiguous => row.resolution = "ambiguous".to_string(),
                            Lookup::Missing => {}
                        }
                    }
                    Lookup::Missing => {}
                },
                _ => row.resolution = "ambiguous".to_string(),
            }
        }
        lookup => apply_symbol_lookup(&mut row, lookup),
    }

    visiting.remove(&key);
    Ok(row)
}

fn binding_key(file_path: &str, binding: &ModuleBinding) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        file_path,
        binding.kind.as_str(),
        binding.source,
        binding.imported_name,
        binding.local_name,
        binding.exported_name,
        binding.at_line
    )
}

fn apply_nested_resolution(row: &mut ModuleBindingRow, nested: ModuleBindingRow) {
    row.source_file = row.source_file.take().or(nested.source_file);
    row.target_symbol_id = nested.target_symbol_id;
    row.resolution = nested.resolution;
    row.confidence = nested.confidence;
}

fn lookup_symbol(conn: &Connection, file_path: &str, name: &str) -> Result<Lookup<SymbolRow>> {
    if name.is_empty() {
        return Ok(Lookup::Missing);
    }
    let matches = queries::symbols::search_symbols_by_exact_name(conn, name, Some(file_path), 64)?;
    collapse_logical_matches(conn, matches)
}

fn lookup_exported_symbol(
    conn: &Connection,
    file_path: &str,
    name: &str,
) -> Result<Lookup<SymbolRow>> {
    if name.is_empty() {
        return Ok(Lookup::Missing);
    }
    let matches = queries::symbols::search_symbols_by_exact_name(conn, name, Some(file_path), 64)?
        .into_iter()
        .filter(|symbol| symbol.exported)
        .collect::<Vec<_>>();
    collapse_logical_matches(conn, matches)
}

/// Multiple declaration rows can be occurrences of one logical symbol (for
/// example overload signatures). Treat that set as one exact binding target,
/// but keep genuinely distinct same-name declarations ambiguous.
fn collapse_logical_matches(
    conn: &Connection,
    matches: Vec<SymbolRow>,
) -> Result<Lookup<SymbolRow>> {
    if matches.is_empty() {
        return Ok(Lookup::Missing);
    }

    let ids = matches
        .iter()
        .map(|symbol| symbol.id.clone())
        .collect::<Vec<_>>();
    let identities = queries::symbol_identities::get_by_symbol_ids(conn, &ids)?;
    let mut by_logical = HashMap::<String, Vec<SymbolRow>>::new();
    for symbol in matches {
        let logical_id = identities
            .get(&symbol.id)
            .map(|identity| identity.logical_id.clone())
            .unwrap_or_else(|| symbol.id.clone());
        by_logical.entry(logical_id).or_default().push(symbol);
    }

    if by_logical.len() != 1 {
        return Ok(Lookup::Ambiguous);
    }
    let (_, mut occurrences) = by_logical.into_iter().next().expect("one logical symbol");
    let canonical = occurrences
        .iter()
        .position(|symbol| {
            identities
                .get(&symbol.id)
                .is_some_and(|identity| identity.is_canonical)
        })
        .unwrap_or(0);
    Ok(Lookup::Exact(occurrences.swap_remove(canonical)))
}

fn lookup_file_root(conn: &Connection, file_path: &str) -> Result<Lookup<SymbolRow>> {
    let matches = queries::symbols::list_symbols_by_file(conn, file_path)?
        .into_iter()
        .filter(|symbol| symbol.kind == "file")
        .collect::<Vec<_>>();
    Ok(match matches.len() {
        0 => Lookup::Missing,
        1 => Lookup::Exact(matches.into_iter().next().expect("one result")),
        _ => Lookup::Ambiguous,
    })
}

fn lookup_persisted_public_target(
    conn: &Connection,
    file_path: &str,
    exported_name: &str,
) -> Result<Lookup<(String, String, f32)>> {
    let matches = queries::module_bindings::list_by_file(conn, file_path)?
        .into_iter()
        .filter(|binding| {
            binding.binding_kind != "import"
                && binding.exported_name == exported_name
                && binding.target_symbol_id.is_some()
                && binding.confidence > 0.0
        })
        .collect::<Vec<_>>();
    let mut by_target = HashMap::<String, (String, f32)>::new();
    for binding in matches {
        let target = binding.target_symbol_id.expect("filtered target");
        by_target
            .entry(target)
            .and_modify(|existing| {
                if binding.confidence > existing.1 {
                    *existing = (binding.resolution.clone(), binding.confidence);
                }
            })
            .or_insert((binding.resolution, binding.confidence));
    }
    Ok(match by_target.len() {
        0 => Lookup::Missing,
        1 => {
            let (target, (resolution, confidence)) =
                by_target.into_iter().next().expect("one target");
            Lookup::Exact((target, resolution, confidence))
        }
        _ => Lookup::Ambiguous,
    })
}

fn apply_symbol_lookup(row: &mut ModuleBindingRow, lookup: Lookup<SymbolRow>) {
    match lookup {
        Lookup::Exact(symbol) => {
            row.target_symbol_id = Some(symbol.id);
            row.resolution = "exact".to_string();
            row.confidence = 1.0;
        }
        Lookup::Ambiguous => {
            row.resolution = "ambiguous".to_string();
            row.confidence = 0.0;
        }
        Lookup::Missing => {}
    }
}

fn binding_edge(file_root: &SymbolRow, row: &ModuleBindingRow, target_id: &str) -> EdgeRow {
    EdgeRow {
        from_symbol_id: file_root.id.clone(),
        to_symbol_id: target_id.to_string(),
        edge_type: row.binding_kind.clone(),
        at_file: Some(row.file_path.clone()),
        at_line: Some(row.at_line),
        confidence: row.confidence,
        evidence_count: 1,
        resolution: row.resolution.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::pipeline::utils::FileFingerprint;
    use crate::storage::sqlite::schema::{SymbolIdentityRow, SCHEMA_SQL};

    fn symbol(id: &str, file: &str, kind: &str, name: &str, exported: bool) -> SymbolRow {
        SymbolRow {
            id: id.into(),
            file_path: file.into(),
            language: "typescript".into(),
            kind: kind.into(),
            name: name.into(),
            exported,
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            end_line: 1,
            text: name.into(),
        }
    }

    fn parsed(binding: ModuleBinding) -> ParsedFile {
        ParsedFile {
            rel_path: "src/index.ts".into(),
            fingerprint: FileFingerprint {
                mtime_ns: 0,
                size_bytes: 0,
            },
            content_hash: "0".repeat(32),
            language: "typescript".into(),
            symbol_rows: vec![symbol(
                "barrel-root",
                "src/index.ts",
                "file",
                "src/index.ts",
                false,
            )],
            symbol_identities: vec![],
            edges: vec![],
            usage_examples: vec![],
            import_tags: String::new(),
            framework_tags: String::new(),
            todos: vec![],
            docstrings: vec![],
            decorators: vec![],
            framework_patterns: vec![],
            is_test_file: false,
            imports: vec![],
            module_bindings: vec![binding],
            type_edges: vec![],
            inheritance_relations: vec![],
            dataflow_edges: vec![],
        }
    }

    fn resolve(parsed: &ParsedFile, conn: &Connection) -> (Vec<ModuleBindingRow>, BindingEdges) {
        let catalog = BindingCatalog::from_parsed_files(std::slice::from_ref(parsed));
        resolve_for_file(parsed, conn, &catalog).unwrap()
    }

    #[test]
    fn overload_occurrences_collapse_to_the_canonical_import_target() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        let mut canonical = symbol("parse", "src/parser.ts", "function", "parse", true);
        canonical.text = "export function parse(value: string): string;".into();
        let mut overload = symbol("parse-number", "src/parser.ts", "function", "parse", true);
        overload.start_byte = 20;
        overload.text = "export function parse(value: number): number;".into();
        queries::symbols::batch_upsert_symbols(&conn, &[canonical.clone(), overload.clone()])
            .unwrap();
        queries::symbol_identities::batch_upsert(
            &conn,
            &[
                SymbolIdentityRow {
                    symbol_id: canonical.id.clone(),
                    logical_id: canonical.id.clone(),
                    qualified_name: "parse".into(),
                    signature: canonical.text.clone(),
                    occurrence_discriminator: "string:0".into(),
                    is_canonical: true,
                },
                SymbolIdentityRow {
                    symbol_id: overload.id.clone(),
                    logical_id: canonical.id.clone(),
                    qualified_name: "parse".into(),
                    signature: overload.text.clone(),
                    occurrence_discriminator: "number:0".into(),
                    is_canonical: false,
                },
            ],
        )
        .unwrap();

        match lookup_exported_symbol(&conn, "src/parser.ts", "parse").unwrap() {
            Lookup::Exact(target) => assert_eq!(target.id, canonical.id),
            Lookup::Ambiguous => panic!("one overload set must not be ambiguous"),
            Lookup::Missing => panic!("overload set must resolve"),
        }
    }

    fn with_path(mut parsed: ParsedFile, path: &str, root_id: &str) -> ParsedFile {
        parsed.rel_path = path.into();
        parsed.symbol_rows[0].id = root_id.into();
        parsed.symbol_rows[0].file_path = path.into();
        parsed.symbol_rows[0].name = path.into();
        parsed
    }

    #[test]
    fn named_reexport_resolves_exact_symbol_and_preserves_alias() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        for row in [
            symbol(
                "impl-root",
                "src/implementation.ts",
                "file",
                "src/implementation.ts",
                false,
            ),
            symbol(
                "impl-symbol",
                "src/implementation.ts",
                "class",
                "Implementation",
                true,
            ),
        ] {
            queries::symbols::upsert_symbol(&conn, &row).unwrap();
        }
        let parsed = parsed(ModuleBinding {
            kind: ModuleBindingKind::ReExport,
            source: "./implementation".into(),
            imported_name: "Implementation".into(),
            local_name: String::new(),
            exported_name: "PublicImplementation".into(),
            at_line: 1,
        });

        let (rows, edges) = resolve(&parsed, &conn);
        assert_eq!(rows[0].target_symbol_id.as_deref(), Some("impl-symbol"));
        assert_eq!(rows[0].exported_name, "PublicImplementation");
        assert_eq!(rows[0].resolution, "exact");
        assert_eq!(edges[0].0.edge_type, "re_export");
    }

    #[test]
    fn chained_named_reexport_targets_source_module_without_false_symbol() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        queries::symbols::upsert_symbol(
            &conn,
            &symbol(
                "first-barrel-root",
                "src/first.ts",
                "file",
                "src/first.ts",
                false,
            ),
        )
        .unwrap();
        let mut parsed = parsed(ModuleBinding {
            kind: ModuleBindingKind::ReExport,
            source: "./first".into(),
            imported_name: "PublicImplementation".into(),
            local_name: String::new(),
            exported_name: "RenamedAgain".into(),
            at_line: 1,
        });

        let (rows, edges) = resolve(&parsed, &conn);
        assert_eq!(
            rows[0].target_symbol_id.as_deref(),
            Some("first-barrel-root")
        );
        assert_eq!(rows[0].resolution, "inferred");
        assert_eq!(edges[0].0.confidence, 0.75);

        parsed.module_bindings[0].kind = ModuleBindingKind::Import;
        let (rows, edges) = resolve(&parsed, &conn);
        assert_eq!(rows[0].target_symbol_id, None);
        assert_eq!(rows[0].resolution, "unresolved");
        assert!(edges.is_empty(), "ordinary imports must not infer a symbol");
    }

    #[test]
    fn local_export_alias_resolves_through_unique_import_binding() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        for row in [
            symbol(
                "worker-root",
                "src/worker.ts",
                "file",
                "src/worker.ts",
                false,
            ),
            symbol("worker", "src/worker.ts", "class", "Worker", true),
        ] {
            queries::symbols::upsert_symbol(&conn, &row).unwrap();
        }
        let mut parsed = parsed(ModuleBinding {
            kind: ModuleBindingKind::Import,
            source: "./worker".into(),
            imported_name: "Worker".into(),
            local_name: "LocalWorker".into(),
            exported_name: String::new(),
            at_line: 1,
        });
        parsed.module_bindings.push(ModuleBinding {
            kind: ModuleBindingKind::Export,
            source: String::new(),
            imported_name: String::new(),
            local_name: "LocalWorker".into(),
            exported_name: "PublicWorker".into(),
            at_line: 2,
        });

        let (rows, edges) = resolve(&parsed, &conn);
        assert_eq!(rows[1].target_symbol_id.as_deref(), Some("worker"));
        assert_eq!(rows[1].exported_name, "PublicWorker");
        assert_eq!(rows[1].resolution, "exact");
        assert!(edges
            .iter()
            .any(|(edge, _)| { edge.edge_type == "export" && edge.to_symbol_id == "worker" }));
    }

    #[test]
    fn default_import_resolves_through_source_public_binding() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        for row in [
            symbol(
                "worker-root",
                "src/worker.ts",
                "file",
                "src/worker.ts",
                false,
            ),
            symbol("worker", "src/worker.ts", "class", "Worker", true),
            symbol(
                "consumer-root",
                "src/consumer.ts",
                "file",
                "src/consumer.ts",
                false,
            ),
        ] {
            queries::symbols::upsert_symbol(&conn, &row).unwrap();
        }
        let worker = with_path(
            parsed(ModuleBinding {
                kind: ModuleBindingKind::Export,
                source: String::new(),
                imported_name: String::new(),
                local_name: "Worker".into(),
                exported_name: "default".into(),
                at_line: 1,
            }),
            "src/worker.ts",
            "worker-root",
        );
        let consumer = with_path(
            parsed(ModuleBinding {
                kind: ModuleBindingKind::Import,
                source: "./worker".into(),
                imported_name: "default".into(),
                local_name: "DefaultWorker".into(),
                exported_name: String::new(),
                at_line: 1,
            }),
            "src/consumer.ts",
            "consumer-root",
        );
        let catalog = BindingCatalog::from_parsed_files(&[worker, consumer.clone()]);

        let (rows, edges) = resolve_for_file(&consumer, &conn, &catalog).unwrap();
        assert_eq!(rows[0].target_symbol_id.as_deref(), Some("worker"));
        assert_eq!(rows[0].resolution, "exact");
        assert!(edges
            .iter()
            .any(|(edge, _)| edge.edge_type == "import" && edge.to_symbol_id == "worker"));
    }

    #[test]
    fn incremental_default_import_reuses_persisted_public_binding() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        for row in [
            symbol(
                "worker-root",
                "src/worker.ts",
                "file",
                "src/worker.ts",
                false,
            ),
            symbol("worker", "src/worker.ts", "class", "Worker", true),
            symbol(
                "consumer-root",
                "src/consumer.ts",
                "file",
                "src/consumer.ts",
                false,
            ),
        ] {
            queries::symbols::upsert_symbol(&conn, &row).unwrap();
        }
        queries::module_bindings::batch_upsert(
            &conn,
            &[ModuleBindingRow {
                id: 0,
                file_path: "src/worker.ts".into(),
                binding_kind: "export".into(),
                source_module: String::new(),
                source_file: None,
                imported_name: String::new(),
                local_name: "Worker".into(),
                exported_name: "default".into(),
                target_symbol_id: Some("worker".into()),
                at_line: 1,
                resolution: "exact".into(),
                confidence: 1.0,
            }],
        )
        .unwrap();
        let consumer = with_path(
            parsed(ModuleBinding {
                kind: ModuleBindingKind::Import,
                source: "./worker".into(),
                imported_name: "default".into(),
                local_name: "DefaultWorker".into(),
                exported_name: String::new(),
                at_line: 1,
            }),
            "src/consumer.ts",
            "consumer-root",
        );
        // Incremental batches contain only changed files, so the source file
        // is intentionally absent from the in-memory catalog.
        let catalog = BindingCatalog::from_parsed_files(std::slice::from_ref(&consumer));

        let (rows, edges) = resolve_for_file(&consumer, &conn, &catalog).unwrap();
        assert_eq!(rows[0].target_symbol_id.as_deref(), Some("worker"));
        assert_eq!(rows[0].resolution, "exact");
        assert!(edges
            .iter()
            .any(|(edge, _)| edge.edge_type == "import" && edge.to_symbol_id == "worker"));
    }

    #[test]
    fn import_does_not_resolve_to_private_same_name_declaration() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        for row in [
            symbol(
                "worker-root",
                "src/worker.ts",
                "file",
                "src/worker.ts",
                false,
            ),
            symbol("private-worker", "src/worker.ts", "class", "Worker", false),
            symbol(
                "consumer-root",
                "src/consumer.ts",
                "file",
                "src/consumer.ts",
                false,
            ),
        ] {
            queries::symbols::upsert_symbol(&conn, &row).unwrap();
        }
        let consumer = with_path(
            parsed(ModuleBinding {
                kind: ModuleBindingKind::Import,
                source: "./worker".into(),
                imported_name: "Worker".into(),
                local_name: "Worker".into(),
                exported_name: String::new(),
                at_line: 1,
            }),
            "src/consumer.ts",
            "consumer-root",
        );
        let catalog = BindingCatalog::from_parsed_files(std::slice::from_ref(&consumer));

        let (rows, edges) = resolve_for_file(&consumer, &conn, &catalog).unwrap();
        assert_eq!(rows[0].target_symbol_id, None);
        assert_eq!(rows[0].resolution, "unresolved");
        assert!(edges.is_empty());
    }

    #[test]
    fn named_reexport_cycle_is_explicit_and_does_not_create_false_edge() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        for row in [
            symbol("a-root", "src/a.ts", "file", "src/a.ts", false),
            symbol("b-root", "src/b.ts", "file", "src/b.ts", false),
        ] {
            queries::symbols::upsert_symbol(&conn, &row).unwrap();
        }
        let a = with_path(
            parsed(ModuleBinding {
                kind: ModuleBindingKind::ReExport,
                source: "./b".into(),
                imported_name: "CyclicName".into(),
                local_name: String::new(),
                exported_name: "CyclicName".into(),
                at_line: 1,
            }),
            "src/a.ts",
            "a-root",
        );
        let b = with_path(
            parsed(ModuleBinding {
                kind: ModuleBindingKind::ReExport,
                source: "./a".into(),
                imported_name: "CyclicName".into(),
                local_name: String::new(),
                exported_name: "CyclicName".into(),
                at_line: 1,
            }),
            "src/b.ts",
            "b-root",
        );
        let catalog = BindingCatalog::from_parsed_files(&[a.clone(), b]);

        let (rows, edges) = resolve_for_file(&a, &conn, &catalog).unwrap();
        assert_eq!(rows[0].resolution, "cyclic");
        assert_eq!(rows[0].target_symbol_id, None);
        assert!(edges.is_empty());
    }
}

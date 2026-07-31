use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::indexer::pipeline::parse::ParsedFile;
use crate::storage::sqlite::queries;
use crate::storage::sqlite::schema::{EdgeEvidenceRow, EdgeRow};
use crate::storage::tantivy::TantivyIndex;

struct ForeignKeysOffGuard<'a> {
    conn: &'a Connection,
}

impl<'a> ForeignKeysOffGuard<'a> {
    fn new(conn: &'a Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys=OFF")?;
        Ok(Self { conn })
    }
}

impl Drop for ForeignKeysOffGuard<'_> {
    fn drop(&mut self) {
        let _ = self.conn.execute_batch("PRAGMA foreign_keys=ON");
    }
}

/// Statistics returned by write_batch
#[derive(Debug, Clone, Default)]
pub struct WriteStats {
    pub files_written: usize,
    pub symbols_written: usize,
    pub sqlite_ms: u64,
    pub tantivy_ms: u64,
}

/// Write parsed files to SQLite and Tantivy in efficient batches.
///
/// Processes files in chunks of 50:
/// - SQLite: one transaction per chunk (delete old + batch upsert)
/// - Tantivy: upsert per chunk, single commit at end
///
/// # Arguments
/// * `parsed_files` - Slice of parsed files with all extracted data
/// * `conn` - SQLite connection (does NOT need to be in a transaction)
/// * `tantivy` - Tantivy index for full-text search
///
/// # Returns
/// WriteStats with counts of files and symbols written
pub fn write_batch(
    parsed_files: &[ParsedFile],
    conn: &Connection,
    tantivy: &TantivyIndex,
) -> Result<WriteStats> {
    const CHUNK_SIZE: usize = 50;
    let mut stats = WriteStats {
        files_written: 0,
        symbols_written: 0,
        sqlite_ms: 0,
        tantivy_ms: 0,
    };

    // Disable FK enforcement during batch writes. Chunks of files are written
    // in order, and usage_examples.from_symbol_id may reference symbols from
    // files in later chunks that haven't been inserted yet.
    let _fk_guard = ForeignKeysOffGuard::new(conn)?;

    for chunk in parsed_files.chunks(CHUNK_SIZE) {
        // --- SQLite: one transaction per chunk ---
        let sqlite_t = std::time::Instant::now();
        let tx = conn
            .unchecked_transaction()
            .context("Failed to begin transaction for write_batch chunk")?;

        for file in chunk {
            // Delete old data for this file
            // Outgoing edges are source-owned and must be regenerated from the
            // changed file. Incoming edges are preserved across stable target
            // IDs and orphan-pruned after all replacement symbols are present.
            queries::edges::delete_outgoing_edges_by_file(&tx, &file.rel_path).with_context(
                || {
                    format!(
                        "Failed to delete outgoing edges for file: {}",
                        file.rel_path
                    )
                },
            )?;
            queries::module_bindings::delete_by_file(&tx, &file.rel_path).with_context(|| {
                format!(
                    "Failed to delete module bindings for file: {}",
                    file.rel_path
                )
            })?;
            queries::data_flow::delete_by_file(&tx, &file.rel_path).with_context(|| {
                format!(
                    "Failed to delete data-flow facts for file: {}",
                    file.rel_path
                )
            })?;
            // Foreign keys are intentionally disabled for chunk replacement,
            // so delete occurrence metadata explicitly before its symbols.
            queries::symbol_identities::delete_by_file(&tx, &file.rel_path).with_context(|| {
                format!(
                    "Failed to delete symbol identities for file: {}",
                    file.rel_path
                )
            })?;
            queries::symbols::delete_symbols_by_file(&tx, &file.rel_path)
                .with_context(|| format!("Failed to delete symbols for file: {}", file.rel_path))?;
            queries::misc::delete_usage_examples_by_file(&tx, &file.rel_path).with_context(
                || {
                    format!(
                        "Failed to delete usage examples for file: {}",
                        file.rel_path
                    )
                },
            )?;
            queries::todos::delete_todos_by_file(&tx, &file.rel_path)
                .with_context(|| format!("Failed to delete todos for file: {}", file.rel_path))?;
            queries::docstrings::delete_docstrings_by_file(&tx, &file.rel_path).with_context(
                || format!("Failed to delete docstrings for file: {}", file.rel_path),
            )?;
            queries::decorators::delete_decorators_by_file(&tx, &file.rel_path).with_context(
                || format!("Failed to delete decorators for file: {}", file.rel_path),
            )?;
            queries::framework::delete_framework_patterns_by_file(&tx, &file.rel_path)
                .with_context(|| {
                    format!(
                        "Failed to delete framework patterns for file: {}",
                        file.rel_path
                    )
                })?;

            // Validate occurrence ids before any symbol row can overwrite an
            // unrelated declaration, then persist symbols and their logical
            // identity metadata in the same transaction.
            queries::symbol_identities::validate_no_collisions(&tx, &file.symbol_identities)
                .with_context(|| {
                    format!(
                        "Failed symbol identity collision validation for file: {}",
                        file.rel_path
                    )
                })?;
            queries::symbols::batch_upsert_symbols(&tx, &file.symbol_rows).with_context(|| {
                format!("Failed to batch upsert symbols for file: {}", file.rel_path)
            })?;
            queries::symbol_identities::batch_upsert(&tx, &file.symbol_identities).with_context(
                || {
                    format!(
                        "Failed to batch upsert symbol identities for file: {}",
                        file.rel_path
                    )
                },
            )?;

            queries::edges::batch_upsert_edges(&tx, &file.edges).with_context(|| {
                format!("Failed to batch upsert edges for file: {}", file.rel_path)
            })?;

            queries::misc::batch_upsert_usage_examples(&tx, &file.usage_examples).with_context(
                || {
                    format!(
                        "Failed to batch upsert usage examples for file: {}",
                        file.rel_path
                    )
                },
            )?;

            if !file.todos.is_empty() {
                queries::todos::batch_upsert_todos(&tx, &file.todos).with_context(|| {
                    format!("Failed to batch upsert todos for file: {}", file.rel_path)
                })?;
            }

            if !file.docstrings.is_empty() {
                queries::docstrings::batch_upsert_docstrings(&tx, &file.docstrings).with_context(
                    || {
                        format!(
                            "Failed to batch upsert docstrings for file: {}",
                            file.rel_path
                        )
                    },
                )?;
            }

            if !file.decorators.is_empty() {
                queries::decorators::batch_upsert_decorators(&tx, &file.decorators).with_context(
                    || {
                        format!(
                            "Failed to batch upsert decorators for file: {}",
                            file.rel_path
                        )
                    },
                )?;
            }

            if !file.framework_patterns.is_empty() {
                queries::framework::batch_upsert_framework_patterns(&tx, &file.framework_patterns)
                    .with_context(|| {
                        format!(
                            "Failed to batch upsert framework patterns for file: {}",
                            file.rel_path
                        )
                    })?;
            }

            // Test links (best-effort, non-critical)
            if file.is_test_file {
                let _ = queries::tests::create_test_links_for_file(&tx, &file.rel_path);
            }

            // Fingerprint
            queries::files::upsert_file_fingerprint(
                &tx,
                &file.rel_path,
                file.fingerprint.mtime_ns,
                file.fingerprint.size_bytes,
                Some(&file.content_hash),
            )
            .with_context(|| {
                format!(
                    "Failed to upsert file fingerprint for file: {}",
                    file.rel_path
                )
            })?;

            stats.files_written += 1;
            stats.symbols_written += file.symbol_rows.len();
        }

        tx.commit()
            .context("Failed to commit transaction for write_batch chunk")?;
        stats.sqlite_ms = stats
            .sqlite_ms
            .saturating_add(elapsed_ms(sqlite_t.elapsed()));

        // --- Tantivy: upsert for this chunk, no commit yet ---
        let tantivy_t = std::time::Instant::now();
        for file in chunk {
            tantivy
                .delete_symbols_by_file(&file.rel_path)
                .with_context(|| {
                    format!(
                        "Failed to delete symbols from Tantivy for file: {}",
                        file.rel_path
                    )
                })?;

            for row in &file.symbol_rows {
                tantivy
                    .upsert_symbol(row, &file.import_tags, &file.framework_tags, None)
                    .with_context(|| {
                        format!(
                            "Failed to upsert symbol to Tantivy: file={}, symbol={}",
                            file.rel_path, row.id
                        )
                    })?;
            }
        }
        stats.tantivy_ms = stats
            .tantivy_ms
            .saturating_add(elapsed_ms(tantivy_t.elapsed()));
    }

    // Foreign keys are disabled while files are replaced so incoming edges to
    // stable IDs survive. Once every replacement symbol has been written,
    // remove incoming edges whose targets genuinely disappeared.
    let sqlite_t = std::time::Instant::now();
    let orphaned_edges = queries::edges::delete_orphan_edges(conn)?;
    if orphaned_edges > 0 {
        tracing::debug!(
            orphaned_edges,
            "Removed orphan graph edges after symbol replacement"
        );
    }
    let orphaned_bindings = queries::module_bindings::clear_orphan_targets(conn)?;
    if orphaned_bindings > 0 {
        tracing::debug!(
            orphaned_bindings,
            "Cleared orphan module-binding targets after symbol replacement"
        );
    }
    let orphaned_usage_examples = queries::misc::delete_orphan_usage_examples(conn)?;
    if orphaned_usage_examples > 0 {
        tracing::debug!(
            orphaned_usage_examples,
            "Removed usage examples whose symbol endpoints do not exist"
        );
    }
    stats.sqlite_ms = stats
        .sqlite_ms
        .saturating_add(elapsed_ms(sqlite_t.elapsed()));

    // Single Tantivy commit after ALL chunks
    let tantivy_t = std::time::Instant::now();
    tantivy
        .commit()
        .context("Failed to commit Tantivy index after write_batch")?;
    stats.tantivy_ms = stats
        .tantivy_ms
        .saturating_add(elapsed_ms(tantivy_t.elapsed()));

    Ok(stats)
}

fn elapsed_ms(elapsed: std::time::Duration) -> u64 {
    elapsed.as_millis().min(u64::MAX as u128) as u64
}

pub fn write_module_bindings_batch(
    bundles: &[Vec<crate::storage::sqlite::ModuleBindingRow>],
    conn: &Connection,
) -> Result<usize> {
    let tx = conn
        .unchecked_transaction()
        .context("Failed to begin module binding write transaction")?;
    let mut written = 0usize;
    for rows in bundles {
        queries::module_bindings::batch_upsert(&tx, rows)
            .context("Failed to batch upsert module bindings")?;
        written += rows.len();
    }
    tx.commit()
        .context("Failed to commit module binding write transaction")?;
    Ok(written)
}

pub fn write_data_flow_facts_batch(
    bundles: &[Vec<crate::storage::sqlite::DataFlowFactRow>],
    conn: &Connection,
) -> Result<usize> {
    if bundles.iter().all(|bundle| bundle.is_empty()) {
        return Ok(0);
    }

    let foreign_keys_enabled: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .context("Failed to read SQLite foreign_keys setting")?;
    anyhow::ensure!(
        foreign_keys_enabled == 1,
        "write_data_flow_facts_batch requires SQLite foreign_keys=ON"
    );

    let tx = conn
        .unchecked_transaction()
        .context("Failed to begin data-flow fact write transaction")?;
    let mut written = 0usize;
    for rows in bundles {
        queries::data_flow::batch_upsert(&tx, rows)
            .context("Failed to batch upsert typed data-flow facts")?;
        written += rows.len();
    }
    tx.commit()
        .context("Failed to commit data-flow fact write transaction")?;
    queries::edges::validate_graph_integrity(conn)
        .context("Graph integrity validation failed after typed data-flow write")?;
    Ok(written)
}

/// Write the per-file edge bundles produced by the deferred edge-extraction
/// phase. The deferred phase runs `extract_edges_for_parsed_file` AFTER
/// `write_batch` has persisted symbols, so receiver-based cross-file
/// resolution can find class methods that were indexed in the same run.
///
/// `edge_bundles` is aligned with the original `ParsedFile` slice: index `i`
/// holds the edges for the i-th parsed file. The slice may contain empty
/// inner vecs (no edges for that file); they are skipped cheaply.
pub fn write_edges_batch(
    edge_bundles: &[Vec<(EdgeRow, Vec<EdgeEvidenceRow>)>],
    conn: &Connection,
) -> Result<usize> {
    if edge_bundles.iter().all(|b| b.is_empty()) {
        return Ok(0);
    }

    const CHUNK_SIZE: usize = 50;
    let mut edges_written = 0usize;

    // All symbols for the run were persisted by write_batch. Keep foreign-key
    // enforcement enabled here so a bad resolver cannot create orphan graph
    // endpoints.
    let foreign_keys_enabled: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .context("Failed to read SQLite foreign_keys setting")?;
    anyhow::ensure!(
        foreign_keys_enabled == 1,
        "write_edges_batch requires SQLite foreign_keys=ON"
    );

    for chunk in edge_bundles.chunks(CHUNK_SIZE) {
        let tx = conn
            .unchecked_transaction()
            .context("Failed to begin transaction for write_edges_batch chunk")?;

        for bundle in chunk {
            if bundle.is_empty() {
                continue;
            }
            queries::edges::batch_upsert_edges(&tx, bundle)
                .context("Failed to batch upsert edges in write_edges_batch")?;
            edges_written += bundle.len();
        }

        tx.commit()
            .context("Failed to commit transaction for write_edges_batch chunk")?;
    }

    queries::edges::validate_graph_integrity(conn)
        .context("Graph integrity validation failed after deferred edge write")?;

    Ok(edges_written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::pipeline::utils::FileFingerprint;
    use crate::path::Utf8PathBuf;
    use crate::storage::sqlite::schema::{SymbolIdentityRow, SymbolRow};
    use crate::storage::sqlite::SqliteStore;
    use crate::storage::tantivy::TantivyIndex;
    use std::time::SystemTime;

    fn temp_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid = std::process::id();
        std::env::temp_dir().join(format!("write_batch_test_{}_{}", pid, nanos))
    }

    #[test]
    fn write_batch_inserts_and_commits() {
        // Create temp dir
        let tmp_dir = temp_dir();
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let base_dir = Utf8PathBuf::from_path_buf(tmp_dir.clone()).unwrap();
        let db_path = base_dir.join("test.db");
        let tantivy_path = base_dir.join("tantivy");

        // Create SqliteStore and init schema
        let sqlite = SqliteStore::open(&db_path).unwrap();
        sqlite.init().unwrap();
        let conn = sqlite.write().unwrap();

        // Create TantivyIndex
        let tantivy = TantivyIndex::open_or_create(&tantivy_path).unwrap();

        // Build a ParsedFile with 2 symbols
        let parsed_file = ParsedFile {
            rel_path: "test.rs".to_string(),
            fingerprint: FileFingerprint {
                mtime_ns: 123456789,
                size_bytes: 100,
            },
            content_hash: "0".repeat(32),
            language: "rust".to_string(),
            symbol_rows: vec![
                SymbolRow {
                    id: "s1".to_string(),
                    file_path: "test.rs".to_string(),
                    language: "rust".to_string(),
                    kind: "function".to_string(),
                    name: "foo".to_string(),
                    exported: true,
                    start_byte: 0,
                    end_byte: 10,
                    start_line: 1,
                    end_line: 3,
                    text: "fn foo() {}".to_string(),
                },
                SymbolRow {
                    id: "s2".to_string(),
                    file_path: "test.rs".to_string(),
                    language: "rust".to_string(),
                    kind: "function".to_string(),
                    name: "bar".to_string(),
                    exported: false,
                    start_byte: 11,
                    end_byte: 20,
                    start_line: 4,
                    end_line: 6,
                    text: "fn bar() {}".to_string(),
                },
            ],
            symbol_identities: vec![
                SymbolIdentityRow {
                    symbol_id: "s1".into(),
                    logical_id: "s1".into(),
                    qualified_name: "foo".into(),
                    signature: "fn foo()".into(),
                    occurrence_discriminator: "foo:0".into(),
                    is_canonical: true,
                },
                SymbolIdentityRow {
                    symbol_id: "s2".into(),
                    logical_id: "s2".into(),
                    qualified_name: "bar".into(),
                    signature: "fn bar()".into(),
                    occurrence_discriminator: "bar:0".into(),
                    is_canonical: true,
                },
            ],
            edges: vec![],
            usage_examples: vec![],
            import_tags: "std".to_string(),
            framework_tags: "".to_string(),
            todos: vec![],
            docstrings: vec![],
            decorators: vec![],
            framework_patterns: vec![],
            is_test_file: false,
            imports: vec![],
            module_bindings: vec![],
            type_edges: vec![],
            extends_edges: vec![],
            dataflow_edges: vec![],
        };

        // Call write_batch
        let stats = write_batch(&[parsed_file], &conn, &tantivy).unwrap();

        // Assert stats
        assert_eq!(stats.files_written, 1);
        assert_eq!(stats.symbols_written, 2);

        // Assert SQLite has the symbols
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let identity_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbol_identities", [], |r| r.get(0))
            .unwrap();
        assert_eq!(identity_count, 2);

        // Assert fingerprint was stored
        let fp_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM file_fingerprints", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fp_count, 1);

        // Assert Tantivy can search for the symbols
        let hits = tantivy.search("foo", 10).unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|h| h.id == "s1"));

        // Cleanup
        drop(conn);
        drop(sqlite);
        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    #[test]
    fn write_batch_chunks_correctly() {
        // Create temp dir
        let tmp_dir = temp_dir();
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let base_dir = Utf8PathBuf::from_path_buf(tmp_dir.clone()).unwrap();
        let db_path = base_dir.join("test.db");
        let tantivy_path = base_dir.join("tantivy");

        // Create SqliteStore and init schema
        let sqlite = SqliteStore::open(&db_path).unwrap();
        sqlite.init().unwrap();
        let conn = sqlite.write().unwrap();

        // Create TantivyIndex
        let tantivy = TantivyIndex::open_or_create(&tantivy_path).unwrap();

        // Create 120 ParsedFile entries (exercises 3 chunks of 50+50+20)
        let mut parsed_files = vec![];
        for i in 0..120 {
            let file_path = format!("file{}.rs", i);
            let symbol_id = format!("s{}", i);
            parsed_files.push(ParsedFile {
                rel_path: file_path.clone(),
                fingerprint: FileFingerprint {
                    mtime_ns: 123456789 + i as i64,
                    size_bytes: 100,
                },
                content_hash: "0".repeat(32),
                language: "rust".to_string(),
                symbol_rows: vec![SymbolRow {
                    id: symbol_id.clone(),
                    file_path: file_path.clone(),
                    language: "rust".to_string(),
                    kind: "function".to_string(),
                    name: format!("func{}", i),
                    exported: true,
                    start_byte: 0,
                    end_byte: 10,
                    start_line: 1,
                    end_line: 3,
                    text: format!("fn func{}() {{}}", i),
                }],
                symbol_identities: vec![],
                edges: vec![],
                usage_examples: vec![],
                import_tags: "".to_string(),
                framework_tags: "".to_string(),
                todos: vec![],
                docstrings: vec![],
                decorators: vec![],
                framework_patterns: vec![],
                is_test_file: false,
                imports: vec![],
                module_bindings: vec![],
                type_edges: vec![],
                extends_edges: vec![],
                dataflow_edges: vec![],
            });
        }

        // Call write_batch
        let stats = write_batch(&parsed_files, &conn, &tantivy).unwrap();

        // Assert all 120 files' symbols are in SQLite
        assert_eq!(stats.files_written, 120);
        assert_eq!(stats.symbols_written, 120);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 120);

        // Assert fingerprints were stored for all files
        let fp_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM file_fingerprints", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fp_count, 120);

        // Cleanup
        drop(conn);
        drop(sqlite);
        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    #[test]
    fn write_batch_handles_empty_input() {
        // Create temp dir
        let tmp_dir = temp_dir();
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let base_dir = Utf8PathBuf::from_path_buf(tmp_dir.clone()).unwrap();
        let db_path = base_dir.join("test.db");
        let tantivy_path = base_dir.join("tantivy");

        // Create SqliteStore and init schema
        let sqlite = SqliteStore::open(&db_path).unwrap();
        sqlite.init().unwrap();
        let conn = sqlite.write().unwrap();

        // Create TantivyIndex
        let tantivy = TantivyIndex::open_or_create(&tantivy_path).unwrap();

        // Call write_batch with empty slice
        let stats = write_batch(&[], &conn, &tantivy).unwrap();

        // Assert no files written
        assert_eq!(stats.files_written, 0);
        assert_eq!(stats.symbols_written, 0);

        // Cleanup
        drop(conn);
        drop(sqlite);
        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    #[test]
    fn write_batch_overwrites_existing_symbols() {
        // Create temp dir
        let tmp_dir = temp_dir();
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let base_dir = Utf8PathBuf::from_path_buf(tmp_dir.clone()).unwrap();
        let db_path = base_dir.join("test.db");
        let tantivy_path = base_dir.join("tantivy");

        // Create SqliteStore and init schema
        let sqlite = SqliteStore::open(&db_path).unwrap();
        sqlite.init().unwrap();
        let conn = sqlite.write().unwrap();

        // Create TantivyIndex
        let tantivy = TantivyIndex::open_or_create(&tantivy_path).unwrap();

        // Write initial file with one symbol
        let parsed_file_v1 = ParsedFile {
            rel_path: "test.rs".to_string(),
            fingerprint: FileFingerprint {
                mtime_ns: 123456789,
                size_bytes: 100,
            },
            content_hash: "0".repeat(32),
            language: "rust".to_string(),
            symbol_rows: vec![SymbolRow {
                id: "s1".to_string(),
                file_path: "test.rs".to_string(),
                language: "rust".to_string(),
                kind: "function".to_string(),
                name: "foo".to_string(),
                exported: true,
                start_byte: 0,
                end_byte: 10,
                start_line: 1,
                end_line: 3,
                text: "fn foo() {}".to_string(),
            }],
            symbol_identities: vec![],
            edges: vec![],
            usage_examples: vec![],
            import_tags: "".to_string(),
            framework_tags: "".to_string(),
            todos: vec![],
            docstrings: vec![],
            decorators: vec![],
            framework_patterns: vec![],
            is_test_file: false,
            imports: vec![],
            module_bindings: vec![],
            type_edges: vec![],
            extends_edges: vec![],
            dataflow_edges: vec![],
        };

        write_batch(&[parsed_file_v1], &conn, &tantivy).unwrap();

        // Verify initial write
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Write updated file with two symbols (should delete old and insert new)
        let parsed_file_v2 = ParsedFile {
            rel_path: "test.rs".to_string(),
            fingerprint: FileFingerprint {
                mtime_ns: 987654321,
                size_bytes: 200,
            },
            content_hash: "0".repeat(32),
            language: "rust".to_string(),
            symbol_rows: vec![
                SymbolRow {
                    id: "s2".to_string(),
                    file_path: "test.rs".to_string(),
                    language: "rust".to_string(),
                    kind: "function".to_string(),
                    name: "bar".to_string(),
                    exported: true,
                    start_byte: 0,
                    end_byte: 10,
                    start_line: 1,
                    end_line: 3,
                    text: "fn bar() {}".to_string(),
                },
                SymbolRow {
                    id: "s3".to_string(),
                    file_path: "test.rs".to_string(),
                    language: "rust".to_string(),
                    kind: "function".to_string(),
                    name: "baz".to_string(),
                    exported: true,
                    start_byte: 11,
                    end_byte: 20,
                    start_line: 4,
                    end_line: 6,
                    text: "fn baz() {}".to_string(),
                },
            ],
            symbol_identities: vec![],
            edges: vec![],
            usage_examples: vec![],
            import_tags: "".to_string(),
            framework_tags: "".to_string(),
            todos: vec![],
            docstrings: vec![],
            decorators: vec![],
            framework_patterns: vec![],
            is_test_file: false,
            imports: vec![],
            module_bindings: vec![],
            type_edges: vec![],
            extends_edges: vec![],
            dataflow_edges: vec![],
        };

        write_batch(&[parsed_file_v2], &conn, &tantivy).unwrap();

        // Verify old symbol deleted and new symbols inserted
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        // Verify old symbol "foo" is gone
        let foo_exists: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols WHERE name = 'foo'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(foo_exists, 0);

        // Verify new symbols "bar" and "baz" exist
        let bar_exists: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols WHERE name = 'bar'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(bar_exists, 1);

        let baz_exists: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols WHERE name = 'baz'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(baz_exists, 1);

        // Cleanup
        drop(conn);
        drop(sqlite);
        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    #[test]
    fn deferred_edge_write_rejects_missing_symbol_endpoint() {
        let sqlite = SqliteStore::open_in_memory().unwrap();
        sqlite.init().unwrap();
        let conn = sqlite.write().unwrap();
        queries::symbols::upsert_symbol(
            &conn,
            &SymbolRow {
                id: "source".to_string(),
                file_path: "src/lib.rs".to_string(),
                language: "rust".to_string(),
                kind: "function".to_string(),
                name: "source".to_string(),
                exported: false,
                start_byte: 0,
                end_byte: 20,
                start_line: 1,
                end_line: 2,
                text: "fn source() {}".to_string(),
            },
        )
        .unwrap();

        let bundles = vec![vec![(
            EdgeRow {
                from_symbol_id: "source".to_string(),
                to_symbol_id: "missing".to_string(),
                edge_type: "reads".to_string(),
                at_file: Some("src/lib.rs".to_string()),
                at_line: Some(1),
                confidence: 0.7,
                evidence_count: 1,
                resolution: "unknown".to_string(),
            },
            vec![],
        )]];

        let error = write_edges_batch(&bundles, &conn).unwrap_err().to_string();
        assert!(error.contains("batch upsert edge"), "{error}");
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

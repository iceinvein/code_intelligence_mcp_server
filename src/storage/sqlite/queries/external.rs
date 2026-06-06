use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::storage::sqlite::schema::{ExternalReferenceRow, ExternalSymbolRow};

pub struct ExternalIndexInsert<'a> {
    pub id: &'a str,
    pub source_kind: &'a str,
    pub producer: &'a str,
    pub language: &'a str,
    pub root_path: &'a str,
    pub artifact_path: &'a str,
    pub artifact_hash: &'a str,
    pub status: &'a str,
    pub diagnostics_json: &'a str,
}

pub struct ExternalSymbolInsert<'a> {
    pub id: &'a str,
    pub external_index_id: &'a str,
    pub external_symbol: &'a str,
    pub display_name: &'a str,
    pub language: &'a str,
    pub kind: &'a str,
    pub file_path: Option<&'a str>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub start_byte: Option<u32>,
    pub end_byte: Option<u32>,
    pub metadata_json: &'a str,
}

pub struct ExternalReferenceInsert<'a> {
    pub external_index_id: &'a str,
    pub from_external_symbol_id: Option<&'a str>,
    pub to_external_symbol_id: Option<&'a str>,
    pub relationship: &'a str,
    pub file_path: &'a str,
    pub line: u32,
    pub column: Option<u32>,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
    pub confidence: f32,
    pub provenance: &'a str,
    pub metadata_json: &'a str,
}

pub struct SymbolMappingInsert<'a> {
    pub external_symbol_id: &'a str,
    pub internal_symbol_id: &'a str,
    pub mapping_kind: &'a str,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIndexStats {
    pub symbol_count: u64,
    pub reference_count: u64,
    pub mapping_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalOverlayStats {
    pub index_count: u64,
    pub symbol_count: u64,
    pub reference_count: u64,
    pub mapped_symbol_count: u64,
}

pub fn upsert_external_index(conn: &Connection, index: &ExternalIndexInsert<'_>) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO external_indexes (
  id, source_kind, producer, language, root_path, artifact_path,
  artifact_hash, status, diagnostics_json, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, unixepoch())
ON CONFLICT(id) DO UPDATE SET
  source_kind=excluded.source_kind,
  producer=excluded.producer,
  language=excluded.language,
  root_path=excluded.root_path,
  artifact_path=excluded.artifact_path,
  artifact_hash=excluded.artifact_hash,
  status=excluded.status,
  diagnostics_json=excluded.diagnostics_json,
  updated_at=unixepoch()
"#,
        params![
            index.id,
            index.source_kind,
            index.producer,
            index.language,
            index.root_path,
            index.artifact_path,
            index.artifact_hash,
            index.status,
            index.diagnostics_json,
        ],
    )
    .with_context(|| format!("Failed to upsert external index: id={}", index.id))?;
    Ok(())
}

pub fn upsert_external_symbol(conn: &Connection, symbol: &ExternalSymbolInsert<'_>) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO external_symbols (
  id, external_index_id, external_symbol, display_name, language, kind,
  file_path, start_line, end_line, start_byte, end_byte, metadata_json, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, unixepoch())
ON CONFLICT(id) DO UPDATE SET
  external_index_id=excluded.external_index_id,
  external_symbol=excluded.external_symbol,
  display_name=excluded.display_name,
  language=excluded.language,
  kind=excluded.kind,
  file_path=excluded.file_path,
  start_line=excluded.start_line,
  end_line=excluded.end_line,
  start_byte=excluded.start_byte,
  end_byte=excluded.end_byte,
  metadata_json=excluded.metadata_json,
  updated_at=unixepoch()
"#,
        params![
            symbol.id,
            symbol.external_index_id,
            symbol.external_symbol,
            symbol.display_name,
            symbol.language,
            symbol.kind,
            symbol.file_path,
            symbol.start_line.map(i64::from),
            symbol.end_line.map(i64::from),
            symbol.start_byte.map(i64::from),
            symbol.end_byte.map(i64::from),
            symbol.metadata_json,
        ],
    )
    .with_context(|| {
        format!(
            "Failed to upsert external symbol: id={}, index_id={}",
            symbol.id, symbol.external_index_id
        )
    })?;
    Ok(())
}

pub fn upsert_external_reference(
    conn: &Connection,
    reference: &ExternalReferenceInsert<'_>,
) -> Result<i64> {
    let dedupe_key = external_reference_dedupe_key(reference);
    conn.execute(
        r#"
INSERT INTO external_references (
  external_index_id, from_external_symbol_id, to_external_symbol_id, relationship,
  file_path, line, column, end_line, end_column, confidence, provenance, dedupe_key, metadata_json,
  updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, unixepoch())
ON CONFLICT(external_index_id, dedupe_key) DO UPDATE SET
  confidence=MAX(external_references.confidence, excluded.confidence),
  provenance=excluded.provenance,
  metadata_json=excluded.metadata_json,
  updated_at=unixepoch()
"#,
        params![
            reference.external_index_id,
            reference.from_external_symbol_id,
            reference.to_external_symbol_id,
            reference.relationship,
            reference.file_path,
            i64::from(reference.line),
            reference.column.map(i64::from),
            reference.end_line.map(i64::from),
            reference.end_column.map(i64::from),
            reference.confidence,
            reference.provenance,
            dedupe_key,
            reference.metadata_json,
        ],
    )
    .with_context(|| {
        format!(
            "Failed to insert external reference: index_id={}, relationship={}, file_path={}, line={}",
            reference.external_index_id, reference.relationship, reference.file_path, reference.line
        )
    })?;
    let id = conn
        .query_row(
            "SELECT id FROM external_references WHERE external_index_id = ?1 AND dedupe_key = ?2",
            params![reference.external_index_id, dedupe_key],
            |row| row.get(0),
        )
        .with_context(|| {
            format!(
                "Failed to read upserted external reference id: index_id={}, dedupe_key={}",
                reference.external_index_id, dedupe_key
            )
        })?;
    Ok(id)
}

pub fn upsert_symbol_mapping(conn: &Connection, mapping: &SymbolMappingInsert<'_>) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO symbol_mappings (
  external_symbol_id, internal_symbol_id, mapping_kind, confidence, created_at
)
VALUES (?1, ?2, ?3, ?4, unixepoch())
ON CONFLICT(external_symbol_id) DO UPDATE SET
  internal_symbol_id=excluded.internal_symbol_id,
  mapping_kind=excluded.mapping_kind,
  confidence=excluded.confidence
"#,
        params![
            mapping.external_symbol_id,
            mapping.internal_symbol_id,
            mapping.mapping_kind,
            mapping.confidence,
        ],
    )
    .with_context(|| {
        format!(
            "Failed to upsert symbol mapping: external_symbol_id={}, internal_symbol_id={}",
            mapping.external_symbol_id, mapping.internal_symbol_id
        )
    })?;
    Ok(())
}

pub fn list_external_symbols_for_index(
    conn: &Connection,
    external_index_id: &str,
    limit: usize,
) -> Result<Vec<ExternalSymbolRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  id, external_index_id, external_symbol, display_name, language, kind,
  file_path, start_line, end_line, start_byte, end_byte, metadata_json
FROM external_symbols
WHERE external_index_id = ?1
ORDER BY display_name ASC, id ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_external_symbols_for_index")?;

    let mut rows = stmt
        .query(params![external_index_id, limit as i64])
        .with_context(|| {
            format!("Failed to query external symbols for index: index_id={external_index_id}")
        })?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(read_external_symbol_row(row)?);
    }
    Ok(out)
}

pub fn list_external_references_to_internal_symbol(
    conn: &Connection,
    internal_symbol_id: &str,
    relationship: Option<&str>,
    limit: usize,
) -> Result<Vec<ExternalReferenceRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  er.id, er.external_index_id, er.from_external_symbol_id, er.to_external_symbol_id,
  er.relationship, er.file_path, er.line, er.column, er.end_line, er.end_column,
  er.confidence, er.provenance, er.metadata_json
FROM symbol_mappings sm
JOIN external_symbols es ON es.id = sm.external_symbol_id
JOIN external_references er ON er.to_external_symbol_id = es.id
WHERE sm.internal_symbol_id = ?1
  AND (?2 IS NULL OR er.relationship = ?2)
ORDER BY er.confidence DESC, er.file_path ASC, er.line ASC, er.id ASC
LIMIT ?3
"#,
        )
        .context("Failed to prepare list_external_references_to_internal_symbol")?;

    let mut rows = stmt
        .query(params![internal_symbol_id, relationship, limit as i64])
        .with_context(|| {
            format!(
                "Failed to query external references to internal symbol: internal_symbol_id={internal_symbol_id}"
            )
        })?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(read_external_reference_row(row)?);
    }
    Ok(out)
}

pub fn has_external_mapping_for_internal_symbol(
    conn: &Connection,
    internal_symbol_id: &str,
) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbol_mappings WHERE internal_symbol_id = ?1",
            params![internal_symbol_id],
            |row| row.get(0),
        )
        .with_context(|| {
            format!(
                "Failed to check external mapping for internal symbol: internal_symbol_id={internal_symbol_id}"
            )
        })?;
    Ok(count > 0)
}

pub fn external_index_stats(
    conn: &Connection,
    external_index_id: &str,
) -> Result<ExternalIndexStats> {
    let symbol_count = count_for_index(
        conn,
        "SELECT COUNT(*) FROM external_symbols WHERE external_index_id = ?1",
        external_index_id,
        "external symbols",
    )?;
    let reference_count = count_for_index(
        conn,
        "SELECT COUNT(*) FROM external_references WHERE external_index_id = ?1",
        external_index_id,
        "external references",
    )?;
    let mapping_count = count_for_index(
        conn,
        r#"
SELECT COUNT(*)
FROM symbol_mappings sm
JOIN external_symbols es ON es.id = sm.external_symbol_id
WHERE es.external_index_id = ?1
"#,
        external_index_id,
        "symbol mappings",
    )?;

    Ok(ExternalIndexStats {
        symbol_count,
        reference_count,
        mapping_count,
    })
}

pub fn external_overlay_stats(conn: &Connection) -> Result<ExternalOverlayStats> {
    let index_count = count_all(
        conn,
        "SELECT COUNT(*) FROM external_indexes",
        "external indexes",
    )?;
    let symbol_count = count_all(
        conn,
        "SELECT COUNT(*) FROM external_symbols",
        "external symbols",
    )?;
    let reference_count = count_all(
        conn,
        "SELECT COUNT(*) FROM external_references",
        "external references",
    )?;
    let mapped_symbol_count = count_all(
        conn,
        "SELECT COUNT(*) FROM symbol_mappings",
        "external symbol mappings",
    )?;

    Ok(ExternalOverlayStats {
        index_count,
        symbol_count,
        reference_count,
        mapped_symbol_count,
    })
}

fn count_for_index(
    conn: &Connection,
    sql: &str,
    external_index_id: &str,
    label: &str,
) -> Result<u64> {
    let count: i64 = conn
        .query_row(sql, params![external_index_id], |row| row.get(0))
        .with_context(|| {
            format!("Failed to count {label} for external index: index_id={external_index_id}")
        })?;
    Ok(count.max(0) as u64)
}

fn count_all(conn: &Connection, sql: &str, label: &str) -> Result<u64> {
    let count: i64 = conn
        .query_row(sql, [], |row| row.get(0))
        .with_context(|| format!("Failed to count {label}"))?;
    Ok(count.max(0) as u64)
}

fn read_external_symbol_row(row: &rusqlite::Row<'_>) -> Result<ExternalSymbolRow> {
    Ok(ExternalSymbolRow {
        id: row.get(0)?,
        external_index_id: row.get(1)?,
        external_symbol: row.get(2)?,
        display_name: row.get(3)?,
        language: row.get(4)?,
        kind: row.get(5)?,
        file_path: row.get(6)?,
        start_line: opt_u32(row, 7)?,
        end_line: opt_u32(row, 8)?,
        start_byte: opt_u32(row, 9)?,
        end_byte: opt_u32(row, 10)?,
        metadata_json: row.get(11)?,
    })
}

fn read_external_reference_row(row: &rusqlite::Row<'_>) -> Result<ExternalReferenceRow> {
    Ok(ExternalReferenceRow {
        id: row.get(0)?,
        external_index_id: row.get(1)?,
        from_external_symbol_id: row.get(2)?,
        to_external_symbol_id: row.get(3)?,
        relationship: row.get(4)?,
        file_path: row.get(5)?,
        line: u32_from_i64(row.get(6)?)?,
        column: opt_u32(row, 7)?,
        end_line: opt_u32(row, 8)?,
        end_column: opt_u32(row, 9)?,
        confidence: row.get::<_, f64>(10)? as f32,
        provenance: row.get(11)?,
        metadata_json: row.get(12)?,
    })
}

fn opt_u32(row: &rusqlite::Row<'_>, idx: usize) -> Result<Option<u32>> {
    row.get::<_, Option<i64>>(idx)?
        .map(u32_from_i64)
        .transpose()
}

fn u32_from_i64(value: i64) -> Result<u32> {
    u32::try_from(value).with_context(|| format!("Invalid u32 value from sqlite: {value}"))
}

fn external_reference_dedupe_key(reference: &ExternalReferenceInsert<'_>) -> String {
    let mut key = String::new();
    push_opt_str_key(&mut key, reference.from_external_symbol_id);
    push_opt_str_key(&mut key, reference.to_external_symbol_id);
    push_str_key(&mut key, reference.relationship);
    push_str_key(&mut key, reference.file_path);
    push_u32_key(&mut key, reference.line);
    push_opt_u32_key(&mut key, reference.column);
    push_opt_u32_key(&mut key, reference.end_line);
    push_opt_u32_key(&mut key, reference.end_column);
    key
}

fn push_str_key(key: &mut String, value: &str) {
    key.push('s');
    key.push_str(&value.len().to_string());
    key.push(':');
    key.push_str(value);
    key.push(';');
}

fn push_opt_str_key(key: &mut String, value: Option<&str>) {
    match value {
        Some(value) => push_str_key(key, value),
        None => key.push_str("n;"),
    }
}

fn push_u32_key(key: &mut String, value: u32) {
    key.push('u');
    key.push_str(&value.to_string());
    key.push(';');
}

fn push_opt_u32_key(key: &mut String, value: Option<u32>) {
    match value {
        Some(value) => push_u32_key(key, value),
        None => key.push_str("n;"),
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};

    use super::*;
    use crate::storage::sqlite::schema::SymbolRow;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::storage::sqlite::schema::SCHEMA_SQL)
            .unwrap();
        conn
    }

    fn insert_test_symbol(conn: &Connection, id: &str) {
        let symbol = SymbolRow {
            id: id.to_string(),
            file_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: "internal_target".to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 64,
            start_line: 1,
            end_line: 5,
            text: "pub fn internal_target() {}".to_string(),
        };

        conn.execute(
            r#"
INSERT INTO symbols (
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
"#,
            params![
                symbol.id,
                symbol.file_path,
                symbol.language,
                symbol.kind,
                symbol.name,
                if symbol.exported { 1 } else { 0 },
                symbol.start_byte,
                symbol.end_byte,
                symbol.start_line,
                symbol.end_line,
                symbol.text
            ],
        )
        .unwrap();
    }

    #[test]
    fn upserts_external_index_and_symbol_then_lists_symbols_for_index() {
        let conn = setup_test_db();

        upsert_external_index(
            &conn,
            &ExternalIndexInsert {
                id: "idx-rust-analyzer",
                source_kind: "lsp",
                producer: "rust-analyzer",
                language: "rust",
                root_path: "/repo",
                artifact_path: "/repo/.cache/ra.json",
                artifact_hash: "sha256:abc",
                status: "ready",
                diagnostics_json: "{}",
            },
        )
        .unwrap();

        upsert_external_symbol(
            &conn,
            &ExternalSymbolInsert {
                id: "ext:Foo",
                external_index_id: "idx-rust-analyzer",
                external_symbol: "crate::Foo",
                display_name: "Foo",
                language: "rust",
                kind: "struct",
                file_path: Some("src/lib.rs"),
                start_line: Some(3),
                end_line: Some(8),
                start_byte: Some(20),
                end_byte: Some(80),
                metadata_json: r#"{"visibility":"pub"}"#,
            },
        )
        .unwrap();

        let symbols = list_external_symbols_for_index(&conn, "idx-rust-analyzer", 10).unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].id, "ext:Foo");
        assert_eq!(symbols[0].external_index_id, "idx-rust-analyzer");
        assert_eq!(symbols[0].external_symbol, "crate::Foo");
        assert_eq!(symbols[0].display_name, "Foo");
        assert_eq!(symbols[0].file_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(symbols[0].start_line, Some(3));
        assert_eq!(symbols[0].metadata_json, r#"{"visibility":"pub"}"#);
    }

    #[test]
    fn maps_external_symbol_and_lists_references_by_internal_symbol() {
        let conn = setup_test_db();
        insert_test_symbol(&conn, "sym-internal-target");

        upsert_external_index(
            &conn,
            &ExternalIndexInsert {
                id: "idx-rust-analyzer",
                source_kind: "lsp",
                producer: "rust-analyzer",
                language: "rust",
                root_path: "/repo",
                artifact_path: "/repo/.cache/ra.json",
                artifact_hash: "sha256:abc",
                status: "ready",
                diagnostics_json: "{}",
            },
        )
        .unwrap();

        upsert_external_symbol(
            &conn,
            &ExternalSymbolInsert {
                id: "ext:target",
                external_index_id: "idx-rust-analyzer",
                external_symbol: "crate::target",
                display_name: "target",
                language: "rust",
                kind: "function",
                file_path: Some("src/lib.rs"),
                start_line: Some(1),
                end_line: Some(5),
                start_byte: Some(0),
                end_byte: Some(64),
                metadata_json: "{}",
            },
        )
        .unwrap();

        upsert_symbol_mapping(
            &conn,
            &SymbolMappingInsert {
                external_symbol_id: "ext:target",
                internal_symbol_id: "sym-internal-target",
                mapping_kind: "exact",
                confidence: 0.99,
            },
        )
        .unwrap();

        upsert_external_reference(
            &conn,
            &ExternalReferenceInsert {
                external_index_id: "idx-rust-analyzer",
                from_external_symbol_id: None,
                to_external_symbol_id: Some("ext:target"),
                relationship: "reference",
                file_path: "src/main.rs",
                line: 42,
                column: Some(7),
                end_line: Some(42),
                end_column: Some(13),
                confidence: 0.9,
                provenance: "rust-analyzer",
                metadata_json: "{}",
            },
        )
        .unwrap();

        assert!(has_external_mapping_for_internal_symbol(&conn, "sym-internal-target").unwrap());

        let references = list_external_references_to_internal_symbol(
            &conn,
            "sym-internal-target",
            Some("reference"),
            10,
        )
        .unwrap();
        assert_eq!(references.len(), 1);
        assert_eq!(
            references[0].to_external_symbol_id.as_deref(),
            Some("ext:target")
        );
        assert_eq!(references[0].relationship, "reference");
        assert_eq!(references[0].file_path, "src/main.rs");
        assert_eq!(references[0].line, 42);
        assert_eq!(references[0].column, Some(7));
        assert_eq!(references[0].confidence, 0.9);

        let stats = external_index_stats(&conn, "idx-rust-analyzer").unwrap();
        assert_eq!(stats.symbol_count, 1);
        assert_eq!(stats.reference_count, 1);
        assert_eq!(stats.mapping_count, 1);

        let overlay_stats = external_overlay_stats(&conn).unwrap();
        assert_eq!(overlay_stats.index_count, 1);
        assert_eq!(overlay_stats.symbol_count, 1);
        assert_eq!(overlay_stats.reference_count, 1);
        assert_eq!(overlay_stats.mapped_symbol_count, 1);
    }

    #[test]
    fn upserting_same_external_reference_deduplicates_and_updates_metadata() {
        let conn = setup_test_db();
        insert_test_symbol(&conn, "sym-internal-target");

        upsert_external_index(
            &conn,
            &ExternalIndexInsert {
                id: "idx-rust-analyzer",
                source_kind: "lsp",
                producer: "rust-analyzer",
                language: "rust",
                root_path: "/repo",
                artifact_path: "/repo/.cache/ra.json",
                artifact_hash: "sha256:abc",
                status: "ready",
                diagnostics_json: "{}",
            },
        )
        .unwrap();
        upsert_external_symbol(
            &conn,
            &ExternalSymbolInsert {
                id: "ext:target",
                external_index_id: "idx-rust-analyzer",
                external_symbol: "crate::target",
                display_name: "target",
                language: "rust",
                kind: "function",
                file_path: Some("src/lib.rs"),
                start_line: Some(1),
                end_line: Some(5),
                start_byte: Some(0),
                end_byte: Some(64),
                metadata_json: "{}",
            },
        )
        .unwrap();
        upsert_symbol_mapping(
            &conn,
            &SymbolMappingInsert {
                external_symbol_id: "ext:target",
                internal_symbol_id: "sym-internal-target",
                mapping_kind: "exact",
                confidence: 0.99,
            },
        )
        .unwrap();

        let first_id = upsert_external_reference(
            &conn,
            &ExternalReferenceInsert {
                external_index_id: "idx-rust-analyzer",
                from_external_symbol_id: None,
                to_external_symbol_id: Some("ext:target"),
                relationship: "reference",
                file_path: "src/main.rs",
                line: 42,
                column: None,
                end_line: Some(42),
                end_column: Some(13),
                confidence: 0.5,
                provenance: "first-import",
                metadata_json: r#"{"pass":1}"#,
            },
        )
        .unwrap();
        let second_id = upsert_external_reference(
            &conn,
            &ExternalReferenceInsert {
                external_index_id: "idx-rust-analyzer",
                from_external_symbol_id: None,
                to_external_symbol_id: Some("ext:target"),
                relationship: "reference",
                file_path: "src/main.rs",
                line: 42,
                column: None,
                end_line: Some(42),
                end_column: Some(13),
                confidence: 0.9,
                provenance: "second-import",
                metadata_json: r#"{"pass":2}"#,
            },
        )
        .unwrap();

        assert_eq!(first_id, second_id);

        let references = list_external_references_to_internal_symbol(
            &conn,
            "sym-internal-target",
            Some("reference"),
            10,
        )
        .unwrap();
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].confidence, 0.9);
        assert_eq!(references[0].provenance, "second-import");
        assert_eq!(references[0].metadata_json, r#"{"pass":2}"#);

        let stats = external_index_stats(&conn, "idx-rust-analyzer").unwrap();
        assert_eq!(stats.reference_count, 1);
    }
}

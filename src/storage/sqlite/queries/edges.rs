use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};

use crate::storage::sqlite::schema::{EdgeEvidenceRow, EdgeRow, SymbolRow};

fn sqlite_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

pub fn upsert_edge(conn: &Connection, edge: &EdgeRow) -> Result<()> {
    let resolution_rank = edge_resolution_rank(edge.resolution.as_str());
    conn.execute(
        r#"
INSERT INTO edges(from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count, resolution, resolution_rank)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
ON CONFLICT(from_symbol_id, to_symbol_id, edge_type) DO UPDATE SET
  at_file=COALESCE(edges.at_file, excluded.at_file),
  at_line=COALESCE(edges.at_line, excluded.at_line),
  confidence=MAX(edges.confidence, excluded.confidence),
  evidence_count=MAX(edges.evidence_count, excluded.evidence_count),
  resolution_rank=MAX(edges.resolution_rank, excluded.resolution_rank),
  resolution=CASE
    WHEN excluded.resolution_rank > edges.resolution_rank THEN excluded.resolution
    ELSE edges.resolution
  END
"#,
        params![
            edge.from_symbol_id,
            edge.to_symbol_id,
            edge.edge_type,
            edge.at_file,
            edge.at_line.map(|v| v as i64),
            edge.confidence,
            edge.evidence_count as i64,
            edge.resolution,
            resolution_rank
        ],
    )
    .with_context(|| {
        format!(
            "Failed to upsert edge: from={}, to={}, type={}",
            edge.from_symbol_id, edge.to_symbol_id, edge.edge_type
        )
    })?;
    Ok(())
}

pub fn upsert_edge_evidence(conn: &Connection, evidence: &EdgeEvidenceRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO edge_evidence(from_symbol_id, to_symbol_id, edge_type, at_file, at_line, count)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(from_symbol_id, to_symbol_id, edge_type, at_file, at_line) DO UPDATE SET
  count=MAX(edge_evidence.count, excluded.count)
"#,
        params![
            evidence.from_symbol_id,
            evidence.to_symbol_id,
            evidence.edge_type,
            evidence.at_file,
            evidence.at_line as i64,
            evidence.count as i64
        ],
    )
    .with_context(|| {
        format!(
            "Failed to upsert edge evidence: from={}, to={}, type={}, file={}",
            evidence.from_symbol_id, evidence.to_symbol_id, evidence.edge_type, evidence.at_file
        )
    })?;
    Ok(())
}

pub fn batch_upsert_edges(
    conn: &Connection,
    edges: &[(EdgeRow, Vec<EdgeEvidenceRow>)],
) -> Result<()> {
    let mut edge_stmt = conn.prepare_cached(
        r#"
INSERT INTO edges(from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count, resolution, resolution_rank)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
ON CONFLICT(from_symbol_id, to_symbol_id, edge_type) DO UPDATE SET
  at_file=COALESCE(edges.at_file, excluded.at_file),
  at_line=COALESCE(edges.at_line, excluded.at_line),
  confidence=MAX(edges.confidence, excluded.confidence),
  evidence_count=MAX(edges.evidence_count, excluded.evidence_count),
  resolution_rank=MAX(edges.resolution_rank, excluded.resolution_rank),
  resolution=CASE
    WHEN excluded.resolution_rank > edges.resolution_rank THEN excluded.resolution
    ELSE edges.resolution
  END
"#,
    )?;
    let mut ev_stmt = conn.prepare_cached(
        r#"
INSERT INTO edge_evidence(from_symbol_id, to_symbol_id, edge_type, at_file, at_line, count)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(from_symbol_id, to_symbol_id, edge_type, at_file, at_line) DO UPDATE SET
  count=MAX(edge_evidence.count, excluded.count)
"#,
    )?;
    for (edge, evidence) in edges {
        let resolution_rank = edge_resolution_rank(edge.resolution.as_str());
        edge_stmt
            .execute(params![
                edge.from_symbol_id,
                edge.to_symbol_id,
                edge.edge_type,
                edge.at_file,
                edge.at_line.map(|v| v as i64),
                edge.confidence,
                edge.evidence_count as i64,
                edge.resolution,
                resolution_rank
            ])
            .with_context(|| {
                format!(
                    "Failed to batch upsert edge: from={}, to={}, type={}",
                    edge.from_symbol_id, edge.to_symbol_id, edge.edge_type
                )
            })?;
        for ev in evidence {
            ev_stmt
                .execute(params![
                    ev.from_symbol_id,
                    ev.to_symbol_id,
                    ev.edge_type,
                    ev.at_file,
                    ev.at_line as i64,
                    ev.count as i64
                ])
                .with_context(|| {
                    format!(
                        "Failed to batch upsert edge evidence: from={}, to={}, type={}, file={}",
                        ev.from_symbol_id, ev.to_symbol_id, ev.edge_type, ev.at_file
                    )
                })?;
        }
    }
    Ok(())
}

/// Delete outgoing graph facts owned by symbols in a changed file.
///
/// Incoming edges are deliberately preserved: stable target IDs remain valid
/// when a declaration is rewritten in place, and unchanged source files are
/// not re-parsed during an incremental index. Orphaned incoming edges are
/// removed after all replacement symbols have been written.
pub fn delete_outgoing_edges_by_file(conn: &Connection, file_path: &str) -> Result<u64> {
    conn.execute(
        r#"
DELETE FROM edge_evidence
WHERE from_symbol_id IN (SELECT id FROM symbols WHERE file_path = ?1)
"#,
        params![file_path],
    )
    .with_context(|| format!("Failed to delete edge evidence owned by file: {file_path}"))?;

    let deleted = conn
        .execute(
            r#"
DELETE FROM edges
WHERE from_symbol_id IN (SELECT id FROM symbols WHERE file_path = ?1)
"#,
            params![file_path],
        )
        .with_context(|| format!("Failed to delete outgoing edges owned by file: {file_path}"))?;
    Ok(deleted as u64)
}

/// Remove graph rows whose endpoints disappeared while foreign-key checks
/// were temporarily disabled for the symbol replacement phase.
pub fn delete_orphan_edges(conn: &Connection) -> Result<u64> {
    conn.execute(
        r#"
DELETE FROM edge_evidence
WHERE NOT EXISTS (SELECT 1 FROM symbols s WHERE s.id = edge_evidence.from_symbol_id)
   OR NOT EXISTS (SELECT 1 FROM symbols s WHERE s.id = edge_evidence.to_symbol_id)
"#,
        [],
    )
    .context("Failed to delete orphan edge evidence")?;

    let deleted = conn
        .execute(
            r#"
DELETE FROM edges
WHERE NOT EXISTS (SELECT 1 FROM symbols s WHERE s.id = edges.from_symbol_id)
   OR NOT EXISTS (SELECT 1 FROM symbols s WHERE s.id = edges.to_symbol_id)
"#,
            [],
        )
        .context("Failed to delete orphan edges")?;
    Ok(deleted as u64)
}

/// Enforce the graph contracts that can be checked without source parsing.
pub fn validate_graph_integrity(conn: &Connection) -> Result<()> {
    for table_name in ["edges", "edge_evidence"] {
        let sql = format!("PRAGMA foreign_key_check({table_name})");
        let mut fk_stmt = conn
            .prepare(&sql)
            .context("Failed to prepare graph foreign-key validation")?;
        let mut fk_rows = fk_stmt
            .query([])
            .context("Failed to run graph foreign-key validation")?;
        if let Some(row) = fk_rows.next()? {
            let table: String = row.get(0)?;
            let rowid: Option<i64> = row.get(1)?;
            let parent: String = row.get(2)?;
            let fk_index: i64 = row.get(3)?;
            bail!(
                "Graph foreign-key violation: table={table}, rowid={rowid:?}, parent={parent}, fk_index={fk_index}"
            );
        }
    }

    let invalid_locations: i64 = conn
        .query_row(
            r#"
SELECT COUNT(*)
FROM edges e
JOIN symbols source ON source.id = e.from_symbol_id
WHERE e.edge_type IN ('reads', 'writes', 'async_call', 'spawn')
  AND (
    e.at_file IS NULL
    OR e.at_file != source.file_path
    OR e.at_line IS NULL
    OR e.at_line < source.start_line
    OR e.at_line > source.end_line
  )
"#,
            [],
            |row| row.get(0),
        )
        .context("Failed to validate data-flow edge locations")?;
    if invalid_locations > 0 {
        bail!(
            "Graph integrity violation: {invalid_locations} data-flow edges fall outside their source symbol"
        );
    }

    Ok(())
}

pub fn list_edge_evidence(
    conn: &Connection,
    from_symbol_id: &str,
    to_symbol_id: &str,
    edge_type: &str,
    limit: usize,
) -> Result<Vec<EdgeEvidenceRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  from_symbol_id, to_symbol_id, edge_type, at_file, at_line, count
FROM edge_evidence
WHERE from_symbol_id = ?1 AND to_symbol_id = ?2 AND edge_type = ?3
ORDER BY count DESC, at_file ASC, at_line ASC, id ASC
LIMIT ?4
"#,
        )
        .context("Failed to prepare list_edge_evidence")?;

    let mut rows = stmt.query(params![
        from_symbol_id,
        to_symbol_id,
        edge_type,
        limit as i64
    ])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(EdgeEvidenceRow {
            from_symbol_id: row.get(0)?,
            to_symbol_id: row.get(1)?,
            edge_type: row.get(2)?,
            at_file: row.get(3)?,
            at_line: u32::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
            count: u32::try_from(row.get::<_, i64>(5)?).unwrap_or(1),
        });
    }
    Ok(out)
}

pub fn list_edges_from(
    conn: &Connection,
    from_symbol_id: &str,
    limit: usize,
) -> Result<Vec<EdgeRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count, resolution
FROM edges
WHERE from_symbol_id = ?1
ORDER BY edge_type ASC, to_symbol_id ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_edges_from")?;

    let mut rows = stmt.query(params![from_symbol_id, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(EdgeRow {
            from_symbol_id: row.get(0)?,
            to_symbol_id: row.get(1)?,
            edge_type: row.get(2)?,
            at_file: row.get(3)?,
            at_line: row
                .get::<_, Option<i64>>(4)?
                .and_then(|v| u32::try_from(v).ok()),
            confidence: row.get::<_, f64>(5)? as f32,
            evidence_count: u32::try_from(row.get::<_, i64>(6)?).unwrap_or(1),
            resolution: row.get(7)?,
        });
    }
    Ok(out)
}

pub fn list_edges_to(conn: &Connection, to_symbol_id: &str, limit: usize) -> Result<Vec<EdgeRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count, resolution
FROM edges
WHERE to_symbol_id = ?1
ORDER BY edge_type ASC, from_symbol_id ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_edges_to")?;

    let mut rows = stmt.query(params![to_symbol_id, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(EdgeRow {
            from_symbol_id: row.get(0)?,
            to_symbol_id: row.get(1)?,
            edge_type: row.get(2)?,
            at_file: row.get(3)?,
            at_line: row
                .get::<_, Option<i64>>(4)?
                .and_then(|v| u32::try_from(v).ok()),
            confidence: row.get::<_, f64>(5)? as f32,
            evidence_count: u32::try_from(row.get::<_, i64>(6)?).unwrap_or(1),
            resolution: row.get(7)?,
        });
    }
    Ok(out)
}

pub fn list_edges_to_by_type(
    conn: &Connection,
    to_symbol_id: &str,
    edge_type: &str,
    limit: usize,
) -> Result<Vec<EdgeRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  from_symbol_id, to_symbol_id, edge_type, at_file, at_line, confidence, evidence_count, resolution
FROM edges
WHERE to_symbol_id = ?1 AND edge_type = ?2
ORDER BY edge_type ASC, from_symbol_id ASC
LIMIT ?3
"#,
        )
        .context("Failed to prepare list_edges_to_by_type")?;

    let mut rows = stmt.query(params![to_symbol_id, edge_type, sqlite_limit(limit)])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(EdgeRow {
            from_symbol_id: row.get(0)?,
            to_symbol_id: row.get(1)?,
            edge_type: row.get(2)?,
            at_file: row.get(3)?,
            at_line: row
                .get::<_, Option<i64>>(4)?
                .and_then(|v| u32::try_from(v).ok()),
            confidence: row.get::<_, f64>(5)? as f32,
            evidence_count: u32::try_from(row.get::<_, i64>(6)?).unwrap_or(1),
            resolution: row.get(7)?,
        });
    }
    Ok(out)
}

pub fn count_incoming_edges(conn: &Connection, to_symbol_id: &str) -> Result<u64> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE to_symbol_id = ?1",
            params![to_symbol_id],
            |row| row.get(0),
        )
        .context("Failed to count incoming edges")?;
    Ok(count.max(0) as u64)
}

pub fn count_edges(conn: &Connection) -> Result<u64> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))
        .context("Failed to count edges")?;
    Ok(count.max(0) as u64)
}

pub fn list_all_edges(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT from_symbol_id, to_symbol_id
FROM edges
"#,
        )
        .context("Failed to prepare list_all_edges")?;

    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push((row.get(0)?, row.get(1)?));
    }
    Ok(out)
}

pub fn list_all_symbol_ids(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT id, kind FROM symbols
"#,
        )
        .context("Failed to prepare list_all_symbol_ids")?;

    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push((row.get(0)?, row.get(1)?));
    }
    Ok(out)
}

pub fn find_dead_symbols(
    conn: &Connection,
    file_path: Option<&str>,
    language: Option<&str>,
    kind: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    let mut conditions = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    // Core condition: no incoming edges (LEFT JOIN + IS NULL)
    // Already handled in the JOIN clause

    // Exclude structural kinds that are not callable
    conditions.push("s.kind NOT IN ('file', 'module', 'impl')".to_string());

    // Exclude program entry points
    conditions.push("s.name != 'main'".to_string());

    // Exclude framework entry points
    conditions.push(
        "NOT EXISTS (SELECT 1 FROM framework_patterns fp WHERE fp.name = s.name AND fp.file_path = s.file_path)"
            .to_string(),
    );

    // Filter out test files unless include_tests is true
    if !include_tests {
        conditions.push(
            "(s.file_path NOT LIKE '%test%' AND s.file_path NOT LIKE '%.test.%' AND s.file_path NOT LIKE '%.spec.%')"
                .to_string(),
        );
    }

    // Optional filters
    if let Some(fp) = file_path {
        conditions.push("s.file_path = ?".to_string());
        param_values.push(Box::new(fp.to_string()));
    }
    if let Some(lang) = language {
        conditions.push("s.language = ?".to_string());
        param_values.push(Box::new(lang.to_string()));
    }
    if let Some(k) = kind {
        conditions.push("s.kind = ?".to_string());
        param_values.push(Box::new(k.to_string()));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        r#"
SELECT s.id, s.file_path, s.language, s.kind, s.name, s.exported, s.start_byte, s.end_byte, s.start_line, s.end_line, s.text
FROM symbols s
LEFT JOIN edges e ON s.id = e.to_symbol_id
{}
  AND e.to_symbol_id IS NULL
ORDER BY s.exported DESC, s.file_path ASC, s.start_line ASC
LIMIT ?
"#,
        where_clause
    );

    let mut stmt = conn
        .prepare(&sql)
        .context("Failed to prepare find_dead_symbols")?;

    let mut param_refs: Vec<&dyn rusqlite::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let limit_i64 = limit as i64;
    param_refs.push(&limit_i64);

    let mut rows = stmt.query(param_refs.as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SymbolRow {
            id: row.get(0)?,
            file_path: row.get(1)?,
            language: row.get(2)?,
            kind: row.get(3)?,
            name: row.get(4)?,
            exported: row.get(5)?,
            start_byte: row.get::<_, i64>(6)? as u32,
            end_byte: row.get::<_, i64>(7)? as u32,
            start_line: row.get::<_, i64>(8)? as u32,
            end_line: row.get::<_, i64>(9)? as u32,
            text: row.get(10)?,
        });
    }
    Ok(out)
}

fn edge_resolution_rank(resolution: &str) -> i64 {
    match resolution {
        "local" => 3,
        "import" => 2,
        "heuristic" => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod dead_code_tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::storage::sqlite::schema::SCHEMA_SQL)
            .unwrap();
        conn
    }

    fn insert_symbol(
        conn: &Connection,
        id: &str,
        file_path: &str,
        language: &str,
        kind: &str,
        name: &str,
        exported: bool,
    ) {
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 100, 1, 10, '')",
            params![id, file_path, language, kind, name, exported as i32],
        )
        .unwrap();
    }

    fn insert_edge(conn: &Connection, from_id: &str, to_id: &str, edge_type: &str) {
        conn.execute(
            "INSERT INTO edges (from_symbol_id, to_symbol_id, edge_type) VALUES (?1, ?2, ?3)",
            params![from_id, to_id, edge_type],
        )
        .unwrap();
    }

    #[test]
    fn test_list_edges_to_by_type_filters_before_limit() {
        let conn = setup_test_db();
        insert_symbol(
            &conn,
            "target",
            "src/target.rs",
            "rust",
            "function",
            "target",
            false,
        );
        insert_symbol(
            &conn,
            "caller",
            "src/caller.rs",
            "rust",
            "function",
            "caller",
            false,
        );

        for index in 0..60 {
            let from_id = format!("noise_{index:02}");
            insert_symbol(
                &conn,
                &from_id,
                "src/noise.rs",
                "rust",
                "function",
                &from_id,
                false,
            );
            insert_edge(&conn, &from_id, "target", "aaa_noise");
        }
        insert_edge(&conn, "caller", "target", "call");

        let edges = list_edges_to_by_type(&conn, "target", "call", 1).unwrap();

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from_symbol_id, "caller");
        assert_eq!(edges[0].edge_type, "call");
    }

    #[test]
    fn test_sqlite_limit_clamps_usize_max_to_non_negative_i64() {
        assert_eq!(sqlite_limit(usize::MAX), i64::MAX);
    }

    #[test]
    fn changed_file_cleanup_removes_only_source_owned_edges() {
        let conn = setup_test_db();
        insert_symbol(
            &conn,
            "changed",
            "src/changed.rs",
            "rust",
            "function",
            "changed",
            false,
        );
        insert_symbol(
            &conn,
            "target",
            "src/target.rs",
            "rust",
            "function",
            "target",
            false,
        );
        insert_symbol(
            &conn,
            "caller",
            "src/caller.rs",
            "rust",
            "function",
            "caller",
            false,
        );
        insert_edge(&conn, "changed", "target", "call");
        insert_edge(&conn, "caller", "changed", "call");

        assert_eq!(
            delete_outgoing_edges_by_file(&conn, "src/changed.rs").unwrap(),
            1
        );
        assert!(list_edges_from(&conn, "changed", 10).unwrap().is_empty());
        assert_eq!(
            list_edges_to(&conn, "changed", 10).unwrap().len(),
            1,
            "incoming edges survive while the stable target ID is replaced"
        );
    }

    #[test]
    fn graph_integrity_rejects_dataflow_location_outside_owner() {
        let conn = setup_test_db();
        insert_symbol(
            &conn,
            "source",
            "src/lib.rs",
            "rust",
            "function",
            "source",
            false,
        );
        insert_symbol(
            &conn,
            "target",
            "src/lib.rs",
            "rust",
            "function",
            "target",
            false,
        );
        conn.execute(
            r#"
INSERT INTO edges(from_symbol_id, to_symbol_id, edge_type, at_file, at_line)
VALUES ('source', 'target', 'reads', 'src/lib.rs', 99)
"#,
            [],
        )
        .unwrap();

        let error = validate_graph_integrity(&conn).unwrap_err().to_string();
        assert!(error.contains("outside their source symbol"), "{error}");
    }

    #[test]
    fn test_find_dead_symbols_returns_unreferenced() {
        let conn = setup_test_db();
        // A calls B, C has no edges at all
        insert_symbol(
            &conn,
            "a",
            "src/lib.rs",
            "rust",
            "function",
            "func_a",
            false,
        );
        insert_symbol(
            &conn,
            "b",
            "src/lib.rs",
            "rust",
            "function",
            "func_b",
            false,
        );
        insert_symbol(
            &conn,
            "c",
            "src/lib.rs",
            "rust",
            "function",
            "func_c",
            false,
        );
        insert_edge(&conn, "a", "b", "call");

        let dead = find_dead_symbols(&conn, None, None, None, true, 100).unwrap();
        let dead_ids: Vec<&str> = dead.iter().map(|s| s.id.as_str()).collect();

        // B has an incoming edge (a->b), so B is NOT dead
        assert!(
            !dead_ids.contains(&"b"),
            "b has incoming edge, should not be dead"
        );
        // C has no incoming edges, so C IS dead
        assert!(
            dead_ids.contains(&"c"),
            "c has no incoming edges, should be dead"
        );
        // A only has outgoing edges, no incoming, so A IS dead
        assert!(
            dead_ids.contains(&"a"),
            "a has only outgoing edges, should be dead"
        );
    }

    #[test]
    fn test_find_dead_symbols_excludes_file_and_module_kinds() {
        let conn = setup_test_db();
        insert_symbol(&conn, "f1", "src/lib.rs", "rust", "file", "lib.rs", false);
        insert_symbol(&conn, "m1", "src/lib.rs", "rust", "module", "my_mod", false);
        insert_symbol(&conn, "i1", "src/lib.rs", "rust", "impl", "MyStruct", false);
        // Also add a regular function with no edges to confirm it IS returned
        insert_symbol(
            &conn,
            "fn1",
            "src/lib.rs",
            "rust",
            "function",
            "helper",
            false,
        );

        let dead = find_dead_symbols(&conn, None, None, None, true, 100).unwrap();
        let dead_ids: Vec<&str> = dead.iter().map(|s| s.id.as_str()).collect();

        assert!(!dead_ids.contains(&"f1"), "file kind should be excluded");
        assert!(!dead_ids.contains(&"m1"), "module kind should be excluded");
        assert!(!dead_ids.contains(&"i1"), "impl kind should be excluded");
        assert!(
            dead_ids.contains(&"fn1"),
            "regular function should be returned"
        );
    }

    #[test]
    fn test_find_dead_symbols_excludes_framework_entry_points() {
        let conn = setup_test_db();
        insert_symbol(
            &conn,
            "h1",
            "src/routes.rs",
            "rust",
            "function",
            "handle_login",
            true,
        );
        // Insert a matching framework_patterns row
        conn.execute(
            "INSERT INTO framework_patterns (id, file_path, line, framework, kind, name)
             VALUES ('fp1', 'src/routes.rs', 1, 'axum', 'route', 'handle_login')",
            [],
        )
        .unwrap();

        let dead = find_dead_symbols(&conn, None, None, None, true, 100).unwrap();
        let dead_ids: Vec<&str> = dead.iter().map(|s| s.id.as_str()).collect();

        assert!(
            !dead_ids.contains(&"h1"),
            "framework entry point should not be reported as dead"
        );
    }

    #[test]
    fn test_find_dead_symbols_filters_by_file_path() {
        let conn = setup_test_db();
        insert_symbol(
            &conn,
            "a1",
            "src/alpha.rs",
            "rust",
            "function",
            "alpha_fn",
            false,
        );
        insert_symbol(
            &conn,
            "b1",
            "src/beta.rs",
            "rust",
            "function",
            "beta_fn",
            false,
        );

        // Filter to only src/alpha.rs
        let dead = find_dead_symbols(&conn, Some("src/alpha.rs"), None, None, true, 100).unwrap();
        let dead_ids: Vec<&str> = dead.iter().map(|s| s.id.as_str()).collect();

        assert!(
            dead_ids.contains(&"a1"),
            "alpha_fn should be returned for its file"
        );
        assert!(
            !dead_ids.contains(&"b1"),
            "beta_fn should be excluded by file filter"
        );
    }

    #[test]
    fn test_find_dead_symbols_exported_first() {
        let conn = setup_test_db();
        // Insert private first (lower start_line to ensure ordering is by exported, not insertion)
        insert_symbol(
            &conn,
            "priv1",
            "src/lib.rs",
            "rust",
            "function",
            "private_fn",
            false,
        );
        insert_symbol(
            &conn,
            "pub1",
            "src/lib.rs",
            "rust",
            "function",
            "public_fn",
            true,
        );

        let dead = find_dead_symbols(&conn, None, None, None, true, 100).unwrap();
        assert!(dead.len() >= 2, "should have at least 2 dead symbols");

        // Find positions of both symbols
        let pub_pos = dead.iter().position(|s| s.id == "pub1").unwrap();
        let priv_pos = dead.iter().position(|s| s.id == "priv1").unwrap();
        assert!(
            pub_pos < priv_pos,
            "exported symbol should come before private: pub_pos={}, priv_pos={}",
            pub_pos,
            priv_pos
        );
    }
}

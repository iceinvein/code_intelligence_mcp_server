//! Cross-repo edge queries — track references from symbols in this repo to symbols in other repos

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::storage::sqlite::schema::CrossRepoEdgeRow;

/// Insert or update a cross-repo edge (upsert on conflict).
///
/// On conflict, updates confidence and resolution if the new values are higher.
pub fn upsert_cross_repo_edge(conn: &Connection, edge: &CrossRepoEdgeRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO cross_repo_edges(from_symbol_id, to_repo_hash, to_symbol_name, to_symbol_file,
    to_symbol_id, edge_type, at_file, at_line, confidence, resolution)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
ON CONFLICT(from_symbol_id, to_repo_hash, to_symbol_name, edge_type) DO UPDATE SET
    to_symbol_file = COALESCE(excluded.to_symbol_file, cross_repo_edges.to_symbol_file),
    to_symbol_id = COALESCE(excluded.to_symbol_id, cross_repo_edges.to_symbol_id),
    at_file = COALESCE(excluded.at_file, cross_repo_edges.at_file),
    at_line = COALESCE(excluded.at_line, cross_repo_edges.at_line),
    confidence = MAX(cross_repo_edges.confidence, excluded.confidence),
    resolution = CASE
        WHEN excluded.confidence > cross_repo_edges.confidence THEN excluded.resolution
        ELSE cross_repo_edges.resolution
    END
"#,
        params![
            edge.from_symbol_id,
            edge.to_repo_hash,
            edge.to_symbol_name,
            edge.to_symbol_file,
            edge.to_symbol_id,
            edge.edge_type,
            edge.at_file,
            edge.at_line.map(|v| v as i64),
            edge.confidence,
            edge.resolution,
        ],
    )
    .with_context(|| {
        format!(
            "Failed to upsert cross-repo edge: from={}, to_repo={}, to_name={}, type={}",
            edge.from_symbol_id, edge.to_repo_hash, edge.to_symbol_name, edge.edge_type
        )
    })?;
    Ok(())
}

/// List cross-repo edges originating from a given symbol.
pub fn list_cross_repo_edges_from(
    conn: &Connection,
    from_symbol_id: &str,
    limit: usize,
) -> Result<Vec<CrossRepoEdgeRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT id, from_symbol_id, to_repo_hash, to_symbol_name, to_symbol_file,
       to_symbol_id, edge_type, at_file, at_line, confidence, resolution
FROM cross_repo_edges
WHERE from_symbol_id = ?1
ORDER BY confidence DESC, edge_type ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_cross_repo_edges_from")?;

    let mut rows = stmt.query(params![from_symbol_id, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(read_cross_repo_edge_row(row)?);
    }
    Ok(out)
}

/// List cross-repo edges originating from any symbol in this repo that target a specific repo hash.
pub fn list_cross_repo_edges_to_repo(
    conn: &Connection,
    to_repo_hash: &str,
    limit: usize,
) -> Result<Vec<CrossRepoEdgeRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT id, from_symbol_id, to_repo_hash, to_symbol_name, to_symbol_file,
       to_symbol_id, edge_type, at_file, at_line, confidence, resolution
FROM cross_repo_edges
WHERE to_repo_hash = ?1
ORDER BY confidence DESC, edge_type ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_cross_repo_edges_to_repo")?;

    let mut rows = stmt.query(params![to_repo_hash, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(read_cross_repo_edge_row(row)?);
    }
    Ok(out)
}

/// List all unresolved cross-repo edges targeting a specific repo hash.
///
/// Used for lazy resolution: when a new repo finishes indexing, resolve pending
/// cross-repo edges from other repos that reference it.
pub fn list_unresolved_edges_to_repo(
    conn: &Connection,
    to_repo_hash: &str,
    limit: usize,
) -> Result<Vec<CrossRepoEdgeRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT id, from_symbol_id, to_repo_hash, to_symbol_name, to_symbol_file,
       to_symbol_id, edge_type, at_file, at_line, confidence, resolution
FROM cross_repo_edges
WHERE to_repo_hash = ?1 AND resolution = 'cross_repo_unresolved'
ORDER BY from_symbol_id ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_unresolved_edges_to_repo")?;

    let mut rows = stmt.query(params![to_repo_hash, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(read_cross_repo_edge_row(row)?);
    }
    Ok(out)
}

/// Mark a cross-repo edge as resolved, setting the target symbol ID.
pub fn resolve_cross_repo_edge(
    conn: &Connection,
    edge_id: i64,
    to_symbol_id: &str,
) -> Result<()> {
    conn.execute(
        r#"
UPDATE cross_repo_edges
SET to_symbol_id = ?1, resolution = 'cross_repo_resolved'
WHERE id = ?2
"#,
        params![to_symbol_id, edge_id],
    )
    .with_context(|| {
        format!(
            "Failed to resolve cross-repo edge: id={}, to_symbol_id={}",
            edge_id, to_symbol_id
        )
    })?;
    Ok(())
}

/// Count total cross-repo edges in this repo's database.
pub fn count_cross_repo_edges(conn: &Connection) -> Result<u64> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cross_repo_edges",
            [],
            |row| row.get(0),
        )
        .context("Failed to count cross-repo edges")?;
    Ok(count.max(0) as u64)
}

/// Helper to read a CrossRepoEdgeRow from a rusqlite Row.
fn read_cross_repo_edge_row(row: &rusqlite::Row<'_>) -> Result<CrossRepoEdgeRow> {
    Ok(CrossRepoEdgeRow {
        id: row.get(0)?,
        from_symbol_id: row.get(1)?,
        to_repo_hash: row.get(2)?,
        to_symbol_name: row.get(3)?,
        to_symbol_file: row.get(4)?,
        to_symbol_id: row.get(5)?,
        edge_type: row.get(6)?,
        at_file: row.get(7)?,
        at_line: row
            .get::<_, Option<i64>>(8)?
            .and_then(|v| u32::try_from(v).ok()),
        confidence: row.get::<_, f64>(9)? as f32,
        resolution: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::storage::sqlite::schema::SCHEMA_SQL)
            .unwrap();
        conn
    }

    fn insert_test_symbol(conn: &Connection, id: &str, name: &str) {
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES (?1, 'src/lib.rs', 'rust', 'function', ?2, 1, 0, 100, 1, 10, '')",
            params![id, name],
        )
        .unwrap();
    }

    fn make_edge(from_id: &str, to_repo: &str, to_name: &str, edge_type: &str) -> CrossRepoEdgeRow {
        CrossRepoEdgeRow {
            id: 0, // auto-increment
            from_symbol_id: from_id.to_string(),
            to_repo_hash: to_repo.to_string(),
            to_symbol_name: to_name.to_string(),
            to_symbol_file: None,
            to_symbol_id: None,
            edge_type: edge_type.to_string(),
            at_file: Some("src/lib.rs".to_string()),
            at_line: Some(42),
            confidence: 0.5,
            resolution: "cross_repo_unresolved".to_string(),
        }
    }

    #[test]
    fn insert_and_list_cross_repo_edges() {
        let conn = setup_test_db();
        insert_test_symbol(&conn, "sym1", "my_function");

        let edge = make_edge("sym1", "abc123def456abcd", "RemoteType", "reference");
        upsert_cross_repo_edge(&conn, &edge).unwrap();

        let edges = list_cross_repo_edges_from(&conn, "sym1", 10).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from_symbol_id, "sym1");
        assert_eq!(edges[0].to_repo_hash, "abc123def456abcd");
        assert_eq!(edges[0].to_symbol_name, "RemoteType");
        assert_eq!(edges[0].edge_type, "reference");
        assert_eq!(edges[0].resolution, "cross_repo_unresolved");
    }

    #[test]
    fn upsert_updates_on_conflict() {
        let conn = setup_test_db();
        insert_test_symbol(&conn, "sym1", "my_function");

        let edge = make_edge("sym1", "abc123def456abcd", "RemoteType", "reference");
        upsert_cross_repo_edge(&conn, &edge).unwrap();

        // Upsert again with higher confidence
        let mut edge2 = make_edge("sym1", "abc123def456abcd", "RemoteType", "reference");
        edge2.confidence = 0.9;
        edge2.resolution = "cross_repo_resolved".to_string();
        edge2.to_symbol_id = Some("remote_sym_123".to_string());
        upsert_cross_repo_edge(&conn, &edge2).unwrap();

        let edges = list_cross_repo_edges_from(&conn, "sym1", 10).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].confidence, 0.9);
        assert_eq!(edges[0].resolution, "cross_repo_resolved");
        assert_eq!(edges[0].to_symbol_id, Some("remote_sym_123".to_string()));
    }

    #[test]
    fn list_edges_to_repo() {
        let conn = setup_test_db();
        insert_test_symbol(&conn, "sym1", "func_a");
        insert_test_symbol(&conn, "sym2", "func_b");

        let e1 = make_edge("sym1", "repo_hash_aaa", "TypeA", "reference");
        let e2 = make_edge("sym2", "repo_hash_aaa", "TypeB", "call");
        let e3 = make_edge("sym1", "repo_hash_bbb", "TypeC", "reference");
        upsert_cross_repo_edge(&conn, &e1).unwrap();
        upsert_cross_repo_edge(&conn, &e2).unwrap();
        upsert_cross_repo_edge(&conn, &e3).unwrap();

        let edges = list_cross_repo_edges_to_repo(&conn, "repo_hash_aaa", 10).unwrap();
        assert_eq!(edges.len(), 2);

        // Only 1 edge to repo_hash_bbb
        let edges_b = list_cross_repo_edges_to_repo(&conn, "repo_hash_bbb", 10).unwrap();
        assert_eq!(edges_b.len(), 1);
    }

    #[test]
    fn list_unresolved_and_resolve() {
        let conn = setup_test_db();
        insert_test_symbol(&conn, "sym1", "func_a");

        let edge = make_edge("sym1", "repo_hash_aaa", "TypeA", "reference");
        upsert_cross_repo_edge(&conn, &edge).unwrap();

        // Should show up as unresolved
        let unresolved = list_unresolved_edges_to_repo(&conn, "repo_hash_aaa", 10).unwrap();
        assert_eq!(unresolved.len(), 1);
        let edge_id = unresolved[0].id;

        // Resolve it
        resolve_cross_repo_edge(&conn, edge_id, "resolved_sym_id").unwrap();

        // Should no longer be unresolved
        let unresolved_after = list_unresolved_edges_to_repo(&conn, "repo_hash_aaa", 10).unwrap();
        assert!(unresolved_after.is_empty());

        // But still in the full list
        let all = list_cross_repo_edges_from(&conn, "sym1", 10).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].resolution, "cross_repo_resolved");
        assert_eq!(all[0].to_symbol_id, Some("resolved_sym_id".to_string()));
    }

    #[test]
    fn count_cross_repo_edges_works() {
        let conn = setup_test_db();
        insert_test_symbol(&conn, "sym1", "func_a");

        assert_eq!(count_cross_repo_edges(&conn).unwrap(), 0);

        upsert_cross_repo_edge(&conn, &make_edge("sym1", "repo1", "A", "call")).unwrap();
        upsert_cross_repo_edge(&conn, &make_edge("sym1", "repo2", "B", "reference")).unwrap();

        assert_eq!(count_cross_repo_edges(&conn).unwrap(), 2);
    }

    #[test]
    fn cascade_delete_removes_cross_repo_edges() {
        let conn = setup_test_db();
        insert_test_symbol(&conn, "sym1", "func_a");

        upsert_cross_repo_edge(&conn, &make_edge("sym1", "repo1", "A", "call")).unwrap();
        assert_eq!(count_cross_repo_edges(&conn).unwrap(), 1);

        // Delete the symbol — CASCADE should remove cross-repo edges
        conn.execute("DELETE FROM symbols WHERE id = 'sym1'", [])
            .unwrap();
        assert_eq!(count_cross_repo_edges(&conn).unwrap(), 0);
    }
}

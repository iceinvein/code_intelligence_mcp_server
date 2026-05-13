use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::{SimilarityClusterRow, SymbolRow, UsageExampleRow};

pub fn upsert_similarity_cluster(conn: &Connection, row: &SimilarityClusterRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO similarity_clusters(symbol_id, cluster_key, updated_at)
VALUES (?1, ?2, unixepoch())
ON CONFLICT(symbol_id) DO UPDATE SET
  cluster_key=excluded.cluster_key,
  updated_at=unixepoch()
"#,
        params![row.symbol_id, row.cluster_key],
    )
    .context("Failed to upsert similarity cluster")?;
    Ok(())
}

pub fn clear_similarity_clusters(conn: &Connection) -> Result<u64> {
    conn.execute("DELETE FROM similarity_clusters", [])
        .context("Failed to clear similarity_clusters")?;
    Ok(conn.changes())
}

pub fn get_similarity_cluster_key(conn: &Connection, symbol_id: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT cluster_key FROM similarity_clusters WHERE symbol_id = ?1",
        params![symbol_id],
        |row| row.get(0),
    )
    .optional()
    .context("Failed to query similarity cluster key")
}

pub fn list_symbols_in_cluster(
    conn: &Connection,
    cluster_key: &str,
    limit: usize,
) -> Result<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT s.id, s.name
FROM similarity_clusters c
JOIN symbols s ON s.id = c.symbol_id
WHERE c.cluster_key = ?1
ORDER BY s.name ASC, s.file_path ASC, s.kind ASC, s.id ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_symbols_in_cluster")?;
    let mut rows = stmt.query(params![cluster_key, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push((row.get(0)?, row.get(1)?));
    }
    Ok(out)
}

pub fn delete_usage_examples_by_file(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM usage_examples WHERE file_path = ?1",
        params![file_path],
    )
    .with_context(|| format!("Failed to delete usage examples for file: {file_path}"))?;
    Ok(())
}

pub fn upsert_usage_example(conn: &Connection, example: &UsageExampleRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO usage_examples(
  to_symbol_id, from_symbol_id, example_type, file_path, line, snippet
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(to_symbol_id, example_type, file_path, line, snippet) DO NOTHING
"#,
        params![
            example.to_symbol_id,
            example.from_symbol_id,
            example.example_type,
            example.file_path,
            example.line.map(|v| v as i64),
            example.snippet
        ],
    )
    .context("Failed to upsert usage example")?;
    Ok(())
}

pub fn batch_upsert_usage_examples(conn: &Connection, examples: &[UsageExampleRow]) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        r#"
INSERT INTO usage_examples(
  to_symbol_id, from_symbol_id, example_type, file_path, line, snippet
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(to_symbol_id, example_type, file_path, line, snippet) DO NOTHING
"#,
    )?;
    for ex in examples {
        stmt.execute(params![
            ex.to_symbol_id,
            ex.from_symbol_id,
            ex.example_type,
            ex.file_path,
            ex.line.map(|v| v as i64),
            ex.snippet
        ])
        .context("Failed to batch upsert usage example")?;
    }
    Ok(())
}

pub fn list_usage_examples_for_symbol(
    conn: &Connection,
    to_symbol_id: &str,
    limit: usize,
) -> Result<Vec<UsageExampleRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  to_symbol_id, from_symbol_id, example_type, file_path, line, snippet
FROM usage_examples
WHERE to_symbol_id = ?1
ORDER BY example_type ASC, file_path ASC, line ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare list_usage_examples_for_symbol")?;

    let mut rows = stmt.query(params![to_symbol_id, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(UsageExampleRow {
            to_symbol_id: row.get(0)?,
            from_symbol_id: row.get(1)?,
            example_type: row.get(2)?,
            file_path: row.get(3)?,
            line: row
                .get::<_, Option<i64>>(4)?
                .and_then(|v| u32::try_from(v).ok()),
            snippet: row.get(5)?,
        });
    }
    Ok(out)
}

/// A cluster member with symbol details, used by `list_cluster_members_with_details`.
#[derive(Debug, Clone)]
pub struct ClusterMemberRow {
    pub symbol_id: String,
    pub name: String,
    pub file_path: String,
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    pub exported: bool,
}

/// List cluster keys that have 2+ members, ordered by member count descending.
pub fn list_duplicate_clusters(conn: &Connection, limit: usize) -> Result<Vec<(String, usize)>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT cluster_key, COUNT(*) as member_count
FROM similarity_clusters
WHERE cluster_key != '__skipped__'
GROUP BY cluster_key
HAVING COUNT(*) >= 2
ORDER BY member_count DESC
LIMIT ?1
"#,
        )
        .context("Failed to prepare list_duplicate_clusters")?;
    let mut rows = stmt.query(params![limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        out.push((key, count as usize));
    }
    Ok(out)
}

/// List members of a specific cluster with full symbol details.
pub fn list_cluster_members_with_details(
    conn: &Connection,
    cluster_key: &str,
) -> Result<Vec<ClusterMemberRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT s.id, s.name, s.file_path, s.kind, s.start_line, s.end_line, s.exported
FROM similarity_clusters c
JOIN symbols s ON s.id = c.symbol_id
WHERE c.cluster_key = ?1
ORDER BY s.file_path ASC, s.name ASC
"#,
        )
        .context("Failed to prepare list_cluster_members_with_details")?;
    let mut rows = stmt.query(params![cluster_key])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(ClusterMemberRow {
            symbol_id: row.get(0)?,
            name: row.get(1)?,
            file_path: row.get(2)?,
            kind: row.get(3)?,
            start_line: row.get(4)?,
            end_line: row.get(5)?,
            exported: row.get::<_, i64>(6)? != 0,
        });
    }
    Ok(out)
}

/// List symbols that don't have similarity clusters (i.e., no embeddings generated yet)
///
/// This is used to find symbols that need embeddings after parallel indexing.
pub fn list_symbols_without_similarity_clusters(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT
  s.id, s.file_path, s.language, s.kind, s.name, s.exported,
  s.start_byte, s.end_byte, s.start_line, s.end_line, s.text
FROM symbols s
LEFT JOIN similarity_clusters c ON s.id = c.symbol_id
WHERE c.symbol_id IS NULL
  AND s.kind != 'file'
ORDER BY s.exported DESC, (s.end_line - s.start_line) DESC
LIMIT ?1
"#,
        )
        .context("Failed to prepare list_symbols_without_similarity_clusters")?;

    let mut rows = stmt.query(params![limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SymbolRow {
            id: row.get(0)?,
            file_path: row.get(1)?,
            language: row.get(2)?,
            kind: row.get(3)?,
            name: row.get(4)?,
            exported: row.get::<_, i64>(5)? != 0,
            start_byte: row.get(6)?,
            end_byte: row.get(7)?,
            start_line: row.get(8)?,
            end_line: row.get(9)?,
            text: row.get(10)?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::storage::sqlite::schema::SCHEMA_SQL)
            .unwrap();
        conn
    }

    fn insert_symbol(conn: &Connection, id: &str, file_path: &str, kind: &str, name: &str) {
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES (?1, ?2, 'rust', ?3, ?4, 1, 0, 100, 1, 10, '')",
            params![id, file_path, kind, name],
        )
        .unwrap();
    }

    fn insert_cluster(conn: &Connection, symbol_id: &str, cluster_key: &str) {
        upsert_similarity_cluster(
            conn,
            &SimilarityClusterRow {
                symbol_id: symbol_id.to_string(),
                cluster_key: cluster_key.to_string(),
            },
        )
        .unwrap();
    }

    #[test]
    fn list_duplicate_clusters_returns_only_multi_member() {
        let conn = setup_test_db();
        insert_symbol(&conn, "a", "src/a.rs", "function", "parse_config");
        insert_symbol(&conn, "b", "src/b.rs", "function", "parse_settings");
        insert_symbol(&conn, "c", "src/c.rs", "function", "lonely_func");

        insert_cluster(&conn, "a", "cluster_1");
        insert_cluster(&conn, "b", "cluster_1");
        insert_cluster(&conn, "c", "cluster_2"); // singleton

        let clusters = list_duplicate_clusters(&conn, 100).unwrap();
        assert_eq!(clusters.len(), 1, "Only clusters with 2+ members");
        assert_eq!(clusters[0].0, "cluster_1");
        assert_eq!(clusters[0].1, 2);
    }

    #[test]
    fn list_duplicate_clusters_orders_by_count_desc() {
        let conn = setup_test_db();
        insert_symbol(&conn, "a", "src/a.rs", "function", "fn_a");
        insert_symbol(&conn, "b", "src/b.rs", "function", "fn_b");
        insert_symbol(&conn, "c", "src/c.rs", "function", "fn_c");
        insert_symbol(&conn, "d", "src/d.rs", "function", "fn_d");
        insert_symbol(&conn, "e", "src/e.rs", "function", "fn_e");

        insert_cluster(&conn, "a", "small_cluster");
        insert_cluster(&conn, "b", "small_cluster");
        insert_cluster(&conn, "c", "big_cluster");
        insert_cluster(&conn, "d", "big_cluster");
        insert_cluster(&conn, "e", "big_cluster");

        let clusters = list_duplicate_clusters(&conn, 100).unwrap();
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].0, "big_cluster");
        assert_eq!(clusters[0].1, 3);
        assert_eq!(clusters[1].0, "small_cluster");
        assert_eq!(clusters[1].1, 2);
    }

    #[test]
    fn list_duplicate_clusters_respects_limit() {
        let conn = setup_test_db();
        for i in 0..6 {
            let id = format!("s{}", i);
            let cluster = format!("c{}", i / 2);
            insert_symbol(&conn, &id, &format!("src/{}.rs", id), "function", &id);
            insert_cluster(&conn, &id, &cluster);
        }

        let clusters = list_duplicate_clusters(&conn, 1).unwrap();
        assert_eq!(clusters.len(), 1);
    }

    #[test]
    fn list_cluster_members_with_details_returns_symbol_info() {
        let conn = setup_test_db();
        insert_symbol(&conn, "a", "src/a.rs", "function", "parse_config");
        insert_symbol(&conn, "b", "src/b.rs", "function", "parse_settings");

        insert_cluster(&conn, "a", "cluster_1");
        insert_cluster(&conn, "b", "cluster_1");

        let members = list_cluster_members_with_details(&conn, "cluster_1").unwrap();
        assert_eq!(members.len(), 2);

        // Ordered by file_path ASC, name ASC
        assert_eq!(members[0].name, "parse_config");
        assert_eq!(members[0].file_path, "src/a.rs");
        assert_eq!(members[0].kind, "function");
        assert_eq!(members[1].name, "parse_settings");
        assert_eq!(members[1].file_path, "src/b.rs");
    }

    #[test]
    fn list_cluster_members_empty_for_unknown_key() {
        let conn = setup_test_db();
        let members = list_cluster_members_with_details(&conn, "nonexistent").unwrap();
        assert!(members.is_empty());
    }

    #[test]
    fn list_duplicate_clusters_excludes_skipped() {
        let conn = setup_test_db();
        insert_symbol(&conn, "a", "src/a.rs", "function", "fn_a");
        insert_symbol(&conn, "b", "src/b.rs", "function", "fn_b");
        insert_symbol(&conn, "c", "src/c.rs", "file", "c.rs");
        insert_symbol(&conn, "d", "src/d.rs", "file", "d.rs");

        // Real cluster with 2 members
        insert_cluster(&conn, "a", "real_cluster");
        insert_cluster(&conn, "b", "real_cluster");
        // __skipped__ cluster with 2 members — should be excluded
        insert_cluster(&conn, "c", "__skipped__");
        insert_cluster(&conn, "d", "__skipped__");

        let clusters = list_duplicate_clusters(&conn, 100).unwrap();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].0, "real_cluster");
    }
}

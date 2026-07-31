use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::FileFingerprintRow;

/// Ensure the persisted graph was produced by the current extraction format.
///
/// A version change removes graph rows and fingerprints atomically. Symbols,
/// vectors, and search telemetry remain available while the next index run
/// rebuilds every source-owned graph fact from source.
pub fn ensure_graph_index_version(conn: &Connection, current_version: &str) -> Result<bool> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT value FROM index_metadata WHERE key = 'graph_index_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("Failed to read graph index version")?;

    if existing.as_deref() == Some(current_version) {
        return Ok(false);
    }

    let tx = conn
        .unchecked_transaction()
        .context("Failed to start graph index version transaction")?;
    tx.execute("DELETE FROM edge_evidence", [])
        .context("Failed to clear edge evidence for graph format change")?;
    tx.execute("DELETE FROM edges", [])
        .context("Failed to clear edges for graph format change")?;
    tx.execute("DELETE FROM data_flow_facts", [])
        .context("Failed to clear data-flow facts for graph format change")?;
    tx.execute("DELETE FROM module_bindings", [])
        .context("Failed to clear module bindings for graph format change")?;
    tx.execute("DELETE FROM file_fingerprints", [])
        .context("Failed to clear fingerprints for graph format change")?;
    tx.execute(
        r#"
INSERT INTO index_metadata(key, value, updated_at)
VALUES ('graph_index_version', ?1, unixepoch())
ON CONFLICT(key) DO UPDATE SET
  value=excluded.value,
  updated_at=excluded.updated_at
"#,
        params![current_version],
    )
    .context("Failed to persist graph index version")?;
    tx.commit()
        .context("Failed to commit graph index version transaction")?;
    Ok(true)
}

pub fn get_file_fingerprint(
    conn: &Connection,
    file_path: &str,
) -> Result<Option<FileFingerprintRow>> {
    conn.query_row(
        r#"
SELECT file_path, mtime_ns, size_bytes, content_hash
FROM file_fingerprints
WHERE file_path = ?1
"#,
        params![file_path],
        |row| {
            Ok(FileFingerprintRow {
                file_path: row.get(0)?,
                mtime_ns: row.get(1)?,
                size_bytes: row.get::<_, i64>(2)?.max(0) as u64,
                content_hash: row.get(3)?,
            })
        },
    )
    .optional()
    .context("Failed to query file fingerprint")
}

pub fn upsert_file_fingerprint(
    conn: &Connection,
    file_path: &str,
    mtime_ns: i64,
    size_bytes: u64,
    content_hash: Option<&str>,
) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO file_fingerprints(file_path, mtime_ns, size_bytes, content_hash, updated_at)
VALUES (?1, ?2, ?3, ?4, unixepoch())
ON CONFLICT(file_path) DO UPDATE SET
  mtime_ns=excluded.mtime_ns,
  size_bytes=excluded.size_bytes,
  content_hash=excluded.content_hash,
  updated_at=unixepoch()
"#,
        params![file_path, mtime_ns, size_bytes as i64, content_hash],
    )
    .context("Failed to upsert file fingerprint")?;
    Ok(())
}

pub fn delete_file_fingerprint(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM file_fingerprints WHERE file_path = ?1",
        params![file_path],
    )
    .with_context(|| format!("Failed to delete file fingerprint for {file_path}"))?;
    Ok(())
}

pub fn clear_all_file_fingerprints(conn: &Connection) -> Result<u64> {
    let count = conn
        .execute("DELETE FROM file_fingerprints", [])
        .context("Failed to clear file_fingerprints table")?;
    Ok(count as u64)
}

pub fn list_all_file_fingerprints(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<FileFingerprintRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT file_path, mtime_ns, size_bytes, content_hash
FROM file_fingerprints
ORDER BY file_path ASC
LIMIT ?1
"#,
        )
        .context("Failed to prepare list_all_file_fingerprints")?;

    let mut rows = stmt.query(params![limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(FileFingerprintRow {
            file_path: row.get(0)?,
            mtime_ns: row.get(1)?,
            size_bytes: row.get::<_, i64>(2)?.max(0) as u64,
            content_hash: row.get(3)?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::schema::SCHEMA_SQL;

    #[test]
    fn graph_version_change_clears_edges_and_forces_source_reparse() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();

        assert!(ensure_graph_index_version(&conn, "1").unwrap());

        for (id, name) in [("source", "source"), ("target", "target")] {
            conn.execute(
                r#"
INSERT INTO symbols(
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
)
VALUES (?1, 'src/lib.rs', 'rust', 'function', ?2, 0, 0, 10, 1, 2, 'fn f() {}')
"#,
                params![id, name],
            )
            .unwrap();
        }
        conn.execute(
            r#"
INSERT INTO edges(from_symbol_id, to_symbol_id, edge_type, at_file, at_line)
VALUES ('source', 'target', 'reads', 'src/lib.rs', 1)
"#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"
INSERT INTO data_flow_facts(
  owner_symbol_id, entity_name, entity_kind, access_kind, at_file, at_line
)
VALUES ('source', 'local_value', 'value', 'read', 'src/lib.rs', 1)
"#,
            [],
        )
        .unwrap();
        upsert_file_fingerprint(&conn, "src/lib.rs", 123, 456, None).unwrap();

        assert!(!ensure_graph_index_version(&conn, "1").unwrap());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );

        assert!(ensure_graph_index_version(&conn, "2").unwrap());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM file_fingerprints", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM data_flow_facts", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2,
            "symbol/search data remains available until the rebuild replaces it"
        );
    }

    #[test]
    fn fingerprint_round_trips_content_hash() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();

        upsert_file_fingerprint(&conn, "src/lib.rs", 123, 456, Some("abc123")).unwrap();
        let row = get_file_fingerprint(&conn, "src/lib.rs").unwrap().unwrap();
        assert_eq!(row.mtime_ns, 123);
        assert_eq!(row.size_bytes, 456);
        assert_eq!(row.content_hash.as_deref(), Some("abc123"));

        // A later upsert replaces the hash rather than keeping the stale one.
        upsert_file_fingerprint(&conn, "src/lib.rs", 789, 456, Some("def456")).unwrap();
        let row = get_file_fingerprint(&conn, "src/lib.rs").unwrap().unwrap();
        assert_eq!(row.mtime_ns, 789);
        assert_eq!(row.content_hash.as_deref(), Some("def456"));

        // A NULL hash is representable, which is what legacy rows carry.
        upsert_file_fingerprint(&conn, "src/other.rs", 1, 2, None).unwrap();
        let row = get_file_fingerprint(&conn, "src/other.rs")
            .unwrap()
            .unwrap();
        assert_eq!(row.content_hash, None);

        let all = list_all_file_fingerprints(&conn, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].file_path, "src/lib.rs");
        assert_eq!(all[0].content_hash.as_deref(), Some("def456"));
    }
}

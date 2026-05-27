//! Cached aggregate stats for the dashboard.
//!
//! `/api/repos/:id` used to run four COUNT(*) scans and a MAX(updated_at)
//! scan on every request, which becomes seconds on multi-million-row
//! tables (e.g. the wolfmax repo's ~6M edges). We instead refresh a
//! single-row `repo_stats` table at the end of every indexing run and
//! serve the dashboard from that.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{descriptions, edges, symbols};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedRepoStats {
    pub symbols: u64,
    pub edges: u64,
    pub descriptions: u64,
    pub undescribed_symbols: u64,
    pub last_updated_unix_s: Option<i64>,
    pub computed_at_unix_s: i64,
}

/// Read the cached stats row, if it has ever been written.
pub fn read_cached(conn: &Connection) -> Result<Option<CachedRepoStats>> {
    conn.query_row(
        r#"
SELECT symbols, edges, descriptions, undescribed_symbols,
       last_updated_unix_s, computed_at_unix_s
FROM repo_stats WHERE id = 1
"#,
        [],
        |row| {
            Ok(CachedRepoStats {
                symbols: u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                edges: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                descriptions: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                undescribed_symbols: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                last_updated_unix_s: row.get::<_, Option<i64>>(4)?,
                computed_at_unix_s: row.get::<_, i64>(5)?,
            })
        },
    )
    .optional()
    .context("Failed to read repo_stats cache")
}

/// Run the live counts and UPSERT them into `repo_stats`. Returns the
/// freshly written snapshot. Safe to call concurrently with reads
/// because the UPSERT is a single statement.
pub fn recompute(conn: &Connection) -> Result<CachedRepoStats> {
    let symbols = symbols::count_symbols(conn)?;
    let edges = edges::count_edges(conn)?;
    let descriptions = descriptions::count_descriptions(conn)? as u64;
    let undescribed = descriptions::count_undescribed_symbols(conn)? as u64;
    let last_updated = symbols::most_recent_symbol_update(conn)?;
    let computed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    conn.execute(
        r#"
INSERT INTO repo_stats(
  id, symbols, edges, descriptions, undescribed_symbols,
  last_updated_unix_s, computed_at_unix_s
)
VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(id) DO UPDATE SET
  symbols = excluded.symbols,
  edges = excluded.edges,
  descriptions = excluded.descriptions,
  undescribed_symbols = excluded.undescribed_symbols,
  last_updated_unix_s = excluded.last_updated_unix_s,
  computed_at_unix_s = excluded.computed_at_unix_s
"#,
        params![
            symbols as i64,
            edges as i64,
            descriptions as i64,
            undescribed as i64,
            last_updated,
            computed_at,
        ],
    )
    .context("Failed to upsert repo_stats")?;

    Ok(CachedRepoStats {
        symbols,
        edges,
        descriptions,
        undescribed_symbols: undescribed,
        last_updated_unix_s: last_updated,
        computed_at_unix_s: computed_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::schema::SCHEMA_SQL;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn
    }

    fn insert_symbol(conn: &Connection, id: &str, name: &str, kind: &str) {
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
             VALUES (?1, 'a.ts', 'typescript', ?2, ?3, 0, 0, 0, 0, 0, '')",
            params![id, kind, name],
        )
        .unwrap();
    }

    #[test]
    fn read_cached_returns_none_when_table_empty() {
        let conn = setup();
        assert!(read_cached(&conn).unwrap().is_none());
    }

    #[test]
    fn recompute_writes_counts_to_cache() {
        let conn = setup();
        insert_symbol(&conn, "s1", "foo", "function");
        insert_symbol(&conn, "s2", "bar", "function");
        insert_symbol(&conn, "s3", "f.ts", "file");

        let snap = recompute(&conn).unwrap();
        assert_eq!(snap.symbols, 3);
        assert_eq!(snap.edges, 0);
        assert_eq!(snap.descriptions, 0);
        // Only kind != 'file' counts toward undescribed.
        assert_eq!(snap.undescribed_symbols, 2);

        let cached = read_cached(&conn).unwrap().expect("row written");
        assert_eq!(cached, snap);
    }

    #[test]
    fn recompute_upserts_existing_row() {
        let conn = setup();
        insert_symbol(&conn, "s1", "foo", "function");
        let first = recompute(&conn).unwrap();
        assert_eq!(first.symbols, 1);

        insert_symbol(&conn, "s2", "bar", "function");
        let second = recompute(&conn).unwrap();
        assert_eq!(second.symbols, 2);

        // Still exactly one row in repo_stats (PRIMARY KEY CHECK enforces it,
        // but verify defensively).
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM repo_stats", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn recompute_captures_most_recent_symbol_update() {
        let conn = setup();
        conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text, updated_at)
             VALUES ('s1', 'a.ts', 'typescript', 'function', 'foo', 0, 0, 0, 0, 0, '', 1234567890)",
            [],
        ).unwrap();

        let snap = recompute(&conn).unwrap();
        assert_eq!(snap.last_updated_unix_s, Some(1234567890));
    }
}

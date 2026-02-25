//! CRUD operations for co_changes table (change impact prediction)

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::storage::sqlite::schema::CoChangeRow;

/// Insert or replace a co-change pair with computed confidence.
pub fn upsert_co_change(
    conn: &Connection,
    file_a: &str,
    file_b: &str,
    co_change_count: u32,
    total_a: u32,
    total_b: u32,
) -> Result<()> {
    let min_total = total_a.min(total_b).max(1);
    let confidence = (co_change_count as f32 / min_total as f32).min(1.0);

    conn.execute(
        r#"
INSERT OR REPLACE INTO co_changes (file_a, file_b, co_change_count, total_commits_a, total_commits_b, confidence)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
"#,
        params![
            file_a,
            file_b,
            co_change_count as i64,
            total_a as i64,
            total_b as i64,
            confidence as f64,
        ],
    )
    .with_context(|| {
        format!(
            "Failed to upsert co_change: file_a={}, file_b={}",
            file_a, file_b
        )
    })?;
    Ok(())
}

/// Get top co-changed files for a given file path, ordered by confidence DESC.
pub fn get_co_changes_for_file(
    conn: &Connection,
    file_path: &str,
    limit: usize,
) -> Result<Vec<CoChangeRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT file_a, file_b, co_change_count, total_commits_a, total_commits_b, confidence
FROM co_changes
WHERE file_a = ?1 OR file_b = ?1
ORDER BY confidence DESC
LIMIT ?2
"#,
        )
        .context("Failed to prepare get_co_changes_for_file")?;

    let mut rows = stmt.query(params![file_path, limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(CoChangeRow {
            file_a: row.get(0)?,
            file_b: row.get(1)?,
            co_change_count: row.get::<_, i64>(2)? as u32,
            total_commits_a: row.get::<_, i64>(3)? as u32,
            total_commits_b: row.get::<_, i64>(4)? as u32,
            confidence: row.get::<_, f64>(5)? as f32,
        });
    }
    Ok(out)
}

/// Delete all rows from co_changes table (for rebuild).
pub fn clear_co_changes(conn: &Connection) -> Result<u64> {
    let count = conn
        .execute("DELETE FROM co_changes", [])
        .context("Failed to clear co_changes")?;
    Ok(count as u64)
}

/// Count total rows in co_changes table.
pub fn count_co_changes(conn: &Connection) -> Result<u64> {
    conn.query_row("SELECT COUNT(*) FROM co_changes", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|c| c as u64)
    .context("Failed to count co_changes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::storage::sqlite::schema::SCHEMA_SQL)
            .unwrap();
        conn
    }

    #[test]
    fn test_upsert_and_get_co_changes() {
        let conn = setup_db();

        upsert_co_change(&conn, "src/a.rs", "src/b.rs", 5, 10, 8).unwrap();
        upsert_co_change(&conn, "src/a.rs", "src/c.rs", 3, 10, 6).unwrap();
        upsert_co_change(&conn, "src/d.rs", "src/a.rs", 2, 4, 10).unwrap();

        let results = get_co_changes_for_file(&conn, "src/a.rs", 10).unwrap();
        assert_eq!(results.len(), 3);
        // Should be ordered by confidence DESC
        assert!(results[0].confidence >= results[1].confidence);
        assert!(results[1].confidence >= results[2].confidence);
    }

    #[test]
    fn test_upsert_replaces_existing() {
        let conn = setup_db();

        upsert_co_change(&conn, "src/a.rs", "src/b.rs", 5, 10, 8).unwrap();
        upsert_co_change(&conn, "src/a.rs", "src/b.rs", 8, 12, 10).unwrap();

        let results = get_co_changes_for_file(&conn, "src/a.rs", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].co_change_count, 8);
    }

    #[test]
    fn test_clear_co_changes() {
        let conn = setup_db();

        upsert_co_change(&conn, "src/a.rs", "src/b.rs", 5, 10, 8).unwrap();
        upsert_co_change(&conn, "src/a.rs", "src/c.rs", 3, 10, 6).unwrap();

        let count = count_co_changes(&conn).unwrap();
        assert_eq!(count, 2);

        clear_co_changes(&conn).unwrap();

        let count = count_co_changes(&conn).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_confidence_calculation() {
        let conn = setup_db();

        // co_change_count=5, min(total_a=10, total_b=8) = 8
        // confidence = 5/8 = 0.625
        upsert_co_change(&conn, "src/a.rs", "src/b.rs", 5, 10, 8).unwrap();

        let results = get_co_changes_for_file(&conn, "src/a.rs", 10).unwrap();
        assert_eq!(results.len(), 1);
        let expected = 5.0_f32 / 8.0;
        assert!((results[0].confidence - expected).abs() < 0.001);
    }

    #[test]
    fn test_count_co_changes() {
        let conn = setup_db();
        assert_eq!(count_co_changes(&conn).unwrap(), 0);

        upsert_co_change(&conn, "src/a.rs", "src/b.rs", 1, 2, 3).unwrap();
        assert_eq!(count_co_changes(&conn).unwrap(), 1);

        upsert_co_change(&conn, "src/c.rs", "src/d.rs", 1, 2, 3).unwrap();
        assert_eq!(count_co_changes(&conn).unwrap(), 2);
    }
}

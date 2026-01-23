//! CRUD operations for user_file_affinity table

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::UserFileAffinityRow;

pub fn upsert_file_affinity(
    conn: &Connection,
    file_path: &str,
    view_increment: u32,
    edit_increment: u32,
) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO user_file_affinity (file_path, view_count, edit_count, last_accessed_at, updated_at)
VALUES (?1, ?2, ?3, unixepoch(), unixepoch())
ON CONFLICT(file_path) DO UPDATE SET
  view_count = view_count + ?2,
  edit_count = edit_count + ?3,
  last_accessed_at = unixepoch(),
  updated_at = unixepoch()
"#,
        params![file_path, view_increment as i64, edit_increment as i64],
    )
    .context("Failed to upsert file affinity")?;
    Ok(())
}

pub fn get_file_affinity(conn: &Connection, file_path: &str) -> Result<Option<UserFileAffinityRow>> {
    conn.query_row(
        r#"
SELECT file_path, view_count, edit_count, last_accessed_at, updated_at
FROM user_file_affinity
WHERE file_path = ?1
"#,
        params![file_path],
        |row| {
            Ok(UserFileAffinityRow {
                file_path: row.get(0)?,
                view_count: row.get::<_, i64>(1)? as u32,
                edit_count: row.get::<_, i64>(2)? as u32,
                last_accessed_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .optional()
    .context("Failed to query file affinity")
}

pub fn get_top_affinity_files(conn: &Connection, limit: usize) -> Result<Vec<UserFileAffinityRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT file_path, view_count, edit_count, last_accessed_at, updated_at
FROM user_file_affinity
ORDER BY (view_count + edit_count * 2) DESC, last_accessed_at DESC
LIMIT ?1
"#,
        )
        .context("Failed to prepare get_top_affinity_files")?;

    let mut rows = stmt.query(params![limit as i64])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(UserFileAffinityRow {
            file_path: row.get(0)?,
            view_count: row.get::<_, i64>(1)? as u32,
            edit_count: row.get::<_, i64>(2)? as u32,
            last_accessed_at: row.get(3)?,
            updated_at: row.get(4)?,
        });
    }
    Ok(out)
}

pub fn delete_file_affinity(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM user_file_affinity WHERE file_path = ?1",
        params![file_path],
    )
    .context("Failed to delete file affinity")?;
    Ok(())
}

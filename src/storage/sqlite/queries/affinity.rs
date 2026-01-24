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

/// Batch query file affinity boost scores for multiple file paths
///
/// Returns a HashMap mapping file_path to affinity_score (0.0-1.0).
/// The affinity score combines view_count and edit_count with time decay:
/// - access_score = (view_count + edit_count * 2) / max_score (normalized)
/// - edit_count weighted 2x (edits indicate stronger engagement)
/// - time_decay = exp(-0.05 * age_in_days) with lambda=0.05 (slower decay than selections)
/// - Returns 0.0 for files not found in user_file_affinity table
pub fn batch_get_affinity_boosts(
    conn: &Connection,
    file_paths: &[&str],
) -> Result<std::collections::HashMap<String, f32>> {
    use std::collections::HashMap;

    if file_paths.is_empty() {
        return Ok(HashMap::new());
    }

    // Build IN clause placeholders using VALUES approach
    let placeholders: Vec<String> = (0..file_paths.len())
        .map(|i| format!("(?{})", i + 1))
        .collect();

    let query = format!(
        r#"
SELECT file_path,
       (view_count + edit_count * 2.0) * exp(-0.05 * ((unixepoch() - last_accessed_at) / 86400.0)) as affinity_score
FROM user_file_affinity
WHERE file_path IN ({})
"#,
        placeholders.join(", ")
    );

    let mut stmt = conn.prepare(&query).context("Failed to prepare batch_get_affinity_boosts")?;

    // Build params as owned strings for rusqlite compatibility
    let params: Vec<rusqlite::types::Value> = file_paths
        .iter()
        .map(|s| rusqlite::types::Value::Text(s.to_string()))
        .collect();

    let mut rows = stmt
        .query(rusqlite::params_from_iter(params))
        .context("Failed to query affinity boosts")?;

    let mut result = HashMap::new();
    while let Some(row) = rows.next()? {
        let file_path: String = row.get(0)?;
        let affinity_score: f64 = row.get(1)?;
        result.insert(file_path, affinity_score as f32);
    }

    // Fill in 0.0 for files not found (implicitly via HashMap::get returning None)
    Ok(result)
}

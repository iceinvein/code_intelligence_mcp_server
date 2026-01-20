use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::FileFingerprintRow;

pub fn get_file_fingerprint(
    conn: &Connection,
    file_path: &str,
) -> Result<Option<FileFingerprintRow>> {
    conn.query_row(
        r#"
SELECT file_path, mtime_ns, size_bytes
FROM file_fingerprints
WHERE file_path = ?1
"#,
        params![file_path],
        |row| {
            Ok(FileFingerprintRow {
                file_path: row.get(0)?,
                mtime_ns: row.get(1)?,
                size_bytes: row.get::<_, i64>(2)?.max(0) as u64,
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
) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO file_fingerprints(file_path, mtime_ns, size_bytes, updated_at)
VALUES (?1, ?2, ?3, unixepoch())
ON CONFLICT(file_path) DO UPDATE SET
  mtime_ns=excluded.mtime_ns,
  size_bytes=excluded.size_bytes,
  updated_at=unixepoch()
"#,
        params![file_path, mtime_ns, size_bytes as i64],
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

pub fn list_all_file_fingerprints(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<FileFingerprintRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT file_path, mtime_ns, size_bytes
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
        });
    }
    Ok(out)
}

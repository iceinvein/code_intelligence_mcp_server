use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::storage::sqlite::schema::FrameworkPatternRow;

/// Upsert a single framework pattern entry.
pub fn upsert_framework_pattern(conn: &Connection, pattern: &FrameworkPatternRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO framework_patterns (
    id, file_path, line, framework, kind, http_method, path, name, handler, arguments, parent_chain, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, unixepoch())
ON CONFLICT(id) DO UPDATE SET
    file_path=excluded.file_path,
    line=excluded.line,
    framework=excluded.framework,
    kind=excluded.kind,
    http_method=excluded.http_method,
    path=excluded.path,
    name=excluded.name,
    handler=excluded.handler,
    arguments=excluded.arguments,
    parent_chain=excluded.parent_chain,
    updated_at=unixepoch()
"#,
        params![
            pattern.id,
            pattern.file_path,
            pattern.line,
            pattern.framework,
            pattern.kind,
            pattern.http_method,
            pattern.path,
            pattern.name,
            pattern.handler,
            pattern.arguments,
            pattern.parent_chain,
        ],
    )
    .context("Failed to upsert framework pattern")?;
    Ok(())
}

/// Batch upsert framework patterns in a transaction.
pub fn batch_upsert_framework_patterns(
    conn: &Connection,
    patterns: &[FrameworkPatternRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for pattern in patterns {
        upsert_framework_pattern(&tx, pattern)?;
    }
    tx.commit()?;
    Ok(())
}

/// Delete all framework patterns for a specific file.
pub fn delete_framework_patterns_by_file(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM framework_patterns WHERE file_path = ?1",
        params![file_path],
    )
    .context("Failed to delete framework patterns for file")?;
    Ok(())
}

/// Search framework patterns with optional filters.
#[allow(clippy::too_many_arguments)]
pub fn search_framework_patterns(
    conn: &Connection,
    framework: Option<&str>,
    kind: Option<&str>,
    http_method: Option<&str>,
    path: Option<&str>,
    name: Option<&str>,
    file_path: Option<&str>,
    limit: usize,
) -> Result<Vec<FrameworkPatternRow>> {
    let mut conditions = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(f) = framework {
        conditions.push("framework = ?");
        param_values.push(Box::new(f.to_string()));
    }
    if let Some(k) = kind {
        conditions.push("kind = ?");
        param_values.push(Box::new(k.to_string()));
    }
    if let Some(m) = http_method {
        conditions.push("http_method = ?");
        param_values.push(Box::new(m.to_uppercase()));
    }
    if let Some(p) = path {
        conditions.push("path LIKE ?");
        param_values.push(Box::new(format!("%{}%", p)));
    }
    if let Some(n) = name {
        conditions.push("name LIKE ?");
        param_values.push(Box::new(format!("%{}%", n)));
    }
    if let Some(fp) = file_path {
        conditions.push("file_path = ?");
        param_values.push(Box::new(fp.to_string()));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        r#"
SELECT id, file_path, line, framework, kind, http_method, path, name, handler, arguments, parent_chain, updated_at
FROM framework_patterns
{}
ORDER BY file_path ASC, line ASC
LIMIT ?
"#,
        where_clause
    );

    let mut stmt = conn.prepare(&sql).context("Failed to prepare search query")?;

    let mut param_refs: Vec<&dyn rusqlite::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let limit_i64 = limit as i64;
    param_refs.push(&limit_i64);

    let mut rows = stmt.query(param_refs.as_slice())?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        results.push(FrameworkPatternRow {
            id: row.get(0)?,
            file_path: row.get(1)?,
            line: row.get::<_, i64>(2)? as u32,
            framework: row.get(3)?,
            kind: row.get(4)?,
            http_method: row.get(5)?,
            path: row.get(6)?,
            name: row.get(7)?,
            handler: row.get(8)?,
            arguments: row.get(9)?,
            parent_chain: row.get(10)?,
            updated_at: row.get(11)?,
        });
    }

    Ok(results)
}

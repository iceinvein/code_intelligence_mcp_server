use anyhow::{bail, Context, Result};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};

use crate::storage::sqlite::schema::{SymbolIdentityRow, SymbolRow};

pub fn delete_by_file(conn: &Connection, file_path: &str) -> Result<u64> {
    let deleted = conn
        .execute(
            r#"
DELETE FROM symbol_identities
WHERE symbol_id IN (SELECT id FROM symbols WHERE file_path = ?1)
"#,
            params![file_path],
        )
        .with_context(|| format!("Failed to delete symbol identities for file: {file_path}"))?;
    Ok(deleted as u64)
}

/// Reject occurrence-id reuse before any symbol row can be overwritten. This
/// is intentionally a hard failure: a hash/allocator collision is an index
/// integrity error, not an upsert of the same declaration.
pub fn validate_no_collisions(conn: &Connection, rows: &[SymbolIdentityRow]) -> Result<()> {
    let mut symbol_ids = HashSet::new();
    let mut logical_occurrences = HashSet::new();
    for row in rows {
        if !symbol_ids.insert(&row.symbol_id) {
            bail!(
                "Duplicate symbol occurrence id in indexing batch: symbol_id={}, logical_id={}, qualified_name={}",
                row.symbol_id,
                row.logical_id,
                row.qualified_name
            );
        }
        if !logical_occurrences.insert((&row.logical_id, &row.occurrence_discriminator)) {
            bail!(
                "Duplicate logical occurrence discriminator in indexing batch: logical_id={}, occurrence={}",
                row.logical_id,
                row.occurrence_discriminator
            );
        }
    }

    let incoming = rows
        .iter()
        .map(|row| (row.symbol_id.as_str(), row))
        .collect::<HashMap<_, _>>();
    for chunk in rows.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            r#"
SELECT symbol_id, logical_id, qualified_name, occurrence_discriminator
FROM symbol_identities
WHERE symbol_id IN ({placeholders})
"#
        );
        let mut stmt = conn.prepare(&sql)?;
        let existing = stmt.query_map(
            params_from_iter(chunk.iter().map(|row| &row.symbol_id)),
            |sql_row| {
                Ok((
                    sql_row.get::<_, String>(0)?,
                    sql_row.get::<_, String>(1)?,
                    sql_row.get::<_, String>(2)?,
                    sql_row.get::<_, String>(3)?,
                ))
            },
        )?;
        for existing in existing {
            let (symbol_id, logical_id, qualified_name, occurrence) = existing?;
            let row = incoming
                .get(symbol_id.as_str())
                .expect("queried id came from incoming identities");
            if logical_id != row.logical_id
                || qualified_name != row.qualified_name
                || occurrence != row.occurrence_discriminator
            {
                bail!(
                    "Symbol occurrence id collision: symbol_id={}, incoming={}:{}:{}, existing={}:{}:{}",
                    row.symbol_id,
                    row.logical_id,
                    row.qualified_name,
                    row.occurrence_discriminator,
                    logical_id,
                    qualified_name,
                    occurrence
                );
            }
        }
    }
    Ok(())
}

pub fn batch_upsert(conn: &Connection, rows: &[SymbolIdentityRow]) -> Result<()> {
    validate_no_collisions(conn, rows)?;
    let mut stmt = conn.prepare_cached(
        r#"
INSERT INTO symbol_identities(
  symbol_id, logical_id, qualified_name, signature,
  occurrence_discriminator, is_canonical, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())
ON CONFLICT(symbol_id) DO UPDATE SET
  logical_id=excluded.logical_id,
  qualified_name=excluded.qualified_name,
  signature=excluded.signature,
  occurrence_discriminator=excluded.occurrence_discriminator,
  is_canonical=excluded.is_canonical,
  updated_at=unixepoch()
"#,
    )?;
    for row in rows {
        stmt.execute(params![
            row.symbol_id,
            row.logical_id,
            row.qualified_name,
            row.signature,
            row.occurrence_discriminator,
            if row.is_canonical { 1 } else { 0 },
        ])
        .with_context(|| {
            format!(
                "Failed to upsert symbol identity: symbol_id={}, logical_id={}, qualified_name={}",
                row.symbol_id, row.logical_id, row.qualified_name
            )
        })?;
    }
    Ok(())
}

pub fn get_by_symbol_id(conn: &Connection, symbol_id: &str) -> Result<Option<SymbolIdentityRow>> {
    conn.query_row(
        r#"
SELECT symbol_id, logical_id, qualified_name, signature,
       occurrence_discriminator, is_canonical
FROM symbol_identities
WHERE symbol_id = ?1
"#,
        params![symbol_id],
        row_from_sql,
    )
    .optional()
    .context("Failed to get symbol identity")
}

pub fn get_by_symbol_ids(
    conn: &Connection,
    symbol_ids: &[String],
) -> Result<HashMap<String, SymbolIdentityRow>> {
    if symbol_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = std::iter::repeat_n("?", symbol_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        r#"
SELECT symbol_id, logical_id, qualified_name, signature,
       occurrence_discriminator, is_canonical
FROM symbol_identities
WHERE symbol_id IN ({placeholders})
"#
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(symbol_ids.iter()), row_from_sql)?;
    let identities = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(identities
        .into_iter()
        .map(|row| (row.symbol_id.clone(), row))
        .collect())
}

pub fn list_by_logical_id(conn: &Connection, logical_id: &str) -> Result<Vec<SymbolIdentityRow>> {
    let mut stmt = conn.prepare(
        r#"
SELECT symbol_id, logical_id, qualified_name, signature,
       occurrence_discriminator, is_canonical
FROM symbol_identities
WHERE logical_id = ?1
ORDER BY is_canonical DESC, occurrence_discriminator ASC
"#,
    )?;
    let rows = stmt.query_map(params![logical_id], row_from_sql)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("Failed to list logical symbol occurrences")
}

/// Resolve an owner-qualified member inside one indexed file. The caller
/// supplies both dot and `::` spellings when the language permits them; a
/// result is returned only when the identity is unique.
pub fn find_unique_by_qualified_names(
    conn: &Connection,
    file_path: &str,
    qualified_names: &[String],
) -> Result<Option<String>> {
    if qualified_names.is_empty() {
        return Ok(None);
    }
    let placeholders = std::iter::repeat_n("?", qualified_names.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        r#"
SELECT DISTINCT si.logical_id
FROM symbol_identities si
JOIN symbols s ON s.id = si.symbol_id
WHERE s.file_path = ?1
  AND si.qualified_name IN ({placeholders})
LIMIT 2
"#
    );
    let params = std::iter::once(file_path.to_string())
        .chain(qualified_names.iter().cloned())
        .collect::<Vec<_>>();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |row| {
        row.get::<_, String>(0)
    })?;
    let logical_ids = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(match logical_ids.as_slice() {
        [logical_id] => Some(logical_id.clone()),
        _ => None,
    })
}

pub fn search_symbols_by_qualified_name(
    conn: &Connection,
    qualified_name: &str,
    file_path: Option<&str>,
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    let sql = if file_path.is_some() {
        r#"
SELECT s.id, s.file_path, s.language, s.kind, s.name, s.exported,
       s.start_byte, s.end_byte, s.start_line, s.end_line, s.text
FROM symbol_identities si
JOIN symbols s ON s.id = si.symbol_id
WHERE si.qualified_name = ?1 AND s.file_path = ?2
ORDER BY si.is_canonical DESC, s.start_byte ASC
LIMIT ?3
"#
    } else {
        r#"
SELECT s.id, s.file_path, s.language, s.kind, s.name, s.exported,
       s.start_byte, s.end_byte, s.start_line, s.end_line, s.text
FROM symbol_identities si
JOIN symbols s ON s.id = si.symbol_id
WHERE si.qualified_name = ?1
ORDER BY si.is_canonical DESC, s.file_path ASC, s.start_byte ASC
LIMIT ?2
"#
    };
    let mut stmt = conn.prepare(sql)?;
    let map = |row: &rusqlite::Row<'_>| {
        Ok(SymbolRow {
            id: row.get(0)?,
            file_path: row.get(1)?,
            language: row.get(2)?,
            kind: row.get(3)?,
            name: row.get(4)?,
            exported: row.get::<_, i64>(5)? != 0,
            start_byte: row.get::<_, i64>(6)?.max(0) as u32,
            end_byte: row.get::<_, i64>(7)?.max(0) as u32,
            start_line: row.get::<_, i64>(8)?.max(0) as u32,
            end_line: row.get::<_, i64>(9)?.max(0) as u32,
            text: row.get(10)?,
        })
    };
    let rows = if let Some(file_path) = file_path {
        stmt.query_map(
            params![
                qualified_name,
                file_path,
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            map,
        )?
    } else {
        stmt.query_map(
            params![qualified_name, i64::try_from(limit).unwrap_or(i64::MAX)],
            map,
        )?
    };
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("Failed to search symbols by qualified name")
}

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<SymbolIdentityRow> {
    Ok(SymbolIdentityRow {
        symbol_id: row.get(0)?,
        logical_id: row.get(1)?,
        qualified_name: row.get(2)?,
        signature: row.get(3)?,
        occurrence_discriminator: row.get(4)?,
        is_canonical: row.get::<_, i64>(5)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::queries;
    use crate::storage::sqlite::schema::{SymbolRow, SCHEMA_SQL};

    fn symbol(id: &str, name: &str, start: u32) -> SymbolRow {
        SymbolRow {
            id: id.into(),
            file_path: "src/service.ts".into(),
            language: "typescript".into(),
            kind: "function".into(),
            name: name.into(),
            exported: true,
            start_byte: start,
            end_byte: start + 10,
            start_line: 1,
            end_line: 1,
            text: format!("function {name}() {{}}"),
        }
    }

    fn identity(symbol_id: &str, logical_id: &str, occurrence: &str) -> SymbolIdentityRow {
        SymbolIdentityRow {
            symbol_id: symbol_id.into(),
            logical_id: logical_id.into(),
            qualified_name: "Service.run".into(),
            signature: "run()".into(),
            occurrence_discriminator: occurrence.into(),
            is_canonical: symbol_id == logical_id,
        }
    }

    #[test]
    fn logical_identity_lists_distinct_occurrences() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        queries::symbols::batch_upsert_symbols(
            &conn,
            &[symbol("logical", "run", 10), symbol("occ-2", "run", 30)],
        )
        .unwrap();
        batch_upsert(
            &conn,
            &[
                identity("logical", "logical", "run()#0"),
                identity("occ-2", "logical", "run()#1"),
            ],
        )
        .unwrap();

        let occurrences = list_by_logical_id(&conn, "logical").unwrap();
        assert_eq!(occurrences.len(), 2);
        assert!(occurrences[0].is_canonical);
        assert_ne!(occurrences[0].symbol_id, occurrences[1].symbol_id);

        let qualified = search_symbols_by_qualified_name(&conn, "Service.run", None, 10).unwrap();
        assert_eq!(qualified.len(), 2);
        assert_eq!(qualified[0].id, "logical");
    }

    #[test]
    fn collision_is_rejected_instead_of_overwriting_identity() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        queries::symbols::upsert_symbol(&conn, &symbol("same", "run", 10)).unwrap();
        batch_upsert(&conn, &[identity("same", "logical-a", "run()#0")]).unwrap();

        let mut conflicting = identity("same", "logical-b", "run(i32)#0");
        conflicting.qualified_name = "Other.run".into();
        let error = batch_upsert(&conn, &[conflicting]).unwrap_err().to_string();
        assert!(error.contains("collision"), "unexpected error: {error}");
        assert_eq!(
            get_by_symbol_id(&conn, "same").unwrap().unwrap().logical_id,
            "logical-a"
        );
    }
}

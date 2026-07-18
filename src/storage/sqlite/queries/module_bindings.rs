use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::BTreeSet;

use crate::storage::sqlite::schema::ModuleBindingRow;

pub fn delete_by_file(conn: &Connection, file_path: &str) -> Result<u64> {
    let deleted = conn
        .execute(
            "DELETE FROM module_bindings WHERE file_path = ?1",
            params![file_path],
        )
        .with_context(|| format!("Failed to delete module bindings for file: {file_path}"))?;
    Ok(deleted as u64)
}

/// Null targets whose declarations disappeared while symbol replacement ran
/// with foreign-key enforcement temporarily disabled. The binding syntax is
/// retained so a later source refresh can resolve it again.
pub fn clear_orphan_targets(conn: &Connection) -> Result<u64> {
    let updated = conn
        .execute(
            r#"
UPDATE module_bindings
SET target_symbol_id = NULL,
    resolution = 'unresolved',
    confidence = 0.0,
    updated_at = unixepoch()
WHERE target_symbol_id IS NOT NULL
  AND NOT EXISTS (
    SELECT 1 FROM symbols WHERE symbols.id = module_bindings.target_symbol_id
  )
"#,
            [],
        )
        .context("Failed to clear orphan module binding targets")?;
    Ok(updated as u64)
}

pub fn batch_upsert(conn: &Connection, rows: &[ModuleBindingRow]) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        r#"
INSERT INTO module_bindings(
  file_path, binding_kind, source_module, source_file,
  imported_name, local_name, exported_name, target_symbol_id,
  at_line, resolution, confidence, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, unixepoch())
ON CONFLICT(
  file_path, binding_kind, source_module, imported_name,
  local_name, exported_name, at_line
) DO UPDATE SET
  source_file=excluded.source_file,
  target_symbol_id=excluded.target_symbol_id,
  resolution=excluded.resolution,
  confidence=excluded.confidence,
  updated_at=unixepoch()
"#,
    )?;

    for row in rows {
        stmt.execute(params![
            row.file_path,
            row.binding_kind,
            row.source_module,
            row.source_file,
            row.imported_name,
            row.local_name,
            row.exported_name,
            row.target_symbol_id,
            row.at_line,
            row.resolution,
            row.confidence,
        ])
        .with_context(|| {
            format!(
                "Failed to upsert module binding: file={}, kind={}, exported_name={}",
                row.file_path, row.binding_kind, row.exported_name
            )
        })?;
    }
    Ok(())
}

pub fn list_by_file(conn: &Connection, file_path: &str) -> Result<Vec<ModuleBindingRow>> {
    let mut stmt = conn.prepare(
        r#"
SELECT
  id, file_path, binding_kind, source_module, source_file,
  imported_name, local_name, exported_name, target_symbol_id,
  at_line, resolution, confidence
FROM module_bindings
WHERE file_path = ?1
ORDER BY at_line ASC, binding_kind ASC, exported_name ASC
"#,
    )?;
    let rows = stmt.query_map(params![file_path], row_from_sql)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("Failed to list module bindings by file")
}

/// Return the unique symbol targets exposed under `exported_name` by a file.
/// Multiple syntactic bindings that converge on the same declaration collapse
/// to one target; genuinely distinct targets remain ambiguous to callers.
pub fn list_public_target_ids(
    conn: &Connection,
    file_path: &str,
    exported_name: &str,
) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        r#"
SELECT target_symbol_id
FROM module_bindings
WHERE file_path = ?1
  AND exported_name = ?2
  AND binding_kind IN ('export', 're_export')
  AND target_symbol_id IS NOT NULL
  AND confidence > 0.0
"#,
    )?;
    let rows = stmt.query_map(params![file_path, exported_name], |row| {
        row.get::<_, String>(0)
    })?;
    let mut targets = BTreeSet::new();
    for target in rows {
        targets.insert(target?);
    }
    Ok(targets.into_iter().collect())
}

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModuleBindingRow> {
    Ok(ModuleBindingRow {
        id: row.get(0)?,
        file_path: row.get(1)?,
        binding_kind: row.get(2)?,
        source_module: row.get(3)?,
        source_file: row.get(4)?,
        imported_name: row.get(5)?,
        local_name: row.get(6)?,
        exported_name: row.get(7)?,
        target_symbol_id: row.get(8)?,
        at_line: row.get::<_, i64>(9)? as u32,
        resolution: row.get(10)?,
        confidence: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::schema::SCHEMA_SQL;

    fn binding(exported_name: &str) -> ModuleBindingRow {
        ModuleBindingRow {
            id: 0,
            file_path: "src/index.ts".into(),
            binding_kind: "re_export".into(),
            source_module: "./implementation".into(),
            source_file: Some("src/implementation.ts".into()),
            imported_name: "Implementation".into(),
            local_name: String::new(),
            exported_name: exported_name.into(),
            target_symbol_id: None,
            at_line: 1,
            resolution: "unresolved".into(),
            confidence: 0.0,
        }
    }

    #[test]
    fn upsert_preserves_aliases_and_updates_resolution() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        let mut row = binding("PublicImplementation");
        batch_upsert(&conn, &[row.clone()]).unwrap();

        row.resolution = "inferred".into();
        row.confidence = 0.75;
        batch_upsert(&conn, &[row]).unwrap();

        let stored = list_by_file(&conn, "src/index.ts").unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].imported_name, "Implementation");
        assert_eq!(stored[0].exported_name, "PublicImplementation");
        assert_eq!(stored[0].resolution, "inferred");
        assert_eq!(stored[0].confidence, 0.75);
    }

    #[test]
    fn delete_is_scoped_to_owning_file() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        let first = binding("One");
        let mut second = binding("Two");
        second.file_path = "src/other.ts".into();
        batch_upsert(&conn, &[first, second]).unwrap();

        assert_eq!(delete_by_file(&conn, "src/index.ts").unwrap(), 1);
        assert!(list_by_file(&conn, "src/index.ts").unwrap().is_empty());
        assert_eq!(list_by_file(&conn, "src/other.ts").unwrap().len(), 1);
    }

    #[test]
    fn missing_target_is_nullified_without_losing_binding_names() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn.execute(
            r#"
INSERT INTO symbols(
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
)
VALUES ('target', 'src/implementation.ts', 'typescript', 'class',
        'Implementation', 1, 0, 10, 1, 1, 'class Implementation {}')
"#,
            [],
        )
        .unwrap();
        let mut row = binding("PublicImplementation");
        row.target_symbol_id = Some("target".into());
        row.resolution = "exact".into();
        row.confidence = 1.0;
        batch_upsert(&conn, &[row]).unwrap();

        conn.execute("PRAGMA foreign_keys = OFF", []).unwrap();
        conn.execute("DELETE FROM symbols WHERE id = 'target'", [])
            .unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        assert_eq!(clear_orphan_targets(&conn).unwrap(), 1);

        let stored = list_by_file(&conn, "src/index.ts").unwrap();
        assert_eq!(stored[0].target_symbol_id, None);
        assert_eq!(stored[0].resolution, "unresolved");
        assert_eq!(stored[0].exported_name, "PublicImplementation");
    }

    #[test]
    fn public_target_lookup_deduplicates_aliases_to_same_symbol() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn.execute(
            r#"
INSERT INTO symbols(
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
)
VALUES ('target', 'src/worker.ts', 'typescript', 'class',
        'Worker', 1, 0, 10, 1, 1, 'class Worker {}')
"#,
            [],
        )
        .unwrap();
        let mut first = binding("default");
        first.file_path = "src/worker.ts".into();
        first.binding_kind = "export".into();
        first.target_symbol_id = Some("target".into());
        first.resolution = "exact".into();
        first.confidence = 1.0;
        let mut second = first.clone();
        second.local_name = "WorkerAlias".into();
        batch_upsert(&conn, &[first, second]).unwrap();

        assert_eq!(
            list_public_target_ids(&conn, "src/worker.ts", "default").unwrap(),
            vec!["target".to_string()]
        );
    }
}

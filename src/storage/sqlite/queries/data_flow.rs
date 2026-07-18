use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::storage::sqlite::schema::DataFlowFactRow;

pub fn batch_upsert(conn: &Connection, facts: &[DataFlowFactRow]) -> Result<()> {
    if facts.is_empty() {
        return Ok(());
    }

    let mut stmt = conn
        .prepare_cached(
            r#"
INSERT INTO data_flow_facts(
  owner_symbol_id, entity_name, entity_kind, access_kind, at_file, at_line, scope
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
ON CONFLICT(owner_symbol_id, entity_name, entity_kind, access_kind, at_file, at_line)
DO UPDATE SET scope=excluded.scope
"#,
        )
        .context("Failed to prepare data-flow fact upsert")?;

    for fact in facts {
        stmt.execute(params![
            fact.owner_symbol_id,
            fact.entity_name,
            fact.entity_kind,
            fact.access_kind,
            fact.at_file,
            fact.at_line,
            fact.scope,
        ])
        .with_context(|| {
            format!(
                "Failed to upsert {} data-flow fact '{}' for {}",
                fact.access_kind, fact.entity_name, fact.owner_symbol_id
            )
        })?;
    }
    Ok(())
}

/// Delete facts source-owned by a changed file before its symbols are replaced.
pub fn delete_by_file(conn: &Connection, file_path: &str) -> Result<u64> {
    let deleted = conn
        .execute(
            "DELETE FROM data_flow_facts WHERE at_file = ?1",
            params![file_path],
        )
        .with_context(|| format!("Failed to delete data-flow facts for file: {file_path}"))?;
    Ok(deleted as u64)
}

pub fn list_by_owner(
    conn: &Connection,
    owner_symbol_id: &str,
    limit: usize,
) -> Result<Vec<DataFlowFactRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT owner_symbol_id, entity_name, entity_kind, access_kind, at_file, at_line, scope
FROM data_flow_facts
WHERE owner_symbol_id = ?1
ORDER BY at_line ASC, access_kind ASC, entity_name ASC, id ASC
LIMIT ?2
"#,
        )
        .context("Failed to prepare data-flow fact query")?;
    let rows = stmt
        .query_map(params![owner_symbol_id, limit as i64], |row| {
            Ok(DataFlowFactRow {
                owner_symbol_id: row.get(0)?,
                entity_name: row.get(1)?,
                entity_kind: row.get(2)?,
                access_kind: row.get(3)?,
                at_file: row.get(4)?,
                at_line: u32::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
                scope: row.get(6)?,
            })
        })
        .context("Failed to query data-flow facts")?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("Failed to read data-flow facts")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::schema::SCHEMA_SQL;

    fn insert_owner(conn: &Connection, id: &str, file: &str) {
        conn.execute(
            r#"
INSERT INTO symbols(
  id, file_path, language, kind, name, exported,
  start_byte, end_byte, start_line, end_line, text
)
VALUES (?1, ?2, 'rust', 'function', ?1, 0, 0, 100, 1, 20, 'fn owner() {}')
"#,
            params![id, file],
        )
        .unwrap();
    }

    #[test]
    fn data_flow_facts_round_trip_dedupe_and_delete_by_file() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        insert_owner(&conn, "owner-a", "src/a.rs");
        insert_owner(&conn, "owner-b", "src/b.rs");

        let fact = DataFlowFactRow {
            owner_symbol_id: "owner-a".into(),
            entity_name: "pending".into(),
            entity_kind: "async_boundary".into(),
            access_kind: "await".into(),
            at_file: "src/a.rs".into(),
            at_line: 7,
            scope: Some("owner-a".into()),
        };
        batch_upsert(&conn, &[fact.clone(), fact.clone()]).unwrap();
        batch_upsert(
            &conn,
            &[DataFlowFactRow {
                owner_symbol_id: "owner-b".into(),
                entity_name: "value".into(),
                entity_kind: "value".into(),
                access_kind: "write".into(),
                at_file: "src/b.rs".into(),
                at_line: 9,
                scope: None,
            }],
        )
        .unwrap();

        assert_eq!(list_by_owner(&conn, "owner-a", 10).unwrap(), vec![fact]);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM data_flow_facts", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            2
        );
        assert_eq!(delete_by_file(&conn, "src/a.rs").unwrap(), 1);
        assert!(list_by_owner(&conn, "owner-a", 10).unwrap().is_empty());
        assert_eq!(list_by_owner(&conn, "owner-b", 10).unwrap().len(), 1);
    }
}

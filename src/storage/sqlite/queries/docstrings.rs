use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::indexer::extract::symbol::JSDocEntry;
use crate::storage::sqlite::schema::DocstringRow;

pub fn upsert_docstring(conn: &Connection, docstring: &DocstringRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO docstrings (
  symbol_id, raw_text, summary, params_json, returns_text, examples_json, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())
ON CONFLICT(symbol_id) DO UPDATE SET
  raw_text=excluded.raw_text,
  summary=excluded.summary,
  params_json=excluded.params_json,
  returns_text=excluded.returns_text,
  examples_json=excluded.examples_json,
  updated_at=unixepoch()
"#,
        params![
            docstring.symbol_id,
            docstring.raw_text,
            docstring.summary,
            docstring.params_json,
            docstring.returns_text,
            docstring.examples_json,
        ],
    )
    .context("Failed to upsert docstring")?;
    Ok(())
}

pub fn batch_upsert_docstrings(conn: &Connection, entries: &[JSDocEntry]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for entry in entries {
        if entry.symbol_id.is_empty() {
            continue;
        }
        let params_json = if entry.params.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&entry.params).unwrap_or_default())
        };
        let examples_json = if entry.examples.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&entry.examples).unwrap_or_default())
        };

        let row = DocstringRow {
            symbol_id: entry.symbol_id.clone(),
            raw_text: entry.raw_text.clone(),
            summary: entry.summary.clone(),
            params_json,
            returns_text: entry.returns.clone(),
            examples_json,
            updated_at: 0,
        };
        upsert_docstring(&tx, &row)?;
    }
    tx.commit()?;
    Ok(())
}

pub fn get_docstring_by_symbol(conn: &Connection, symbol_id: &str) -> Result<Option<DocstringRow>> {
    match conn.query_row(
        r#"
SELECT symbol_id, raw_text, summary, params_json, returns_text, examples_json
FROM docstrings
WHERE symbol_id = ?1
"#,
        params![symbol_id],
        |row| {
            Ok(DocstringRow {
                symbol_id: row.get(0)?,
                raw_text: row.get(1)?,
                summary: row.get(2)?,
                params_json: row.get(3)?,
                returns_text: row.get(4)?,
                examples_json: row.get(5)?,
                updated_at: 0,
            })
        },
    ) {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e).context("Failed to query docstring by symbol"),
    }
}

pub fn delete_docstrings_by_file(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM docstrings WHERE symbol_id IN (SELECT id FROM symbols WHERE file_path = ?1)",
        params![file_path],
    )
    .context("Failed to delete docstrings for file")?;
    Ok(())
}

pub fn has_docstring(conn: &Connection, symbol_id: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM docstrings WHERE symbol_id = ?1",
            params![symbol_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(count > 0)
}

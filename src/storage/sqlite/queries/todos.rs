use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::indexer::extract::symbol::{TodoEntry, TodoKind};
use crate::storage::sqlite::schema::TodoRow;

/// Upsert a single TODO entry
pub fn upsert_todo(conn: &Connection, todo: &TodoRow) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO todos (id, kind, text, file_path, line, associated_symbol, created_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())
ON CONFLICT(id) DO UPDATE SET
  kind=excluded.kind,
  text=excluded.text,
  associated_symbol=excluded.associated_symbol
"#,
        params![
            todo.id,
            todo.kind,
            todo.text,
            todo.file_path,
            todo.line,
            todo.associated_symbol,
        ],
    )
    .context("Failed to upsert todo")?;
    Ok(())
}

/// Batch upsert TODO entries from extraction
pub fn batch_upsert_todos(conn: &Connection, todos: &[TodoEntry]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for todo in todos {
        let id = format!("{}:{}", todo.file_path, todo.line);
        let row = TodoRow {
            id,
            kind: match todo.kind {
                TodoKind::Todo => "todo",
                TodoKind::Fixme => "fixme",
            }
            .to_string(),
            text: todo.text.clone(),
            file_path: todo.file_path.clone(),
            line: todo.line,
            associated_symbol: todo.associated_symbol.clone(),
            created_at: 0,
        };
        upsert_todo(&tx, &row)?;
    }
    tx.commit()?;
    Ok(())
}

/// Search TODOs with optional filters
pub fn search_todos(
    conn: &Connection,
    keyword: Option<&str>,
    file_path: Option<&str>,
    kind: Option<&str>,
    limit: usize,
) -> Result<Vec<TodoRow>> {
    let sql = match (keyword.is_some(), file_path.is_some()) {
        (true, true) => {
            r#"
            SELECT id, kind, text, file_path, line, associated_symbol
            FROM todos
            WHERE text LIKE ?1 AND file_path = ?2
            ORDER BY file_path ASC, line ASC
            LIMIT ?3
        "#
        }
        (true, false) => {
            r#"
            SELECT id, kind, text, file_path, line, associated_symbol
            FROM todos
            WHERE text LIKE ?1
            ORDER BY file_path ASC, line ASC
            LIMIT ?2
        "#
        }
        (false, true) => {
            r#"
            SELECT id, kind, text, file_path, line, associated_symbol
            FROM todos
            WHERE file_path = ?1
            ORDER BY line ASC
            LIMIT ?2
        "#
        }
        (false, false) => {
            r#"
            SELECT id, kind, text, file_path, line, associated_symbol
            FROM todos
            ORDER BY file_path ASC, line ASC
            LIMIT ?1
        "#
        }
    };

    let mut stmt = conn.prepare(sql)?;

    let mut rows = match (keyword, file_path) {
        (Some(kw), Some(fp)) => {
            let pattern = format!("%{}%", kw);
            stmt.query(params![pattern, fp, limit as i64])?
        }
        (Some(kw), None) => {
            let pattern = format!("%{}%", kw);
            stmt.query(params![pattern, limit as i64])?
        }
        (None, Some(fp)) => stmt.query(params![fp, limit as i64])?,
        (None, None) => stmt.query(params![limit as i64])?,
    };

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let kind_str: String = row.get(1)?;
        let todo_kind = match kind_str.as_str() {
            "todo" => TodoKind::Todo,
            "fixme" => TodoKind::Fixme,
            _ => TodoKind::Todo,
        };

        // Filter by kind if specified
        if let Some(filter_kind) = kind {
            let filter_matches = match filter_kind {
                "todo" => matches!(todo_kind, TodoKind::Todo),
                "fixme" => matches!(todo_kind, TodoKind::Fixme),
                _ => true,
            };
            if !filter_matches {
                continue;
            }
        }

        out.push(TodoRow {
            id: row.get(0)?,
            kind: kind_str,
            text: row.get(2)?,
            file_path: row.get(3)?,
            line: row.get::<_, i64>(4)? as u32,
            associated_symbol: row.get(5)?,
            created_at: 0,
        });
    }
    Ok(out)
}

/// Delete all TODOs for a specific file
pub fn delete_todos_by_file(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute("DELETE FROM todos WHERE file_path = ?1", params![file_path])
        .context("Failed to delete todos for file")?;
    Ok(())
}

/// Count TODOs by kind
pub fn count_todos(conn: &Connection) -> Result<(usize, usize)> {
    let todo_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM todos WHERE kind = 'todo'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let fixme_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM todos WHERE kind = 'fixme'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok((todo_count.max(0) as usize, fixme_count.max(0) as usize))
}

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// A row from the `chat_sessions` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSessionRow {
    pub id: String,
    pub repo_path: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A row from the `chat_messages` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessageRow {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub tool_calls_json: Option<String>,
    pub created_at: i64,
}

/// Create a new chat session.
pub fn create_session(conn: &Connection, id: &str, repo_path: &str, title: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO chat_sessions (id, repo_path, title) VALUES (?1, ?2, ?3)",
        params![id, repo_path, title],
    )
    .with_context(|| format!("Failed to create chat session: id={}", id))?;
    Ok(())
}

/// List chat sessions for a given repo, ordered by most recently updated first.
pub fn list_sessions(conn: &Connection, repo_path: &str, limit: usize) -> Result<Vec<ChatSessionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, repo_path, title, created_at, updated_at
         FROM chat_sessions
         WHERE repo_path = ?1
         ORDER BY updated_at DESC
         LIMIT ?2",
    )?;

    let rows = stmt
        .query_map(params![repo_path, limit as i64], |row| {
            Ok(ChatSessionRow {
                id: row.get(0)?,
                repo_path: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to list chat sessions")?;

    Ok(rows)
}

/// Get a single chat session by ID.
pub fn get_session(conn: &Connection, id: &str) -> Result<Option<ChatSessionRow>> {
    conn.query_row(
        "SELECT id, repo_path, title, created_at, updated_at
         FROM chat_sessions
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(ChatSessionRow {
                id: row.get(0)?,
                repo_path: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .optional()
    .context("Failed to get chat session")
}

/// Delete a chat session (cascades to messages via FK).
pub fn delete_session(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM chat_sessions WHERE id = ?1", params![id])
        .with_context(|| format!("Failed to delete chat session: id={}", id))?;
    Ok(())
}

/// Add a message to a chat session. Returns the auto-generated message row ID.
pub fn add_message(
    conn: &Connection,
    session_id: &str,
    role: &str,
    content: &str,
    tool_calls_json: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO chat_messages (session_id, role, content, tool_calls_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![session_id, role, content, tool_calls_json],
    )
    .with_context(|| {
        format!(
            "Failed to add chat message: session_id={}, role={}",
            session_id, role
        )
    })?;
    Ok(conn.last_insert_rowid())
}

/// List all messages in a session, ordered by creation time ascending.
pub fn list_messages(conn: &Connection, session_id: &str) -> Result<Vec<ChatMessageRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, role, content, tool_calls_json, created_at
         FROM chat_messages
         WHERE session_id = ?1
         ORDER BY created_at ASC, id ASC",
    )?;

    let rows = stmt
        .query_map(params![session_id], |row| {
            Ok(ChatMessageRow {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                tool_calls_json: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to list chat messages")?;

    Ok(rows)
}

/// Update the title of a chat session.
pub fn update_session_title(conn: &Connection, id: &str, title: &str) -> Result<()> {
    conn.execute(
        "UPDATE chat_sessions SET title = ?1, updated_at = unixepoch() WHERE id = ?2",
        params![title, id],
    )
    .with_context(|| format!("Failed to update session title: id={}", id))?;
    Ok(())
}

/// Touch a session to update its `updated_at` timestamp.
pub fn touch_session(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE chat_sessions SET updated_at = unixepoch() WHERE id = ?1",
        params![id],
    )
    .with_context(|| format!("Failed to touch session: id={}", id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        conn.execute_batch(crate::chat::store::CHAT_SCHEMA_SQL).unwrap();
        conn
    }

    #[test]
    fn test_create_and_get_session() {
        let conn = setup_db();
        create_session(&conn, "sess-1", "/home/user/repo", Some("My Chat")).unwrap();

        let session = get_session(&conn, "sess-1").unwrap().unwrap();
        assert_eq!(session.id, "sess-1");
        assert_eq!(session.repo_path, "/home/user/repo");
        assert_eq!(session.title, Some("My Chat".to_string()));
    }

    #[test]
    fn test_create_session_no_title() {
        let conn = setup_db();
        create_session(&conn, "sess-2", "/repo", None).unwrap();

        let session = get_session(&conn, "sess-2").unwrap().unwrap();
        assert!(session.title.is_none());
    }

    #[test]
    fn test_get_session_not_found() {
        let conn = setup_db();
        let result = get_session(&conn, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list_sessions_ordered_by_updated() {
        let conn = setup_db();
        create_session(&conn, "s1", "/repo", Some("First")).unwrap();
        create_session(&conn, "s2", "/repo", Some("Second")).unwrap();
        // Touch s1 to make it more recent
        touch_session(&conn, "s1").unwrap();

        let sessions = list_sessions(&conn, "/repo", 10).unwrap();
        assert_eq!(sessions.len(), 2);
        // s1 was touched last, so it should be first
        assert_eq!(sessions[0].id, "s1");
        assert_eq!(sessions[1].id, "s2");
    }

    #[test]
    fn test_list_sessions_filtered_by_repo() {
        let conn = setup_db();
        create_session(&conn, "s1", "/repo-a", Some("A")).unwrap();
        create_session(&conn, "s2", "/repo-b", Some("B")).unwrap();

        let sessions = list_sessions(&conn, "/repo-a", 10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "s1");
    }

    #[test]
    fn test_list_sessions_respects_limit() {
        let conn = setup_db();
        for i in 0..5 {
            create_session(&conn, &format!("s{}", i), "/repo", None).unwrap();
        }
        let sessions = list_sessions(&conn, "/repo", 3).unwrap();
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn test_delete_session() {
        let conn = setup_db();
        create_session(&conn, "s1", "/repo", None).unwrap();
        add_message(&conn, "s1", "user", "Hello", None).unwrap();

        delete_session(&conn, "s1").unwrap();

        assert!(get_session(&conn, "s1").unwrap().is_none());
        // Messages should be cascade-deleted
        let msgs = list_messages(&conn, "s1").unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_add_and_list_messages() {
        let conn = setup_db();
        create_session(&conn, "s1", "/repo", None).unwrap();

        let id1 = add_message(&conn, "s1", "user", "Hello", None).unwrap();
        let id2 = add_message(&conn, "s1", "assistant", "Hi there!", None).unwrap();
        let id3 = add_message(
            &conn,
            "s1",
            "assistant",
            "Let me search",
            Some(r#"[{"name":"search_code","arguments":{"query":"foo"}}]"#),
        )
        .unwrap();

        assert!(id1 > 0);
        assert!(id2 > id1);
        assert!(id3 > id2);

        let messages = list_messages(&conn, "s1").unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Hello");
        assert!(messages[0].tool_calls_json.is_none());
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[2].tool_calls_json.as_deref(), Some(r#"[{"name":"search_code","arguments":{"query":"foo"}}]"#));
    }

    #[test]
    fn test_update_session_title() {
        let conn = setup_db();
        create_session(&conn, "s1", "/repo", None).unwrap();
        assert!(get_session(&conn, "s1").unwrap().unwrap().title.is_none());

        update_session_title(&conn, "s1", "New Title").unwrap();
        let session = get_session(&conn, "s1").unwrap().unwrap();
        assert_eq!(session.title, Some("New Title".to_string()));
    }

    #[test]
    fn test_touch_session() {
        let conn = setup_db();
        create_session(&conn, "s1", "/repo", None).unwrap();
        let before = get_session(&conn, "s1").unwrap().unwrap().updated_at;

        // Sleep briefly to ensure timestamp changes (SQLite unixepoch() is seconds)
        std::thread::sleep(std::time::Duration::from_secs(1));
        touch_session(&conn, "s1").unwrap();
        let after = get_session(&conn, "s1").unwrap().unwrap().updated_at;

        assert!(after >= before);
    }
}

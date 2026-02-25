//! Chat session persistence — stores conversations in a shared SQLite database.
//!
//! Unlike the per-repo `SqliteStore`, the `ChatStore` uses a single shared
//! database file (`~/.code-intelligence/chat.db`) so that chat sessions are
//! accessible across all repos.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::sync::RwLock;

use crate::path::Utf8Path;
use crate::storage::sqlite::queries::chat::{self, ChatMessageRow, ChatSessionRow};

/// Schema SQL for the chat database (separate from the per-repo schema).
pub const CHAT_SCHEMA_SQL: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS chat_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    repo_path TEXT NOT NULL,
    title TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_chat_sessions_repo ON chat_sessions(repo_path);
CREATE INDEX IF NOT EXISTS idx_chat_sessions_updated ON chat_sessions(updated_at);

CREATE TABLE IF NOT EXISTS chat_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_calls_json TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_chat_messages_session ON chat_messages(session_id);
"#;

/// Persistent store for chat sessions and messages.
///
/// Owns a `rusqlite::Connection` to `~/.code-intelligence/chat.db`, wrapped
/// in an `RwLock` for safe concurrent access from axum handlers.
pub struct ChatStore {
    conn: RwLock<Connection>,
}

// SAFETY: Same justification as SqliteStore — rusqlite::Connection is Send
// but not Sync; wrapping in RwLock provides synchronized access.
unsafe impl Send for ChatStore {}
unsafe impl Sync for ChatStore {}

impl ChatStore {
    /// Open (or create) the chat database at the given path.
    pub fn open(db_path: &Utf8Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create chat db parent dir: {}", parent))?;
        }

        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open chat sqlite db: {}", db_path))?;

        // Enable WAL mode for concurrent reads
        let _ = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
            .ok();
        conn.execute("PRAGMA foreign_keys = ON", [])
            .context("Failed to enable foreign keys on chat db")?;
        let _ = conn.execute("PRAGMA synchronous=NORMAL", []).ok();
        let _ = conn.execute("PRAGMA busy_timeout=5000", []).ok();

        // Initialize schema
        conn.execute_batch(CHAT_SCHEMA_SQL)
            .context("Failed to initialize chat schema")?;

        Ok(Self {
            conn: RwLock::new(conn),
        })
    }

    /// Open an in-memory chat database (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .context("Failed to open in-memory chat db")?;
        conn.execute("PRAGMA foreign_keys = ON", [])
            .context("Failed to enable foreign keys on in-memory chat db")?;
        conn.execute_batch(CHAT_SCHEMA_SQL)
            .context("Failed to initialize in-memory chat schema")?;

        Ok(Self {
            conn: RwLock::new(conn),
        })
    }

    fn read(&self) -> Result<std::sync::RwLockReadGuard<'_, Connection>> {
        self.conn.read().map_err(|e| {
            anyhow::anyhow!("Chat DB read lock poisoned: {}", e)
        })
    }

    fn write(&self) -> Result<std::sync::RwLockWriteGuard<'_, Connection>> {
        self.conn.write().map_err(|e| {
            anyhow::anyhow!("Chat DB write lock poisoned: {}", e)
        })
    }

    // --- Delegate methods ---

    pub fn create_session(&self, id: &str, repo_path: &str, title: Option<&str>) -> Result<()> {
        let conn = self.write()?;
        chat::create_session(&conn, id, repo_path, title)
    }

    pub fn list_sessions(&self, repo_path: &str, limit: usize) -> Result<Vec<ChatSessionRow>> {
        let conn = self.read()?;
        chat::list_sessions(&conn, repo_path, limit)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<ChatSessionRow>> {
        let conn = self.read()?;
        chat::get_session(&conn, id)
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        let conn = self.write()?;
        chat::delete_session(&conn, id)
    }

    pub fn add_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        tool_calls_json: Option<&str>,
    ) -> Result<i64> {
        let conn = self.write()?;
        chat::add_message(&conn, session_id, role, content, tool_calls_json)
    }

    pub fn list_messages(&self, session_id: &str) -> Result<Vec<ChatMessageRow>> {
        let conn = self.read()?;
        chat::list_messages(&conn, session_id)
    }

    pub fn update_session_title(&self, id: &str, title: &str) -> Result<()> {
        let conn = self.write()?;
        chat::update_session_title(&conn, id, title)
    }

    pub fn touch_session(&self, id: &str) -> Result<()> {
        let conn = self.write()?;
        chat::touch_session(&conn, id)
    }
}

use crate::path::{Utf8Path, Utf8PathBuf};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::sync::Mutex;

/// Simple SQLite connection pool backed by Mutex<Vec<Connection>>.
///
/// Connections are lazily created (up to max_size) and reused via RAII guard.
/// Each connection is initialized with WAL mode, foreign keys, synchronous=NORMAL,
/// and busy_timeout=5000ms.
pub struct SqlitePool {
    db_path: Utf8PathBuf,
    pool: Mutex<Vec<Connection>>,
    max_size: usize,
    created: Mutex<usize>,
}

impl SqlitePool {
    /// Create a new connection pool.
    ///
    /// Connections are created lazily on first `get()` call, up to `max_size`.
    /// Creates parent directory if it doesn't exist.
    pub fn new(db_path: &Utf8Path, max_size: usize) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create db parent dir: {}", parent))?;
        }

        Ok(Self {
            db_path: db_path.to_path_buf(),
            pool: Mutex::new(Vec::with_capacity(max_size)),
            max_size,
            created: Mutex::new(0),
        })
    }

    /// Get a connection from the pool or create a new one.
    ///
    /// Returns error if max_size connections are already checked out.
    pub fn get(&self) -> Result<PooledConnection<'_>> {
        // Try to pop from pool
        if let Some(conn) = self.pool.lock().unwrap().pop() {
            return Ok(PooledConnection {
                conn: Some(conn),
                pool: self,
            });
        }

        // Create new connection if under max_size
        let mut created = self.created.lock().unwrap();
        if *created < self.max_size {
            let conn = self.create_connection()?;
            *created += 1;
            return Ok(PooledConnection {
                conn: Some(conn),
                pool: self,
            });
        }

        anyhow::bail!(
            "Connection pool exhausted: {} connections in use",
            self.max_size
        )
    }

    /// Try to get a connection without blocking.
    ///
    /// Returns None if all connections are checked out.
    pub fn try_get(&self) -> Option<PooledConnection<'_>> {
        // Try to pop from pool
        if let Some(conn) = self.pool.lock().unwrap().pop() {
            return Some(PooledConnection {
                conn: Some(conn),
                pool: self,
            });
        }

        // Try to create new connection if under max_size
        let mut created = self.created.lock().unwrap();
        if *created < self.max_size {
            let conn = self.create_connection().ok()?;
            *created += 1;
            return Some(PooledConnection {
                conn: Some(conn),
                pool: self,
            });
        }

        None
    }

    /// Number of connections currently available in the pool.
    pub fn available(&self) -> usize {
        self.pool.lock().unwrap().len()
    }

    /// Create a new connection with standard PRAGMAs.
    fn create_connection(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)
            .with_context(|| format!("Failed to open sqlite db: {}", self.db_path))?;

        // Enable WAL mode for better concurrent access
        let _ = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
            .ok();

        // Enable foreign key constraints
        conn.execute("PRAGMA foreign_keys=ON", [])
            .context("Failed to enable foreign keys")?;

        // Set synchronous mode to NORMAL for better performance
        let _ = conn.execute("PRAGMA synchronous=NORMAL", []).ok();

        // Set busy timeout to 5 seconds
        let _ = conn.execute("PRAGMA busy_timeout=5000", []).ok();

        Ok(conn)
    }

    /// Return a connection to the pool (called by PooledConnection::drop).
    fn return_connection(&self, conn: Connection) {
        self.pool.lock().unwrap().push(conn);
    }
}

/// RAII guard that returns connection to pool on drop.
pub struct PooledConnection<'a> {
    conn: Option<Connection>,
    pool: &'a SqlitePool,
}

impl std::fmt::Debug for PooledConnection<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledConnection")
            .field("has_connection", &self.conn.is_some())
            .finish()
    }
}

impl std::ops::Deref for PooledConnection<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("connection taken from PooledConnection")
    }
}

impl Drop for PooledConnection<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.return_connection(conn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn temp_db_path() -> Utf8PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid = std::process::id();
        Utf8PathBuf::from(format!("/tmp/sqlite_pool_test_{}_{}.db", pid, nanos))
    }

    #[test]
    fn pool_reuses_connections() {
        let db_path = temp_db_path();
        let pool = SqlitePool::new(&db_path, 2).unwrap();

        assert_eq!(pool.available(), 0);

        // Get and drop a connection
        {
            let _conn = pool.get().unwrap();
            assert_eq!(pool.available(), 0);
        }
        assert_eq!(pool.available(), 1);

        // Get the same connection again
        {
            let _conn = pool.get().unwrap();
            assert_eq!(pool.available(), 0);
        }
        assert_eq!(pool.available(), 1);

        // Cleanup
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn pool_respects_max_size() {
        let db_path = temp_db_path();
        let pool = SqlitePool::new(&db_path, 2).unwrap();

        // Checkout max connections
        let conn1 = pool.get().unwrap();
        let conn2 = pool.get().unwrap();
        assert_eq!(pool.available(), 0);

        // Next get should fail
        let result = pool.get();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Connection pool exhausted"));

        // try_get should return None
        assert!(pool.try_get().is_none());

        // Drop one connection
        drop(conn1);
        assert_eq!(pool.available(), 1);

        // Now get should succeed
        let _conn3 = pool.get().unwrap();
        assert_eq!(pool.available(), 0);

        // Cleanup
        drop(conn2);
        drop(_conn3);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn pool_connections_have_wal_and_fk() {
        let db_path = temp_db_path();
        let pool = SqlitePool::new(&db_path, 1).unwrap();

        let conn = pool.get().unwrap();

        // Check journal_mode is WAL
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");

        // Check foreign_keys are enabled
        let foreign_keys: i32 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);

        // Check synchronous mode (returns integer: 0=OFF, 1=NORMAL, 2=FULL, 3=EXTRA)
        let synchronous: i32 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();
        // SQLite returns 1 for NORMAL
        assert_eq!(synchronous, 1);

        // Check busy_timeout
        let busy_timeout: i32 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy_timeout, 5000);

        // Cleanup
        drop(conn);
        let _ = std::fs::remove_file(db_path);
    }
}

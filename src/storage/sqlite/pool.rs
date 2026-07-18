use crate::path::{Utf8Path, Utf8PathBuf};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::sync::{Condvar, Mutex};

#[derive(Default)]
struct PoolState {
    available: Vec<Connection>,
    created: usize,
}

/// Simple SQLite connection pool backed by Mutex<Vec<Connection>>.
///
/// Connections are lazily created (up to max_size) and reused via RAII guard.
/// Each connection is initialized with WAL mode, foreign keys, synchronous=NORMAL,
/// and busy_timeout=5000ms.
pub struct SqlitePool {
    db_path: Utf8PathBuf,
    state: Mutex<PoolState>,
    available: Condvar,
    max_size: usize,
}

impl SqlitePool {
    /// Create a new connection pool.
    ///
    /// Connections are created lazily on first `get()` call, up to `max_size`.
    /// Creates parent directory if it doesn't exist.
    pub fn new(db_path: &Utf8Path, max_size: usize) -> Result<Self> {
        anyhow::ensure!(max_size > 0, "SQLite pool size must be greater than zero");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create db parent dir: {}", parent))?;
        }

        Ok(Self {
            db_path: db_path.to_path_buf(),
            state: Mutex::new(PoolState {
                available: Vec::with_capacity(max_size),
                created: 0,
            }),
            available: Condvar::new(),
            max_size,
        })
    }

    /// Get a connection from the pool or create a new one.
    ///
    /// Waits when all connections are checked out. The returned guard wakes one
    /// waiter when it returns its connection to the pool.
    pub fn get(&self) -> Result<PooledConnection<'_>> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("SQLite pool lock is poisoned: {e}"))?;
        loop {
            if let Some(conn) = state.available.pop() {
                return Ok(PooledConnection {
                    conn: Some(conn),
                    pool: self,
                });
            }
            if state.created < self.max_size {
                let conn = self.create_connection()?;
                state.created += 1;
                return Ok(PooledConnection {
                    conn: Some(conn),
                    pool: self,
                });
            }
            state = self
                .available
                .wait(state)
                .map_err(|e| anyhow::anyhow!("SQLite pool lock is poisoned: {e}"))?;
        }
    }

    /// Try to get a connection without blocking.
    ///
    /// Returns None if all connections are checked out.
    pub fn try_get(&self) -> Option<PooledConnection<'_>> {
        let mut state = self.state.lock().ok()?;
        if let Some(conn) = state.available.pop() {
            return Some(PooledConnection {
                conn: Some(conn),
                pool: self,
            });
        }

        if state.created < self.max_size {
            let conn = self.create_connection().ok()?;
            state.created += 1;
            return Some(PooledConnection {
                conn: Some(conn),
                pool: self,
            });
        }

        None
    }

    /// Number of connections currently available in the pool.
    pub fn available(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .available
            .len()
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

        // Enforce the architectural split: pooled connections serve SELECTs;
        // all mutations go through SqliteStore's single writer connection.
        conn.execute("PRAGMA query_only=ON", [])
            .context("Failed to mark pooled SQLite connection query-only")?;

        Ok(conn)
    }

    /// Return a connection to the pool (called by PooledConnection::drop).
    fn return_connection(&self, conn: Connection) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .available
            .push(conn);
        self.available.notify_one();
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
    use std::sync::mpsc;
    use std::time::Duration;
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

        // Non-blocking checkout reports exhaustion.
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

        let query_only: i32 = conn
            .query_row("PRAGMA query_only", [], |row| row.get(0))
            .unwrap();
        assert_eq!(query_only, 1);

        // Cleanup
        drop(conn);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn pool_waits_until_a_connection_is_returned() {
        let db_path = temp_db_path();
        let pool = SqlitePool::new(&db_path, 1).unwrap();
        let first = pool.get().unwrap();
        let (tx, rx) = mpsc::channel();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _second = pool.get().unwrap();
                tx.send(()).unwrap();
            });

            assert!(rx.recv_timeout(Duration::from_millis(25)).is_err());
            drop(first);
            rx.recv_timeout(Duration::from_secs(1)).unwrap();
        });

        let _ = std::fs::remove_file(db_path);
    }
}

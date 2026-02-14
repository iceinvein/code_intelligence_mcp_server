//! Logging utilities: log cleanup and per-repo logging.

use crate::path::Utf8Path;

/// Delete log files older than `max_age_days` in `dir`.
///
/// Only targets files whose name contains ".log." to avoid deleting
/// non-log files. Silently skips if `dir` doesn't exist.
pub fn cleanup_old_logs(dir: &Utf8Path, max_age_days: u64) {
    let entries = match std::fs::read_dir(dir.as_std_path()) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    let max_age = std::time::Duration::from_secs(max_age_days * 24 * 3600);

    for entry in entries.flatten() {
        let path = entry.path();

        // Only target log files (name contains ".log.")
        match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.contains(".log.") => {}
            _ => continue,
        }

        let metadata = match path.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let age = match std::time::SystemTime::now().duration_since(modified) {
            Ok(d) => d,
            Err(_) => continue,
        };

        if age > max_age {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "Failed to remove old log file"
                );
            } else {
                tracing::debug!(
                    path = %path.display(),
                    age_days = age.as_secs() / 86400,
                    "Removed old log file"
                );
            }
        }
    }
}

/// Per-repo logger that writes directly to a rolling log file.
///
/// Unlike the global tracing subscriber, this writes to repo-specific
/// log files under `<repo_data_dir>/logs/`. Uses `tracing_appender`
/// for daily rotation and non-blocking I/O.
pub struct RepoLogger {
    // Guard must be dropped before writer to ensure flush completes
    _guard: tracing_appender::non_blocking::WorkerGuard,
    writer: tracing_appender::non_blocking::NonBlocking,
    repo_name: String,
}

impl RepoLogger {
    /// Create a new per-repo logger writing to `<repo_data_dir>/logs/index.log.YYYY-MM-DD`.
    ///
    /// Returns `None` if the logs directory can't be created.
    pub fn new(repo_data_dir: &Utf8Path) -> Option<Self> {
        let logs_dir = repo_data_dir.join("logs");
        std::fs::create_dir_all(logs_dir.as_std_path()).ok()?;

        let appender = tracing_appender::rolling::daily(logs_dir.as_std_path(), "index.log");
        let (writer, guard) = tracing_appender::non_blocking(appender);

        let repo_name = repo_data_dir
            .file_name()
            .unwrap_or("unknown")
            .to_string();

        Some(Self {
            writer,
            _guard: guard,
            repo_name,
        })
    }

    /// Write an INFO-level message to the per-repo log.
    pub fn info(&self, message: &str) {
        self.write("INFO", message);
    }

    /// Write a WARN-level message to the per-repo log.
    pub fn warn(&self, message: &str) {
        self.write("WARN", message);
    }

    /// Write an ERROR-level message to the per-repo log.
    pub fn error(&self, message: &str) {
        self.write("ERROR", message);
    }

    fn write(&self, level: &str, message: &str) {
        use std::io::Write;
        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let mut writer = self.writer.clone();
        let _ = writeln!(writer, "{} {} [{}] {}", timestamp, level, self.repo_name, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_cleanup_old_logs_removes_old_files() {
        let dir = TempDir::new().unwrap();
        let dir_path = dir.path();

        // Create a "fresh" log file (now)
        let fresh = dir_path.join("server.log.2026-02-14");
        fs::write(&fresh, "fresh").unwrap();

        // Create an "old" log file and backdate its mtime
        let old = dir_path.join("server.log.2026-01-01");
        fs::write(&old, "old").unwrap();
        let thirty_days_ago = std::time::SystemTime::now()
            - std::time::Duration::from_secs(30 * 24 * 3600);
        filetime::set_file_mtime(
            &old,
            filetime::FileTime::from_system_time(thirty_days_ago),
        )
        .unwrap();

        // Create a non-log file (should NOT be deleted even if old)
        let other = dir_path.join("config.toml");
        fs::write(&other, "config").unwrap();
        filetime::set_file_mtime(
            &other,
            filetime::FileTime::from_system_time(thirty_days_ago),
        )
        .unwrap();

        let dir_path_utf8 = crate::path::Utf8Path::from_path(dir_path).unwrap();
        cleanup_old_logs(dir_path_utf8, 7);

        assert!(fresh.exists(), "Fresh log should remain");
        assert!(!old.exists(), "Old log should be deleted");
        assert!(other.exists(), "Non-log file should remain");
    }

    #[test]
    fn test_cleanup_old_logs_ignores_missing_dir() {
        cleanup_old_logs(crate::path::Utf8Path::new("/nonexistent/path"), 7);
    }

    #[test]
    fn test_repo_logger_creates_log_dir_and_writes() {
        let dir = TempDir::new().unwrap();
        let repo_data_dir = crate::path::Utf8PathBuf::from_path_buf(
            dir.path().to_path_buf()
        ).unwrap();

        let logger = RepoLogger::new(&repo_data_dir).expect("should create logger");

        logger.info("Index started");
        logger.info("Indexed 42 files");

        // Flush by dropping
        drop(logger);

        // Check that a log file was created in <repo_data_dir>/logs/
        let logs_dir = dir.path().join("logs");
        assert!(logs_dir.exists(), "logs/ dir should be created");
        let entries: Vec<_> = std::fs::read_dir(&logs_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!entries.is_empty(), "Should have at least one log file");

        // Read and verify contents
        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("Index started"), "Should contain first message");
        assert!(content.contains("Indexed 42 files"), "Should contain second message");
    }
}

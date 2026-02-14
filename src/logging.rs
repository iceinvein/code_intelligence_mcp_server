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
}

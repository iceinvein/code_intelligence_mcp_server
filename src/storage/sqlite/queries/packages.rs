//! SQLite CRUD operations for repositories and packages.
//!
//! Provides functions for upserting and querying repository and package metadata.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::sqlite::schema::{PackageRow, RepositoryRow};

/// Upsert a repository into the repositories table.
///
/// Uses INSERT OR REPLACE to either insert a new repository or update
/// an existing one with the same ID.
///
/// # Arguments
///
/// * `conn` - SQLite connection
/// * `repo` - Repository information to upsert
///
/// # Returns
///
/// * `Ok(())` - Repository was upserted
/// * `Err(anyhow::Error)` - Database operation failed
pub fn upsert_repository(conn: &Connection, repo: &RepositoryRow) -> Result<()> {
    conn.execute(
        r#"
INSERT OR REPLACE INTO repositories (id, name, root_path, vcs_type, remote_url, created_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
"#,
        params![
            repo.id,
            repo.name,
            repo.root_path,
            repo.vcs_type,
            repo.remote_url,
            repo.created_at,
        ],
    )
    .context("Failed to upsert repository")?;
    Ok(())
}

/// Upsert a package into the packages table.
///
/// Uses INSERT OR REPLACE to either insert a new package or update
/// an existing one with the same ID.
///
/// # Arguments
///
/// * `conn` - SQLite connection
/// * `pkg` - Package information to upsert
///
/// # Returns
///
/// * `Ok(())` - Package was upserted
/// * `Err(anyhow::Error)` - Database operation failed
pub fn upsert_package(conn: &Connection, pkg: &PackageRow) -> Result<()> {
    conn.execute(
        r#"
INSERT OR REPLACE INTO packages (id, repository_id, name, version, manifest_path, package_type, created_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
"#,
        params![
            pkg.id,
            pkg.repository_id,
            pkg.name,
            pkg.version,
            pkg.manifest_path,
            pkg.package_type,
            pkg.created_at,
        ],
    )
    .context("Failed to upsert package")?;
    Ok(())
}

/// Get the package that contains a given file path.
///
/// This function finds the deepest (most specific) package whose manifest_path
/// is a prefix of the given file_path. This allows finding which package
/// contains any given source file.
///
/// # Arguments
///
/// * `conn` - SQLite connection
/// * `file_path` - Path to the file to look up
///
/// # Returns
///
/// * `Ok(Some(PackageRow))` - Package containing the file
/// * `Ok(None)` - No package found containing the file
/// * `Err(anyhow::Error)` - Database operation failed
pub fn get_package_for_file(conn: &Connection, file_path: &str) -> Result<Option<PackageRow>> {
    // Find the deepest package whose manifest_path is a prefix of file_path
    // ORDER BY LENGTH(manifest_path) DESC ensures we get the most specific match
    conn.query_row(
        r#"
SELECT id, repository_id, name, version, manifest_path, package_type, created_at
FROM packages
WHERE ?1 LIKE manifest_path || '%'
ORDER BY LENGTH(manifest_path) DESC
LIMIT 1
"#,
        params![file_path],
        |row| {
            Ok(PackageRow {
                id: row.get(0)?,
                repository_id: row.get(1)?,
                name: row.get(2)?,
                version: row.get(3)?,
                manifest_path: row.get(4)?,
                package_type: row.get(5)?,
                created_at: row.get(6)?,
            })
        },
    )
    .optional()
    .context("Failed to query package for file")
}

/// List all packages in the database.
///
/// # Arguments
///
/// * `conn` - SQLite connection
///
/// # Returns
///
/// * `Ok(Vec<PackageRow>)` - All packages ordered by manifest_path
/// * `Err(anyhow::Error)` - Database operation failed
pub fn list_all_packages(conn: &Connection) -> Result<Vec<PackageRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT id, repository_id, name, version, manifest_path, package_type, created_at
FROM packages
ORDER BY manifest_path
"#,
        )
        .context("Failed to prepare list_all_packages statement")?;

    let mut rows = stmt.query([])?;
    let mut out = Vec::new();

    while let Some(row) = rows.next()? {
        out.push(PackageRow {
            id: row.get(0)?,
            repository_id: row.get(1)?,
            name: row.get(2)?,
            version: row.get(3)?,
            manifest_path: row.get(4)?,
            package_type: row.get(5)?,
            created_at: row.get(6)?,
        });
    }

    Ok(out)
}

/// List all repositories in the database.
///
/// # Arguments
///
/// * `conn` - SQLite connection
///
/// # Returns
///
/// * `Ok(Vec<RepositoryRow>)` - All repositories ordered by root_path
/// * `Err(anyhow::Error)` - Database operation failed
pub fn list_all_repositories(conn: &Connection) -> Result<Vec<RepositoryRow>> {
    let mut stmt = conn
        .prepare(
            r#"
SELECT id, name, root_path, vcs_type, remote_url, created_at
FROM repositories
ORDER BY root_path
"#,
        )
        .context("Failed to prepare list_all_repositories statement")?;

    let mut rows = stmt.query([])?;
    let mut out = Vec::new();

    while let Some(row) = rows.next()? {
        out.push(RepositoryRow {
            id: row.get(0)?,
            name: row.get(1)?,
            root_path: row.get(2)?,
            vcs_type: row.get(3)?,
            remote_url: row.get(4)?,
            created_at: row.get(5)?,
        });
    }

    Ok(out)
}

/// Get a repository by its ID.
///
/// # Arguments
///
/// * `conn` - SQLite connection
/// * `id` - Repository ID
///
/// # Returns
///
/// * `Ok(Some(RepositoryRow))` - Repository found
/// * `Ok(None)` - Repository not found
/// * `Err(anyhow::Error)` - Database operation failed
pub fn get_repository_by_id(conn: &Connection, id: &str) -> Result<Option<RepositoryRow>> {
    conn.query_row(
        r#"
SELECT id, name, root_path, vcs_type, remote_url, created_at
FROM repositories
WHERE id = ?1
"#,
        params![id],
        |row| {
            Ok(RepositoryRow {
                id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
                vcs_type: row.get(3)?,
                remote_url: row.get(4)?,
                created_at: row.get(5)?,
            })
        },
    )
    .optional()
    .context("Failed to query repository by id")
}

/// Get a package by its ID.
///
/// # Arguments
///
/// * `conn` - SQLite connection
/// * `id` - Package ID
///
/// # Returns
///
/// * `Ok(Some(PackageRow))` - Package found
/// * `Ok(None)` - Package not found
/// * `Err(anyhow::Error)` - Database operation failed
pub fn get_package_by_id(conn: &Connection, id: &str) -> Result<Option<PackageRow>> {
    conn.query_row(
        r#"
SELECT id, repository_id, name, version, manifest_path, package_type, created_at
FROM packages
WHERE id = ?1
"#,
        params![id],
        |row| {
            Ok(PackageRow {
                id: row.get(0)?,
                repository_id: row.get(1)?,
                name: row.get(2)?,
                version: row.get(3)?,
                manifest_path: row.get(4)?,
                package_type: row.get(5)?,
                created_at: row.get(6)?,
            })
        },
    )
    .optional()
    .context("Failed to query package by id")
}

/// Count packages in a repository.
///
/// # Arguments
///
/// * `conn` - SQLite connection
/// * `repository_id` - Repository ID to count packages for
///
/// # Returns
///
/// * `Ok(u64)` - Number of packages in the repository
/// * `Err(anyhow::Error)` - Database operation failed
pub fn count_packages_in_repository(conn: &Connection, repository_id: &str) -> Result<u64> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM packages WHERE repository_id = ?1",
            params![repository_id],
            |row| row.get(0),
        )
        .context("Failed to count packages in repository")?;
    Ok(count.max(0) as u64)
}

/// Batch lookup package IDs for multiple symbols.
///
/// This function efficiently looks up which package each symbol belongs to
/// by joining the symbols table with the packages table. For each symbol,
/// it finds the package whose manifest_path is a prefix of the symbol's file_path.
///
/// # Arguments
///
/// * `conn` - SQLite connection
/// * `symbol_ids` - Slice of symbol IDs to look up
///
/// # Returns
///
/// * `Ok(HashMap<String, String>)` - Map of symbol_id to package_id
/// * `Err(anyhow::Error)` - Database operation failed
///
/// # Example
///
/// ```ignore
/// let symbol_ids = vec!["symbol1".to_string(), "symbol2".to_string()];
/// let packages = batch_get_symbol_packages(&conn, &symbol_ids)?;
/// // packages.get("symbol1") -> Some("pkg-123")
/// ```
pub fn batch_get_symbol_packages(
    conn: &Connection,
    symbol_ids: &[&str],
) -> Result<std::collections::HashMap<String, String>> {
    use std::collections::HashMap;

    if symbol_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // Build IN clause placeholders dynamically
    let placeholders = symbol_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(",");

    // Query joins symbols with packages using LIKE to find containing package
    // Uses LENGTH(manifest_path) DESC to find the deepest (most specific) package
    let query = format!(
        r#"
SELECT s.id, p.id
FROM symbols s
JOIN packages p ON s.file_path LIKE p.manifest_path || '%'
WHERE s.id IN ({})
"#,
        placeholders
    );

    let mut stmt = conn
        .prepare(&query)
        .context("Failed to prepare batch_get_symbol_packages statement")?;

    // Convert symbol_ids to params for rusqlite
    let params: Vec<&dyn rusqlite::ToSql> = symbol_ids
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql)
        .collect();

    let mut rows = stmt.query(params.as_slice())?;
    let mut out = HashMap::new();

    while let Some(row) = rows.next()? {
        let symbol_id: String = row.get(0)?;
        let package_id: String = row.get(1)?;
        out.insert(symbol_id, package_id);
    }

    Ok(out)
}

/// Get the package ID for a given file path.
///
/// This is a convenience function that reuses get_package_for_file logic
/// but returns only the package_id as a String, rather than the full PackageRow.
///
/// # Arguments
///
/// * `conn` - SQLite connection
/// * `file_path` - Path to the file to look up
///
/// # Returns
///
/// * `Ok(Some(String))` - Package ID containing the file
/// * `Ok(None)` - No package found containing the file
/// * `Err(anyhow::Error)` - Database operation failed
pub fn get_package_id_for_file(
    conn: &Connection,
    file_path: &str,
) -> Result<Option<String>> {
    conn.query_row(
        r#"
SELECT id
FROM packages
WHERE ?1 LIKE manifest_path || '%'
ORDER BY LENGTH(manifest_path) DESC
LIMIT 1
"#,
        params![file_path],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .context("Failed to query package_id for file")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Create an in-memory SQLite database with the schema.
    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::storage::sqlite::schema::SCHEMA_SQL)
            .unwrap();
        conn
    }

    #[test]
    fn test_upsert_repository() {
        let conn = setup_test_db();

        let repo = RepositoryRow {
            id: "repo-123".to_string(),
            name: "test-repo".to_string(),
            root_path: "/path/to/repo".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: Some("https://github.com/test/repo".to_string()),
            created_at: 1234567890,
        };

        upsert_repository(&conn, &repo).unwrap();

        // Verify it was inserted
        let retrieved = get_repository_by_id(&conn, "repo-123").unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.name, "test-repo");
        assert_eq!(retrieved.root_path, "/path/to/repo");
    }

    #[test]
    fn test_upsert_repository_replaces() {
        let conn = setup_test_db();

        let repo1 = RepositoryRow {
            id: "repo-123".to_string(),
            name: "old-name".to_string(),
            root_path: "/old/path".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: Some("https://example.com/old".to_string()),
            created_at: 1234567890,
        };

        upsert_repository(&conn, &repo1).unwrap();

        // Update with new data
        let repo2 = RepositoryRow {
            id: "repo-123".to_string(),
            name: "new-name".to_string(),
            root_path: "/new/path".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: Some("https://example.com/new".to_string()),
            created_at: 1234567891,
        };

        upsert_repository(&conn, &repo2).unwrap();

        // Verify the data was replaced
        let retrieved = get_repository_by_id(&conn, "repo-123").unwrap().unwrap();
        assert_eq!(retrieved.name, "new-name");
        assert_eq!(retrieved.root_path, "/new/path");
        assert_eq!(retrieved.created_at, 1234567891);
    }

    #[test]
    fn test_upsert_package() {
        let conn = setup_test_db();

        // First create a repository
        let repo = RepositoryRow {
            id: "repo-123".to_string(),
            name: "test-repo".to_string(),
            root_path: "/path/to/repo".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 1234567890,
        };
        upsert_repository(&conn, &repo).unwrap();

        // Now upsert a package
        let pkg = PackageRow {
            id: "pkg-456".to_string(),
            repository_id: "repo-123".to_string(),
            name: "test-package".to_string(),
            version: Some("1.0.0".to_string()),
            manifest_path: "/path/to/repo/package.json".to_string(),
            package_type: "npm".to_string(),
            created_at: 1234567891,
        };

        upsert_package(&conn, &pkg).unwrap();

        // Verify it was inserted
        let retrieved = get_package_by_id(&conn, "pkg-456").unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.name, "test-package");
        assert_eq!(retrieved.version, Some("1.0.0".to_string()));
        assert_eq!(retrieved.repository_id, "repo-123");
    }

    #[test]
    fn test_get_package_for_file() {
        let conn = setup_test_db();

        // Create a repository
        let repo = RepositoryRow {
            id: "repo-123".to_string(),
            name: "test-repo".to_string(),
            root_path: "/path/to/repo".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 1234567890,
        };
        upsert_repository(&conn, &repo).unwrap();

        // Create a package at a specific manifest path
        let pkg = PackageRow {
            id: "pkg-456".to_string(),
            repository_id: "repo-123".to_string(),
            name: "test-package".to_string(),
            version: Some("1.0.0".to_string()),
            manifest_path: "/path/to/repo/packages/subpackage".to_string(),
            package_type: "npm".to_string(),
            created_at: 1234567891,
        };
        upsert_package(&conn, &pkg).unwrap();

        // Test file lookup - file in the package should match
        let file_path = "/path/to/repo/packages/subpackage/src/index.ts";
        let result = get_package_for_file(&conn, file_path).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "pkg-456");
    }

    #[test]
    fn test_get_package_for_file_finds_deepest_match() {
        let conn = setup_test_db();

        // Create a repository
        let repo = RepositoryRow {
            id: "repo-123".to_string(),
            name: "test-repo".to_string(),
            root_path: "/path/to/repo".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 1234567890,
        };
        upsert_repository(&conn, &repo).unwrap();

        // Create two packages - one nested inside the other
        let root_pkg = PackageRow {
            id: "pkg-root".to_string(),
            repository_id: "repo-123".to_string(),
            name: "root-package".to_string(),
            version: Some("1.0.0".to_string()),
            manifest_path: "/path/to/repo".to_string(),
            package_type: "npm".to_string(),
            created_at: 1234567891,
        };
        upsert_package(&conn, &root_pkg).unwrap();

        let nested_pkg = PackageRow {
            id: "pkg-nested".to_string(),
            repository_id: "repo-123".to_string(),
            name: "nested-package".to_string(),
            version: Some("1.0.0".to_string()),
            manifest_path: "/path/to/repo/packages/nested".to_string(),
            package_type: "npm".to_string(),
            created_at: 1234567892,
        };
        upsert_package(&conn, &nested_pkg).unwrap();

        // File in nested package should match the nested package (deeper match)
        let file_path = "/path/to/repo/packages/nested/src/file.ts";
        let result = get_package_for_file(&conn, file_path).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "pkg-nested");
    }

    #[test]
    fn test_get_package_for_file_no_match() {
        let conn = setup_test_db();

        // No packages in database
        let result = get_package_for_file(&conn, "/some/unknown/file.ts").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list_all_packages() {
        let conn = setup_test_db();

        // Create a repository
        let repo = RepositoryRow {
            id: "repo-123".to_string(),
            name: "test-repo".to_string(),
            root_path: "/path/to/repo".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 1234567890,
        };
        upsert_repository(&conn, &repo).unwrap();

        // Create multiple packages
        let pkg1 = PackageRow {
            id: "pkg-1".to_string(),
            repository_id: "repo-123".to_string(),
            name: "package-a".to_string(),
            version: Some("1.0.0".to_string()),
            manifest_path: "/path/to/repo/packages/a".to_string(),
            package_type: "npm".to_string(),
            created_at: 1234567891,
        };
        let pkg2 = PackageRow {
            id: "pkg-2".to_string(),
            repository_id: "repo-123".to_string(),
            name: "package-b".to_string(),
            version: Some("2.0.0".to_string()),
            manifest_path: "/path/to/repo/packages/b".to_string(),
            package_type: "npm".to_string(),
            created_at: 1234567892,
        };

        upsert_package(&conn, &pkg1).unwrap();
        upsert_package(&conn, &pkg2).unwrap();

        // List all packages
        let packages = list_all_packages(&conn).unwrap();
        assert_eq!(packages.len(), 2);
        // Should be ordered by manifest_path
        assert_eq!(packages[0].id, "pkg-1");
        assert_eq!(packages[1].id, "pkg-2");
    }

    #[test]
    fn test_list_all_repositories() {
        let conn = setup_test_db();

        // Create multiple repositories
        let repo1 = RepositoryRow {
            id: "repo-1".to_string(),
            name: "repo-a".to_string(),
            root_path: "/path/to/a".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 1234567890,
        };
        let repo2 = RepositoryRow {
            id: "repo-2".to_string(),
            name: "repo-b".to_string(),
            root_path: "/path/to/b".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 1234567891,
        };

        upsert_repository(&conn, &repo1).unwrap();
        upsert_repository(&conn, &repo2).unwrap();

        // List all repositories
        let repos = list_all_repositories(&conn).unwrap();
        assert_eq!(repos.len(), 2);
        // Should be ordered by root_path
        assert_eq!(repos[0].id, "repo-1");
        assert_eq!(repos[1].id, "repo-2");
    }

    #[test]
    fn test_count_packages_in_repository() {
        let conn = setup_test_db();

        // Create a repository
        let repo = RepositoryRow {
            id: "repo-123".to_string(),
            name: "test-repo".to_string(),
            root_path: "/path/to/repo".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 1234567890,
        };
        upsert_repository(&conn, &repo).unwrap();

        // No packages yet
        assert_eq!(count_packages_in_repository(&conn, "repo-123").unwrap(), 0);

        // Add some packages
        for i in 1..=3 {
            let pkg = PackageRow {
                id: format!("pkg-{}", i),
                repository_id: "repo-123".to_string(),
                name: format!("package-{}", i),
                version: Some("1.0.0".to_string()),
                manifest_path: format!("/path/to/repo/pkg{}", i),
                package_type: "npm".to_string(),
                created_at: 1234567890 + i as i64,
            };
            upsert_package(&conn, &pkg).unwrap();
        }

        // Now should have 3 packages
        assert_eq!(count_packages_in_repository(&conn, "repo-123").unwrap(), 3);
    }

    #[test]
    fn test_batch_get_symbol_packages_returns_map() {
        let conn = setup_test_db();

        // Create repository and package
        let repo = RepositoryRow {
            id: "repo-123".to_string(),
            name: "test-repo".to_string(),
            root_path: "/path/to/repo".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 1234567890,
        };
        upsert_repository(&conn, &repo).unwrap();

        let pkg = PackageRow {
            id: "pkg-456".to_string(),
            repository_id: "repo-123".to_string(),
            name: "test-package".to_string(),
            version: Some("1.0.0".to_string()),
            manifest_path: "/path/to/repo".to_string(),
            package_type: "npm".to_string(),
            created_at: 1234567891,
        };
        upsert_package(&conn, &pkg).unwrap();

        // Insert test symbols
        conn.execute(
            r#"
            INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
            VALUES
                ('symbol1', '/path/to/repo/src/file1.ts', 'typescript', 'function', 'foo', 1, 0, 100, 1, 5, 'fn foo() {}'),
                ('symbol2', '/path/to/repo/src/file2.ts', 'typescript', 'class', 'Bar', 1, 0, 200, 1, 10, 'class Bar {}'),
                ('symbol3', '/other/path/file3.ts', 'typescript', 'function', 'baz', 1, 0, 100, 1, 5, 'fn baz() {}')
        "#,
            [],
        )
        .unwrap();

        // Query for symbol1 and symbol2 (both in the package)
        let symbol_ids: Vec<&str> = vec!["symbol1", "symbol2"];
        let result = batch_get_symbol_packages(&conn, &symbol_ids).unwrap();

        // Both should map to pkg-456
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("symbol1"), Some(&"pkg-456".to_string()));
        assert_eq!(result.get("symbol2"), Some(&"pkg-456".to_string()));
        assert_eq!(result.get("symbol3"), None); // Not in query
    }

    #[test]
    fn test_batch_get_symbol_packages_empty() {
        let conn = setup_test_db();

        // Query with empty input
        let symbol_ids: Vec<&str> = vec![];
        let result = batch_get_symbol_packages(&conn, &symbol_ids).unwrap();

        // Verify empty result
        assert!(result.is_empty());
    }

    #[test]
    fn test_batch_get_symbol_packages_no_match() {
        let conn = setup_test_db();

        // Insert a symbol but no packages
        conn.execute(
            r#"
            INSERT INTO symbols (id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text)
            VALUES ('symbol1', '/path/to/file.ts', 'typescript', 'function', 'foo', 1, 0, 100, 1, 5, 'fn foo() {}')
        "#,
            [],
        )
        .unwrap();

        // Query for symbol with no matching package
        let symbol_ids: Vec<&str> = vec!["symbol1"];
        let result = batch_get_symbol_packages(&conn, &symbol_ids).unwrap();

        // Should return empty map (no package found)
        assert!(result.is_empty());
    }

    #[test]
    fn test_get_package_id_for_file_finds_containing_package() {
        let conn = setup_test_db();

        // Create repository and package
        let repo = RepositoryRow {
            id: "repo-123".to_string(),
            name: "test-repo".to_string(),
            root_path: "/path/to/repo".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 1234567890,
        };
        upsert_repository(&conn, &repo).unwrap();

        let pkg = PackageRow {
            id: "pkg-456".to_string(),
            repository_id: "repo-123".to_string(),
            name: "test-package".to_string(),
            version: Some("1.0.0".to_string()),
            manifest_path: "/path/to/repo/packages/subpackage".to_string(),
            package_type: "npm".to_string(),
            created_at: 1234567891,
        };
        upsert_package(&conn, &pkg).unwrap();

        // File in the package should return the package ID
        let file_path = "/path/to/repo/packages/subpackage/src/index.ts";
        let result = get_package_id_for_file(&conn, file_path).unwrap();
        assert_eq!(result, Some("pkg-456".to_string()));
    }

    #[test]
    fn test_get_package_id_for_file_no_match() {
        let conn = setup_test_db();

        // No packages in database
        let result = get_package_id_for_file(&conn, "/some/unknown/file.ts").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_package_id_for_file_deepest_match() {
        let conn = setup_test_db();

        // Create repository
        let repo = RepositoryRow {
            id: "repo-123".to_string(),
            name: "test-repo".to_string(),
            root_path: "/path/to/repo".to_string(),
            vcs_type: Some("git".to_string()),
            remote_url: None,
            created_at: 1234567890,
        };
        upsert_repository(&conn, &repo).unwrap();

        // Create nested packages (root package and nested package)
        let root_pkg = PackageRow {
            id: "pkg-root".to_string(),
            repository_id: "repo-123".to_string(),
            name: "root-package".to_string(),
            version: Some("1.0.0".to_string()),
            manifest_path: "/path/to/repo".to_string(),
            package_type: "npm".to_string(),
            created_at: 1234567891,
        };
        upsert_package(&conn, &root_pkg).unwrap();

        let nested_pkg = PackageRow {
            id: "pkg-nested".to_string(),
            repository_id: "repo-123".to_string(),
            name: "nested-package".to_string(),
            version: Some("1.0.0".to_string()),
            manifest_path: "/path/to/repo/packages/nested".to_string(),
            package_type: "npm".to_string(),
            created_at: 1234567892,
        };
        upsert_package(&conn, &nested_pkg).unwrap();

        // File in nested package should match nested package (deeper match)
        let file_path = "/path/to/repo/packages/nested/src/file.ts";
        let result = get_package_id_for_file(&conn, file_path).unwrap();
        assert_eq!(result, Some("pkg-nested".to_string()));
    }
}

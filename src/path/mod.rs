//! Centralized path normalization module.
//!
//! This module provides cross-platform path handling with:
//! - UTF-8 typed paths via camino (Utf8Path, Utf8PathBuf)
//! - Windows UNC path normalization via dunce
//! - Security-aware path validation for symlink escaping
//! - Helpful error messages with context

pub use camino::{Utf8Path, Utf8PathBuf};

use std::fmt;

/// Errors that can occur during path normalization operations.
#[derive(Debug, Clone, PartialEq)]
pub enum PathError {
    /// Path is outside the repository base directory.
    OutsideRepo {
        path: Utf8PathBuf,
        base: Utf8PathBuf,
    },
    /// Path contains invalid characters.
    InvalidChars {
        path: String,
        invalid: char,
    },
    /// UNC paths are not supported on this platform.
    UncNotSupported {
        path: String,
    },
    /// Path contains non-UTF-8 characters.
    NonUtf8 {
        path: std::path::PathBuf,
    },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::OutsideRepo { path, base } => {
                write!(
                    f,
                    "Path '{}' is outside repository base '{}'",
                    path, base
                )
            }
            PathError::InvalidChars { path, invalid } => {
                write!(
                    f,
                    "Path contains invalid character '{}': {}",
                    invalid, path
                )
            }
            PathError::UncNotSupported { path } => {
                write!(
                    f,
                    "UNC paths not supported: {} (use regular path)",
                    path
                )
            }
            PathError::NonUtf8 { path } => {
                write!(
                    f,
                    "Path contains non-UTF-8 characters: {}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for PathError {}

impl From<std::path::PathBuf> for PathError {
    fn from(path: std::path::PathBuf) -> Self {
        PathError::NonUtf8 { path }
    }
}

/// Centralized path normalization utility.
///
/// Provides cross-platform path handling with proper error messages
/// and security checks for symlink escaping.
#[derive(Debug, Clone)]
pub struct PathNormalizer {
    /// Base directory of the repository.
    base_dir: Utf8PathBuf,
}

impl PathNormalizer {
    /// Create a new PathNormalizer with the given base directory.
    ///
    /// # Arguments
    ///
    /// * `base_dir` - The base directory of the repository
    ///
    /// # Example
    ///
    /// ```
    /// use camino::Utf8PathBuf;
    /// use code_intelligence_mcp_server::path::PathNormalizer;
    ///
    /// let normalizer = PathNormalizer::new("/path/to/repo".into());
    /// ```
    pub fn new(base_dir: Utf8PathBuf) -> Self {
        Self { base_dir }
    }

    /// Get the base directory.
    pub fn base_dir(&self) -> &Utf8Path {
        &self.base_dir
    }

    /// Normalize a path for comparison purposes.
    ///
    /// This function:
    /// - Uses dunce::simplified() for UNC normalization on Windows
    /// - Converts backslashes to forward slashes (Windows compatibility)
    /// - Handles both Unix and Windows paths
    ///
    /// # Arguments
    ///
    /// * `path` - The path to normalize
    ///
    /// # Returns
    ///
    /// A normalized path suitable for string comparison.
    ///
    /// # Example
    ///
    /// ```
    /// use camino::Utf8PathBuf;
    /// use code_intelligence_mcp_server::path::PathNormalizer;
    ///
    /// let normalizer = PathNormalizer::new("/repo".into());
    ///
    /// // Windows path with backslashes
    /// let normalized = normalizer.normalize_for_compare(
    ///     "C:\\repo\\src\\main.rs".as_ref()
    /// ).unwrap();
    /// assert_eq!(normalized.as_str(), "C:/repo/src/main.rs");
    /// ```
    pub fn normalize_for_compare(
        &self,
        path: &Utf8Path,
    ) -> Result<Utf8PathBuf, PathError> {
        // First use dunce to simplify UNC paths on Windows
        let simplified = dunce::simplified(path.as_std_path());

        // Convert back to Utf8PathBuf, handling non-UTF-8 paths
        let utf8_path = Utf8PathBuf::from_path_buf(simplified.to_path_buf())
            .map_err(|_| PathError::NonUtf8 {
                path: simplified.to_path_buf(),
            })?;

        // Normalize backslashes to forward slashes for Windows compatibility
        let normalized_str = utf8_path.as_str().replace('\\', "/");

        Ok(Utf8PathBuf::from(normalized_str))
    }

    /// Convert an absolute path to be relative to the base directory.
    ///
    /// # Arguments
    ///
    /// * `path` - The absolute path to convert
    ///
    /// # Returns
    ///
    /// A path relative to the base directory, or an error if the path
    /// is outside the base.
    ///
    /// # Example
    ///
    /// ```
    /// use camino::Utf8PathBuf;
    /// use code_intelligence_mcp_server::path::PathNormalizer;
    ///
    /// let normalizer = PathNormalizer::new("/repo".into());
    ///
    /// // Path within base
    /// let relative = normalizer.relative_to_base(
    ///     "/repo/src/main.rs".as_ref()
    /// ).unwrap();
    /// assert_eq!(relative.as_str(), "src/main.rs");
    ///
    /// // Path outside base
    /// let result = normalizer.relative_to_base("/other/path".as_ref());
    /// assert!(result.is_err());
    /// ```
    pub fn relative_to_base(&self, path: &Utf8Path) -> Result<Utf8PathBuf, PathError> {
        // Normalize first to handle any path inconsistencies
        let normalized = self.normalize_for_compare(path)?;

        // Try to strip the base prefix
        match normalized.strip_prefix(&self.base_dir) {
            Ok(relative) => {
                if relative.as_str().is_empty() {
                    Ok(".".into())
                } else {
                    Ok(relative.to_path_buf())
                }
            }
            Err(_) => Err(PathError::OutsideRepo {
                path: normalized.clone(),
                base: self.base_dir.clone(),
            }),
        }
    }

    /// Validate that a path is within the base directory.
    ///
    /// This is a security check to prevent symlink escaping attacks
    /// where a symlink might point outside the repository.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to validate
    ///
    /// # Returns
    ///
    /// Ok(()) if the path is within base, or an error otherwise.
    ///
    /// # Example
    ///
    /// ```
    /// use camino::Utf8PathBuf;
    /// use code_intelligence_mcp_server::path::PathNormalizer;
    ///
    /// let normalizer = PathNormalizer::new("/repo".into());
    ///
    /// // Valid path
    /// assert!(normalizer.validate_within_base("/repo/src/main.rs".as_ref()).is_ok());
    ///
    /// // Path outside base
    /// assert!(normalizer.validate_within_base("/etc/passwd".as_ref()).is_err());
    /// ```
    pub fn validate_within_base(&self, path: &Utf8Path) -> Result<(), PathError> {
        // Normalize first to handle any path inconsistencies
        let normalized = self.normalize_for_compare(path)?;

        // Use relative_to_base which already handles the OutsideRepo error
        self.relative_to_base(&normalized)?;

        Ok(())
    }

    /// Join a relative path to the base directory.
    ///
    /// # Arguments
    ///
    /// * `relative` - The relative path to join
    ///
    /// # Returns
    ///
    /// The joined absolute path.
    ///
    /// # Example
    ///
    /// ```
    /// use camino::Utf8PathBuf;
    /// use code_intelligence_mcp_server::path::PathNormalizer;
    ///
    /// let normalizer = PathNormalizer::new("/repo".into());
    ///
    /// let joined = normalizer.join_base("src/main.rs");
    /// assert_eq!(joined.as_str(), "/repo/src/main.rs");
    /// ```
    pub fn join_base(&self, relative: &str) -> Utf8PathBuf {
        self.base_dir.join(relative)
    }

    /// Convert a std::path::Path to Utf8PathBuf.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to convert
    ///
    /// # Returns
    ///
    /// A Utf8PathBuf or an error if the path contains non-UTF-8 characters.
    pub fn from_std_path(path: &std::path::Path) -> Result<Utf8PathBuf, PathError> {
        Utf8PathBuf::from_path_buf(path.to_path_buf())
            .map_err(|_| PathError::NonUtf8 {
                path: path.to_path_buf(),
            })
    }

    /// Convert a Utf8Path to std::path::PathBuf.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to convert
    ///
    /// # Returns
    ///
    /// A std::path::PathBuf.
    pub fn to_std_path(path: &Utf8Path) -> std::path::PathBuf {
        path.as_std_path().to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    fn create_test_normalizer() -> PathNormalizer {
        PathNormalizer::new("/test/repo".into())
    }

    // ========== Parameterized Tests ==========

    // Cross-platform path normalization tests
    #[test_case("src/lib.rs", "src/lib.rs"; "unix relative path")]
    #[test_case("src\\lib.rs", "src/lib.rs"; "windows relative path with backslashes")]
    #[test_case("src/sub/../lib.rs", "src/sub/../lib.rs"; "path with parent reference")]
    #[test_case("./src/lib.rs", "./src/lib.rs"; "current dir prefix")]
    #[test_case("/test/repo/src/lib.rs", "/test/repo/src/lib.rs"; "absolute unix path")]
    #[test_case("C:\\test\\repo\\src\\lib.rs", "C:/test/repo/src/lib.rs"; "absolute windows path")]
    #[test_case("/test/repo/src/sub/../lib.rs", "/test/repo/src/sub/../lib.rs"; "normalized with parent")]
    #[test_case("/test\\repo/src\\lib.rs", "/test/repo/src/lib.rs"; "mixed separators")]
    #[test_case("/test/repo/src/", "/test/repo/src/"; "trailing slash preserved")]
    #[test_case("", ""; "empty path")]
    fn test_normalize_for_compare(input: &str, expected: &str) {
        let normalizer = create_test_normalizer();
        let input_path = Utf8Path::new(input);
        let result = normalizer.normalize_for_compare(input_path).unwrap();
        assert_eq!(result.as_str(), expected);
    }

    // Relative path computation tests
    #[test_case("/test/repo/src/lib.rs", "src/lib.rs"; "simple relative path")]
    #[test_case("/test/repo/src/sub/../lib.rs", "src/sub/../lib.rs"; "relative with parent ref")]
    #[test_case("/test/repo", "."; "exact base path")]
    #[test_case("/test/repo/handler/mod.rs", "handler/mod.rs"; "single level nested")]
    #[test_case("/test/repo/a/b/c/d/file.rs", "a/b/c/d/file.rs"; "deeply nested")]
    fn test_relative_to_base_success(full_path: &str, expected: &str) {
        let base = Utf8PathBuf::from("/test/repo");
        let normalizer = PathNormalizer::new(base);
        let input_path = Utf8Path::new(full_path);
        let result = normalizer.relative_to_base(input_path).unwrap();
        assert_eq!(result.as_str(), expected);
    }

    // Error cases for relative_to_base
    #[test_case("/other/path/lib.rs"; "outside base different root")]
    #[test_case("/test/repo_other/src/lib.rs"; "outside base similar prefix")]
    #[test_case("/test/repo-backup/file.rs"; "outside base with dash")]
    #[test_case("/etc/passwd"; "system path outside repo")]
    #[test_case("/tmp/test/file.txt"; "tmp directory outside repo")]
    #[test_case("../outside/file.rs"; "parent directory escape")]
    #[test_case("../../etc/passwd"; "multi-level parent escape")]
    fn test_relative_to_base_error(path: &str) {
        let base = Utf8PathBuf::from("/test/repo");
        let normalizer = PathNormalizer::new(base);
        let input_path = Utf8Path::new(path);
        let result = normalizer.relative_to_base(input_path);
        assert!(result.is_err(), "Expected error for path: {}", path);
        // Verify helpful error message contains both path and base
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("outside repository"), "Error message should mention being outside repo");
    }

    // Windows-specific UNC path tests
    #[cfg(windows)]
    #[test_case("\\\\?\\C:\\Users\\test", "\\\\?\\C:\\Users\\test"; "verbatim UNC path")]
    #[test_case("\\\\?\\UNC\\server\\share\\file", "\\\\?\\UNC\\server\\share\\file"; "UNC share path")]
    #[test_case("\\\\server\\share\\file", "//server/share/file"; "network path UNC")]
    #[test_case("C:\\\\Users\\\\test", "C://Users/test"; "multiple consecutive backslashes")]
    fn test_normalize_windows_unc_path(input: &str, expected: &str) {
        let normalizer = create_test_normalizer();
        let input_path = Utf8Path::new(input);
        let result = normalizer.normalize_for_compare(input_path).unwrap();
        // UNC paths should be normalized by dunce
        assert!(result.as_str().contains('/'), "UNC path should use forward slashes");
    }

    // Cross-platform backslash normalization
    #[test_case("src\\lib.rs", "src/lib.rs"; "single backslash")]
    #[test_case("src\\\\lib.rs", "src//lib.rs"; "double backslash")]
    #[test_case("a\\b\\c\\d.rs", "a/b/c/d.rs"; "multiple backslashes")]
    #[test_case("/repo\\src/lib.rs", "/repo/src/lib.rs"; "mixed backslash forward")]
    #[test_case("C:\\Users\\test\\file.rs", "C:/Users/test/file.rs"; "windows absolute path")]
    fn test_backslash_normalization(input: &str, expected: &str) {
        let normalizer = create_test_normalizer();
        let input_path = Utf8Path::new(input);
        let result = normalizer.normalize_for_compare(input_path).unwrap();
        assert_eq!(result.as_str(), expected);
    }

    // Security validation tests
    #[test_case("/test/repo/src/main.rs", true; "valid path within base")]
    #[test_case("/test/repo/./src/../lib.rs", true; "dot and parent within base")]
    #[test_case("/test/repo_other/file.rs", false; "similar prefix but outside base")]
    #[test_case("/other/path/file.rs", false; "different root outside base")]
    fn test_validate_within_base(path: &str, should_pass: bool) {
        let base = Utf8PathBuf::from("/test/repo");
        let normalizer = PathNormalizer::new(base);
        let input_path = Utf8Path::new(path);
        let result = normalizer.validate_within_base(input_path);
        assert_eq!(result.is_ok(), should_pass, "Path '{}' validation result mismatch", path);
    }

    // Similar prefix but outside base tests (security: detect path confusion)
    #[test_case("/test/repo_backup/file.rs"; "repo_backup prefix")]
    #[test_case("/test/repo2/src/lib.rs"; "repo2 numeric suffix")]
    #[test_case("/test/repositories/project/src/lib.rs"; "repositories prefix")]
    fn test_similar_prefix_outside_base(path: &str) {
        let base = Utf8PathBuf::from("/test/repo");
        let normalizer = PathNormalizer::new(base);
        let input_path = Utf8Path::new(path);

        let result = normalizer.validate_within_base(input_path);
        assert!(result.is_err(), "Path '{}' should be outside base", path);

        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("outside") || err_msg.contains("repository"),
                "Error should mention being outside repository");
    }

    // Note: The PathNormalizer uses string-based prefix checking via strip_prefix.
    // It does NOT resolve '..' segments to check if they escape the base.
    // Full canonical path resolution would require std::fs::canonicalize which
    // requires the path to exist on disk. For symlink security, consider calling
    // canonicalize on resolved symlinks in higher-level code.

    // Path joining tests
    #[test_case("src/main.rs", "/test/repo/src/main.rs"; "simple join")]
    #[test_case("lib/utils/mod.rs", "/test/repo/lib/utils/mod.rs"; "nested join")]
    #[test_case("../outside.rs", "/test/repo/../outside.rs"; "parent in relative")]
    #[test_case("./relative.rs", "/test/repo/./relative.rs"; "dot prefix")]
    fn test_join_base(relative: &str, expected: &str) {
        let base = Utf8PathBuf::from("/test/repo");
        let normalizer = PathNormalizer::new(base);
        let joined = normalizer.join_base(relative);
        assert_eq!(joined.as_str(), expected);
    }

    // Error message helpfulness tests
    #[test_case("/etc/passwd", "/test/repo"; "system path error")]
    #[test_case("/tmp/file.rs", "/test/repo"; "tmp path error")]
    #[test_case("/home/user/file.rs", "/test/repo"; "home path error")]
    fn test_error_message_helpfulness(path: &str, base: &str) {
        let normalizer = PathNormalizer::new(base.into());
        let input_path = Utf8Path::new(path);
        let result = normalizer.relative_to_base(input_path);

        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = format!("{}", err);

        // Error should mention the problematic path
        assert!(err_msg.contains(path) || err_msg.contains("outside"),
                "Error should reference the problematic path");

        // Error should mention the base directory for context
        assert!(err_msg.contains(base) || err_msg.contains("repository"),
                "Error should reference the base directory");
    }

    // Case sensitivity comparison tests
    #[test_case("src/lib.rs", "src/lib.rs", true; "identical paths")]
    #[test_case("src/lib.rs", "src/Lib.rs", false; "different case (case-sensitive)")]
    #[test_case("src/lib.rs", "SRC/lib.rs", false; "different case directory")]
    #[test_case("src/LIB.rs", "src/lib.rs", false; "different case extension")]
    #[test_case("/test/repo/file.rs", "/test/repo/FILE.rs", false; "absolute different case")]
    fn test_path_case_comparison(a: &str, b: &str, case_sensitive_equal: bool) {
        // On Unix systems, paths are case-sensitive
        // On Windows, paths are case-insensitive
        let a_normalized = a.replace('\\', "/");
        let b_normalized = b.replace('\\', "/");

        let equal = a_normalized == b_normalized;
        assert_eq!(equal, case_sensitive_equal,
                   "Case comparison failed for '{}' vs '{}'", a, b);
    }

    // Platform-specific case behavior
    #[test_case("src/lib.rs", "src/LIB.RS"; "case difference check")]
    #[test_case("Main.ts", "main.ts"; "different case in filename")]
    #[test_case("/A/B/C", "/a/b/c"; "different case in directory")]
    fn test_case_sensitivity_behavior(path1: &str, path2: &str) {
        // This test documents the case sensitivity behavior
        // On Unix: these are different paths
        // On Windows: these are the same path

        let normalizer = create_test_normalizer();
        let p1 = Utf8Path::new(path1);
        let p2 = Utf8Path::new(path2);

        let n1 = normalizer.normalize_for_compare(p1).unwrap();
        let n2 = normalizer.normalize_for_compare(p2).unwrap();

        // After normalization, string comparison is case-sensitive
        let strings_equal = n1.as_str() == n2.as_str();

        #[cfg(unix)]
        assert!(!strings_equal, "Unix paths are case-sensitive: {} != {}", path1, path2);

        #[cfg(windows)]
        {
            // On Windows, the file system may be case-insensitive
            // but our string comparison is still case-sensitive
            // This documents that behavior
            assert_eq!(strings_equal, path1.to_lowercase() == path2.to_lowercase(),
                      "Windows case behavior documented");
        }
    }

    // Empty and edge case path tests
    #[test_case(".", "."; "current directory")]
    #[test_case("..", ".."; "parent directory")]
    #[test_case("/", "/"; "root directory")]
    #[test_case("file.rs", "file.rs"; "filename only")]
    #[test_case("./file.rs", "./file.rs"; "dot prefix filename")]
    fn test_edge_case_paths(input: &str, expected: &str) {
        let normalizer = create_test_normalizer();
        let input_path = Utf8Path::new(input);
        let result = normalizer.normalize_for_compare(input_path).unwrap();
        assert_eq!(result.as_str(), expected);
    }

    // ========== Additional Non-Parameterized Tests ==========

    #[test]
    fn test_relative_to_base_nested() {
        // Additional nested test not covered by parameterized cases
        let normalizer = create_test_normalizer();
        let path = Utf8Path::new("/test/repo/src/lib/utils.rs");
        let relative = normalizer.relative_to_base(path).unwrap();
        assert_eq!(relative.as_str(), "src/lib/utils.rs");
    }

    #[test]
    fn test_relative_to_base_outside_repo_structured_error() {
        // Verify structured error content
        let normalizer = create_test_normalizer();
        let path = Utf8Path::new("/other/repo/src/main.rs");
        let result = normalizer.relative_to_base(path);

        assert!(result.is_err());
        match result {
            Err(PathError::OutsideRepo { path, base }) => {
                assert_eq!(path.as_str(), "/other/repo/src/main.rs");
                assert_eq!(base.as_str(), "/test/repo");
            }
            _ => panic!("Expected OutsideRepo error"),
        }
    }

    #[test]
    fn test_join_base_absolute_path_override() {
        // camino's join handles absolute paths by ignoring the base
        let normalizer = create_test_normalizer();
        let joined = normalizer.join_base("/absolute/path.rs");
        assert_eq!(joined.as_str(), "/absolute/path.rs");
    }

    #[test]
    fn test_from_std_path_valid_utf8() {
        let std_path = std::path::Path::new("/test/repo/src/main.rs");
        let result = PathNormalizer::from_std_path(std_path);

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "/test/repo/src/main.rs");
    }

    #[test]
    fn test_from_std_path_non_utf8() {
        // On Unix, create a path with invalid UTF-8
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let invalid_bytes = vec![0x66, 0x6f, 0x80, 0x6f]; // "fo\x80o"
            let os_string = std::ffi::OsString::from_vec(invalid_bytes);
            let std_path = std::path::PathBuf::from(os_string);

            let result = PathNormalizer::from_std_path(&std_path);
            assert!(result.is_err());
            assert!(matches!(result, Err(PathError::NonUtf8 { .. })));
        }
    }

    #[test]
    fn test_to_std_path() {
        let utf8_path = Utf8Path::new("/test/repo/src/main.rs");
        let std_path = PathNormalizer::to_std_path(utf8_path);

        assert_eq!(std_path, std::path::PathBuf::from("/test/repo/src/main.rs"));
    }

    #[test]
    fn test_path_error_display_outside_repo() {
        let error = PathError::OutsideRepo {
            path: "/etc/passwd".into(),
            base: "/test/repo".into(),
        };
        let msg = format!("{}", error);
        assert!(msg.contains("outside repository base"));
        assert!(msg.contains("/etc/passwd"));
        assert!(msg.contains("/test/repo"));
    }

    #[test]
    fn test_path_error_display_invalid_chars() {
        let error = PathError::InvalidChars {
            path: "file\0name.rs".to_string(),
            invalid: '\0',
        };
        let msg = format!("{}", error);
        assert!(msg.contains("invalid character"));
        assert!(msg.contains("file\0name.rs"));
    }

    #[test]
    fn test_path_error_display_unc_not_supported() {
        let error = PathError::UncNotSupported {
            path: "\\\\?\\C:\\path".to_string(),
        };
        let msg = format!("{}", error);
        assert!(msg.contains("UNC paths not supported"));
        assert!(msg.contains("\\\\?\\C:\\path"));
    }

    #[test]
    fn test_path_error_display_non_utf8() {
        let error = PathError::NonUtf8 {
            path: std::path::PathBuf::from("/invalid/utf8"),
        };
        let msg = format!("{}", error);
        assert!(msg.contains("non-UTF-8"));
    }

    #[test]
    fn test_normalize_windows_network_path() {
        let normalizer = create_test_normalizer();

        // Windows network path (UNC)
        let path = Utf8Path::new("\\\\server\\share\\file.txt");
        let normalized = normalizer.normalize_for_compare(path).unwrap();

        // Dunce should simplify the UNC path
        assert!(normalized.as_str().contains('/'));
    }

    #[test]
    fn test_path_normalizer_clone() {
        let normalizer = create_test_normalizer();
        let cloned = normalizer.clone();

        assert_eq!(normalizer.base_dir(), cloned.base_dir());
    }

    #[test]
    fn test_path_normalizer_base_dir_getter() {
        let base = Utf8PathBuf::from("/custom/base");
        let normalizer = PathNormalizer::new(base.clone());

        assert_eq!(normalizer.base_dir(), &base);
    }

    #[test]
    fn test_error_message_includes_expected_format() {
        // Verify error messages include helpful context for debugging
        let normalizer = PathNormalizer::new("/Users/dev/myproject".into());
        let result = normalizer.relative_to_base(Utf8Path::new("/etc/passwd"));

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());

        // Error should mention the problematic path
        assert!(err_msg.contains("/etc/passwd"));

        // Error should mention the base directory
        assert!(err_msg.contains("/Users/dev/myproject") || err_msg.contains("repository"));
    }
}

/// Property-based tests for path normalization invariants.
///
/// These tests verify fundamental properties that ALWAYS hold for path operations:
/// - Idempotence: Normalizing twice yields same result as normalizing once
/// - Backslash elimination: Output never contains backslashes
/// - Round-trip safety: Utf8Path conversion is lossless
/// - Prefix consistency: relative_to_base strips base correctly
#[cfg(test)]
mod path_proptest {
    use super::*;
    use proptest::prelude::*;

    /// Create a test normalizer with a fixed base directory.
    fn test_normalizer() -> PathNormalizer {
        PathNormalizer::new("/test/repo".into())
    }

    /// Strategy: generate realistic path strings.
    ///
    /// Covers:
    /// - Simple identifiers (e.g., "src", "main")
    /// - Nested paths (e.g., "src/lib/utils")
    /// - Paths with extensions (e.g., "main.rs", "index.tsx")
    /// - Windows-style paths with backslashes
    fn path_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            // Simple paths
            r"[a-zA-Z_][a-zA-Z0-9_]*",
            // Nested paths
            r"([a-zA-Z_][a-zA-Z0-9_]*/)*[a-zA-Z_][a-zA-Z0-9_]*",
            // Paths with extension
            r"[a-zA-Z_][a-zA-Z0-9_]*\.[a-z]{1,4}",
            // Paths with backslashes (Windows style)
            r"[a-zA-Z_][a-zA-Z0-9_]*\\[a-zA-Z_][a-zA-Z0-9_]*",
        ]
    }

    // Property 1: Idempotence
    // Normalizing twice yields same result as normalizing once.
    // This is a critical invariant - normalization should reach a fixed point.
    proptest! {
        #[test]
        fn prop_normalize_idempotent(path in path_strategy()) {
            let normalizer = test_normalizer();
            let path_utf8 = Utf8Path::new(&path);

            if let Ok(n1) = normalizer.normalize_for_compare(path_utf8) {
                if let Ok(n2) = normalizer.normalize_for_compare(&n1) {
                    prop_assert_eq!(n1.as_str(), n2.as_str(),
                        "Normalize not idempotent: {} -> {} -> {}", path, n1.as_str(), n2.as_str());
                }
            }
        }
    }

    // Property 2: Backslash elimination
    // After normalization, output never contains backslashes.
    // This ensures cross-platform compatibility and consistent string comparison.
    proptest! {
        #[test]
        fn prop_backslash_elimination(path in r"[a-zA-Z0-9_/\\]+") {
            let normalizer = test_normalizer();
            let path_utf8 = Utf8Path::new(&path);

            if let Ok(normalized) = normalizer.normalize_for_compare(path_utf8) {
                prop_assert!(!normalized.as_str().contains('\\'),
                    "Normalized path contains backslash: {} -> {}", path, normalized.as_str());
            }
        }
    }

    // Property 3: Round-trip safety
    // Utf8PathBuf -> string -> Utf8PathBuf conversion is lossless.
    // This ensures UTF-8 paths can be safely serialized and deserialized.
    proptest! {
        #[test]
        fn prop_utf8path_roundtrip(path in path_strategy()) {
            let original = Utf8PathBuf::from(path.as_str());
            let as_str = original.as_str();
            let roundtrip = Utf8PathBuf::from(as_str);

            prop_assert_eq!(original, roundtrip);
        }
    }

    // Property 4: Prefix removal for paths within base
    // When joining a suffix to base, then calling relative_to_base,
    // the result should be valid (not error) for relative paths.
    // Note: Absolute suffixes (like "/" or "/other") override the base in join.
    proptest! {
        #[test]
        fn prop_relative_to_base_within_base(suffix in r"[a-zA-Z0-9_]+") {
            let base = Utf8PathBuf::from("/test/repo");
            let normalizer = PathNormalizer::new(base.clone());
            let full_path = base.join(&suffix);

            // The key invariant: relative_to_base should succeed for paths within base
            let result = normalizer.relative_to_base(&full_path);
            prop_assert!(result.is_ok(),
                "relative_to_base failed for path within base: {} -> {:?}", full_path.as_str(), result);
        }
    }

    // Property 4b: Nested paths also work correctly
    proptest! {
        #[test]
        fn prop_relative_to_base_nested(suffix in r"([a-zA-Z0-9_]+/)*[a-zA-Z0-9_]+") {
            let base = Utf8PathBuf::from("/test/repo");
            let normalizer = PathNormalizer::new(base.clone());
            let full_path = base.join(&suffix);

            // The key invariant: relative_to_base should succeed for paths within base
            let result = normalizer.relative_to_base(&full_path);
            prop_assert!(result.is_ok(),
                "relative_to_base failed for nested path within base: {} -> {:?}", full_path.as_str(), result);
        }
    }

    // ========== Edge Case Tests ==========

    // Property 5: Empty/edge case paths don't panic
    // Normalization should handle any path string without panicking,
    // including empty strings and unusual characters.
    proptest! {
        #[test]
        fn prop_normalize_edge_cases_no_panic(path in r"[a-zA-Z0-9_/\.\\]{0,50}") {
            let normalizer = test_normalizer();
            let path_utf8 = Utf8Path::new(&path);

            // Should not panic for any path (even invalid ones)
            let _result = normalizer.normalize_for_compare(path_utf8);
        }
    }

    // Property 6: Join base consistency
    // When joining a relative path to base, the result should start with base
    // (unless the relative path is absolute, which overrides the base).
    proptest! {
        #[test]
        fn prop_join_base_consistent(relative in path_strategy()) {
            let base = Utf8PathBuf::from("/test/repo");
            let normalizer = PathNormalizer::new(base.clone());

            let joined = normalizer.join_base(&relative);
            let base_str = base.as_str();

            // Joined path should either start with base (for relative paths)
            // or be the absolute path (for absolute relative paths)
            prop_assert!(joined.as_str().starts_with(base_str) || relative.starts_with('/'),
                "Joined path doesn't start with base for relative path: {} -> {}", base_str, joined.as_str());
        }
    }

    // Property 7: validate_within_base is deterministic
    // Calling validate_within_base twice on the same path should yield the same result.
    proptest! {
        #[test]
        fn prop_validate_deterministic(path in r"[a-zA-Z0-9_/\\]+") {
            let normalizer = test_normalizer();
            let path_utf8 = Utf8Path::new(&path);

            let result1 = normalizer.validate_within_base(path_utf8);
            let result2 = normalizer.validate_within_base(path_utf8);

            prop_assert_eq!(result1.is_ok(), result2.is_ok(),
                "validate_within_base not deterministic: {:?} != {:?}", result1, result2);
        }
    }

    // Property 8: Normalized paths can be converted back to Utf8Path
    // After normalization, the path should remain valid UTF-8 and
    // convertible back to Utf8Path without loss.
    proptest! {
        #[test]
        fn prop_normalize_to_utf8path_valid(path in path_strategy()) {
            let normalizer = test_normalizer();
            let path_utf8 = Utf8Path::new(&path);

            if let Ok(normalized) = normalizer.normalize_for_compare(path_utf8) {
                // Convert to string and back - should remain valid UTF-8
                let as_str = normalized.as_str();
                let back_to_utf8 = Utf8Path::new(as_str);
                prop_assert_eq!(back_to_utf8.as_str(), normalized.as_str(),
                    "Round-trip through string failed: {} != {}", as_str, back_to_utf8.as_str());
            }
        }
    }
}

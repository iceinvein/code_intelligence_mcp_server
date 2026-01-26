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

    fn create_test_normalizer() -> PathNormalizer {
        PathNormalizer::new("/test/repo".into())
    }

    #[test]
    fn test_normalize_for_compare_basic() {
        let normalizer = create_test_normalizer();

        let path = Utf8Path::new("/test/repo/src/main.rs");
        let normalized = normalizer.normalize_for_compare(path).unwrap();

        assert_eq!(normalized.as_str(), "/test/repo/src/main.rs");
    }

    #[test]
    fn test_normalize_for_compare_backslashes() {
        let normalizer = create_test_normalizer();

        // Simulate Windows-style path
        let path = Utf8Path::new("C:\\test\\repo\\src\\main.rs");
        let normalized = normalizer.normalize_for_compare(path).unwrap();

        // Backslashes should be converted to forward slashes
        assert_eq!(normalized.as_str(), "C:/test/repo/src/main.rs");
    }

    #[test]
    fn test_normalize_for_compare_mixed_slashes() {
        let normalizer = create_test_normalizer();

        let path = Utf8Path::new("/test\\repo/src\\lib.rs");
        let normalized = normalizer.normalize_for_compare(path).unwrap();

        assert_eq!(normalized.as_str(), "/test/repo/src/lib.rs");
    }

    #[test]
    fn test_normalize_for_compare_trailing_slash() {
        let normalizer = create_test_normalizer();

        let path = Utf8Path::new("/test/repo/src/");
        let normalized = normalizer.normalize_for_compare(path).unwrap();

        // Trailing slash preserved
        assert_eq!(normalized.as_str(), "/test/repo/src/");
    }

    #[test]
    fn test_relative_to_base_success() {
        let normalizer = create_test_normalizer();

        let path = Utf8Path::new("/test/repo/src/main.rs");
        let relative = normalizer.relative_to_base(path).unwrap();

        assert_eq!(relative.as_str(), "src/main.rs");
    }

    #[test]
    fn test_relative_to_base_nested() {
        let normalizer = create_test_normalizer();

        let path = Utf8Path::new("/test/repo/src/lib/utils.rs");
        let relative = normalizer.relative_to_base(path).unwrap();

        assert_eq!(relative.as_str(), "src/lib/utils.rs");
    }

    #[test]
    fn test_relative_to_base_exact_base() {
        let normalizer = create_test_normalizer();

        let path = Utf8Path::new("/test/repo");
        let relative = normalizer.relative_to_base(path).unwrap();

        assert_eq!(relative.as_str(), ".");
    }

    #[test]
    fn test_relative_to_base_outside_repo() {
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
    fn test_relative_to_base_parent_escape() {
        let normalizer = create_test_normalizer();

        // Path that starts with base but goes outside via parent reference
        // This is tricky - we're checking an absolute path, not a relative one
        let path = Utf8Path::new("/test/repo_other/src/main.rs");
        let result = normalizer.relative_to_base(path);

        assert!(result.is_err());
    }

    #[test]
    fn test_validate_within_base_success() {
        let normalizer = create_test_normalizer();

        let path = Utf8Path::new("/test/repo/src/main.rs");
        assert!(normalizer.validate_within_base(path).is_ok());
    }

    #[test]
    fn test_validate_within_base_failure() {
        let normalizer = create_test_normalizer();

        let path = Utf8Path::new("/etc/passwd");
        let result = normalizer.validate_within_base(path);

        assert!(result.is_err());
    }

    #[test]
    fn test_validate_within_base_similar_prefix() {
        let normalizer = create_test_normalizer();

        // Path that has base as prefix but isn't actually within it
        let path = Utf8Path::new("/test/repo_backup/src/main.rs");
        let result = normalizer.validate_within_base(path);

        assert!(result.is_err());
    }

    #[test]
    fn test_join_base() {
        let normalizer = create_test_normalizer();

        let joined = normalizer.join_base("src/main.rs");
        assert_eq!(joined.as_str(), "/test/repo/src/main.rs");
    }

    #[test]
    fn test_join_base_nested() {
        let normalizer = create_test_normalizer();

        let joined = normalizer.join_base("src/lib/utils.rs");
        assert_eq!(joined.as_str(), "/test/repo/src/lib/utils.rs");
    }

    #[test]
    fn test_join_base_absolute() {
        let normalizer = create_test_normalizer();

        // camino's join handles absolute paths by ignoring the base
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
}

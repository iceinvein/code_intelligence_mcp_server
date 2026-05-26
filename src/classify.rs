//! Shared classification utilities for files and symbols.
//!
//! These functions are used by both the indexing pipeline (write path) and
//! retrieval pipeline (read path), so they live in a shared module to avoid
//! cross-concern dependencies.

/// Determine if a file path is a test file.
///
/// Checks common test file patterns across languages: `*.test.*`, `*.spec.*`,
/// `_test.{rs,go,py,ts,tsx,js,jsx}`, `__tests__/`, `tests/`, conftest, mocks,
/// fixtures, and test helpers.
pub fn is_test_file(file_path: &str) -> bool {
    let path_lower = file_path.to_lowercase();
    file_path.contains(".test.")
        || file_path.contains(".spec.")
        || file_path.contains("/__tests__/")
        || file_path.contains("/tests/")
        || file_path.starts_with("tests/")
        || file_path.ends_with("_test.rs")
        || file_path.ends_with("_test.go")
        || file_path.ends_with("_test.py")
        || file_path.ends_with("_test.ts")
        || file_path.ends_with("_test.tsx")
        || file_path.ends_with("_test.js")
        || file_path.ends_with("_test.jsx")
        || file_path.contains("/test_")
        || file_path.contains("/conftest")
        // Mock/fixture/helper files (e.g., test.mocks.ts, __mocks__/, fixtures/)
        || path_lower.contains("mock")
        || path_lower.contains("__fixtures__")
        || path_lower.contains("/fixtures/")
        // Test helper patterns (e.g., test-helpers.ts, admin-test-helpers.ts)
        || path_lower.contains("test-helper")
        // Additional patterns from storage/tests module
        || path_lower.contains("/test/")
        || path_lower.contains("/spec/")
}

/// Check whether a file path is a generated build output (`dist/`,
/// `build/`, `out/`, or any `.min.` minified bundle).
///
/// Build outputs are intentionally indexed when committed to the repo so
/// project-specific search remains possible, but retrieval needs to
/// downrank or filter them so they do not surface as primary evidence
/// for source-code questions. This is the canonical check used by both
/// the retrieval post-process filter and the edge-expansion pruner.
pub fn is_generated_output_path(file_path: &str) -> bool {
    let path = file_path.to_lowercase();
    path.starts_with("out/")
        || path.contains("/out/")
        || path.starts_with("dist/")
        || path.contains("/dist/")
        || path.starts_with("build/")
        || path.contains("/build/")
        || path.contains(".min.")
}

/// Check if a symbol name looks like a test function/helper.
///
/// Matches: `test_*`, `create_test_*`, `make_test_*`, `setup_test*`, `mock_*`,
/// `Mock*`, `*Mock`, `fake*`, `stub*`, `setup`, `teardown`, `tests`,
/// and PascalCase `Test*` patterns.
pub fn is_test_symbol(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with("test_")
        || n.starts_with("create_test_")
        || n.starts_with("make_test_")
        || n.starts_with("setup_test")
        || n.starts_with("mock_")
        || n == "setup"
        || n == "teardown"
        || n == "tests"
        || (n.starts_with("test") && n.len() > 4 && n.as_bytes()[4].is_ascii_uppercase())
        // Mock patterns: MockTransaction, createMockDb, txMock, fakeFoo, stubBar
        || name.starts_with("Mock")
        || name.ends_with("Mock")
        || name.contains("Mock")
        || n.starts_with("fake")
        || n.starts_with("stub")
}

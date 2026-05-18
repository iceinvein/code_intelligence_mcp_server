//! Marker-presence smoke test for the terminal-redesign dashboard.
//! Re-includes ui/dashboard.html and asserts the expected section IDs are present.

const DASHBOARD_HTML: &str = include_str!("../ui/dashboard.html");

#[test]
fn dashboard_contains_terminal_layout_markers() {
    for marker in [
        r#"id="header-strip""#,
        r#"id="status-grid""#,
        r#"id="repo-table""#,
        r#"id="repl""#,
        r#"data-theme-key="cimcp.theme""#,
        r#"class="brand""#,
        r#"id="health-pulse""#,
        r#"id="theme-toggle""#,
    ] {
        assert!(
            DASHBOARD_HTML.contains(marker),
            "missing marker in ui/dashboard.html: {marker}"
        );
    }
}

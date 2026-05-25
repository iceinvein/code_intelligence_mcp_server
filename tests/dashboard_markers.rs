//! Marker-presence smoke test for the palette-redesign dashboard.
//! Re-includes ui/dashboard.html and asserts the expected section IDs are present.

const DASHBOARD_HTML: &str = include_str!("../ui/dashboard.html");

#[test]
fn dashboard_contains_palette_layout_markers() {
    for marker in [
        r#"id="header-strip""#,
        r#"id="status-grid""#,
        r#"id="repo-table""#,
        r#"id="palette""#,
        r#"id="palette-input""#,
        r#"data-theme-key="cimcp.theme""#,
        r#"class="brand""#,
        r#"id="health-pulse""#,
        r#"id="theme-toggle""#,
        r#"id="palette-hint""#,
        r#"data-palette-section="repos""#,
        r#"data-palette-section="sessions""#,
        r#"data-palette-section="actions""#,
    ] {
        assert!(
            DASHBOARD_HTML.contains(marker),
            "missing marker in ui/dashboard.html: {marker}"
        );
    }
}

#[test]
fn dashboard_does_not_contain_repl_markers() {
    for absent in [r#"id="repl""#, r#"id="repl-in""#, r#"id="repl-out""#] {
        assert!(
            !DASHBOARD_HTML.contains(absent),
            "REPL marker still present in ui/dashboard.html: {absent}"
        );
    }
}

#[test]
fn dashboard_does_not_render_fake_job_progress_percent() {
    for absent in [r#"running[0]?.progress"#, r#"bar(runPct)"#] {
        assert!(
            !DASHBOARD_HTML.contains(absent),
            "dashboard should not render fake job progress from missing API field: {absent}"
        );
    }
}

#[test]
fn dashboard_activity_label_uses_persisted_index_activity() {
    assert!(
        DASHBOARD_HTML.contains("latest_index_run"),
        "repo activity should use durable latest_index_run data, not only transient jobs"
    );
}

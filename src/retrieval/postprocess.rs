//! Post-processing: query control filtering and boost application.

use super::query::{Intent, QueryControls};
use super::ranking::{
    apply_doc_status_demotion_with_signals, apply_docstring_boost_with_signals,
    apply_file_affinity_boost_with_signals, apply_package_boost_with_signals,
    apply_popularity_boost_with_signals, apply_selection_boost_with_signals,
};
use super::{HitSignals, RankedHit};
use crate::classify::is_generated_output_path;
use crate::config::Config;
use crate::storage::sqlite::SqliteStore;
use anyhow::Result;
use std::collections::HashMap;

/// Apply query-control filters and all boost signals to ranked hits.
///
/// Pipeline: control filters → exported_only → popularity → docstring →
/// selection → file affinity → package boost.
#[allow(clippy::too_many_arguments)]
pub(super) fn filter_and_boost(
    sqlite: &SqliteStore,
    hits: Vec<RankedHit>,
    hit_signals: &mut HashMap<String, HitSignals>,
    controls: &QueryControls,
    exported_only: bool,
    original_query: &str,
    intent: &Option<Intent>,
    config: &Config,
) -> Result<Vec<RankedHit>> {
    let hits = filter_hits_by_controls(hits, controls);
    let hits = if exported_only {
        hits.into_iter().filter(|h| h.exported).collect::<Vec<_>>()
    } else {
        hits
    };

    let hits = apply_popularity_boost_with_signals(sqlite, hits, hit_signals, config)?;
    let hits = apply_doc_status_demotion_with_signals(sqlite, hits, hit_signals)?;
    let hits = apply_docstring_boost_with_signals(sqlite, hits, hit_signals)?;
    let hits =
        apply_selection_boost_with_signals(sqlite, hits, hit_signals, original_query, config)?;
    let hits = apply_file_affinity_boost_with_signals(sqlite, hits, hit_signals, config)?;

    let query_package_id = controls.package.as_deref();
    let hits = apply_package_boost_with_signals(
        sqlite,
        hits,
        hit_signals,
        query_package_id,
        config,
        intent.clone().unwrap_or(Intent::Definition),
    )?;

    Ok(hits)
}

fn filter_hits_by_controls(hits: Vec<RankedHit>, controls: &QueryControls) -> Vec<RankedHit> {
    let explicit_path_filter = controls.path.is_some() || controls.file.is_some();
    hits.into_iter()
        .filter(|h| explicit_path_filter || !is_generated_output_path(&h.file_path))
        .filter(|h| {
            controls
                .lang
                .as_ref()
                .is_none_or(|l| h.language == l.as_str())
        })
        .filter(|h| {
            controls
                .kind
                .as_ref()
                .is_none_or(|k| kind_matches(&h.kind, k))
        })
        .filter(|h| {
            controls
                .path
                .as_ref()
                .is_none_or(|p| path_matches(&h.file_path, p))
        })
        .filter(|h| {
            controls
                .file
                .as_ref()
                .is_none_or(|f| file_matches(&h.file_path, f))
        })
        .collect()
}

fn kind_matches(kind: &str, control: &str) -> bool {
    control
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .any(|k| kind.eq_ignore_ascii_case(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, file_path: &str) -> RankedHit {
        RankedHit {
            id: id.to_string(),
            score: 1.0,
            name: id.to_string(),
            kind: "function".to_string(),
            file_path: file_path.to_string(),
            exported: true,
            language: "typescript".to_string(),
        }
    }

    #[test]
    fn filters_generated_output_paths_by_default() {
        let hits = vec![
            hit("source", "src/preload/index.ts"),
            hit("out", "out/preload/index.js"),
            hit("dist", "dist/main/index.js"),
            hit("build", "packages/app/build/index.js"),
        ];

        let filtered = filter_hits_by_controls(hits, &QueryControls::default());

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "source");
    }

    #[test]
    fn keeps_generated_output_paths_when_explicitly_requested() {
        let hits = vec![hit("out", "out/preload/index.js")];
        let controls = QueryControls {
            path: Some("out/preload".to_string()),
            ..QueryControls::default()
        };

        let filtered = filter_hits_by_controls(hits, &controls);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "out");
    }
}

fn path_matches(file_path: &str, control: &str) -> bool {
    file_path.to_lowercase().contains(&control.to_lowercase())
}

fn file_matches(file_path: &str, control: &str) -> bool {
    let file_path = file_path.to_lowercase();
    let control = control.to_lowercase();
    match (control.starts_with('*'), control.ends_with('*')) {
        (true, true) => file_path.contains(control.trim_matches('*')),
        (true, false) => file_path.ends_with(control.trim_start_matches('*')),
        (false, true) => file_path.starts_with(control.trim_end_matches('*')),
        (false, false) => file_path.contains(&control),
    }
}

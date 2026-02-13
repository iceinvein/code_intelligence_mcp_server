use crate::config::Config;
use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::Arc;

/// Create an OS-native file watcher that monitors all `config.repo_roots` recursively.
///
/// When a relevant file event occurs (not in `.git`, `target`, `dist`, `build`,
/// or `node_modules`), a `()` signal is sent on `tx`.  The caller should
/// debounce these signals before triggering a re-index.
///
/// Returns the watcher handle — it **must** be kept alive (not dropped) for
/// events to continue flowing.
pub fn create_watcher(
    config: &Arc<Config>,
    tx: tokio::sync::mpsc::UnboundedSender<()>,
) -> Result<RecommendedWatcher> {
    let index_node_modules = config.index_node_modules;

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let event = match res {
            Ok(ev) => ev,
            Err(e) => {
                tracing::warn!(error = %e, "File watcher error");
                return;
            }
        };

        // Check if any path in the event is interesting (not excluded).
        let dominated_by_noise = event.paths.iter().all(|p| {
            let s = p.to_string_lossy();
            is_excluded_path(&s, index_node_modules)
        });

        if dominated_by_noise {
            return;
        }

        // Send wakeup signal; ignore error (receiver dropped = task exiting).
        let _ = tx.send(());
    })
    .context("Failed to create file watcher")?;

    for root in &config.repo_roots {
        watcher
            .watch(root.as_std_path(), RecursiveMode::Recursive)
            .with_context(|| format!("Failed to watch directory: {}", root))?;
        tracing::info!(root = %root, "Watching directory for changes");
    }

    Ok(watcher)
}

/// Fast string-contains check to filter out high-churn directories.
///
/// Intentionally simple — mirrors the logic in `scan.rs::is_excluded` but
/// operates on raw path strings so it can run inside the notify callback
/// without any I/O.
fn is_excluded_path(path: &str, index_node_modules: bool) -> bool {
    if path.contains("/.git/")
        || path.contains("/target/")
        || path.contains("/dist/")
        || path.contains("/build/")
    {
        return true;
    }
    if !index_node_modules && path.contains("/node_modules/") {
        return true;
    }
    false
}

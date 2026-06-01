//! Format-preserving writer for `server.toml`. Sets only the changed keys via
//! `toml_edit`, so user comments and untouched keys survive a Save.

use crate::path::Utf8Path;
use anyhow::{Context, Result};
use toml_edit::{DocumentMut, Item, Table, Value};

const HEADER: &str =
    "# code-intelligence server config.\n# Edited by the Settings editor; hand edits and comments are preserved.\n\n";

/// One change to apply: a dotted path into the TOML document and the value to set.
pub(crate) struct SettingChange {
    pub path: &'static [&'static str],
    pub value: Item,
}

fn ensure_table(item: &mut Item) -> &mut Table {
    match item {
        Item::Table(_) => {}
        Item::Value(Value::InlineTable(inline)) => {
            let table = inline
                .iter()
                .map(|(key, value)| (key.to_string(), Item::Value(value.clone())))
                .collect::<Table>();
            *item = Item::Table(table);
        }
        _ => {
            *item = Item::Table(Table::new());
        }
    }
    item.as_table_mut().expect("item was just made a table")
}

fn set_at_path(doc: &mut DocumentMut, path: &[&str], value: Item) {
    match path {
        [a] => doc[*a] = value,
        [a, b] => {
            {
                let table = ensure_table(&mut doc[*a]);
                table[*b] = value;
            }
            if let Some(mut key) = doc.as_table_mut().key_mut(a) {
                key.fmt();
            }
        }
        [a, b, c] => {
            {
                let table = ensure_table(&mut doc[*a]);
                {
                    let nested = ensure_table(&mut table[*b]);
                    nested[*c] = value;
                }
                if let Some(mut key) = table.key_mut(b) {
                    key.fmt();
                }
            }
            if let Some(mut key) = doc.as_table_mut().key_mut(a) {
                key.fmt();
            }
        }
        _ => panic!("unsupported toml path depth: {path:?}"),
    }
}

/// Apply `changes` to the TOML document at `path`, preserving existing comments
/// and keys. Creates the file (with a header) when absent. Writes atomically
/// via a temp file + rename.
pub(crate) fn write_settings(path: &Utf8Path, changes: &[SettingChange]) -> Result<()> {
    let is_new = !path.exists();
    let mut doc = if is_new {
        DocumentMut::new()
    } else {
        std::fs::read_to_string(path.as_std_path())
            .context("read server.toml")?
            .parse::<DocumentMut>()
            .context("parse server.toml")?
    };

    for change in changes {
        set_at_path(&mut doc, change.path, change.value.clone());
    }

    let body = doc.to_string();
    let out = if is_new {
        format!("{HEADER}{body}")
    } else {
        body
    };

    let tmp = path.with_extension("toml.tmp");
    std::fs::write(tmp.as_std_path(), out).context("write temp server.toml")?;
    std::fs::rename(tmp.as_std_path(), path.as_std_path()).context("rename server.toml")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Utf8PathBuf;

    #[test]
    fn write_settings_preserves_comments_and_unrelated_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(tmp.path().join("server.toml")).unwrap();
        std::fs::write(path.as_std_path(), "# keep me\n[server]\nport = 17800\n").unwrap();

        write_settings(
            &path,
            &[SettingChange {
                path: &["retrieval", "hybrid_alpha"],
                value: toml_edit::value(0.85),
            }],
        )
        .unwrap();

        let out = std::fs::read_to_string(path.as_std_path()).unwrap();
        assert!(out.contains("# keep me"), "comment must survive: {out}");
        assert!(
            out.contains("port = 17800"),
            "existing key must survive: {out}"
        );
        assert!(
            out.contains("hybrid_alpha = 0.85"),
            "new key must be written: {out}"
        );
        assert!(
            out.contains("[retrieval]"),
            "new section must be explicit: {out}"
        );
    }

    #[test]
    fn write_settings_creates_file_with_header_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(tmp.path().join("server.toml")).unwrap();

        write_settings(
            &path,
            &[SettingChange {
                path: &["repos", "defaults", "watch_mode"],
                value: toml_edit::value(false),
            }],
        )
        .unwrap();

        let out = std::fs::read_to_string(path.as_std_path()).unwrap();
        assert!(
            out.starts_with("# code-intelligence server config."),
            "header: {out}"
        );
        assert!(
            out.contains("watch_mode = false"),
            "nested key written: {out}"
        );
        assert!(
            out.contains("[repos.defaults]"),
            "nested section must be explicit: {out}"
        );
    }

    #[test]
    fn write_settings_expands_inline_table_without_dropping_existing_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(tmp.path().join("server.toml")).unwrap();
        std::fs::write(
            path.as_std_path(),
            "retrieval = { max_context_bytes = 1234 }\n",
        )
        .unwrap();

        write_settings(
            &path,
            &[SettingChange {
                path: &["retrieval", "hybrid_alpha"],
                value: toml_edit::value(0.85),
            }],
        )
        .unwrap();

        let out = std::fs::read_to_string(path.as_std_path()).unwrap();
        assert!(
            out.contains("max_context_bytes = 1234"),
            "inline table key must survive: {out}"
        );
        assert!(
            out.contains("hybrid_alpha = 0.85"),
            "new key must be written: {out}"
        );
        assert!(
            out.contains("[retrieval]"),
            "section must be explicit: {out}"
        );
    }
}

//! Documentation cross-link extraction (docs-indexing design, Phase 3).
//!
//! Documents join the code graph through two link kinds:
//! - **Backtick references.** A `` `symbol_name` `` inside a document section
//!   becomes a `documents` edge from the section to the resolved code symbol,
//!   so `find_references` on a function surfaces the docs discussing it.
//! - **TODO↔issue matching.** An issue document carrying front-matter
//!   `number: N` links to the file (via its file-root symbol) of every
//!   TODO/FIXME whose text mentions `#N`, with a `tracks` edge.
//!
//! References that resolve to no indexed symbol are *stale links* — reported
//! as signal, never fatal.

use crate::storage::sqlite::queries::symbols::search_symbols_by_exact_name;
use crate::storage::sqlite::schema::{EdgeEvidenceRow, EdgeRow};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashSet;

/// Edge type for document→code backtick links.
pub const DOC_REFERENCES_EDGE: &str = "documents";
/// Edge type for TODO/FIXME ↔ issue-document links.
pub const TRACKS_EDGE: &str = "tracks";

/// One resolved link edge plus its evidence rows.
pub type DocEdgeBundle = Vec<(EdgeRow, Vec<EdgeEvidenceRow>)>;

/// Extract deduplicated backtick-quoted identifiers from text.
///
/// Only *link candidates* are returned: things shaped like code symbols
/// (`search_code`, `SqliteStore::read`, `parser.for_id`). Inline code that is
/// clearly prose — CLI flags (`--json`), file paths (`src/handlers/graph.rs`,
/// `README.md`), IP literals, issue refs (`#42`) — is excluded so neither
/// edge creation nor stale-link counting chases it.
pub fn extract_backtick_refs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else {
            break;
        };
        let name = &after[..end];
        if is_link_candidate(name) && seen.insert(name.to_string()) {
            out.push(name.to_string());
        }
        rest = &after[end + 1..];
    }
    out
}

/// File extensions that mark a backticked token as a path, not a symbol.
const PATHLIKE_EXTS: &[&str] = &[
    "rs", "md", "mdx", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "kts", "cs", "swift",
    "c", "h", "cpp", "cc", "hpp", "rb", "toml", "yaml", "yml", "json", "sh", "html", "css", "sql",
    "plist",
];

fn is_link_candidate(name: &str) -> bool {
    if name.is_empty() || name.len() > 128 {
        return false;
    }
    // Must start like an identifier: letter or underscore. Rejects flags
    // (`--repo`), numbers/IPs (`127.0.0.1`), and issue refs (`#42`).
    let first = name.chars().next().unwrap();
    if !first.is_alphabetic() && first != '_' {
        return false;
    }
    // Identifier-ish body only.
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.'))
    {
        return false;
    }
    // Path-shaped tokens never resolve to symbol names.
    if name.contains('/') {
        return false;
    }
    let lower = name.to_lowercase();
    !PATHLIKE_EXTS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// Resolve one backticked name to the best matching indexed symbol id.
/// Prefers exported symbols and non-file kinds; documents cannot be targets.
fn resolve_symbol_id(conn: &Connection, name: &str) -> Result<Option<String>> {
    // Exact-name match first; fall back to the unqualified tail of a
    // path-shaped reference like `module::function`.
    let candidates = [Some(name.to_string()), {
        let tail = name.rsplit("::").next().unwrap_or(name);
        (tail != name).then(|| tail.to_string())
    }];
    for candidate in candidates.into_iter().flatten() {
        let rows = search_symbols_by_exact_name(conn, &candidate, None, 25)
            .context("Failed to resolve doc reference")?;
        let best = rows
            .into_iter()
            .filter(|row| row.kind != "document" && row.kind != "file")
            .max_by(|a, b| {
                a.exported
                    .cmp(&b.exported)
                    .then_with(|| b.name.len().cmp(&a.name.len()))
                    .then_with(|| a.file_path.cmp(&b.file_path))
                    .then_with(|| a.id.cmp(&b.id))
            });
        if let Some(row) = best {
            return Ok(Some(row.id));
        }
    }
    Ok(None)
}

/// Build `documents` edges for one document section plus `tracks` edges when
/// the section belongs to an issue-numbered document. Returns the edges and
/// the number of stale (unresolved) references.
pub fn extract_doc_link_edges(
    doc_row: &crate::storage::sqlite::SymbolRow,
    issue_number: Option<i64>,
    conn: &Connection,
) -> Result<(DocEdgeBundle, usize)> {
    let refs = extract_backtick_refs(&doc_row.text);
    let mut edges = Vec::new();
    let mut stale = 0usize;

    for name in refs {
        match resolve_symbol_id(conn, &name)? {
            Some(to_id) => {
                if to_id == doc_row.id {
                    continue;
                }
                edges.push(doc_edge(doc_row, &to_id, DOC_REFERENCES_EDGE));
            }
            None => {
                stale += 1;
            }
        }
    }

    if let Some(number) = issue_number {
        edges.extend(tracks_edges_for_issue(doc_row, number, conn)?);
    }

    Ok((edges, stale))
}

fn doc_edge(
    doc_row: &crate::storage::sqlite::SymbolRow,
    to_id: &str,
    edge_type: &str,
) -> (EdgeRow, Vec<EdgeEvidenceRow>) {
    (
        EdgeRow {
            from_symbol_id: doc_row.id.clone(),
            to_symbol_id: to_id.to_string(),
            edge_type: edge_type.to_string(),
            at_file: Some(doc_row.file_path.clone()),
            at_line: Some(doc_row.start_line),
            confidence: 0.8,
            evidence_count: 1,
            resolution: "db_exact".to_string(),
        },
        vec![EdgeEvidenceRow {
            from_symbol_id: doc_row.id.clone(),
            to_symbol_id: to_id.to_string(),
            edge_type: edge_type.to_string(),
            at_file: doc_row.file_path.clone(),
            at_line: doc_row.start_line,
            count: 1,
        }],
    )
}

/// Link an issue document to files whose TODO/FIXME entries mention its
/// number (`#N`, `fixes #N`, `issue N`). The edge originates at the todo's
/// file-root symbol so the whole file carries the association.
fn tracks_edges_for_issue(
    doc_row: &crate::storage::sqlite::SymbolRow,
    number: i64,
    conn: &Connection,
) -> Result<Vec<(EdgeRow, Vec<EdgeEvidenceRow>)>> {
    let pattern = format!("#{number}");
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT file_path FROM todos \
             WHERE text LIKE ?1 ESCAPE '\\' LIMIT 200",
        )
        .context("Failed to query todos for issue tracking")?;
    let escaped = format!("%{}%", pattern.replace(';', ""));
    let mut files = Vec::new();
    let rows = stmt
        .query_map([escaped], |row| row.get::<_, String>(0))
        .context("Failed to read todos for issue tracking")?;
    for row in rows {
        files.push(row.context("Failed to read todo row")?);
    }
    drop(stmt);

    let mut out = Vec::new();
    for file_path in files {
        let file_root_id =
            crate::indexer::pipeline::utils::stable_symbol_id(&file_path, "FILE_ROOT", 0);
        // Only link when the file-root symbol actually exists in this index.
        let exists = conn
            .query_row(
                "SELECT 1 FROM symbols WHERE id = ?1",
                [&file_root_id],
                |_| Ok(()),
            )
            .is_ok();
        if !exists || file_path == doc_row.file_path {
            continue;
        }
        out.push((
            EdgeRow {
                from_symbol_id: file_root_id,
                to_symbol_id: doc_row.id.clone(),
                edge_type: TRACKS_EDGE.to_string(),
                at_file: Some(file_path),
                at_line: None,
                confidence: 0.7,
                evidence_count: 1,
                resolution: "todo_match".to_string(),
            },
            vec![],
        ));
    }
    Ok(out)
}

/// On-demand stale-link report across all indexed documents (docs-indexing
/// design, Phase 3). Recomputed live so delta index runs stay accurate.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StaleDocLinks {
    pub total: usize,
    pub by_file: std::collections::BTreeMap<String, usize>,
}

pub fn count_stale_doc_links(conn: &Connection) -> Result<StaleDocLinks> {
    // Load all resolvable symbol names once; documents are few but names are
    // checked against the full corpus.
    let mut names: HashSet<String> = HashSet::new();
    {
        let mut stmt = conn
            .prepare("SELECT name FROM symbols WHERE kind NOT IN ('document', 'file')")
            .context("Failed to load symbol names for stale-link check")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .context("Failed to read symbol names")?;
        for row in rows {
            names.insert(row.context("Failed to read symbol name row")?);
        }
    }

    let mut report = StaleDocLinks::default();
    let mut stmt = conn
        .prepare("SELECT file_path, text FROM symbols WHERE kind = 'document'")
        .context("Failed to load documents for stale-link check")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("Failed to read documents")?;
    for row in rows {
        let (file_path, text) = row.context("Failed to read document row")?;
        // Design/spec documents describe future or proposed APIs by design;
        // unresolved references there are intent, not drift. Only operational
        // docs (readme, guides, ADRs, issues, changelogs) count.
        if matches!(
            crate::indexer::extract::markdown::classify_doc_path(&file_path),
            crate::indexer::extract::markdown::DocType::Design
                | crate::indexer::extract::markdown::DocType::Other
        ) {
            continue;
        }
        let mut file_stale = 0usize;
        for name in extract_backtick_refs(&text) {
            if is_common_token(&name) {
                continue;
            }
            let resolved =
                names.contains(&name) || names.contains(name.rsplit("::").next().unwrap_or(&name));
            if !resolved {
                file_stale += 1;
            }
        }
        if file_stale > 0 {
            report.total += file_stale;
            report.by_file.insert(file_path, file_stale);
        }
    }
    Ok(report)
}

/// Language keywords and ubiquitous literals that appear inline-coded in
/// prose but never name symbols.
fn is_common_token(name: &str) -> bool {
    matches!(
        name,
        "true"
            | "false"
            | "null"
            | "None"
            | "Some"
            | "Ok"
            | "Err"
            | "GET"
            | "POST"
            | "PUT"
            | "DELETE"
            | "PATCH"
            | "HEAD"
            | "OPTIONS"
            | "self"
            | "this"
            | "TODO"
            | "FIXME"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_deduped_backtick_refs() {
        let text = "see `authenticate_request` and `auth::verify` and `authenticate_request` again";
        let refs = extract_backtick_refs(text);
        assert_eq!(refs, vec!["authenticate_request", "auth::verify"]);
    }

    #[test]
    fn skips_prose_and_unterminated() {
        assert!(extract_backtick_refs("no code here").is_empty());
        assert!(extract_backtick_refs("unterminated `ref").is_empty());
        // Spaces disqualify.
        assert!(extract_backtick_refs("`two words`").is_empty());
    }

    #[test]
    fn prose_shaped_inline_code_is_not_a_link_candidate() {
        // CLI flags, paths, file names, IPs, and issue refs are not symbols.
        assert!(extract_backtick_refs("run with `--json` flag").is_empty());
        assert!(extract_backtick_refs("see `src/handlers/graph.rs`").is_empty());
        assert!(extract_backtick_refs("read `README.md` first").is_empty());
        assert!(extract_backtick_refs("bind `127.0.0.1` only").is_empty());
        assert!(extract_backtick_refs("tracked in `#42`").is_empty());
    }

    #[test]
    fn symbol_shaped_refs_pass() {
        assert_eq!(
            extract_backtick_refs("`search_code` `SqliteStore::read` `parser.for_id` `_private`"),
            vec![
                "search_code",
                "SqliteStore::read",
                "parser.for_id",
                "_private"
            ]
        );
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::storage::sqlite::schema::{DocMetaRow, SymbolRow};
    use rusqlite::Connection;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::storage::sqlite::schema::SCHEMA_SQL)
            .unwrap();
        conn
    }

    fn code_symbol(id: &str, name: &str, exported: bool) -> SymbolRow {
        SymbolRow {
            id: id.to_string(),
            file_path: format!("src/{name}.rs"),
            language: "rust".into(),
            kind: "function".into(),
            name: name.to_string(),
            exported,
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            end_line: 2,
            text: format!("fn {name}() {{}}"),
        }
    }

    fn doc_symbol(id: &str, text: &str) -> SymbolRow {
        SymbolRow {
            id: id.to_string(),
            file_path: "docs/adr/0001.md".into(),
            language: "markdown".into(),
            kind: "document".into(),
            name: "Decision".into(),
            exported: false,
            start_byte: 0,
            end_byte: text.len() as u32,
            start_line: 5,
            end_line: 9,
            text: text.to_string(),
        }
    }

    #[test]
    fn resolves_backtick_refs_to_documents_edges() {
        let conn = setup();
        for s in [
            code_symbol("sym_1", "search_code", true),
            code_symbol("sym_2", "verify", false),
        ] {
            crate::storage::sqlite::queries::symbols::upsert_symbol(&conn, &s).unwrap();
        }
        let doc = doc_symbol(
            "doc_1",
            "Use `search_code` and `verify` here. `missing_fn` is stale.",
        );
        let (edges, stale) = extract_doc_link_edges(&doc, None, &conn).unwrap();
        assert_eq!(stale, 1);
        assert_eq!(edges.len(), 2);
        assert!(edges
            .iter()
            .all(|(e, _)| e.edge_type == DOC_REFERENCES_EDGE));
        let targets: Vec<&str> = edges.iter().map(|(e, _)| e.to_symbol_id.as_str()).collect();
        assert!(targets.contains(&"sym_1"));
        assert!(targets.contains(&"sym_2"));
        // Evidence rows accompany every edge.
        assert!(edges.iter().all(|(_, ev)| ev.len() == 1));
    }

    #[test]
    fn qualified_ref_falls_back_to_tail() {
        let conn = setup();
        crate::storage::sqlite::queries::symbols::upsert_symbol(
            &conn,
            &code_symbol("sym_1", "hydrate", true),
        )
        .unwrap();
        let doc = doc_symbol("doc_1", "calls `storage::hydrate` internally");
        let (edges, stale) = extract_doc_link_edges(&doc, None, &conn).unwrap();
        assert_eq!((edges.len(), stale), (1, 0));
        assert_eq!(edges[0].0.to_symbol_id, "sym_1");
    }

    #[test]
    fn issue_number_links_todo_files_via_tracks_edges() {
        let conn = setup();
        // Issue doc with number 42.
        crate::storage::sqlite::queries::symbols::upsert_symbol(
            &conn,
            &doc_symbol("doc_issue", "fix plan"),
        )
        .unwrap();
        crate::storage::sqlite::queries::docs::upsert_doc_meta(
            &conn,
            &DocMetaRow {
                file_path: "docs/adr/0001.md".into(),
                doc_type: "issue".into(),
                status: None,
                date: None,
                number: Some(42),
                labels: vec![],
            },
        )
        .unwrap();
        // A todo in another file mentioning #42, plus its file-root symbol.
        conn.execute(
            "INSERT INTO todos(id, kind, text, file_path, line) VALUES ('t1','todo','fixes #42','src/lib.rs',3)",
            [],
        )
        .unwrap();
        let root_id =
            crate::indexer::pipeline::utils::stable_symbol_id("src/lib.rs", "FILE_ROOT", 0);
        crate::storage::sqlite::queries::symbols::upsert_symbol(
            &conn,
            &SymbolRow {
                id: root_id.clone(),
                file_path: "src/lib.rs".into(),
                language: "rust".into(),
                kind: "file".into(),
                name: "src/lib.rs".into(),
                exported: false,
                start_byte: 0,
                end_byte: 1,
                start_line: 1,
                end_line: 10,
                text: String::new(),
            },
        )
        .unwrap();

        let doc = doc_symbol("doc_issue", "fix plan");
        let (edges, _stale) = extract_doc_link_edges(&doc, Some(42), &conn).unwrap();
        let tracks: Vec<_> = edges
            .iter()
            .filter(|(e, _)| e.edge_type == TRACKS_EDGE)
            .collect();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].0.from_symbol_id, root_id);
        assert_eq!(tracks[0].0.resolution, "todo_match");
    }

    #[test]
    fn stale_report_counts_unresolved_refs() {
        let conn = setup();
        crate::storage::sqlite::queries::symbols::upsert_symbol(
            &conn,
            &code_symbol("sym_1", "real_fn", true),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols(id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text) \
             VALUES ('d1','README.md','markdown','document','A',0,0,20,1,3,'see `real_fn` and `ghost_one`')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols(id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text) \
             VALUES ('d2','docs/setup.md','markdown','document','B',0,0,20,1,3,'and `ghost_two` alone')",
            [],
        )
        .unwrap();
        // A design spec referencing a ghost symbol is intent, not drift.
        conn.execute(
            "INSERT INTO symbols(id, file_path, language, kind, name, exported, start_byte, end_byte, start_line, end_line, text) \
             VALUES ('d3','docs/specs/future.md','markdown','document','C',0,0,20,1,3,'planned `ghost_three`')",
            [],
        )
        .unwrap();
        let report = count_stale_doc_links(&conn).unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(report.by_file.get("README.md"), Some(&1));
        assert_eq!(report.by_file.get("docs/setup.md"), Some(&1));
        assert!(!report.by_file.contains_key("docs/specs/future.md"));
    }
}

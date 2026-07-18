use std::collections::BTreeMap;

use anyhow::Result;
use serde_json::{json, Value};

use crate::storage::sqlite::{SqliteStore, SymbolRow};

#[derive(Debug, Clone)]
pub struct LogicalSymbol {
    pub logical_id: String,
    pub qualified_name: String,
    pub canonical: SymbolRow,
    pub occurrences: Vec<SymbolRow>,
}

#[derive(Debug, Clone)]
pub enum SymbolResolution {
    Exact(Box<LogicalSymbol>),
    Ambiguous(Vec<LogicalSymbol>),
    Unresolved,
}

impl SymbolResolution {
    pub fn state(&self) -> &'static str {
        match self {
            Self::Exact(_) => "exact",
            Self::Ambiguous(_) => "ambiguous",
            Self::Unresolved => "unresolved",
        }
    }

    pub fn logical_count(&self) -> usize {
        match self {
            Self::Exact(_) => 1,
            Self::Ambiguous(groups) => groups.len(),
            Self::Unresolved => 0,
        }
    }

    pub fn groups(&self) -> Vec<&LogicalSymbol> {
        match self {
            Self::Exact(group) => vec![group],
            Self::Ambiguous(groups) => groups.iter().collect(),
            Self::Unresolved => Vec::new(),
        }
    }

    pub fn into_exact(self, symbol_name: &str) -> std::result::Result<SymbolRow, Value> {
        match self {
            Self::Exact(group) => Ok(group.canonical),
            Self::Ambiguous(groups) => Err(json!({
                "symbol_name": symbol_name,
                "resolution": "ambiguous",
                "error": "SYMBOL_AMBIGUOUS",
                "message": format!(
                    "Multiple logical '{}' symbols found. Use an owner-qualified name and/or file parameter.",
                    symbol_name
                ),
                "candidates": candidate_values(&groups),
            })),
            Self::Unresolved => Err(json!({
                "symbol_name": symbol_name,
                "resolution": "unresolved",
                "error": "SYMBOL_NOT_FOUND",
                "message": format!("Symbol '{}' not found", symbol_name),
                "candidates": [],
            })),
        }
    }
}

pub fn candidate_values(groups: &[LogicalSymbol]) -> Vec<Value> {
    groups
        .iter()
        .map(|group| {
            json!({
                "logical_id": group.logical_id,
                "occurrence_count": group.occurrences.len(),
                "symbol_id": group.canonical.id,
                "qualified_name": group.qualified_name,
                "name": group.canonical.name,
                "kind": group.canonical.kind,
                "file_path": group.canonical.file_path,
                "start_line": group.canonical.start_line,
            })
        })
        .collect()
}

/// Resolve an exact or owner-qualified query into logical symbol groups.
/// Overload/partial occurrences are one exact result; distinct logical
/// declarations remain ambiguous even when their unqualified names match.
pub fn resolve_symbol(
    sqlite: &SqliteStore,
    symbol_name: &str,
    file: Option<&str>,
    limit: usize,
) -> Result<SymbolResolution> {
    let internal_limit = limit.max(100);
    let mut rows = sqlite.search_symbols_by_exact_name(symbol_name, file, internal_limit)?;
    if rows.is_empty() {
        rows = sqlite.search_symbols_by_qualified_name(symbol_name, file, internal_limit)?;
    }
    if rows.is_empty() {
        return Ok(SymbolResolution::Unresolved);
    }

    let ids = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
    let identities = sqlite.get_symbol_identities(&ids)?;
    let mut grouped = BTreeMap::<String, Vec<SymbolRow>>::new();
    for row in rows {
        let logical_id = identities
            .get(&row.id)
            .map(|identity| identity.logical_id.clone())
            .unwrap_or_else(|| row.id.clone());
        grouped.entry(logical_id).or_default().push(row);
    }

    let groups = grouped
        .into_iter()
        .map(|(logical_id, occurrences)| {
            let qualified_name = occurrences
                .iter()
                .find_map(|row| {
                    identities
                        .get(&row.id)
                        .map(|identity| identity.qualified_name.clone())
                })
                .unwrap_or_else(|| occurrences[0].name.clone());
            let canonical = occurrences
                .iter()
                .find(|row| {
                    identities
                        .get(&row.id)
                        .is_some_and(|identity| identity.is_canonical)
                })
                .cloned()
                .unwrap_or_else(|| occurrences[0].clone());
            LogicalSymbol {
                logical_id,
                qualified_name,
                canonical,
                occurrences,
            }
        })
        .collect::<Vec<_>>();

    Ok(match groups.as_slice() {
        [group] => SymbolResolution::Exact(Box::new(group.clone())),
        _ => SymbolResolution::Ambiguous(groups),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::queries;
    use crate::storage::sqlite::SymbolIdentityRow;

    fn symbol(id: &str, file: &str, start: u32) -> SymbolRow {
        SymbolRow {
            id: id.into(),
            file_path: file.into(),
            language: "typescript".into(),
            kind: "function".into(),
            name: "parse".into(),
            exported: true,
            start_byte: start,
            end_byte: start + 10,
            start_line: start + 1,
            end_line: start + 1,
            text: "function parse() {}".into(),
        }
    }

    #[test]
    fn overloads_are_exact_but_distinct_same_name_symbols_are_ambiguous() {
        let store = SqliteStore::open_in_memory().unwrap();
        store.init().unwrap();
        let conn = store.read().unwrap();
        queries::symbols::batch_upsert_symbols(
            &conn,
            &[
                symbol("parse", "a.ts", 0),
                symbol("parse-overload", "a.ts", 20),
            ],
        )
        .unwrap();
        queries::symbol_identities::batch_upsert(
            &conn,
            &[
                SymbolIdentityRow {
                    symbol_id: "parse".into(),
                    logical_id: "parse".into(),
                    qualified_name: "Parser.parse".into(),
                    signature: "parse(string)".into(),
                    occurrence_discriminator: "string:0".into(),
                    is_canonical: true,
                },
                SymbolIdentityRow {
                    symbol_id: "parse-overload".into(),
                    logical_id: "parse".into(),
                    qualified_name: "Parser.parse".into(),
                    signature: "parse(number)".into(),
                    occurrence_discriminator: "number:0".into(),
                    is_canonical: false,
                },
            ],
        )
        .unwrap();
        drop(conn);

        let exact = resolve_symbol(&store, "parse", Some("a.ts"), 10).unwrap();
        assert!(matches!(exact, SymbolResolution::Exact(_)));
        assert_eq!(exact.logical_count(), 1);

        store
            .upsert_symbol(&symbol("other-parse", "b.ts", 0))
            .unwrap();
        let ambiguous = resolve_symbol(&store, "parse", None, 10).unwrap();
        assert!(matches!(ambiguous, SymbolResolution::Ambiguous(_)));
        assert_eq!(ambiguous.logical_count(), 2);
    }

    #[test]
    fn owner_qualified_query_resolves_without_file_scope() {
        let store = SqliteStore::open_in_memory().unwrap();
        store.init().unwrap();
        store.upsert_symbol(&symbol("parse", "a.ts", 0)).unwrap();
        let conn = store.read().unwrap();
        queries::symbol_identities::batch_upsert(
            &conn,
            &[SymbolIdentityRow {
                symbol_id: "parse".into(),
                logical_id: "parse".into(),
                qualified_name: "Parser.parse".into(),
                signature: "parse()".into(),
                occurrence_discriminator: "parse:0".into(),
                is_canonical: true,
            }],
        )
        .unwrap();
        drop(conn);

        assert!(matches!(
            resolve_symbol(&store, "Parser.parse", None, 10).unwrap(),
            SymbolResolution::Exact(_)
        ));
    }
}

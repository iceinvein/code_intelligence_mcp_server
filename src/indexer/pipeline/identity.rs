use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::indexer::extract::symbol::{ExtractedSymbol, SymbolKind};
use crate::storage::sqlite::{SymbolIdentityRow, SymbolRow};

use super::parsing::symbol_kind_to_string;
use super::utils::{
    fnv1a_64, stable_symbol_id, stable_symbol_occurrence_id, stable_typed_logical_symbol_id,
};

#[derive(Debug)]
struct IdentitySeed {
    source_index: usize,
    qualified_name: String,
    kind: String,
    signature: String,
}

/// Convert extracted declarations into source-addressable symbol occurrences
/// plus location-independent logical identities.
pub fn build_symbol_occurrences(
    file_path: &str,
    language: &str,
    source: &str,
    symbols: &[ExtractedSymbol],
) -> Result<(Vec<SymbolRow>, Vec<SymbolIdentityRow>)> {
    let mut qualified_memo = HashMap::new();
    let seeds = symbols
        .iter()
        .enumerate()
        .filter_map(|(source_index, symbol)| {
            let text = source.get(symbol.bytes.start..symbol.bytes.end)?;
            if text.trim().is_empty() {
                return None;
            }
            Some(IdentitySeed {
                source_index,
                qualified_name: qualified_name_for(
                    source_index,
                    symbols,
                    language,
                    &mut qualified_memo,
                    &mut HashSet::new(),
                ),
                kind: symbol_kind_to_string(symbol.kind),
                signature: declaration_signature(text, symbol.kind),
            })
        })
        .collect::<Vec<_>>();

    // Preserve the legacy logical id for the lexically first kind occupying a
    // qualified name. Languages with separate type/value namespaces can
    // legitimately expose another kind under the same name; those receive a
    // typed logical id rather than overwriting the first occurrence.
    let mut kinds_by_name = BTreeMap::<String, BTreeSet<String>>::new();
    for seed in &seeds {
        kinds_by_name
            .entry(seed.qualified_name.clone())
            .or_default()
            .insert(seed.kind.clone());
    }

    let mut groups = BTreeMap::<(String, String), Vec<&IdentitySeed>>::new();
    for seed in &seeds {
        groups
            .entry((seed.qualified_name.clone(), seed.kind.clone()))
            .or_default()
            .push(seed);
    }

    let mut allocated = HashMap::<usize, (String, SymbolIdentityRow)>::new();
    let mut seen_ids = HashSet::new();
    for ((qualified_name, kind), mut occurrences) in groups {
        occurrences.sort_by_key(|seed| {
            let symbol = &symbols[seed.source_index];
            (seed.signature.clone(), symbol.bytes.start, symbol.bytes.end)
        });
        let canonical_kind = kinds_by_name
            .get(&qualified_name)
            .and_then(|kinds| kinds.first())
            .expect("identity group has a kind");
        let logical_id = if canonical_kind == &kind {
            stable_symbol_id(file_path, &qualified_name, 0)
        } else {
            stable_typed_logical_symbol_id(file_path, &qualified_name, &kind)
        };
        let mut duplicate_ordinals = HashMap::<String, usize>::new();

        for (group_index, seed) in occurrences.into_iter().enumerate() {
            let duplicate_ordinal = duplicate_ordinals
                .entry(seed.signature.clone())
                .and_modify(|ordinal| *ordinal += 1)
                .or_insert(0);
            let occurrence_discriminator = format!(
                "{:016x}:{}",
                fnv1a_64(seed.signature.as_bytes()),
                *duplicate_ordinal
            );
            let symbol_id = if group_index == 0 {
                logical_id.clone()
            } else {
                stable_symbol_occurrence_id(&logical_id, &seed.signature, *duplicate_ordinal)
            };
            if !seen_ids.insert(symbol_id.clone()) {
                bail!(
                    "Symbol occurrence id collision while parsing {file_path}: id={symbol_id}, qualified_name={qualified_name}, kind={kind}"
                );
            }
            allocated.insert(
                seed.source_index,
                (
                    symbol_id.clone(),
                    SymbolIdentityRow {
                        symbol_id,
                        logical_id: logical_id.clone(),
                        qualified_name: qualified_name.clone(),
                        signature: seed.signature.clone(),
                        occurrence_discriminator,
                        is_canonical: group_index == 0,
                    },
                ),
            );
        }
    }

    let mut rows = Vec::with_capacity(allocated.len());
    let mut identities = Vec::with_capacity(allocated.len());
    for (source_index, symbol) in symbols.iter().enumerate() {
        let Some((id, identity)) = allocated.remove(&source_index) else {
            continue;
        };
        let text = source
            .get(symbol.bytes.start..symbol.bytes.end)
            .unwrap_or_default()
            .to_string();
        rows.push(SymbolRow {
            id,
            file_path: file_path.to_string(),
            language: language.to_string(),
            kind: symbol_kind_to_string(symbol.kind),
            name: symbol.name.clone(),
            exported: symbol.exported,
            start_byte: symbol.bytes.start as u32,
            end_byte: symbol.bytes.end as u32,
            start_line: symbol.lines.start,
            end_line: symbol.lines.end,
            text,
        });
        identities.push(identity);
    }
    Ok((rows, identities))
}

fn qualified_name_for(
    index: usize,
    symbols: &[ExtractedSymbol],
    language: &str,
    memo: &mut HashMap<usize, String>,
    visiting: &mut HashSet<usize>,
) -> String {
    if let Some(name) = memo.get(&index) {
        return name.clone();
    }
    let symbol = &symbols[index];
    if symbol.name.contains('.') || symbol.name.contains("::") || !visiting.insert(index) {
        return symbol.name.clone();
    }

    let parent = symbols
        .iter()
        .enumerate()
        .filter(|(candidate_index, candidate)| {
            *candidate_index != index
                && is_identity_owner(candidate.kind)
                && candidate.bytes.start <= symbol.bytes.start
                && candidate.bytes.end >= symbol.bytes.end
                && (candidate.bytes.start < symbol.bytes.start
                    || candidate.bytes.end > symbol.bytes.end)
        })
        .min_by_key(|(_, candidate)| candidate.bytes.end.saturating_sub(candidate.bytes.start))
        .map(|(candidate_index, _)| candidate_index);

    let qualified = if let Some(parent_index) = parent {
        let parent_name = qualified_name_for(parent_index, symbols, language, memo, visiting);
        let separator = if matches!(language, "rust" | "cpp" | "ruby") {
            "::"
        } else {
            "."
        };
        format!("{parent_name}{separator}{}", symbol.name)
    } else {
        symbol.name.clone()
    };
    visiting.remove(&index);
    memo.insert(index, qualified.clone());
    qualified
}

fn is_identity_owner(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::Enum
            | SymbolKind::Const
            | SymbolKind::Struct
            | SymbolKind::Trait
            | SymbolKind::Impl
            | SymbolKind::Module
    )
}

/// Compact declaration header used to distinguish overload occurrences while
/// ignoring bodies and source position. It is intentionally language-agnostic
/// because the extractors already provide the exact declaration span.
fn declaration_signature(text: &str, kind: SymbolKind) -> String {
    let mut header = String::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut chars = text.trim().chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' if paren_depth == 0 && bracket_depth == 0 => break,
            ';' if paren_depth == 0 && bracket_depth == 0 => {
                header.push(ch);
                break;
            }
            ':' if paren_depth == 0
                && bracket_depth == 0
                && matches!(kind, SymbolKind::Function | SymbolKind::Class) =>
            {
                header.push(ch);
                break;
            }
            '=' if paren_depth == 0 && bracket_depth == 0 && chars.peek() == Some(&'>') => break,
            '\n' if paren_depth == 0 && bracket_depth == 0 && !header.trim().is_empty() => break,
            _ => {}
        }
        header.push(ch);
        if header.len() >= 512 {
            break;
        }
    }
    let normalized = header.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        text.lines().next().unwrap_or_default().trim().to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::extract::symbol::{ByteSpan, LineSpan};
    use crate::indexer::parser::LanguageId;

    fn extracted(
        name: &str,
        kind: SymbolKind,
        exported: bool,
        start: usize,
        end: usize,
    ) -> ExtractedSymbol {
        ExtractedSymbol {
            name: name.into(),
            kind,
            exported,
            bytes: ByteSpan { start, end },
            lines: LineSpan { start: 1, end: 1 },
        }
    }

    #[test]
    fn duplicate_methods_are_qualified_by_owner() {
        let source = "class One { run() {} }\nclass Two { run() {} }";
        let one_end = source.find("\nclass").unwrap();
        let one_run = source.find("run").unwrap();
        let two_start = one_end + 1;
        let two_run = source[two_start..].find("run").unwrap() + two_start;
        let symbols = vec![
            extracted("One", SymbolKind::Class, true, 0, one_end),
            extracted("run", SymbolKind::Function, false, one_run, one_run + 8),
            extracted("Two", SymbolKind::Class, true, two_start, source.len()),
            extracted("run", SymbolKind::Function, false, two_run, two_run + 8),
        ];

        let (_, identities) =
            build_symbol_occurrences("src/service.ts", "typescript", source, &symbols).unwrap();
        let qualified = identities
            .iter()
            .map(|identity| identity.qualified_name.as_str())
            .collect::<HashSet<_>>();
        assert!(qualified.contains("One.run"));
        assert!(qualified.contains("Two.run"));
        assert_eq!(
            identities
                .iter()
                .map(|identity| &identity.logical_id)
                .collect::<HashSet<_>>()
                .len(),
            4
        );
    }

    #[test]
    fn real_typescript_extraction_qualifies_duplicate_and_nested_members() {
        let source = r#"
export class Alpha { run(value: string) { return value; } }
export class Beta { run(value: number) { return value; } }
export function outer() { function inner() { return 1; } return inner(); }
"#;
        let extracted = crate::indexer::extract::typescript::extract_typescript_symbols(
            LanguageId::Typescript,
            source,
        )
        .unwrap();
        let (rows, identities) =
            build_symbol_occurrences("src/service.ts", "typescript", source, &extracted.symbols)
                .unwrap();
        let qualified = identities
            .iter()
            .map(|identity| identity.qualified_name.as_str())
            .collect::<HashSet<_>>();

        assert!(qualified.contains("Alpha.run"), "{qualified:?}");
        assert!(qualified.contains("Beta.run"), "{qualified:?}");
        assert!(qualified.contains("outer.inner"), "{qualified:?}");
        let run_ids = rows
            .iter()
            .filter(|row| row.name == "run")
            .map(|row| row.id.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(run_ids.len(), 2);
    }

    #[test]
    fn overloads_share_logical_identity_but_keep_unique_occurrences() {
        let source = "function parse(x: string): string;\nfunction parse(x: number): number;";
        let second = source.find("\n").unwrap() + 1;
        let symbols = vec![
            extracted("parse", SymbolKind::Function, true, 0, second - 1),
            extracted("parse", SymbolKind::Function, true, second, source.len()),
        ];

        let (rows, identities) =
            build_symbol_occurrences("src/parse.ts", "typescript", source, &symbols).unwrap();
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].id, rows[1].id);
        assert_eq!(identities[0].logical_id, identities[1].logical_id);
        assert_ne!(identities[0].signature, identities[1].signature);
        assert_eq!(identities.iter().filter(|row| row.is_canonical).count(), 1);
    }

    #[test]
    fn unique_symbol_identity_survives_source_movement() {
        let first_source = "export function run() {}";
        let moved_source = "\n\nexport function run() {}";
        let first = vec![extracted(
            "run",
            SymbolKind::Function,
            true,
            0,
            first_source.len(),
        )];
        let moved = vec![extracted(
            "run",
            SymbolKind::Function,
            true,
            2,
            moved_source.len(),
        )];

        let (first_rows, first_ids) =
            build_symbol_occurrences("src/run.ts", "typescript", first_source, &first).unwrap();
        let (moved_rows, moved_ids) =
            build_symbol_occurrences("src/run.ts", "typescript", moved_source, &moved).unwrap();
        assert_eq!(first_rows[0].id, moved_rows[0].id);
        assert_eq!(first_ids[0].logical_id, moved_ids[0].logical_id);
    }

    #[test]
    fn identical_partial_declarations_are_source_addressable() {
        let source = "partial class Service {}\npartial class Service {}";
        let second = source.find('\n').unwrap() + 1;
        let symbols = vec![
            extracted("Service", SymbolKind::Class, true, 0, second - 1),
            extracted("Service", SymbolKind::Class, true, second, source.len()),
        ];

        let (rows, identities) =
            build_symbol_occurrences("src/Service.cs", "csharp", source, &symbols).unwrap();
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].id, rows[1].id);
        assert_eq!(identities[0].logical_id, identities[1].logical_id);
        assert_ne!(
            identities[0].occurrence_discriminator,
            identities[1].occurrence_discriminator
        );
    }
}

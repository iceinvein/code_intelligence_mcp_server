use std::collections::{HashMap, HashSet};

use crate::indexer::extract::symbol::Import;
use crate::storage::sqlite::{SymbolRow, UsageExampleRow};

use super::parsing::{extract_callee_names, extract_identifiers, extract_usage_line, trim_snippet};
use super::utils::{build_import_map, resolve_imported_symbol_id};

pub fn extract_usage_examples_for_file(
    file_path: &str,
    source: &str,
    name_to_id: &HashMap<String, String>,
    imports: &[Import],
    symbol_rows: &[SymbolRow],
) -> Vec<UsageExampleRow> {
    let mut out = Vec::new();
    let mut seen: HashSet<(String, String, String, Option<u32>, String)> = HashSet::new();

    // Map import alias/name to Import struct
    let import_map = build_import_map(imports);

    for row in symbol_rows {
        for callee in extract_callee_names(&row.text) {
            let to_id = if let Some(local_id) = name_to_id.get(&callee) {
                if local_id == &row.id {
                    continue;
                }
                Some(local_id.clone())
            } else if let Some(imp) = import_map.get(callee.as_str()) {
                resolve_imported_symbol_id(file_path, imp)
            } else {
                None
            };

            let Some(to_id) = to_id else {
                continue;
            };

            let snippet =
                extract_usage_line(&row.text, &callee).unwrap_or_else(|| format!("{callee}("));
            let key = (
                to_id.clone(),
                "call".to_string(),
                file_path.to_string(),
                Some(row.start_line),
                snippet.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            out.push(UsageExampleRow {
                to_symbol_id: to_id.clone(),
                from_symbol_id: Some(row.id.clone()),
                example_type: "call".to_string(),
                file_path: file_path.to_string(),
                line: Some(row.start_line),
                snippet,
            });
        }

        let mut added = 0usize;
        for ident in extract_identifiers(&row.text) {
            if added >= 20 {
                break;
            }
            if ident == row.name {
                continue;
            }
            let to_id = if let Some(local_id) = name_to_id.get(&ident) {
                if local_id == &row.id {
                    continue;
                }
                Some(local_id.clone())
            } else if let Some(imp) = import_map.get(ident.as_str()) {
                resolve_imported_symbol_id(file_path, imp)
            } else {
                None
            };

            let Some(to_id) = to_id else {
                continue;
            };

            let snippet =
                extract_usage_line(&row.text, &ident).unwrap_or_else(|| ident.to_string());
            let key = (
                to_id.clone(),
                "reference".to_string(),
                file_path.to_string(),
                Some(row.start_line),
                snippet.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            out.push(UsageExampleRow {
                to_symbol_id: to_id.clone(),
                from_symbol_id: Some(row.id.clone()),
                example_type: "reference".to_string(),
                file_path: file_path.to_string(),
                line: Some(row.start_line),
                snippet,
            });
            added += 1;
        }
    }

    // Import usage examples
    for (idx, line) in source.lines().enumerate() {
        if !line.contains("import") {
            continue;
        }
        let line_no = u32::try_from(idx + 1).ok();

        for imp in imports {
            let name = imp.alias.as_ref().unwrap_or(&imp.name);
            if !line.contains(name) {
                continue;
            }

            let Some(to_id) = resolve_imported_symbol_id(file_path, imp) else {
                continue;
            };

            let snippet = trim_snippet(line, 200);
            let key = (
                to_id.clone(),
                "import".to_string(),
                file_path.to_string(),
                line_no,
                snippet.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            out.push(UsageExampleRow {
                to_symbol_id: to_id.clone(),
                from_symbol_id: None,
                example_type: "import".to_string(),
                file_path: file_path.to_string(),
                line: line_no,
                snippet,
            });
        }
    }

    out
}

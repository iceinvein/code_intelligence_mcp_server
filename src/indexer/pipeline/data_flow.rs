use std::collections::HashSet;

use crate::indexer::extract::symbol::{DataFlowEdge, DataFlowType};
use crate::storage::sqlite::{DataFlowFactRow, SymbolRow};

use super::parse::ParsedFile;

/// Return whether a file-level extracted fact belongs to this declaration.
///
/// Both lexical context and source span are required. This prevents a fact
/// from leaking into an enclosing type or another same-named method.
pub(crate) fn belongs_to_symbol(edge: &DataFlowEdge, row: &SymbolRow) -> bool {
    if row.kind == "file" {
        return false;
    }
    let context_matches =
        edge.to_symbol == row.name || edge.scope.as_deref() == Some(row.name.as_str());
    let location_matches = edge.at_line >= row.start_line && edge.at_line <= row.end_line;
    context_matches && location_matches
}

fn fact_for_edge(edge: &DataFlowEdge, owner: &SymbolRow) -> Option<DataFlowFactRow> {
    if !belongs_to_symbol(edge, owner) {
        return None;
    }

    let (entity_name, entity_kind, access_kind) =
        if let Some(name) = edge.from_symbol.strip_prefix("await:") {
            (name, "async_boundary", "await")
        } else if let Some(name) = edge.from_symbol.strip_prefix("spawn:") {
            (name, "async_boundary", "spawn")
        } else {
            let access = match edge.flow_type {
                DataFlowType::Reads => "read",
                DataFlowType::Writes => "write",
            };
            (edge.from_symbol.as_str(), "value", access)
        };

    if entity_name.trim().is_empty() {
        return None;
    }

    Some(DataFlowFactRow {
        owner_symbol_id: owner.id.clone(),
        entity_name: entity_name.to_string(),
        entity_kind: entity_kind.to_string(),
        access_kind: access_kind.to_string(),
        at_file: owner.file_path.clone(),
        at_line: edge.at_line,
        scope: edge.scope.clone(),
    })
}

/// Materialize typed facts for all local values and async boundaries in a file.
/// Resolved declarations remain in this relation as source-level facts while
/// also participating in the generic symbol graph.
pub fn extract_facts(parsed: &ParsedFile) -> Vec<DataFlowFactRow> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for owner in &parsed.symbol_rows {
        for edge in &parsed.dataflow_edges {
            let Some(fact) = fact_for_edge(edge, owner) else {
                continue;
            };
            let key = (
                fact.owner_symbol_id.clone(),
                fact.entity_name.clone(),
                fact.entity_kind.clone(),
                fact.access_kind.clone(),
                fact.at_file.clone(),
                fact.at_line,
            );
            if seen.insert(key) {
                out.push(fact);
            }
        }
    }
    out.sort_by(|a, b| {
        (
            &a.owner_symbol_id,
            a.at_line,
            &a.access_kind,
            &a.entity_name,
        )
            .cmp(&(
                &b.owner_symbol_id,
                b.at_line,
                &b.access_kind,
                &b.entity_name,
            ))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(id: &str, name: &str, start_line: u32, end_line: u32) -> SymbolRow {
        SymbolRow {
            id: id.into(),
            file_path: "src/lib.rs".into(),
            language: "rust".into(),
            kind: "function".into(),
            name: name.into(),
            exported: false,
            start_byte: 0,
            end_byte: 100,
            start_line,
            end_line,
            text: "fn work() {}".into(),
        }
    }

    #[test]
    fn fact_ownership_requires_context_and_source_span() {
        let edge = DataFlowEdge {
            from_symbol: "local_value".into(),
            to_symbol: "work".into(),
            flow_type: DataFlowType::Reads,
            at_line: 8,
            scope: Some("work".into()),
        };
        let first = symbol("first", "work", 1, 5);
        let second = symbol("second", "work", 7, 12);

        assert!(fact_for_edge(&edge, &first).is_none());
        let fact = fact_for_edge(&edge, &second).unwrap();
        assert_eq!(fact.owner_symbol_id, "second");
        assert_eq!(fact.entity_kind, "value");
        assert_eq!(fact.access_kind, "read");
    }

    #[test]
    fn async_prefixes_become_typed_boundaries() {
        let owner = symbol("owner", "work", 1, 20);
        for (prefix, access) in [("await:request", "await"), ("spawn:worker", "spawn")] {
            let edge = DataFlowEdge {
                from_symbol: prefix.into(),
                to_symbol: "work".into(),
                flow_type: DataFlowType::Reads,
                at_line: 4,
                scope: Some("work".into()),
            };
            let fact = fact_for_edge(&edge, &owner).unwrap();
            assert_eq!(fact.entity_kind, "async_boundary");
            assert_eq!(fact.access_kind, access);
            assert!(!fact.entity_name.contains(':'));
        }
    }
}

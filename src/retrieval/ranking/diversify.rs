use crate::retrieval::RankedHit;
use crate::storage::sqlite::SqliteStore;
use std::collections::HashMap;

/// Diversify results by similarity cluster
pub fn diversify_by_cluster(
    sqlite: &SqliteStore,
    hits: Vec<RankedHit>,
    limit: usize,
) -> Vec<RankedHit> {
    if hits.is_empty() || limit <= 1 {
        return hits;
    }

    let max_per_cluster = 2usize;
    let mut out = Vec::with_capacity(limit.min(hits.len()));
    let mut deferred = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();

    for h in hits {
        if out.len() >= limit {
            break;
        }
        let key = sqlite.get_similarity_cluster_key(&h.id).ok().flatten();
        match key {
            Some(k) => {
                let n = counts.get(&k).copied().unwrap_or(0);
                if n < max_per_cluster {
                    counts.insert(k, n + 1);
                    out.push(h);
                } else {
                    deferred.push(h);
                }
            }
            None => out.push(h),
        }
    }

    for h in deferred {
        if out.len() >= limit {
            break;
        }
        out.push(h);
    }

    out
}

/// Check if a kind represents a definition
pub fn is_definition_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class"
            | "interface"
            | "type_alias"
            | "struct"
            | "enum"
            | "function"
            | "method"
            | "const"
            | "trait"
            | "module"
    )
}

/// Diversify results by kind (definitions, tests, others)
pub fn diversify_by_kind(hits: Vec<RankedHit>, limit: usize) -> Vec<RankedHit> {
    if hits.len() <= limit {
        return hits;
    }

    let mut defs = Vec::new();
    let mut tests = Vec::new();
    let mut others = Vec::new();

    for h in hits {
        let is_test = h.file_path.contains(".test.")
            || h.file_path.contains(".spec.")
            || h.file_path.contains("/tests/")
            || h.file_path.contains("/__tests__/");

        if is_test {
            tests.push(h);
        } else if is_definition_kind(&h.kind) {
            defs.push(h);
        } else {
            others.push(h);
        }
    }

    let mut out = Vec::with_capacity(limit);
    let mut d_idx = 0;
    let mut t_idx = 0;
    let mut o_idx = 0;

    // Ensure diversity: pick top 1 from each category if available
    if d_idx < defs.len() {
        out.push(defs[d_idx].clone());
        d_idx += 1;
    }
    if o_idx < others.len() && out.len() < limit {
        out.push(others[o_idx].clone());
        o_idx += 1;
    }
    if t_idx < tests.len() && out.len() < limit {
        out.push(tests[t_idx].clone());
        t_idx += 1;
    }

    // Fill the rest by score
    while out.len() < limit {
        let d_score = defs.get(d_idx).map(|h| h.score).unwrap_or(-1.0);
        let t_score = tests.get(t_idx).map(|h| h.score).unwrap_or(-1.0);
        let o_score = others.get(o_idx).map(|h| h.score).unwrap_or(-1.0);

        if d_score < 0.0 && t_score < 0.0 && o_score < 0.0 {
            break;
        }

        if d_score >= t_score && d_score >= o_score {
            out.push(defs[d_idx].clone());
            d_idx += 1;
        } else if t_score >= d_score && t_score >= o_score {
            out.push(tests[t_idx].clone());
            t_idx += 1;
        } else {
            out.push(others[o_idx].clone());
            o_idx += 1;
        }
    }

    out
}

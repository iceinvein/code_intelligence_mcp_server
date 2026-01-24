# Phase 05: Learning System - Research

**Researched:** 2026-01-24
**Domain:** User feedback learning for code search ranking
**Confidence:** HIGH

## Summary

This phase implements a learning system that improves search results by learning from user selections. The system tracks two types of user behavior: (1) which results users select for specific queries, and (2) which files users frequently access. These signals are then used to boost relevant results in future searches.

The SQLite schema for learning (`query_selections` and `user_file_affinity` tables) already exists from FNDN-09 and FNDN-10. The query modules (`selections.rs` and `affinity.rs`) are already implemented. This phase focuses on integrating these features into the ranking pipeline and creating the MCP handler for reporting selections.

**Primary recommendation:** Use position-aware selection tracking with time-decay weighting, implemented as simple additive score boosts in the existing ranking pipeline. No external ML libraries needed - SQLite aggregates and time-based decay functions are sufficient.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| rusqlite | 0.32 | SQLite operations | Already in use, provides ON CONFLICT upsert needed |
| std::collections | built-in | HashMap, HashSet | For batch lookups of selection/affinity data |
| std::time | built-in | SystemTime, UNIX epoch | For time-decay calculations |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| anyhow | 1.0 | Error handling | Already in use throughout codebase |
| serde | 1.0 | Serialization | For tool argument parsing |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| rusqlite ON CONFLICT | Separate SELECT + INSERT/UPDATE | ON CONFLICT is atomic and simpler |
| Time-based decay in Rust | SQL time calculations | Rust gives more control over decay functions |

**Installation:**
No new dependencies required. All libraries already in `Cargo.toml`.

## Architecture Patterns

### Recommended Project Structure
```
src/
├── storage/
│   └── sqlite/
│       └── queries/
│           ├── selections.rs    # Already exists: insert, get_selections_for_query
│           └── affinity.rs      # Already exists: upsert, get_file_affinity
├── retrieval/
│   └── ranking/
│       ├── learning.rs          # NEW: Selection boost, file affinity boost
│       └── score.rs             # EXISTING: Apply learning boosts here
├── handlers/
│   └── mod.rs                   # ADD: handle_report_selection
└── tools/
    └── mod.rs                   # ADD: ReportSelectionTool
```

### Pattern 1: Selection Tracking with Position Awareness

**What:** Record user selections with the position they appeared in results. This enables position-bias-aware scoring (selections from lower positions are stronger signals).

**When to use:** For every user selection reported via the `report_selection` tool.

**Example:**
```rust
// In queries/selections.rs - already implemented
pub fn insert_query_selection(
    conn: &Connection,
    query_text: &str,
    query_normalized: &str,
    selected_symbol_id: &str,
    position: u32,
) -> Result<i64>
```

### Pattern 2: Query Normalization for Similarity Matching

**What:** Normalize queries (lowercase, trim, remove extra spaces) so similar queries match when computing selection boosts.

**When to use:** Before inserting selections AND before querying for past selections.

**Example:**
```rust
fn normalize_query_for_learning(query: &str) -> String {
    query
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
```

### Pattern 3: Time-Decay Weighting

**What:** Apply exponential decay to historical selections so recent behavior matters more than old behavior.

**When to use:** When computing selection boost scores.

**Example:**
```rust
// Source: Research on time-decay in collaborative filtering
// Exponential decay: weight = e^(-lambda * age_in_days)
fn compute_time_decay(created_at: i64, lambda: f64) -> f32 {
    let now = unix_now_s();
    let age_seconds = now - created_at;
    let age_days = age_seconds as f64 / 86400.0;
    (-lambda * age_days).exp() as f32
}
```

### Anti-Patterns to Avoid

- **Don't use position as raw score boost:** A selection at position 1 is less informative than position 10 (position bias). Apply position discount: `boost / log(position + 2)`.
- **Don't ignore query similarity:** "get user" and "getUser" should match. Always normalize before lookup.
- **Don't use linear decay:** Old selections never fully disappear with linear decay. Use exponential decay.
- **Don't skip file path normalization:** Ensure file paths are canonicalized before storing in affinity table.

## Don't Hand-Roll

Problems that look simple but have existing solutions:

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| SQLite upsert | INSERT OR check-exists-then-UPDATE | `ON CONFLICT DO UPDATE` | Atomic, no race conditions |
| Time calculation | Manual timestamp arithmetic | `unixepoch()` in SQL or `SystemTime::now()` | Handles edge cases |
| Query normalization | Custom string manipulation | Existing `normalize_query` in `src/retrieval/query.rs` | Already battle-tested |
| Batch lookups | N+1 individual queries | `IN` clause or `HashMap` batch load | O(1) vs O(N) database round trips |

**Key insight:** The learning system is essentially a lookup + boost problem. SQLite's aggregate functions and Rust's HashMap are sufficient. No ML framework needed.

## Common Pitfalls

### Pitfall 1: Position Bias Not Accounted For

**What goes wrong:** Selections from position 1 are treated same as position 10. This over-weights obvious top results.

**Why it happens:** Ignoring that users disproportionately click top results regardless of true relevance.

**How to avoid:** Apply position discount: `raw_boost * 1.0 / ln(position + 2)` or similar. This gives higher weight to selections from lower positions.

**Warning signs:** Top results stay at top even after alternative selections; learning feels "stuck."

### Pitfall 2: No Time Decay

**What goes wrong:** Old selections (months old) continue to influence rankings, preventing the system from adapting to changing codebases or user preferences.

**Why it happens:** Using raw selection counts without temporal weighting.

**How to avoid:** Always apply exponential time decay: `weight = e^(-lambda * age_days)`. Lambda of 0.1 means selections lose significance over ~10 days.

**Warning signs:** Recently added files never get boosted; user changes in working patterns aren't reflected.

### Pitfall 3: Query Normalization Mismatch

**What goes wrong:** Selections stored for "getUser" aren't found when user searches "get user".

**Why it happens:** Inconsistent normalization between insert and lookup.

**How to avoid:** Use the same `normalize_query_for_learning` function for both storage and retrieval. Reuse existing `normalize_query` from `src/retrieval/query.rs` if compatible.

**Warning signs:** Selection history never matches current queries; zero learning boost applied.

### Pitfall 4: File Affinity Not Updated

**What goes wrong:** File affinity boosts never apply because affinity table is never populated.

**Why it happens:** `report_selection` handler only updates query_selections, not user_file_affinity.

**How to avoid:** When recording a selection, also update the file affinity for that symbol's file path. Use `upsert_file_affinity` with `view_increment=1`.

**Warning signs:** Frequently edited files don't rise in rankings over time.

### Pitfall 5: Learning Not Toggleable

**What goes wrong:** Cannot disable learning to A/B test or debug ranking issues.

**Why it happens:** Learning boosts are always applied without config check.

**How to avoid:** Always check `config.learning_enabled` before applying boosts. Return early if disabled.

**Warning signs:** Can't isolate whether ranking changes are from learning vs other signals.

## Code Examples

### Selection Boost Computation

```rust
// Compute selection boost for a set of candidate symbols
// Returns: HashMap<symbol_id, boost_score>
pub fn compute_selection_boost(
    sqlite: &SqliteStore,
    query_normalized: &str,
    candidate_ids: &[String],
    config: &Config,
) -> Result<HashMap<String, f32>> {
    if !config.learning_enabled || candidate_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let selections = sqlite.get_selections_for_query(query_normalized, 100)?;
    let mut boosts = HashMap::new();

    for selection in selections {
        if !candidate_ids.contains(&selection.selected_symbol_id) {
            continue;
        }

        // Position discount: lower position = higher discount
        let position_discount = 1.0 / ((selection.position as f32) + 2.0).ln();

        // Time decay: recent selections weighted higher
        let age_days = (unix_now_s() - selection.created_at) as f32 / 86400.0;
        let time_decay = (-0.1 * age_days).exp(); // lambda = 0.1

        let boost = position_discount * time_decay * config.learning_selection_boost;
        boosts.entry(selection.selected_symbol_id.clone())
            .and_modify(|e| *e += boost)
            .or_insert(boost);
    }

    Ok(boosts)
}
```

### File Affinity Boost Computation

```rust
// Compute file affinity boost for candidate symbols
// Returns: HashMap<file_path, boost_score>
pub fn compute_file_affinity_boost(
    sqlite: &SqliteStore,
    candidate_files: &[String],
    config: &Config,
) -> Result<HashMap<String, f32>> {
    if !config.learning_enabled || candidate_files.is_empty() {
        return Ok(HashMap::new());
    }

    let mut boosts = HashMap::new();

    for file_path in candidate_files {
        if let Some(affinity) = sqlite.get_file_affinity(file_path)? {
            // Combine view and edit counts (edit counts weighted higher)
            let raw_score = affinity.view_count as f32 + (affinity.edit_count as f32 * 2.0);

            // Time decay on last_accessed_at
            let age_days = (unix_now_s() - affinity.last_accessed_at) as f32 / 86400.0;
            let time_decay = (-0.05 * age_days).exp(); // Slower decay for affinity

            let boost = (raw_score.min(10.0) / 10.0) * time_decay * config.learning_file_affinity_boost;
            boosts.insert(file_path.clone(), boost);
        }
    }

    Ok(boosts)
}
```

### Integration into Ranking Pipeline

```rust
// In src/retrieval/ranking/score.rs - add to existing rank_hits_with_signals
pub fn rank_hits_with_signals(
    keyword_hits: &[KeywordHit],
    vector_hits: &[VectorHit],
    config: &Config,
    intent: &Option<Intent>,
    query: &str,
    sqlite: &SqliteStore,
) -> (Vec<RankedHit>, HashMap<String, HitSignals>) {
    // ... existing ranking code ...

    // NEW: Apply learning boosts if enabled
    if config.learning_enabled {
        let query_normalized = normalize_query_for_learning(query);

        // Collect candidate IDs and file paths
        let symbol_ids: Vec<String> = merged.keys().cloned().collect();
        let file_paths: Vec<String> = merged.values().map(|h| h.file_path.clone()).collect();

        // Compute boosts
        let selection_boosts = compute_selection_boost(sqlite, &query_normalized, &symbol_ids, config)
            .unwrap_or_default();
        let affinity_boosts = compute_file_affinity_boost(sqlite, &file_paths, config)
            .unwrap_or_default();

        // Apply boosts to scores
        for hit in merged.values_mut() {
            if let Some(&sel_boost) = selection_boosts.get(&hit.id) {
                hit.score += sel_boost;
                signals.entry(hit.id.clone()).and_modify(|s| s.selection_boost += sel_boost);
            }
            if let Some(&aff_boost) = affinity_boosts.get(&hit.file_path) {
                hit.score += aff_boost;
                signals.entry(hit.id.clone()).and_modify(|s| s.affinity_boost += aff_boost);
            }
        }
    }

    // ... rest of existing ranking code ...
}
```

### Report Selection Handler

```rust
// In src/tools/mod.rs
#[macros::mcp_tool(
    name = "report_selection",
    description = "Report user's selection from search results for learning."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ReportSelectionTool {
    pub query: String,
    pub selected_symbol_id: String,
    pub position: u32,
}

// In src/handlers/mod.rs
pub async fn handle_report_selection(
    state: &AppState,
    tool: ReportSelectionTool,
) -> Result<serde_json::Value, anyhow::Error> {
    let sqlite = SqliteStore::open(&state.config.db_path)?;
    sqlite.init()?;

    // Normalize query for consistent matching
    let query_normalized = normalize_query_for_learning(&tool.query);

    // Record selection
    let _id = sqlite.insert_query_selection(
        &tool.query,
        &query_normalized,
        &tool.selected_symbol_id,
        tool.position,
    )?;

    // Also update file affinity
    if let Some(symbol) = sqlite.get_symbol_by_id(&tool.selected_symbol_id)? {
        sqlite.upsert_file_affinity(&symbol.file_path, 1, 0)?;
    }

    Ok(json!({
        "ok": true,
        "recorded": true,
    }))
}
```

### HitSignals Extension

```rust
// In src/retrieval/mod.rs - extend HitSignals
#[derive(Debug, Clone, Serialize)]
pub struct HitSignals {
    pub keyword_score: f32,
    pub vector_score: f32,
    pub base_score: f32,
    pub structural_adjust: f32,
    pub intent_mult: f32,
    pub definition_bias: f32,
    pub popularity_boost: f32,
    // NEW fields for learning
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_boost: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affinity_boost: Option<f32>,
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Static ranking only | Learning from user feedback | 2020s | Search personalization improves user satisfaction |
| Simple click counting | Position-bias-aware models | 2018+ | More accurate learning by accounting for position bias |
| Time-invariant user models | Time-decay preference models | 2023+ | System adapts to changing behavior faster |
| ML-based learning to rank | Simple statistical boost | 2026 | For code search, simple boosts work well without ML complexity |

**Deprecated/outdated:**
- **Raw click-through rate (CTR)**: Doesn't account for position bias, replaced by position-aware models
- **Long-term user profiles without decay**: Causes staleness, replaced by time-decay weighted profiles
- **Complex ML models for small-scale search**: Overkill, replaced by simple additive boosts

## Open Questions

1. **Query similarity threshold:** Should we use exact normalized query match, or enable fuzzy matching (e.g., embedding similarity) for "similar" queries?
   - What we know: Exact match is simpler and already implemented in schema via `query_normalized` index
   - What's unclear: Whether embedding-based similarity would provide significant additional value
   - Recommendation: Start with exact match, add similarity search if data shows many near-duplicate queries

2. **Optimal decay rate:** What lambda value for exponential decay best balances recency vs history?
   - What we know: Research suggests 0.05-0.1 for daily-decay applications
   - What's unclear: Optimal value for code search specifically
   - Recommendation: Make configurable via environment var, default to 0.1

3. **Position bias function:** Should we use logarithmic discount (`1/ln(pos+2)`) or another function?
   - What we know: Logarithmic is standard in learning-to-rank literature
   - What's unclear: Whether code search has different position bias characteristics
   - Recommendation: Use logarithmic initially, make function configurable if needed

## Sources

### Primary (HIGH confidence)

- **[Rusqlite Documentation](https://docs.rs/rusqlite/)** - ON CONFLICT clause, parameter binding
- **[Query Normalization (Zilliz)](https://zilliz.com/ai-faq/what-is-search-query-normalization)** - Query normalization best practices
- **[Existing codebase schema.rs](https://github.com/.../src/storage/sqlite/schema.rs)** - `query_selections` and `user_file_affinity` table definitions (lines 298-319)
- **[Existing selections.rs](https://github.com/.../src/storage/sqlite/queries/selections.rs)** - `insert_query_selection`, `get_selections_for_query` functions
- **[Existing affinity.rs](https://github.com/.../src/storage/sqlite/queries/affinity.rs)** - `upsert_file_affinity`, `get_file_affinity` functions

### Secondary (MEDIUM confidence)

- **[Position Bias Estimation for Unbiased Learning to Rank (Google Research, 2018)](https://research.google/pubs/archive/46485.pdf)** - EM algorithm for position bias, highly cited (350+)
- **[Time Weight Collaborative Filtering (Ding, 2005)](https://cseweb.ucsd.edu/classes/fa17/cse291-b/reading/p485-ding.pdf)** - Time decay foundation, 727 citations
- **[Adaptive Collaborative Filtering with Personalized Time Decay (Ghiye, 2023)](https://arxiv.org/pdf/2308.01208)** - Modern time decay approach, 22 citations
- **[Mastering Learning-to-Rank Algorithms]((https://medium.com/@amit25173/mastering-learning-to-rank-algorithms-practical-implementation-for-search-engines-and-5e7bd5bb709c))** - Practical LTR implementation guide
- **[Complete Guide to Embeddings in 2026](https://encord.com/blog/complete-guide-to-embeddings-in-2026/)** - Current embedding practices for query similarity
- **[Learning to Rank in Web Search](https://hav4ik.github.io/learning-to-rank/)** - Comprehensive LTR overview

### Tertiary (LOW confidence)

- **[Effects of Position Bias on Click-Based Recommender Systems]((http://anneschuth.nl/assets/hofmann-effects-2014.pdf))** - Position bias in recommenders
- **[Recency Adapted Next Basket Recommendation (Hurenkamp thesis)](https://thesis.eur.nl/pub/63345/FinalThesis_HannaHurenkamp.pdf)** - Recency weighting in practice

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - All dependencies already in use, verified in Cargo.toml
- Architecture: HIGH - Schema and query modules already exist, pattern established
- Pitfalls: MEDIUM - Based on general learning-to-rank research, code search specifics inferred

**Research date:** 2026-01-24
**Valid until:** 2026-03-01 (60 days - stable domain, existing schema)

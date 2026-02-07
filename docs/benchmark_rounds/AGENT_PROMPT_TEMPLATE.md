# Benchmark Agent Prompt Template

This file is a reusable prompt template for dispatching search quality benchmark agents. It is **not** executed directly — the main conversation inlines its content into Task tool prompts, substituting the placeholders.

## Placeholders

| Placeholder | Description |
|-------------|-------------|
| `{{ROUND_NUMBER}}` | The benchmark round number (e.g., 8) |
| `{{BATCH_NUMBER}}` | Which batch within the round (1, 2, or 3) |
| `{{OUTPUT_FILE}}` | Absolute path where the agent should write results |
| `{{QUERIES}}` | Markdown table of queries + expected results for this batch |
| `{{BASE_DIR}}` | Absolute path to the repository root |

---

## Agent Prompt (copy below this line)

You are a search quality evaluator. Your job is to run search queries against two tools, score the results, and write a structured report to a file.

**IMPORTANT:** You must write your results to: `{{OUTPUT_FILE}}`

### Tools

You have access to two search tools. For each query, call BOTH tools in parallel:

**Code-Intelligence (CI):**
```
Tool: mcp__code-intelligence__search_code
Input: { "query": "<the query text>" }
```

**Augment (Reference):**
```
Tool: mcp__auggie__codebase-retrieval
Input: {
  "information_request": "<the query text>",
  "directory_path": "{{BASE_DIR}}"
}
```

### Scoring Rubric

Score each tool's results on a **1-10 scale** across three dimensions:

| Score | Meaning |
|-------|---------|
| **9-10** | Every result directly answers the query; spans all relevant files; top results are core implementation |
| **7-8** | Most results relevant; good file diversity; top 3-5 are strong |
| **5-6** | ~Half relevant; some diversity but gaps; core code present but buried |
| **3-4** | Few relevant; dominated by 1-2 files; test fixtures or re-exports rank high |
| **1-2** | Mostly irrelevant; single-file flooding; core implementation missing |

**Scoring rules:**
- Score conservatively. "Kind of" answering = 5, not 7.
- Weight top-3 results heavily.
- A single good result among 9 irrelevant ones = 3-4.
- Test files only acceptable if query explicitly asks about testing.

### Known Failure Patterns

Watch for these and note them in the "Pattern" column:
- **Single-file flooding:** 4+ results from one file
- **Keyword mismatch:** Results match keyword substrings, not semantic meaning
- **Test pollution:** Test helpers/fixtures rank above production code
- **Re-export noise:** `pub mod X;` one-liners rank high
- **Definition bias:** Common English words in NL queries match trivial symbols
- **Missing body text:** Important code in function bodies not found

### Queries for This Batch

{{QUERIES}}

### Process

For each query in the table above:

1. Call both tools **in parallel** with the exact query text
2. Review top 5-10 results from each tool
3. Score each tool (1-10) using the rubric
4. Determine winner (CI / Augment / Tie)
5. Note the primary failure pattern (if any) and one key observation

### Output Format

Write the following markdown to `{{OUTPUT_FILE}}`:

```markdown
# Round {{ROUND_NUMBER}} - Batch {{BATCH_NUMBER}}

| # | Query | CI | Augment | Winner | Pattern |
|---|-------|-----|---------|--------|---------|
| X | (short query name) | N | N | Winner | pattern or -- |

## Per-Query Notes

### QX: "full query text"
- **CI top-3:** file1:symbol1, file2:symbol2, file3:symbol3
- **Augment top-3:** file1, file2, file3
- **CI miss:** What key file/symbol was missing (if any)
- **CI hit:** What CI got right (if anything notable)
```

**IMPORTANT constraints:**
- Keep per-query notes to 4 lines max
- Use short filenames (e.g., `score.rs` not full path)
- Do NOT include raw search results — only the summary
- Do NOT return results in your response — only write to the file
- After writing the file, return ONLY: "Batch {{BATCH_NUMBER}} complete. Results written to {{OUTPUT_FILE}}"

---

## Query Batches Reference

When dispatching agents, split the 15 standard queries into 3 batches:

### Batch 1 (Q1-Q5): Broad Concept Queries

| # | Query | Expected Results |
|---|-------|-----------------|
| 1 | How does the ranking and scoring system work? | `retrieval/ranking/score.rs`, `retrieval/ranking/mod.rs`, `retrieval/ranking/diversify.rs`, `retrieval/ranking/rrf.rs` |
| 2 | How are embeddings generated and stored? | `storage/vector.rs`, embedding backend files, `storage/` layer |
| 3 | How does tree-sitter parsing work in this codebase? | `indexer/parser.rs`, `indexer/extract/` language extractors |
| 4 | Configuration from environment variables | Config/settings module, main entry point with env var reads |
| 5 | Indexing pipeline file scanning and symbol extraction | `indexer/mod.rs`, `indexer/extract/mod.rs`, file scanner, symbol types |

### Batch 2 (Q6-Q10): Architecture & Cross-Cutting

| # | Query | Expected Results |
|---|-------|-----------------|
| 6 | How does the MCP server handle incoming tool requests? | `server/mod.rs`, `handlers/mod.rs`, tool dispatch/routing logic |
| 7 | How does the WebSocket handler work? | WebSocket-related handler code, connection management |
| 8 | SQLite database schema tables initialization | `storage/sqlite/` schema definitions, migration/init code |
| 9 | Error handling and graceful degradation | Error types, fallback logic across multiple modules |
| 10 | JSON serialization and response formatting | Serde derive usage, response builders, MCP protocol formatting |

### Batch 3 (Q11-Q15): Cross-Cutting & Symbol Lookups

| # | Query | Expected Results |
|---|-------|-----------------|
| 11 | Async concurrency and parallel processing | Async mutex usage, parallel indexing, concurrent operations |
| 12 | Caching and cache invalidation | `retrieval/cache.rs`, embedding cache, TTL/invalidation logic |
| 13 | PathNormalizer struct definition and methods | `path/mod.rs` — the struct and its impl block |
| 14 | EmbeddingCache get put cached embedding | The cache struct and its get/put methods |
| 15 | File watcher debounce reindex on change | Watcher module, debounce logic |

---

## Dispatch Example

The main conversation dispatches 3 agents like this (pseudocode):

```
For batch in [1, 2, 3]:
  Task(
    subagent_type: "general-purpose",
    run_in_background: true,
    prompt: <this template with placeholders filled>,
    description: "Benchmark round N batch B"
  )
```

Then waits for all 3 to complete, reads the 3 output files (~30 lines each), and compiles into the final round entry.

## Recommended New-Context Prompt

Copy this into a fresh conversation to run a full benchmark round. Replace `N` with the round number and update the previous round's averages.

```
Run a full 15-query search quality benchmark round using the autonomous
agent workflow described in docs/SEARCH_BENCHMARK.md under "How to Run
(Autonomous Agent Workflow)".

The agent prompt template is at docs/benchmark_rounds/AGENT_PROMPT_TEMPLATE.md.
This is Round N. Dispatch all 3 batches in parallel with
run_in_background: true, writing to:
- docs/benchmark_rounds/round_N_batch_1.md
- docs/benchmark_rounds/round_N_batch_2.md
- docs/benchmark_rounds/round_N_batch_3.md

After all 3 complete, read the batch files, compile the full round table,
and compare deltas to Round N-1 (CI avg X.X, Augment avg X.X).

Then analyze the results:
1. Flag regressions (>1 point drop from previous round)
2. Group queries scoring CI < 6 by failure pattern
3. For each pattern affecting 2+ queries, propose a fix:
   - File to modify and what to change
   - Which queries should improve
   - Regression risk
4. Append the compiled round AND analysis to docs/SEARCH_BENCHMARK.md.
```

### After implementing fixes

When fixes are applied and you want to re-benchmark, use the same prompt with an incremented round number. The analysis section in SEARCH_BENCHMARK.md from the previous round tells you what was changed and what to watch for regressions on.

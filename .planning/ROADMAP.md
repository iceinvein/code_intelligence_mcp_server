# Roadmap: Ultimate Context Engine

## Overview

Transform the Code Intelligence MCP Server into the best local context engine for LLM agents. Nine phases deliver progressively: foundation infrastructure, graph intelligence with PageRank, advanced retrieval with code-specific embeddings and reranking, token-aware context assembly, learning from user selections, seven new MCP tools, enhanced language extraction, performance optimizations, and finally multi-repo support. Each phase builds on the previous, with the retrieval and learning enhancements representing the core quality improvements.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

- [x] **Phase 1: Foundation** - Infrastructure for config, schema, and query understanding (completed 2026-01-23)
- [x] **Phase 2: Graph Intelligence** - PageRank and cross-file resolution (completed 2026-01-23)
- [x] **Phase 3: Retrieval Enhancements** - Jina Code embeddings, cross-encoder reranking, RRF, HyDE (completed 2026-01-24)
- [x] **Phase 4: Context Assembly** - Token-aware budgeting and smart formatting (completed 2026-01-24)
- [x] **Phase 5: Learning System** - Selection tracking and file affinity (completed 2026-01-24)
- [x] **Phase 6: New MCP Tools** - Seven powerful new tools (completed 2026-01-24)
- [x] **Phase 7: Language Enhancements** - JSDoc, decorators, TODOs, test linking (completed 2026-01-24)
- [x] **Phase 8: Performance & Scale** - Parallel indexing, caching, metrics (completed 2026-01-25)
- [ ] **Phase 9: Multi-Repo Support** - Package detection and cross-repo resolution

## Phase Details

### Phase 1: Foundation
**Goal**: Establish all infrastructure needed for subsequent phases
**Depends on**: Nothing (first phase)
**Requirements**: FNDN-01, FNDN-02, FNDN-03, FNDN-04, FNDN-05, FNDN-06, FNDN-07, FNDN-08, FNDN-09, FNDN-10, FNDN-11, FNDN-12, FNDN-13, FNDN-14, FNDN-15, FNDN-16, FNDN-17, FNDN-18, FNDN-19
**Success Criteria** (what must be TRUE):
  1. All new dependencies compile and are available (tiktoken-rs, rayon, prometheus, bincode, ort)
  2. Environment variables for query, reranker, learning, token, performance, and PageRank config are parsed and accessible
  3. SQLite schema includes all new tables (symbol_metrics, query_selections, user_file_affinity, repositories, packages, docstrings)
  4. Query decomposition splits compound queries into sub-queries that can be searched independently
  5. Synonym and acronym expansion transforms queries before search

**Plans:**
- [x] 01-01-PLAN.md — Dependencies and config infrastructure (completed 2026-01-23)
- [x] 01-02-PLAN.md — SQLite schema extensions and CRUD operations (completed 2026-01-23)
- [x] 01-03-PLAN.md — Query understanding enhancements (completed 2026-01-23)

### Phase 2: Graph Intelligence
**Goal**: Add PageRank-based symbol importance and improve graph resolution
**Depends on**: Phase 1
**Requirements**: GRPH-01, GRPH-02, GRPH-03, GRPH-04, GRPH-05, GRPH-06
**Success Criteria** (what must be TRUE):
  1. PageRank scores are computed for all symbols after indexing completes
  2. Search results rank symbols by PageRank instead of raw edge count
  3. Import edges resolve to actual symbol IDs across files (not just file references)
  4. Data flow edges (reads/writes) are extracted from TypeScript code

**Plans:**
- [x] 02-01-PLAN.md — PageRank implementation and integration (completed 2026-01-23)
- [x] 02-02-PLAN.md — PageRank-based ranking integration (completed 2026-01-23)
- [x] 02-03-PLAN.md — Cross-file resolution and data flow edges (completed 2026-01-23)

### Phase 3: Retrieval Enhancements
**Goal**: Implement code-specific embeddings, cross-encoder reranker, RRF, and HyDE
**Depends on**: Phase 2
**Requirements**: RETR-01, RETR-02, RETR-03, RETR-04, RETR-05, RETR-06, RETR-07
**Success Criteria** (what must be TRUE):
  1. Jina Code embeddings (jina-embeddings-v2-base-code) are used by default for all vector operations
  2. Cross-encoder reranker runs on every search, reordering top candidates
  3. Reciprocal Rank Fusion combines keyword, vector, and graph scores into final ranking
  4. HyDE generates hypothetical code snippets to improve embedding-based retrieval

**Plans:** 3 plans in 3 waves

**Plan List:**
- [x] 03-01-PLAN.md — Jina Code embeddings integration (completed 2026-01-24)
- [x] 03-02-PLAN.md — Cross-encoder reranker with ORT (completed 2026-01-24)
- [x] 03-03-PLAN.md — RRF and HyDE implementation (completed 2026-01-24)
- [x] UAT: Manual testing (completed 2026-01-24)

### Phase 4: Context Assembly
**Goal**: Token-aware budgeting and smarter context formatting
**Depends on**: Phase 3
**Requirements**: CTXT-01, CTXT-02, CTXT-03, CTXT-04, CTXT-05
**Success Criteria** (what must be TRUE):
  1. Context budgets are enforced in tokens (not bytes) using tiktoken
  2. Truncation preserves lines most relevant to the query
  3. Related symbols (return types, parameter types, parent classes) are auto-included
  4. Output is organized into structured sections (Definitions, Examples, Related)

**Plans:** 2 plans in 2 waves

**Plan List:**
- [x] 04-01-PLAN.md — Token counting and budget enforcement (completed 2026-01-24)
- [x] 04-02-PLAN.md — Smart truncation and structured output (completed 2026-01-24)

### Phase 5: Learning System
**Goal**: Learn from user selections to improve future results
**Depends on**: Phase 4
**Requirements**: LRNG-01, LRNG-02, LRNG-03, LRNG-04, LRNG-05, LRNG-06
**Success Criteria** (what must be TRUE):
  1. User selections are recorded via report_selection handler
  2. Selection history influences ranking (previously-selected results for similar queries rank higher)
  3. Frequently accessed files receive affinity boost in search results
  4. Learning can be enabled/disabled via environment variable

**Plans:** 2 plans in 2 waves

**Plan List:**
- [x] 05-01-PLAN.md — Selection tracking and learning boost (completed 2026-01-24)
- [x] 05-02-PLAN.md — File affinity tracking and integration (completed 2026-01-24)

### Phase 6: New MCP Tools
**Goal**: Add seven powerful new MCP tools
**Depends on**: Phase 5
**Requirements**: TOOL-01, TOOL-02, TOOL-03, TOOL-04, TOOL-05, TOOL-06, TOOL-07
**Success Criteria** (what must be TRUE):
  1. explain_search returns detailed scoring breakdown for any search
  2. find_similar_code finds code semantically similar to a given snippet
  3. summarize_file generates a summary of file contents and structure
  4. trace_data_flow traces variable reads/writes through the codebase
  5. find_affected_code identifies reverse dependencies (what would break if X changes)
  6. get_module_summary lists all exports with their signatures
  7. report_selection records user feedback on search results

**Plans:** 10 plans in 3 waves

**Plan List:**
- [x] 06-01-PLAN.md — explain_search tool definition and handler (with get_embedding_by_id) (completed 2026-01-24)
- [x] 06-02-PLAN.md — find_similar_code tool definition and handler (completed 2026-01-24)
- [x] 06-03-PLAN.md — Routing for explain_search and find_similar_code (completed 2026-01-24)
- [x] 06-04-PLAN.md — summarize_file tool definition and handler (completed 2026-01-24)
- [x] 06-05-PLAN.md — get_module_summary tool definition and handler (completed 2026-01-24)
- [x] 06-06-PLAN.md — Routing for summarize_file and get_module_summary (completed 2026-01-24)
- [x] 06-07-PLAN.md — trace_data_flow tool definition and handler (completed 2026-01-24)
- [x] 06-08-PLAN.md — find_affected_code tool definition and handler (completed 2026-01-24)
- [x] 06-09-PLAN.md — Routing for trace_data_flow and find_affected_code (completed 2026-01-24)
- [x] 06-10-PLAN.md — Integration tests for all tools (completed 2026-01-24)

### Phase 7: Language Enhancements
**Goal**: Better extraction for TypeScript and cross-language features
**Depends on**: Phase 6
**Requirements**: LANG-01, LANG-02, LANG-03, LANG-04
**Success Criteria** (what must be TRUE):
  1. JSDoc comments (@param, @returns, @example) are extracted and searchable
  2. TypeScript decorators (@Component, @Injectable, route decorators) are captured
  3. TODO and FIXME comments are extracted and queryable
  4. Test files are linked to the code they test

**Plans:** 7 plans (4 initial + 3 gap closure)

**Plan List:**
- [x] 07-01A-PLAN.md — JSDoc and decorator types and schema (completed 2026-01-24)
- [x] 07-01B-PLAN.md — JSDoc and decorator extraction, storage, and ranking boost (completed 2026-01-24)
- [x] 07-02A-PLAN.md — TODO and test link types and schema (completed 2026-01-24)
- [x] 07-02B-PLAN.md — TODO extraction, search_todos tool, find_tests_for_symbol tool (completed 2026-01-24)
- [x] 07-03-PLAN.md — Wire JSDoc examples into context assembly (completed 2026-01-24)
- [x] 07-04-PLAN.md — Apply 1.5x docstring boost in search pipeline (completed 2026-01-24)
- [x] 07-05-PLAN.md — Add search_decorators MCP tool (completed 2026-01-24)

### Phase 8: Performance & Scale
**Goal**: Parallel indexing, caching, and observability
**Depends on**: Phase 7
**Requirements**: PERF-01, PERF-02, PERF-03, PERF-04, PERF-05
**Success Criteria** (what must be TRUE):
  1. Indexing uses multiple CPU cores via rayon for parallel processing
  2. Embeddings are cached persistently (re-indexing unchanged files is fast)
  3. Prometheus metrics are exposed at /metrics endpoint
  4. Large codebases (100k+ symbols) index without degradation

**Plans:** 3 plans in 3 waves

**Plan List:**
- [x] 08-01-PLAN.md — Parallel indexing with rayon (completed 2026-01-25)
- [x] 08-02-PLAN.md — Persistent embedding cache (completed 2026-01-25)
- [x] 08-03-PLAN.md — Prometheus metrics (completed 2026-01-25)

### Phase 9: Multi-Repo Support
**Goal**: Handle monorepos and multiple repositories
**Depends on**: Phase 8
**Requirements**: REPO-01, REPO-02, REPO-03, REPO-04
**Success Criteria** (what must be TRUE):
  1. Package boundaries are detected (package.json, Cargo.toml, go.mod, etc.)
  2. Symbols can be resolved across repository boundaries
  3. Same-package results rank higher than cross-package results
  4. Multi-repo workspaces index and search correctly

**Plans:** 7 plans in 3 waves

**Plan List:**
- [ ] 09-01-PLAN.md — Module structure and manifest detector
- [ ] 09-02-PLAN.md — Language-specific manifest parsers (npm, Cargo, Go, Python)
- [ ] 09-03-PLAN.md — Git detection and SQLite CRUD operations
- [ ] 09-04-PLAN.md — Pipeline integration and configuration
- [ ] 09-05-PLAN.md — Batch package queries and ranking module
- [ ] 09-06-PLAN.md — Query controls and search integration
- [ ] 09-07-PLAN.md — Cross-package edge resolution

## Progress

**Execution Order:**
Phases execute in numeric order: 1 -> 2 -> 3 -> 4 -> 5 -> 6 -> 7 -> 8 -> 9

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Foundation | 3/3 | Complete | 2026-01-23 |
| 2. Graph Intelligence | 3/3 | Complete | 2026-01-23 |
| 3. Retrieval Enhancements | 3/3 | Complete | 2026-01-24 |
| 4. Context Assembly | 2/2 | Complete | 2026-01-24 |
| 5. Learning System | 2/2 | Complete | 2026-01-24 |
| 6. New MCP Tools | 10/10 | Complete | 2026-01-24 |
| 7. Language Enhancements | 7/7 | Complete | 2026-01-24 |
| 8. Performance & Scale | 3/3 | Complete | 2026-01-25 |
| 9. Multi-Repo Support | 0/7 | Not started | - |

**Total:** 33/40 plans complete (82%)

---
*Roadmap created: 2026-01-22*
*Last updated: 2026-01-25 (Phase 9 revised to 7 plans for better scope management)*

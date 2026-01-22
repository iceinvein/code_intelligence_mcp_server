# Ultimate Context Engine

## What This Is

A local code indexing and semantic search engine that provides structure-aware code navigation for LLM agents. Transforms the existing Code Intelligence MCP Server into the best context engine available - with code-specific embeddings, cross-encoder reranking, PageRank-based symbol importance, learning from user selections, and token-aware context assembly.

## Core Value

Search results are highly relevant and contextually rich - the right code, with the right context, every time. Quality and speed balanced equally.

## Requirements

### Validated

- ✓ Hybrid search (Tantivy keyword + LanceDB vector) — existing
- ✓ Tree-sitter parsing for 9 languages (Rust, TypeScript, JavaScript, Python, Go, Java, C, C++) — existing
- ✓ MCP tools: search_code, get_definition, find_references, get_call_hierarchy, get_type_graph, explore_dependency_graph, get_file_symbols, get_usage_examples, refresh_index — existing
- ✓ SQLite metadata storage with symbols, edges, files tables — existing
- ✓ FastEmbed embeddings (BGE-base-en-v1.5) — existing
- ✓ Intent detection for query understanding — existing
- ✓ Ranking signals: test penalty, glue code filter, directory semantics, export boost — existing

### Active

- [ ] Jina Code embeddings as default model
- [ ] Cross-encoder reranker (always-on, ORT-based)
- [ ] PageRank for symbol importance scoring
- [ ] Reciprocal Rank Fusion (RRF) combining keyword + vector + graph
- [ ] HyDE (Hypothetical Document Embedding) for better retrieval
- [ ] Token-aware context budgeting (tiktoken)
- [ ] Learning from user selections to improve results
- [ ] File affinity tracking and boosting
- [ ] Query decomposition ("X and Y" → sub-queries)
- [ ] Synonym and acronym expansion
- [ ] Data flow edge extraction (reads/writes)
- [ ] Cross-file symbol resolution
- [ ] JSDoc/docstring extraction and indexing
- [ ] TODO/FIXME comment extraction
- [ ] 7 new MCP tools: explain_search, find_similar_code, summarize_file, trace_data_flow, find_affected_code, get_module_summary, report_selection
- [ ] Parallel indexing with rayon
- [ ] Persistent embedding cache
- [ ] Prometheus metrics
- [ ] Multi-repo/monorepo support
- [ ] Package-aware scoring

### Out of Scope

- Cloud APIs for embeddings or inference — local models only
- User authentication — single-user local tool
- Web UI — CLI/MCP only for now
- GPU support beyond Metal — CPU + Metal only

## Context

**Existing Codebase:**
- Rust 2021, ~56 source files
- Multi-backend storage: SQLite + Tantivy + LanceDB
- MCP server via rust-mcp-sdk (stdio transport)
- FastEmbed for local embeddings
- Codebase already mapped in `.planning/codebase/`

**Technical Foundation:**
- Tree-sitter AST parsing is solid across 9 languages
- Ranking system has multiple signals but uses simple edge count for popularity
- Context assembly is byte-based, not token-aware
- No learning or personalization currently

**Detailed Roadmap:**
- User-provided roadmap in `.plans/CONTEXT_ENGINE_ROADMAP.md`
- 9 batches, 65 tasks
- Estimated effort: 25-33 hours

## Constraints

- **Models**: Local only — FastEmbed + ORT for cross-encoder, no cloud APIs
- **Embedding Model**: Jina Code as default (jinaai/jina-embeddings-v2-base-code)
- **Reranker**: Cross-encoder always-on via ORT
- **Learning**: Enabled by default, local storage only
- **Config**: All settings via environment variables

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Jina Code embeddings | Code-specific model outperforms general BGE for code search | — Pending |
| Cross-encoder always-on | Significant quality improvement justifies latency cost | — Pending |
| ORT for reranker | Consistent with local-only approach, cross-platform | — Pending |
| Learning on by default | Local storage means no privacy concerns | — Pending |
| Token-based budgeting | LLMs count tokens not bytes; better context utilization | — Pending |

---
*Last updated: 2026-01-22 after initialization*

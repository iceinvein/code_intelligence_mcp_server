# Code Intelligence MCP Server

> **Give your AI coding agent a deep understanding of your codebase.**

[![NPM Version](https://img.shields.io/npm/v/@iceinvein/code-intelligence-mcp?style=flat-square&color=blue)](https://www.npmjs.com/package/@iceinvein/code-intelligence-mcp)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](LICENSE)
[![MCP](https://img.shields.io/badge/MCP-Enabled-orange?style=flat-square)](https://modelcontextprotocol.io)
[![Platform](https://img.shields.io/badge/platform-macOS%20(Apple%20Silicon)-lightgrey?style=flat-square)]()

A local code indexing engine that gives LLM agents like **Claude Code**, **Cursor**, **Trae**, and **OpenCode** semantic search, call graphs, type hierarchies, and impact analysis across your codebase. Written in Rust with Metal GPU acceleration.

**Zero config. Runs via `npx`. Indexes in the background.**

---

## Install

### Claude Code

```bash
claude mcp add code-intelligence -- npx -y @iceinvein/code-intelligence-mcp
```

<details>
<summary>Or add manually to <code>~/.claude.json</code></summary>

```json
{
  "mcpServers": {
    "code-intelligence": {
      "command": "npx",
      "args": ["-y", "@iceinvein/code-intelligence-mcp"],
      "env": {}
    }
  }
}
```
</details>

### Cursor

Add to `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "code-intelligence": {
      "command": "npx",
      "args": ["-y", "@iceinvein/code-intelligence-mcp"]
    }
  }
}
```

### OpenCode / Trae

Add to `opencode.json`:

```json
{
  "mcp": {
    "code-intelligence": {
      "type": "local",
      "command": ["npx", "-y", "@iceinvein/code-intelligence-mcp"],
      "enabled": true
    }
  }
}
```

> On first launch, the server downloads the embedding model (~531 MB) and LLM (~1.1 GB), then indexes your project in the background. Models are cached in `~/.code-intelligence/models/`.

---

## What It Does

Unlike basic text search (grep/ripgrep), this server builds a **local knowledge graph** of your code and exposes it through 23 MCP tools.

| Capability | How It Works |
|---|---|
| **Hybrid search** | BM25 keyword search (Tantivy) + semantic vector search (LanceDB) merged via Reciprocal Rank Fusion |
| **On-device LLM descriptions** | Qwen2.5-Coder-1.5B generates natural-language summaries for every symbol, bridging the gap between how you search ("auth handler") and how code is named (`authenticate_request`) |
| **Graph intelligence** | Call hierarchies, type graphs, dependency trees, and PageRank-based importance scoring |
| **Impact analysis** | Find all code affected by a change before you make it |
| **Smart ranking** | Test detection, export boosting, directory semantics, intent detection, edge expansion, and score-gap filtering |
| **Multi-repo** | Index and search across multiple repositories simultaneously |
| **Auto-reindex** | OS-native file watching (FSEvents) keeps the index fresh as you code |

---

## Tools (23)

### Search & Navigation

| Tool | What It Does |
|---|---|
| `search_code` | Semantic + keyword hybrid search. Handles natural language ("how does auth work?") and structural queries ("class User") |
| `get_definition` | Jump to a symbol's full definition |
| `find_references` | Find all usages of a function, class, or variable |
| `get_call_hierarchy` | Upstream callers and downstream callees |
| `get_type_graph` | Inheritance chains, type aliases, implements relationships |
| `explore_dependency_graph` | Module-level import/export dependencies |
| `get_file_symbols` | All symbols defined in a file |
| `get_usage_examples` | Real-world usage examples from the codebase |

### Analysis

| Tool | What It Does |
|---|---|
| `find_affected_code` | Reverse dependency analysis — what breaks if this changes? |
| `trace_data_flow` | Follow variable reads and writes through the code |
| `find_similar_code` | Semantically similar code to a given symbol |
| `get_similarity_cluster` | Symbols in the same semantic cluster |
| `explain_search` | Scoring breakdown explaining why results ranked as they did |
| `summarize_file` | File summary with symbol counts and key exports |
| `get_module_summary` | All exported symbols from a module with signatures |

### Testing, Frameworks & Discovery

| Tool | What It Does |
|---|---|
| `find_tests_for_symbol` | Find tests that cover a given symbol |
| `search_todos` | Search TODO/FIXME comments |
| `search_decorators` | Find TypeScript/JavaScript decorators |
| `search_framework_patterns` | Find framework-specific patterns (routes, middleware, WebSocket handlers) |

### Index Management

| Tool | What It Does |
|---|---|
| `hydrate_symbols` | Load full context for a set of symbol IDs |
| `report_selection` | Feedback loop — tell the server which result was useful |
| `refresh_index` | Manually trigger re-indexing |
| `get_index_stats` | Index statistics (files, symbols, edges, last updated) |

---

## Supported Languages

Rust, TypeScript/TSX, JavaScript, Python, Go, Java, C, C++

---

## Standalone Mode (Multi-Client)

By default each MCP client spawns its own server process. If you run multiple clients (e.g. 5 Claude Code sessions across 3 repos), standalone mode loads the models **once** and shares them:

```bash
npx @iceinvein/code-intelligence-mcp-standalone
```

Then point all clients to `http://localhost:3333/mcp`:

<details>
<summary>Claude Code</summary>

```bash
claude mcp add --transport http code-intelligence http://localhost:3333/mcp
```
</details>

<details>
<summary>Cursor</summary>

```json
{
  "mcpServers": {
    "code-intelligence": {
      "url": "http://localhost:3333/mcp"
    }
  }
}
```
</details>

<details>
<summary>OpenCode</summary>

```json
{
  "mcp": {
    "code-intelligence": {
      "type": "remote",
      "url": "http://localhost:3333/mcp",
      "enabled": true
    }
  }
}
```
</details>

The server auto-detects each client's workspace via the MCP `roots` capability. Separate indexes are maintained per repo, models are shared.

```
┌──────────┐  ┌──────────┐  ┌──────────┐
│ Claude A  │  │ Cursor B │  │  Trae C  │
└─────┬─────┘  └─────┬─────┘  └─────┬─────┘
      │              │              │
      └──────── POST /mcp ─────────┘
                     │
        ┌────────────┴────────────┐
        │   Standalone Server     │
        │  (shared models, once)  │
        ├─────────────────────────┤
        │ Repo A   Repo B  Repo C│
        │ indexes  indexes indexes│
        └─────────────────────────┘
```

---

## Configuration

Works out of the box with no configuration. All settings are optional environment variables.

<details>
<summary><strong>Environment variables</strong></summary>

**Core:**

| Variable | Default | Description |
|---|---|---|
| `WATCH_MODE` | `true` | Auto-reindex on file changes |
| `INDEX_PATTERNS` | `**/*.ts,**/*.rs,...` | Glob patterns to index |
| `EXCLUDE_PATTERNS` | `**/node_modules/**,...` | Glob patterns to exclude |
| `REPO_ROOTS` | — | Comma-separated paths for multi-repo |

**Embeddings:**

| Variable | Default | Description |
|---|---|---|
| `EMBEDDINGS_BACKEND` | `llamacpp` | `llamacpp` or `hash` (fast testing, no model download) |
| `EMBEDDINGS_DEVICE` | `metal` | `metal` (GPU) or `cpu` |

**Ranking:**

| Variable | Default | Description |
|---|---|---|
| `HYBRID_ALPHA` | `0.7` | Vector vs keyword weight (0 = all keyword, 1 = all vector) |
| `RANK_EXPORTED_BOOST` | `1.0` | Boost for exported/public symbols |
| `RANK_TEST_PENALTY` | `0.1` | Penalty multiplier for test files |
| `RANK_POPULARITY_WEIGHT` | `0.05` | PageRank influence on ranking |

**Context:**

| Variable | Default | Description |
|---|---|---|
| `MAX_CONTEXT_TOKENS` | `8192` | Token budget for assembled context |
| `MAX_CONTEXT_BYTES` | `200000` | Byte-based fallback limit |

**Learning (off by default):**

| Variable | Default | Description |
|---|---|---|
| `LEARNING_ENABLED` | `false` | Track user selections to personalize results |
| `LEARNING_SELECTION_BOOST` | `0.1` | Max boost from selection history |
| `LEARNING_FILE_AFFINITY_BOOST` | `0.05` | Max boost from file access frequency |

</details>

<details>
<summary><strong>Standalone server config (<code>~/.code-intelligence/server.toml</code>)</strong></summary>

```toml
[server]
host = "127.0.0.1"
port = 3333

[embeddings]
backend = "llamacpp"
device = "metal"

[repos.defaults]
index_patterns = "**/*.ts,**/*.tsx,**/*.rs,**/*.py,**/*.go"
exclude_patterns = "**/node_modules/**,**/dist/**,**/.git/**"
watch_mode = true

[lifecycle]
warm_ttl_seconds = 300      # How long idle repos stay in memory
```

Priority: CLI flags > Environment variables > `server.toml` > Defaults

</details>

---

## How Ranking Works

The search pipeline runs keyword search (BM25) and semantic vector search in parallel, merges them with Reciprocal Rank Fusion, then applies structural signals:

- **Intent detection** — "struct User" boosts definitions, "who calls login" triggers graph lookup, "User schema" boosts models 50-75x
- **Query decomposition** — "authentication and authorization" automatically splits into sub-queries
- **LLM-enriched index** — on-device Qwen2.5-Coder generates descriptions bridging vocabulary gaps
- **PageRank** — graph-based importance scoring identifies central, heavily-used symbols
- **Morphological expansion** — `watch` matches `watcher`, `index` matches `reindex`
- **Multi-layer test detection** — file paths, symbol names, and AST-level analysis (`#[test]`, `mod tests`)
- **Edge expansion** — high-ranking symbols pull in structurally related code (callers, type members)
- **Export boost** — public API surface ranks above private helpers
- **Score-gap detection** — drops trailing results that fall off a relevance cliff
- **Token-aware truncation** — context assembly keeps query-relevant lines within token budgets

For the full deep dive, see [System Architecture](SYSTEM_ARCHITECTURE.md).

---

## Data Storage

All data lives in `~/.code-intelligence/`:

```
~/.code-intelligence/
├── models/                     # Shared (embedding ~531 MB, LLM ~1.1 GB)
├── repos/
│   ├── registry.json           # Tracks all known repos
│   └── <hash>/                 # Per-repo (SHA256 of repo path)
│       ├── code-intelligence.db   # SQLite (symbols, edges, metadata)
│       ├── tantivy-index/         # BM25 full-text search
│       └── vectors/               # LanceDB vector embeddings
├── logs/
└── server.toml                 # Standalone config (optional)
```

---

## Development

```bash
cargo build --release
cargo test                                        # Full test suite
EMBEDDINGS_BACKEND=hash cargo test                # Fast (no model download)
./scripts/start_mcp.sh                            # Start MCP server
```

<details>
<summary>Project structure</summary>

```
src/
├── indexer/          # File scanning, Tree-Sitter parsing, symbol extraction, embeddings, LLM descriptions
├── storage/          # SQLite, Tantivy (BM25), LanceDB (vectors)
├── retrieval/        # Hybrid search, ranking signals, RRF, context assembly, reranker
├── graph/            # PageRank, call hierarchy, type graphs
├── handlers/         # MCP tool implementations
├── server/           # MCP protocol routing (embedded + standalone)
├── tools/            # Tool definitions (23 MCP tools)
├── embeddings/       # jina-code-0.5b (GGUF via llama.cpp)
├── llm/              # Qwen2.5-Coder-1.5B (GGUF via llama.cpp)
├── reranker/         # Cross-encoder reranker
└── path/             # UTF-8 path normalization (camino)
```
</details>

---

## License

MIT

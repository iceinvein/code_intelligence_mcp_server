# Code Intelligence MCP Server

> **Semantic search and code navigation for LLM agents.**

[![NPM Version](https://img.shields.io/npm/v/@iceinvein/code-intelligence-mcp?style=flat-square&color=blue)](https://www.npmjs.com/package/@iceinvein/code-intelligence-mcp)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](LICENSE)
[![MCP](https://img.shields.io/badge/MCP-Enabled-orange?style=flat-square)](https://modelcontextprotocol.io)

---

This server indexes your codebase locally to provide **fast, semantic, and structure-aware** code navigation to tools like OpenCode, Trae, and Cursor.

## Why Use This Server?

Unlike basic text search, this server builds a local knowledge graph to understand your code.

* **Advanced Hybrid Search**: Combines **Tantivy** (keyword BM25) + **LanceDB** (semantic vector) + **Jina Code embeddings** (768-dim code-specific model) with Reciprocal Rank Fusion (RRF).
* **Cross-Encoder Reranking**: Always-on ORT-based reranker for precision result ranking.
* **Smart Context Assembly**: Token-aware budgeting with query-aware truncation that keeps relevant lines within context limits.
* **On-Device LLM Descriptions**: Automatically generates natural-language descriptions for every symbol using a local **Qwen2.5-Coder-1.5B** model (llama.cpp with Metal GPU), enriching search with human-readable summaries.
* **PageRank Scoring**: Graph-based symbol importance scoring that identifies central, heavily-used components.
* **Learns from Feedback**: Optional learning system that adapts to user selections over time.
* **Production First**: Multi-layer test detection (file paths, symbol names, and AST-level `#[test]`/`mod tests` analysis) ensures implementation code ranks above test helpers.
* **Multi-Repo Support**: Index and search across multiple repositories/monorepos simultaneously.
* **OS-Native File Watching**: Uses the `notify` crate with macOS FSEvents for instant re-indexing on file changes.
* **Fast & Local**: Written in **Rust** with Metal GPU acceleration on Apple Silicon. Parallel indexing with persistent caching.

---

## Quick Start

Runs directly via `npx` without requiring a local Rust toolchain.

### OpenCode / Trae

Add to your `opencode.json` (or global config):

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

*The server will automatically download the embedding model (~300MB) and LLM (~1.8GB) on first launch, then index your project in the background.*

---

## Standalone Server Mode

By default, each MCP client spawns its own server process (stdio transport). If you run multiple clients — say 5-6 Claude Code instances — each loads its own copy of the embedding model (~500MB), consuming ~3.6GB total.

**Standalone mode** runs a single long-lived HTTP server that all clients share. One embedding model, one process, ~70% memory reduction.

### Starting the Server

```bash
# Default: localhost:3333
npx @iceinvein/code-intelligence-mcp-standalone

# Custom host/port
npx @iceinvein/code-intelligence-mcp-standalone --port 4444 --host 0.0.0.0

# From source
./target/release/code-intelligence-mcp-server --standalone
./target/release/code-intelligence-mcp-server --standalone --port 4444

# Via environment variable
CIMCP_MODE=standalone ./target/release/code-intelligence-mcp-server
```

### Connecting MCP Clients

Point your MCP clients to the standalone server using Streamable HTTP transport:

**Claude Code** (`~/.claude/claude_desktop_config.json`):
```json
{
  "mcpServers": {
    "code-intelligence": {
      "type": "streamable-http",
      "url": "http://localhost:3333/mcp"
    }
  }
}
```

**OpenCode** (`opencode.json`):
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

**Cursor** (`.cursor/mcp.json`):
```json
{
  "mcpServers": {
    "code-intelligence": {
      "url": "http://localhost:3333/mcp"
    }
  }
}
```

The server auto-detects each client's workspace root via the MCP `roots` capability — no `BASE_DIR` needed.

### How It Works

```mermaid
flowchart TB
  A[Claude Code - Session A] & B[Cursor - Session B] & C[Trae - Session C]
  A & B & C -- "POST /mcp (Streamable HTTP)" --> Server

  Server["Standalone MCP Server<br/>(single process, shared embedding model)"]

  Server --> RA["Repo A indexes<br/>SQLite + Tantivy + LanceDB"]
  Server --> RB["Repo B indexes<br/>SQLite + Tantivy + LanceDB"]
  Server --> RC["Repo C indexes<br/>SQLite + Tantivy + LanceDB"]
```

Each client session is bound to its workspace root. The server maintains separate indexes per repo but shares the embedding model across all of them.

### Data Storage

Both embedded (stdio) and standalone (HTTP) modes store all data in `~/.code-intelligence/`:

```text
~/.code-intelligence/
├── server.toml              # Optional config file (standalone only)
├── models/                  # Shared models (loaded once, shared across repos)
│   ├── jina-code-onnx/      # Embedding model (~500MB)
│   └── qwen2.5-coder-1.5b-gguf/  # LLM model (~1.1GB)
├── logs/
│   └── server.log
└── repos/
    ├── registry.json        # Tracks all known repos
    ├── a1b2c3d4e5f6a7b8/   # Per-repo data (SHA256 hash of repo path)
    │   ├── code-intelligence.db
    │   ├── tantivy-index/
    │   └── vectors/
    └── f8e7d6c5b4a3f2e1/
        └── ...
```

The same repo always maps to the same hash regardless of mode, so embedded and standalone can share the same index data.

### Configuration

Standalone mode is configured via `~/.code-intelligence/server.toml` (created on first run with defaults). Environment variables and CLI flags override TOML settings.

**Priority:** CLI flags > Environment variables > `server.toml` > Defaults

**Example `server.toml`:**

```toml
[server]
host = "127.0.0.1"
port = 3333

[embeddings]
backend = "jinacode"        # jinacode (default), fastembed, hash
device = "metal"            # cpu or metal (macOS GPU)
auto_download = false
model_repo = "jinaai/jina-embeddings-v2-base-code"

[repos.defaults]
index_patterns = "**/*.ts,**/*.tsx,**/*.rs,**/*.py,**/*.go"
exclude_patterns = "**/node_modules/**,**/dist/**,**/.git/**"
watch_mode = true           # Auto-reindex on file changes

[lifecycle]
warm_ttl_seconds = 300      # How long idle repos stay in memory
```

**Environment variable overrides (same as embedded mode):**

| Variable | Example | Description |
| -------- | ------- | ----------- |
| `CIMCP_MODE` | `standalone` | Alternative to `--standalone` flag |
| `EMBEDDINGS_BACKEND` | `hash` | Override embedding backend |
| `EMBEDDINGS_DEVICE` | `metal` | Override device (cpu/metal) |
| `EMBEDDINGS_MODEL_REPO` | `jinaai/...` | Override model repo |
| `EMBEDDINGS_MODEL_DIR` | `/path/to/model` | Override model directory |
| `EMBEDDINGS_MAX_THREADS` | `4` | Limit embedding threads |

---

## Capabilities

Available tools for the agent (23 tools total):

### Core Search & Navigation

| Tool                       | Description                                                                                                                                                             |
| :------------------------- | :---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `search_code`              | **Primary Search.** Finds code by meaning ("how does auth work?") or structure ("class User"). Supports query decomposition (e.g., "authentication and authorization"). |
| `get_definition`           | Retrieves the full definition of a specific symbol with disambiguation support.                                                                                         |
| `find_references`          | Finds all usages of a function, class, or variable.                                                                                                                     |
| `get_call_hierarchy`       | Specifies upstream callers and downstream callees.                                                                                                                      |
| `get_type_graph`           | Explores inheritance (extends/implements) and type aliases.                                                                                                             |
| `explore_dependency_graph` | Explores module-level dependencies upstream or downstream.                                                                                                              |
| `get_file_symbols`         | Lists all symbols defined in a specific file.                                                                                                                           |
| `get_usage_examples`       | Returns real-world examples of how a symbol is used in the codebase.                                                                                                    |

### Advanced Analysis

| Tool                     | Description                                                                               |
| :----------------------- | :---------------------------------------------------------------------------------------- |
| `explain_search`         | Returns detailed scoring breakdown to understand why results ranked as they did.          |
| `find_similar_code`      | Finds code semantically similar to a given symbol or code snippet.                        |
| `trace_data_flow`        | Traces variable reads and writes through the codebase to understand data flow.            |
| `find_affected_code`     | Finds code that would be affected if a symbol changes (reverse dependencies).             |
| `get_similarity_cluster` | Returns symbols in the same semantic similarity cluster as a given symbol.                |
| `summarize_file`         | Generates a summary of file contents including symbol counts, structure, and key exports. |
| `get_module_summary`     | Lists all exported symbols from a module/file with their signatures.                      |

### Testing, Frameworks & Documentation

| Tool                       | Description                                                                                                               |
| :------------------------- | :------------------------------------------------------------------------------------------------------------------------ |
| `search_todos`             | Searches for TODO and FIXME comments to track technical debt.                                                             |
| `find_tests_for_symbol`    | Finds test files that test a given symbol or source file.                                                                 |
| `search_decorators`        | Searches for TypeScript/JavaScript decorators (@Component, @Controller, @Get, @Post, etc.).                               |
| `search_framework_patterns`| Searches for framework-specific patterns (e.g., Elysia routes, WebSocket handlers, middleware) with method/path filtering.|

### Context & Learning

| Tool               | Description                                                                     |
| :----------------- | :------------------------------------------------------------------------------ |
| `hydrate_symbols`  | Hydrates full context for a set of symbol IDs.                                  |
| `report_selection` | Records user selection feedback for learning (call when user selects a result). |
| `refresh_index`    | Manually triggers a re-index of the codebase.                                   |
| `get_index_stats`  | Returns index statistics (files, symbols, edges, last updated).                 |

---

## Supported Languages

The server supports semantic navigation and symbol extraction for the following languages:

* **Rust**
* **TypeScript / TSX**
* **JavaScript**
* **Python**
* **Go**
* **Java**
* **C**
* **C++**

---

## Smart Ranking & Context Enhancement

The ranking engine optimizes results for relevance using sophisticated signals:

1. **PageRank Symbol Importance**: Graph-based scoring that identifies central, heavily-used components (similar to Google's PageRank).
2. **Cross-Encoder Reranking**: Always-on ORT-based reranker applies deep learning to fine-tune result order.
3. **Reciprocal Rank Fusion (RRF)**: Combines keyword, vector, and graph search results using statistically optimal rank fusion.
4. **Query Decomposition**: Complex queries ("X and Y") are automatically split into sub-queries for better coverage.
5. **Token-Aware Truncation**: Context assembly keeps query-relevant lines within token budgets using BM25-style relevance scoring.
6. **LLM-Enriched Indexing**: On-device Qwen2.5-Coder generates natural-language descriptions for each symbol, bridging the vocabulary gap between how developers search and how code is named.
7. **Morphological Variants**: Function names are expanded with stems and derivations (e.g., `watch` → `watcher`, `index` → `reindex`) to improve recall for natural-language queries.
8. **Multi-Layer Test Detection**: Three mechanisms — file path patterns (`*.test.ts`), symbol name heuristics (`test_*`), and SQL-based AST analysis (`#[test]`, `mod tests`) — with a final enforcement pass that prevents test code from escaping via edge expansion.
9. **Edge Expansion**: High-ranking symbols pull in structurally related code (callers, type members) with importance filtering to avoid noise from private helpers.
10. **Directory Semantics**: Implementation directories (`src`, `lib`, `app`) are boosted, while build artifacts (`dist`, `build`) and `node_modules` are penalized.
11. **Exported Symbol Boost**: Exported/public symbols receive a ranking boost as they represent the primary API surface.
12. **Glue Code Filtering**: Re-export files (e.g., `index.ts`) are deprioritized in favor of the actual implementation.
13. **JSDoc Boost**: Symbols with documentation receive a ranking boost, and examples are included in search results.
14. **Learning from Feedback** (optional): Tracks user selections to personalize future search results.
15. **Package-Aware Scoring** (multi-repo): Boosts results from the same package when working in monorepos.

### Intent Detection

The system detects query intent and adjusts ranking accordingly:

| Query Pattern     | Intent                    | Effect                                  |
| ----------------- | ------------------------- | --------------------------------------- |
| "struct User"     | Definition                | Boosts type definitions (1.5x)          |
| "who calls login" | Callers                   | Triggers graph lookup                   |
| "verify login"    | Testing                   | Boosts test files                       |
| "User schema"     | Schema/Model              | Boosts schema/model files (50-75x)      |
| "auth and authz"  | Multi-query decomposition | Splits into sub-queries, merges via RRF |

For a deep dive into the system's design, see [System Architecture](SYSTEM_ARCHITECTURE.md).

---

## Configuration (Optional)

Works without configuration by default. You can customize behavior via environment variables:

### Core Settings

```json
"env": {
  "BASE_DIR": "/path/to/repo",           // Required: Repository root
  "WATCH_MODE": "true",                  // Watch for file changes (Default: true)
  "INDEX_PATTERNS": "**/*.ts,**/*.go",   // File patterns to index
  "EXCLUDE_PATTERNS": "**/node_modules/**",
  "REPO_ROOTS": "/path/to/repo1,/path/to/repo2"  // Multi-repo support
}
```

### Embedding Model

```json
"env": {
  "EMBEDDINGS_BACKEND": "jinacode",      // jinacode (default), fastembed, hash
  "EMBEDDINGS_DEVICE": "cpu",            // cpu or metal (macOS GPU)
  "EMBEDDING_BATCH_SIZE": "32"
}
```

### Context Assembly

```json
"env": {
  "MAX_CONTEXT_TOKENS": "8192",          // Token budget for context (default: 8192)
  "TOKEN_ENCODING": "o200k_base",        // tiktoken encoding model
  "MAX_CONTEXT_BYTES": "200000"          // Legacy byte-based limit (fallback)
}
```

### Ranking & Retrieval

```json
"env": {
  "RANK_EXPORTED_BOOST": "1.0",          // Boost for exported symbols
  "RANK_TEST_PENALTY": "0.1",            // Penalty for test files
  "RANK_POPULARITY_WEIGHT": "0.05",      // PageRank influence
  "RRF_ENABLED": "true",                 // Enable Reciprocal Rank Fusion
  "HYBRID_ALPHA": "0.7"                  // Vector vs keyword weight (0-1)
}
```

### Learning System (Optional)

```json
"env": {
  "LEARNING_ENABLED": "false",           // Enable selection tracking (default: false)
  "LEARNING_SELECTION_BOOST": "0.1",     // Boost for previously selected symbols
  "LEARNING_FILE_AFFINITY_BOOST": "0.05" // Boost for frequently accessed files
}
```

### Performance

```json
"env": {
  "PARALLEL_WORKERS": "1",               // Indexing parallelism (default: 1 for SQLite)
  "EMBEDDING_CACHE_ENABLED": "true",     // Persistent embedding cache
  "PAGERANK_ITERATIONS": "20",           // PageRank computation iterations
  "METRICS_ENABLED": "true",             // Prometheus metrics
  "METRICS_PORT": "9090"
}
```

### Query Expansion

```json
"env": {
  "SYNONYM_EXPANSION_ENABLED": "true",   // Expand "auth" → "authentication"
  "ACRONYM_EXPANSION_ENABLED": "true"    // Expand "db" → "database"
}
```

---

## Architecture

```mermaid
flowchart LR
  Client[MCP Client] <==> Tools

  subgraph Server [Code Intelligence Server]
    direction TB
    Tools[Tool Router]

    subgraph Indexer [Indexing Pipeline]
      direction TB
      Watch[OS-Native File Watcher] --> Scan[File Scan]
      Scan --> Parse[Tree-Sitter]
      Parse --> Extract[Symbol Extraction]
      Extract --> PageRank[PageRank Compute]
      Extract --> Embed[Jina Code Embeddings]
      Extract --> LLMDesc[LLM Descriptions - Qwen2.5-Coder]
      Extract --> JSDoc[JSDoc/Decorator/TODO Extract]
    end

    subgraph Storage [Storage Engine]
      direction TB
      SQLite[(SQLite)]
      Tantivy[(Tantivy)]
      Lance[(LanceDB)]
      Cache[(Embedding Cache)]
    end

    subgraph Retrieval [Retrieval Engine]
      direction TB
      QueryExpand[Query Expansion]
      Hybrid[Hybrid Search RRF]
      Rerank[Cross-Encoder Reranker]
      Signals[Ranking Signals]
      Context[Token-Aware Assembly]
    end

    %% Data Flow
    Tools -- Index --> Watch
    PageRank --> SQLite
    Embed --> Lance
    Embed --> Cache
    LLMDesc --> SQLite
    JSDoc --> SQLite

    Tools -- Query --> QueryExpand
    QueryExpand --> Hybrid
    Hybrid --> Rerank
    Rerank --> Signals
    Signals --> Context
    Context --> Tools
  end
```

---

## Development

1. **Prerequisites**: Rust (stable), `protobuf`.
2. **Build**: `cargo build --release`
3. **Run**: `./scripts/start_mcp.sh`
4. **Test**: `cargo test` or `EMBEDDINGS_BACKEND=hash cargo test` (faster, skips model download)

### Quick Testing with Hash Backend

For faster development iteration, use the hash embedding backend which skips model downloads:

```bash
EMBEDDINGS_BACKEND=hash BASE_DIR=/path/to/repo ./target/release/code-intelligence-mcp-server
```

### Project Structure

```text
src/
├── indexer/
│   ├── extract/       # Language-specific symbol extractors (Rust, TS, Python, Go, Java, C, C++)
│   ├── pipeline/      # Indexing pipeline stages (scan, parse, embed, watch, describe)
│   └── package/       # Package detection (npm, Cargo, Go, Python)
├── storage/
│   ├── sqlite/        # SQLite schema, queries, operations
│   ├── tantivy.rs     # BM25 full-text search with n-gram tokenization
│   └── vector.rs      # LanceDB vector embeddings
├── retrieval/
│   ├── ranking/       # Scoring signals, RRF, diversity, edge expansion, reranker
│   ├── assembler/     # Token-aware context assembly and formatting
│   ├── hyde/          # Hypothetical document expansion
│   ├── mod.rs         # Search pipeline orchestrator
│   ├── hybrid.rs      # Hybrid BM25 + vector scoring loop
│   └── postprocess.rs # Final enforcement, vector promotion
├── graph/             # PageRank, call hierarchy, type graphs
├── handlers/          # MCP tool handlers
├── server/            # MCP protocol routing (embedded + standalone)
│   ├── mod.rs         # Shared tool dispatch, embedded handler
│   └── standalone.rs  # Standalone HTTP handler with session routing
├── tools/             # Tool definitions (23 MCP tools)
├── embeddings/        # Jina Code embedding model wrapper
├── llm/               # On-device LLM (Qwen2.5-Coder-1.5B via llama.cpp)
├── reranker/          # Cross-encoder ORT implementation
├── path/              # Cross-platform path normalization (camino)
├── text.rs            # Text processing (synonym expansion, morphological variants)
├── metrics/           # Prometheus metrics
├── config.rs          # Configuration (embedded + standalone)
├── session.rs         # Multi-repo session management (standalone)
└── registry.rs        # Repo registry with path hashing (standalone)
```

## License

MIT

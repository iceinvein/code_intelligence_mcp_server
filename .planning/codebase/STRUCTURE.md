# Codebase Structure

**Analysis Date:** 2026-01-22

## Directory Layout

```
code-intelligence-mcp-server/
├── src/                           # Rust source code
│   ├── main.rs                    # Server entry point, initialization
│   ├── lib.rs                     # Module declarations
│   ├── cli.rs                     # CLI argument parsing
│   ├── config.rs                  # Config loading from environment
│   ├── text.rs                    # Text normalization utilities
│   │
│   ├── indexer/                   # Indexing pipeline
│   │   ├── mod.rs                 # IndexPipeline orchestration
│   │   ├── parser.rs              # Tree-Sitter language detection
│   │   ├── extract/               # Language-specific symbol extractors
│   │   │   ├── mod.rs             # Dispatcher
│   │   │   ├── symbol.rs          # Common symbol types
│   │   │   ├── rust.rs            # Rust symbol extraction
│   │   │   ├── typescript.rs       # TypeScript symbol extraction
│   │   │   ├── javascript.rs       # JavaScript symbol extraction
│   │   │   ├── python.rs           # Python symbol extraction
│   │   │   ├── go.rs              # Go symbol extraction
│   │   │   ├── java.rs            # Java symbol extraction
│   │   │   ├── c.rs               # C symbol extraction
│   │   │   └── cpp.rs             # C++ symbol extraction
│   │   │
│   │   └── pipeline/              # Indexing stages
│   │       ├── mod.rs             # Main pipeline logic
│   │       ├── scan.rs            # File discovery and filtering
│   │       ├── parsing.rs         # Tree-Sitter parsing stage
│   │       ├── edges.rs           # Call/reference edge extraction
│   │       ├── usage.rs           # Usage example extraction
│   │       ├── stats.rs           # Index run statistics
│   │       └── utils.rs           # Fingerprinting, hashing utilities
│   │
│   ├── retrieval/                 # Search and result assembly
│   │   ├── mod.rs                 # Retriever main orchestration
│   │   ├── query.rs               # Query normalization, intent detection
│   │   ├── cache.rs               # LRU cache for retriever
│   │   ├── ranking/               # Result ranking and scoring
│   │   │   ├── mod.rs             # Ranking module exports
│   │   │   ├── score.rs           # Scoring signals and weights
│   │   │   ├── diversify.rs       # Cluster/kind diversification
│   │   │   └── expansion.rs       # Graph-based result expansion
│   │   │
│   │   └── assembler/             # Context assembly for LLM
│   │       ├── mod.rs             # ContextAssembler main
│   │       ├── formatting.rs       # Code simplification, formatting
│   │       └── graph.rs            # Graph-based context expansion
│   │
│   ├── storage/                   # Multi-backend persistence
│   │   ├── mod.rs                 # Storage layer exports
│   │   ├── sqlite/                # SQLite (metadata, edges)
│   │   │   ├── mod.rs             # SqliteStore API
│   │   │   ├── operations.rs       # Store connection management
│   │   │   ├── schema.rs           # Data row types (SymbolRow, EdgeRow, etc.)
│   │   │   └── queries/            # Query implementations by domain
│   │   │       ├── mod.rs
│   │   │       ├── symbols.rs      # Symbol CRUD
│   │   │       ├── edges.rs        # Edge CRUD and traversal
│   │   │       ├── files.rs        # File fingerprint tracking
│   │   │       ├── stats.rs        # Index/search run telemetry
│   │   │       └── misc.rs         # Misc queries
│   │   │
│   │   ├── tantivy.rs             # Full-text search (BM25)
│   │   └── vector.rs              # Vector embeddings (LanceDB)
│   │
│   ├── graph/                     # Graph traversal
│   │   └── mod.rs                 # Call hierarchy, type graphs, dep graphs
│   │
│   ├── embeddings/                # Embedding generation
│   │   ├── mod.rs                 # Embedder trait definition
│   │   ├── fastembed.rs           # FastEmbed implementation (real)
│   │   └── hash.rs                # Deterministic hash embedding (testing)
│   │
│   ├── handlers/                  # MCP tool implementations
│   │   ├── mod.rs                 # Handler functions for all tools
│   │   └── state.rs               # AppState (shared across handlers)
│   │
│   ├── server/                    # MCP protocol
│   │   └── mod.rs                 # MCP server handler implementation
│   │
│   ├── tools/                     # Tool definitions
│   │   └── mod.rs                 # 11 MCP tool definitions
│   │
│   ├── bin/                        # Binary entry points
│   │
│   └── web_ui.rs                  # Optional web UI (feature-gated)
│
├── tests/                         # Integration tests
│   └── integration_index_search.rs # Full indexing + retrieval tests
│
├── test_workspace/                # Dummy workspace for testing
│   └── src/                       # Sample code files
│
├── scripts/                       # Build & run scripts
│   ├── start_mcp.sh               # Start server (stdio transport)
│   └── test_local.sh              # Run integration tests
│
├── Cargo.toml                     # Manifest (Rust 2021, version 0.2.4)
├── Cargo.lock                     # Dependency lock
├── README.md                      # Project overview
├── SYSTEM_ARCHITECTURE.md         # Higher-level architecture docs
├── TESTING.md                     # Testing guide
├── CLAUDE.md                      # Development instructions
│
└── .cimcp/                        # Runtime data (generated)
    ├── code-intelligence.db       # SQLite database
    ├── tantivy-index/             # Tantivy full-text index
    ├── vectors/                   # LanceDB vector store
    └── embeddings-cache/          # FastEmbed model cache
```

## Directory Purposes

**src/**: Rust source code root
- 56 Rust files across 13 modules
- Entry point: `src/main.rs`
- Library: exported by `src/lib.rs`

**src/indexer/**: File discovery, parsing, and symbol extraction
- Stages: scan → parse → extract symbols → extract edges → embed → persist
- Language support: 9 languages via Tree-Sitter
- Output: SymbolRow, EdgeRow records to storage

**src/indexer/extract/**: Language-specific AST traversal
- Each file (rust.rs, typescript.rs, etc.) exports `extract_X_symbols(code) -> Vec<SymbolRow>`
- All follow same interface pattern
- Symbol types: function, class, interface, type alias, constant, struct, enum, trait, impl

**src/indexer/pipeline/**: Orchestration of indexing stages
- `scan.rs`: File discovery, filtering, fingerprint comparison
- `parsing.rs`: Tree-Sitter CST generation
- `edges.rs`: Edge extraction, name resolution, evidence tracking
- `usage.rs`: Extract usage examples for documentation
- `stats.rs`: Track index run metrics
- `mod.rs`: IndexPipeline state machine (index_all, index_paths, watch_loop)

**src/retrieval/**: Search and result ranking
- Query normalization and intent detection
- Hybrid search (Tantivy + LanceDB)
- Multi-signal ranking (keyword, vector, structural, intent, popularity)
- Context assembly with byte budgeting

**src/retrieval/ranking/**: Scoring and diversification
- `score.rs`: Base score, intent multipliers, popularity boost, structural adjustments
- `diversify.rs`: Remove duplicates, ensure kind variety
- `expansion.rs`: Graph traversal for context enrichment

**src/retrieval/assembler/**: Context formatting for LLM
- Full code display for root symbols
- Simplified code for context symbols
- Byte budget management (70% roots, 30% context)

**src/storage/**: Abstraction over three storage backends
- **SQLite**: Symbols (definitions), edges (relationships), file fingerprints, telemetry
- **Tantivy**: BM25 full-text search with n-gram tokenization
- **LanceDB**: Vector embeddings with similarity search

**src/storage/sqlite/**: Metadata persistence
- `schema.rs`: SymbolRow, EdgeRow, EdgeEvidenceRow, FileFingerprintRow, IndexRunRow, SearchRunRow, SimilarityClusterRow
- `queries/`: Grouped by domain (symbols.rs, edges.rs, files.rs, stats.rs, misc.rs)
- Pattern: Each query function takes rusqlite::Connection and returns Vec<T> or scalar

**src/graph/**: Call hierarchy and dependency traversal
- Build JSON graph structures
- Bidirectional traversal (upstream/downstream)
- Evidence tracking (file/line locations)

**src/embeddings/**: Pluggable embedding backends
- `Embedder` trait: async batch API, configurable dimensions
- `FastEmbedder`: Real embeddings via fastembed (BGE-base-en-v1.5 ONNX)
- `HashEmbedder`: Deterministic hash-based (for testing without model download)

**src/handlers/**: Tool business logic
- One function per tool (handle_search_code, handle_refresh_index, etc.)
- Access AppState (config, indexer, retriever)
- Convert anyhow::Error to serde_json::Value for response

**src/server/**: MCP protocol implementation
- `CodeIntelligenceHandler`: Implements ServerHandler trait
- `handle_list_tools_request`: Return tool schemas
- `handle_call_tool_request`: Dispatch by name, error handling

**src/tools/**: Tool definitions using rust-mcp-sdk macros
- 11 tools with descriptions and schemas
- Tools: search_code, refresh_index, get_definition, find_references, get_file_symbols, get_call_hierarchy, explore_dependency_graph, get_type_graph, get_usage_examples, get_index_stats, hydrate_symbols

**tests/**: Integration tests
- `integration_index_search.rs`: Full pipeline tests (index + search)
- Run with `cargo test`
- Can use `EMBEDDINGS_BACKEND=hash` for faster testing

**.cimcp/**: Runtime-generated index data
- Created automatically by `.cimcp/` path default in config
- Contains SQLite database, Tantivy index, LanceDB vectors, embedding model cache
- Location: `{BASE_DIR}/.cimcp/`

## Key File Locations

**Entry Points:**
- `src/main.rs`: Server startup, config loading, component initialization
- `src/lib.rs`: Module declarations for library consumers
- `src/server/mod.rs`: MCP protocol handler, tool routing

**Configuration:**
- `src/config.rs`: Environment variable parsing, defaults
- `Cargo.toml`: Dependencies, feature flags (web-ui)
- `.env`: Optional environment override (not tracked)

**Core Logic:**
- `src/indexer/mod.rs`: Indexing state machine
- `src/indexer/pipeline/mod.rs`: Pipeline stages
- `src/retrieval/mod.rs`: Search orchestration
- `src/handlers/mod.rs`: Tool implementations

**Storage Access:**
- `src/storage/sqlite/mod.rs`: SqliteStore API
- `src/storage/tantivy.rs`: TantivyIndex wrapper
- `src/storage/vector.rs`: LanceVectorTable wrapper

**Data Models:**
- `src/storage/sqlite/schema.rs`: All SymbolRow, EdgeRow, etc. types
- `src/retrieval/mod.rs`: RankedHit, SearchResponse, HitSignals

**Testing:**
- `tests/integration_index_search.rs`: Full integration tests
- `test_workspace/`: Sample code for testing

## Naming Conventions

**Files:**
- Module files: `{module_name}.rs` (mod.rs for directory roots)
- Convention: snake_case file names matching module names

**Functions:**
- Pattern: `extract_{lang}_symbols()`, `handle_{tool_name}()`, `list_{entity_type}()`, `upsert_{entity}()`
- Convention: snake_case, verb-first for actions, descriptive of domain

**Variables:**
- Mutable state: `let mut store`, `let mut seen`, `let mut nodes`
- Ownership: Clear move semantics, borrowing explicit
- Convention: snake_case, clear intent from name

**Types:**
- Rows: `{Entity}Row` (SymbolRow, EdgeRow, FileFingerprintRow)
- Containers: `Vec<T>`, `HashMap<K,V>`, `HashSet<T>`
- Enums: PascalCase variants (LanguageId::Typescript, Intent::Definition)
- Convention: PascalCase for types, descriptive purpose

**Constants:**
- Feature flags: UPPERCASE (web-ui feature)
- Config defaults: inline with descriptive comments

## Where to Add New Code

**New Language Support:**
1. Add tree-sitter dependency to `Cargo.toml`: `tree-sitter-{lang} = "0.X"`
2. Create `src/indexer/extract/{lang}.rs` with `pub fn extract_{lang}_symbols(code: &str) -> Vec<SymbolRow>`
3. Register in `src/indexer/extract/mod.rs` dispatcher module
4. Update `src/indexer/parser.rs`: Add `LanguageId::{Lang}` variant, add to `language_id_for_path()`
5. Add test to `src/indexer/parser.rs::creates_parsers_for_languages()`

**New MCP Tool:**
1. Define tool in `src/tools/mod.rs` with `#[macros::mcp_tool]` macro and JsonSchema derive
2. Create handler in `src/handlers/mod.rs` named `pub async fn handle_{tool_name}()`
3. Add dispatch case in `src/server/mod.rs::handle_call_tool_request()` match
4. Add tool to `src/server/mod.rs::handle_list_tools_request()` tools vector

**New Ranking Signal:**
- Add function to `src/retrieval/ranking/score.rs`
- Integrate into `rank_hits_with_signals()` calculation
- Add to `HitSignals` struct if user-visible

**New Storage Query:**
1. Create function in `src/storage/sqlite/queries/{domain}.rs`
2. Add wrapper method to `src/storage/sqlite/mod.rs` impl SqliteStore
3. Document parameter types and return types

**New Graph Analysis:**
- Implement in `src/graph/mod.rs`
- Return serde_json::Value for flexibility
- Call from handler function

**Testing Code:**
- Fixtures: `test_workspace/src/` (sample code)
- Integration tests: `tests/integration_index_search.rs`
- Unit tests: Same module with `#[cfg(test)] mod tests { ... }`

## Special Directories

**target/**: Build output (git-ignored)
- Generated by `cargo build`
- Binaries in `target/debug/` or `target/release/`

**.cimcp/**: Runtime index data (git-ignored)
- Created automatically on first run
- Contains: SQLite DB, search indexes, embeddings
- Location: `{BASE_DIR}/.cimcp/` by default
- Clean with: `rm -rf .cimcp/`

**test_workspace/**: Static test fixtures
- Committed to repo
- Used by `./scripts/test_local.sh`
- Contains sample source files for indexing tests

**.planning/codebase/**: GSD planning documents
- ARCHITECTURE.md: This architecture doc
- STRUCTURE.md: This structure doc
- Generated by `/gsd:map-codebase` command
- Used by `/gsd:plan-phase` and `/gsd:execute-phase`

---

*Structure analysis: 2026-01-22*

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Code Intelligence MCP Server is a Rust-based local code indexing and semantic search engine that provides structure-aware code navigation for LLM agents. It implements the Model Context Protocol (MCP) and integrates with tools like OpenCode, Trae, and Cursor.

**Platform:** macOS only (Apple Silicon with Metal GPU acceleration).

**Core technologies:** Rust 2021, Tree-Sitter (parsing), SQLite (metadata), Tantivy (full-text search), LanceDB (vector embeddings), llama.cpp (Metal GPU inference for both embeddings and LLM descriptions).

## Glossary

Key acronyms and concepts used throughout the codebase:

| Term | Full Name | Description |
|------|-----------|-------------|
| **MCP** | Model Context Protocol | Open protocol for connecting LLM agents to external tools and data sources. This server implements MCP to expose code search/navigation tools. |
| **BM25** | Best Matching 25 | Probabilistic text retrieval algorithm used by Tantivy. Ranks documents by term frequency (TF) and inverse document frequency (IDF) — how often a term appears in a document vs. how rare it is across the corpus. |
| **IDF** | Inverse Document Frequency | A BM25 component measuring term rarity. High IDF = rare term = more discriminating. Low IDF = common term (e.g., "error") = less useful for ranking. IDF dilution is a recurring concern when adding synonyms or descriptions. |
| **RRF** | Reciprocal Rank Fusion | Technique for combining ranked lists from different search systems (BM25 keyword search + vector semantic search). Merges by reciprocal rank position rather than raw scores, making it robust across different scoring scales. |
| **GGUF** | GGML Unified Format | Binary format for quantized LLM model weights. Used by llama.cpp for the embedding model (jina-code-embeddings-1.5b, Q8_0), the description LLM (Qwen2.5-Coder-1.5B-Instruct, Q4_K_M), and the cross-encoder reranker (bge-reranker-v2-m3, Q8_0). |
| **LLM** | Large Language Model | Used on-device (Qwen2.5-Coder-1.5B via llama.cpp) to generate natural-language descriptions for each indexed symbol, enriching BM25 search with human-readable terms. |

## Usage in Claude Code

Since v4.0 the server runs as a single HTTP daemon managed by launchd. Embedded stdio mode and leader election are gone.

### One-time install

```bash
npx -y @iceinvein/code-intelligence-mcp install   # writes plist + bootstraps daemon
npx -y @iceinvein/code-intelligence-mcp migrate   # rewrites v3 stdio entries in ~/.claude.json
```

The `install` subcommand:
- Writes the launchd plist to `~/Library/LaunchAgents/com.iceinvein.code-intelligence.plist`.
- Bootstraps the service via `launchctl bootstrap gui/<uid>` (macOS 13+).
- Prompts before patching `~/.claude.json`; pass `--patch-claude-json` or `--no-patch-claude-json` to skip the prompt.
- Default port: 17800 (configurable via `--port`).

After install, MCP clients connect to `http://127.0.0.1:17800/mcp`. The first launch downloads one GGUF model by default (~1.5 GB embedding model). The other two are off by default and download only when opted into: the description LLM (~1.0 GB) when `DESCRIPTIONS_ENABLED=1` (or `ASK_CODE_LLM_SYNTHESIS`), and the cross-encoder reranker (~600 MB) when `RERANKER_ENABLED=1`.

### Session binding

v4 tries four binding sources in order; first match wins.

1. **`?repo=/abs/path` URL query** — primary, works on every MCP client. The proxy in front of the SDK captures the query, pairs it with the `mcp-session-id` assigned by the upstream response, and stashes the pair in `PendingRepos` so `StandaloneHandler::resolve_state` can promote it on the first tool call.
2. **MCP `roots/list`** — Claude Code negotiates this automatically. Opportunistic if no URL was provided.
3. **`bind_workspace(repo="/abs/path")` tool** — manual escape hatch for clients that can't set query strings.
4. **Single-repo registry fallback** — auto-binds when the registry has exactly one repo. Disables itself the moment a second repo is added.

Unbound tool calls return an actionable JSON-RPC error pointing at all four options.

### What Claude Code Gets

Once connected, Claude Code gains access to 18 advertised MCP tools (the full list is `all_tools()` in `src/server/mod.rs`; a further 7 operational tools remain dispatchable but unadvertised):

**Core retrieval**
- **`ask_code`** - Single-call entry point for any code question. Runs `investigate` server-side and returns verified `evidence[]` (symbol name, file path, line range, code body) plus a shape classification. The agent synthesises the user-facing answer from that evidence; the server does NOT generate prose by default (see `ASK_CODE_LLM_SYNTHESIS` below).
- **`investigate`** - Composite multi-hop retrieval. Use directly when you want raw evidence without going through `ask_code`'s caching layer.
- **`search_code`** - Primary semantic + keyword hybrid search (e.g., "how does auth work?" or "class User"). Returns ranked hits with symbol IDs only, no bodies.
- **`hydrate_symbols`** - Fetch source bodies for symbol IDs returned by any other tool, instead of falling back to Read/grep.

**Navigation**
- **`get_definition`** / **`find_references`** - Jump to definitions and find all usages
- **`get_call_hierarchy`** / **`get_type_graph`** - Navigate call chains and type hierarchies
- **`explore_dependency_graph`** - Trace module-level imports/exports
- **`trace_data_flow`** - Follow variable reads/writes through the code
- **`find_affected_code`** - Impact analysis before refactoring

**Overview and tests**
- **`summarize_file`** - Symbol-level summary of one file: defined symbols, kinds, brief descriptions
- **`get_module_summary`** - Public API surface of a module or directory
- **`find_tests_for_symbol`** - Tests linked to a symbol or file (path-pattern test-file links plus call-graph test callers)

**Lifecycle / admin**
- **`get_index_stats`** / **`refresh_index`** - Monitor and trigger re-indexing
- **`bind_workspace`** - Bind the session to a repo by absolute path (for clients without roots or query-string support)
- **`approve_indexing`** - Approve or decline a repository's first full index. Every never-indexed repository returns `consent_required`, including explicit `?repo=` and `bind_workspace` selections. Tell the user in chat that indexing uses local compute, memory, and disk, wait for explicit approval, then call `approve_indexing`. Approval starts a background `InitialBind` job immediately. Later watcher updates and manual reindexes do not ask again. A worktree of an already-indexed repository is the exception: it seeds its index from the base repo (a SQLite `VACUUM INTO` snapshot plus APFS copy-on-write clones of `tantivy-index/` and `vectors/`, then a delta pass) and starts indexing without a prompt, because the cost is seconds rather than a full GPU pass.

### Dashboard and JSON API

Open `http://127.0.0.1:17802/` to see repos (expandable for per-repo stats), MCP sessions (connected + bound counts), background jobs (running + finished, 15-minute retention), and a live log stream. The same data is available via JSON at `/api/repos`, `/api/repos/:id`, `/api/sessions`, `/api/jobs`, `/api/status`. Reindex with `POST /api/repos/:id/reindex`; drop a repo entirely with `DELETE /api/repos/:id` (removes the registry entry and the on-disk data directory).

## Build & Run Commands

```bash
# Build
cargo build                    # Debug build
cargo build --release          # Release build

# Run tests (fast path: no llama.cpp compile, no CMake, no model download)
EMBEDDINGS_BACKEND=hash cargo test --no-default-features
EMBEDDINGS_BACKEND=hash cargo test --no-default-features <filter>   # single test / module
cargo test                                      # full build (compiles llama.cpp, needs CMake)
./scripts/test_local.sh                         # End-to-end test with dummy workspace

# Lint gates (CI "Rust gates (macOS)" check; required for release tags)
cargo fmt --all -- --check
EMBEDDINGS_BACKEND=hash cargo clippy --all-targets --no-default-features -- -D warnings

# Engine-only retrieval quality gate (recall@5, MRR, nDCG@5, graph precision on a temp polyglot index)
EMBEDDINGS_BACKEND=hash cargo test --no-default-features \
  --test deterministic_quality deterministic_engine_quality_gate

# Run the server in the foreground (for benchmarks / dev)
./target/release/code-intelligence-mcp-server                       # default port 17800
./target/release/code-intelligence-mcp-server --port 18000          # custom port

# Lifecycle subcommands (production install)
./target/release/code-intelligence-mcp-server install               # write plist + bootstrap
./target/release/code-intelligence-mcp-server status                # PID, port, plist state
./target/release/code-intelligence-mcp-server stop
./target/release/code-intelligence-mcp-server uninstall             # bootout + remove plist
./target/release/code-intelligence-mcp-server migrate [--dry-run]   # rewrite stdio configs
```

`TESTING.md` covers the full gate list and toolchain prerequisites (pinned protoc, CMake for Metal builds).

## Benchmark Harness

`bench/` is a Python LLM-judge harness that gates releases: it runs agent arms (with and without this MCP server) against fixture repos, scores answers mechanically (citation verification against pinned SHAs, forbidden terms) and via a tiered judge panel, then renders a markdown report. Rounds are numbered `R<NNN>`; records live in `bench/results/`. Fixture repos: `workings` (formerly wolfmax) and `django`.

```bash
make -C bench install                     # PyYAML + pytest
make -C bench test                        # harness unit tests
python3 -m bench.run full --arms default,code_intel_shipped --repos workings,django --repeats 3
python3 -m bench.run full --question-set iteration   # 16-question subset for cheap A/Bs
python3 -m bench.run report R041
python3 -m bench.rescore R041             # zero-token re-score after scoring-logic changes
```

Runs are crash-safe and resumable (`full --round <N>` skips completed runs); index caches key on the daemon binary hash, so rebuild after Rust changes. The full 40-question fixtures are the release gate; the iteration set is for A/Bs only. Details and scoring history in `bench/README.md`.

## Releasing

`./scripts/release.sh <patch|minor|major|x.y.z>` bumps the Cargo and npm versions, generates release notes, commits, and tags. Pushing the tag triggers the GitHub "Release" workflow (gated on the "Rust gates (macOS)" check). After CI finishes, `./scripts/release-post-ci.sh` downloads the tarball, pins its sha256 into the Homebrew formula, pushes the tap, and attaches `RELEASE_NOTES.md` to the GitHub release. npm publish steps are in `PUBLISHING.md`.

## Daemon architecture

The server runs as a single HTTP daemon. Three ports:

- **`mcp_port` (default 17800)** — public-facing MCP proxy (axum). Reads `?repo=` URL bindings, forwards to the internal SDK listener, captures `mcp-session-id` from the response.
- **`mcp_port + 1`** — discovery endpoint at `/.well-known/mcp`.
- **`mcp_port + 2`** — JSON API and embedded dashboard (repo CRUD, sessions, jobs, SSE log stream).
- **`mcp_port + 100`** — internal-only rust-mcp-sdk Streamable HTTP transport, bound to 127.0.0.1.

All public ports bind 127.0.0.1 and reject non-localhost `Origin` headers (DNS-rebinding defence). The embedding model (~1.5 GB) loads once and stays resident for queries, shared across sessions. Two optional models are off by default: the description LLM (~1.0 GB), which runs an index-time backfill worker only when `DESCRIPTIONS_ENABLED=1` and is freed afterwards; and the cross-encoder reranker (~600 MB), which when `RERANKER_ENABLED=1` loads in the background (shared across repos) and stays resident, applying a query-time reorder on top of the RRF-fused results. Both default off: neither moved the benchmark judge score and each adds setup cost (descriptions a multi-hour index backfill, the reranker GPU residency). Per-repo indexes live under `~/.code-intelligence/repos/<hash>/`.

Binding a git worktree of an indexed repo clones that repo's index into the worktree's own directory rather than reindexing from scratch, then runs one pass that reparses only files whose content actually differs. The two indexes are independent from that point on. Seeded worktree indexes are pruned once their checkout has been absent for two consecutive idle sweeps (roughly two minutes), as long as no index job is running against them; a repo registered by hand is never auto-pruned this way.

Configure MCP clients to connect:
```json
{
  "mcpServers": {
    "code-intelligence": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:17800/mcp?repo=/abs/path/to/your/repo"
    }
  }
}
```

Drop `?repo=...` only when running Claude Code (which negotiates roots) or when the registry has exactly one repo.

Data stored in `~/.code-intelligence/` (repos, models, config).
Optional config: `~/.code-intelligence/server.toml`.

## Architecture

### Data Flow

1. **Indexing Pipeline** (`src/indexer/`): File scanning → Tree-Sitter parsing → Symbol extraction → Embedding generation → Multi-modal storage
2. **Retrieval Engine** (`src/retrieval/`): Query normalization → Hybrid search (Tantivy + LanceDB) → Intent detection → Signal-based ranking → Context assembly
3. **Graph Engine** (`src/graph/`): Call hierarchy, type graphs, and dependency graph traversal

### Key Directories

- `src/indexer/extract/` - Tree-Sitter symbol extractors: Rust, TypeScript/JavaScript, Python, Go, Java, Kotlin, C#, Swift, C, C++, Ruby, plus route/endpoint extractors for web frameworks (Django, FastAPI, Express, Next.js, NestJS, Spring, Axum, Actix, Hono, tRPC, and more)
- `src/external_index/` - External-index import path: discovers per-language producers, imports their symbol/edge artifacts (`EXTERNAL_INDEX_*` env vars)
- `producers/` - Out-of-process index producers for 11 languages (`producers/manifest.json`)
- `src/storage/` - SQLite, Tantivy, and LanceDB storage layers
- `src/retrieval/ranking/` - Scoring signals and ranking logic
- `src/handlers/` - MCP tool implementations
- `src/server/` - MCP protocol handler routing
- `bench/` - LLM-judge benchmark harness (see Benchmark Harness above)

Deeper design docs: `SYSTEM_ARCHITECTURE.md`, `DESIGN.md`, `docs/MIGRATION-v3-to-v4.md`.

### Storage Layers

- **SQLite** (`storage/sqlite/`): Symbols, edges, file metadata, index/search telemetry, LLM descriptions
- **Tantivy** (`storage/tantivy.rs`): Full-text search using BM25 ranking with n-gram tokenization. Indexes symbol names, code text (comments stripped), morphological variants, and LLM-generated descriptions.
- **LanceDB** (`storage/vector.rs`): Vector embeddings (1536-dim Matryoshka, jina-code-embeddings-1.5b Q8_0 via llama.cpp + Metal) for semantic similarity search. Combined with Tantivy results via RRF, then re-scored by the bge-reranker-v2-m3 cross-encoder.

### Runtime Data Location

All data stored under `~/.code-intelligence/`:
- `repos/<hash>/code-intelligence.db` (SQLite, per-repo)
- `repos/<hash>/vectors/` (LanceDB, per-repo)
- `repos/<hash>/tantivy-index/` (per-repo)
- `repos/registry.json` (shared repo registry)
- `models/jina-code-embeddings-1.5b-gguf/` (shared embedding model, ~1.5 GB Q8_0, GGUF via llama.cpp)
- `models/qwen2.5-coder-1.5b-gguf/` (shared description LLM, ~1.0 GB Q4_K_M, GGUF via llama.cpp)
- `models/bge-reranker-v2-m3-gguf/` (shared cross-encoder reranker, ~600 MB Q8_0, GGUF via llama.cpp)
- `logs/` (shared log files)

The `<hash>` is the first 16 characters of `SHA256(repo_path)` where `repo_path` is the canonical absolute path the session bound to (via `?repo=`, roots, or `bind_workspace`).

## Configuration

The server reads configuration from environment variables. Key ones below; the full set (including `EXTERNAL_INDEX_*` producer knobs, `HYDE_*`, metrics, and ranking weights) lives in `src/config.rs`.

| Variable | Default | Description |
|----------|---------|-------------|
| `BASE_DIR` | — | Legacy v3 single-repo override. v4 derives the repo per-session from `?repo=`, roots, or `bind_workspace`, so this is only honored when launching the binary directly (not via launchd) without any session binding. |
| `EMBEDDINGS_BACKEND` | `llamacpp` | `llamacpp` (default) or `hash` (fast testing) |
| `EMBEDDINGS_DEVICE` | `metal` | `metal` (Metal GPU) or `cpu` |
| `WATCH_MODE` | `true` | Auto-reindex on file changes |
| `INDEX_PATTERNS` | 14 globs: ts, tsx, js, jsx, rs, py, go, java, c, h, cpp, cc, cxx, hpp | Comma-separated glob patterns to index (defaults in `src/config.rs`) |
| `HYBRID_ALPHA` | `0.7` | Vector vs keyword weight (0-1) |
| `MAX_CONTEXT_BYTES` | `200000` | Context window size limit |
| `RERANKER_ENABLED` | `false` | Load the bge-reranker-v2-m3 cross-encoder (~600 MB) and apply a query-time reorder on top of RRF results. Off by default (unproven quality benefit, GPU-resident). Loads in the background when enabled. |
| `DESCRIPTIONS_ENABLED` | `false` | Spawn the index-time LLM description worker (Qwen2.5-Coder-1.5B) that backfills natural-language descriptions into the Tantivy index. Off by default: a multi-hour index-time backfill with no proven judge benefit (R005/R006). |
| `INDEX_CONSENT_REQUIRED` | `true` | Ask once before the first full index for every binding source, including `?repo=` and `bind_workspace`. Approval starts a background `InitialBind` job immediately. `false` skips the prompt for CI and benchmarks but still starts the first index immediately. |
| `LEARNING_ENABLED` | `true` | Enable selection/affinity learning |
| `LEARNING_SELECTION_BOOST` | `0.1` | Max boost from user selection history |
| `LEARNING_FILE_AFFINITY_BOOST` | `0.05` | Max boost from file access frequency |
| `ASK_CODE_LLM_SYNTHESIS` | unset (false) | Opt back into local-LLM prose synthesis in `ask_code`. Default behaviour returns verified evidence and leaves synthesis to the calling agent (see `feat(ask_code)` in v3.3). Accepts `1`, `true`, `yes`, `on`. |
| `ANSWER_LLM_N_CTX` | `32768` | llama.cpp context size for the `ask_code` answer LLM when synthesis is enabled. Sized to fit a full evidence-bearing prompt plus the generated answer. Minimum `512`. |
| `LLM_HF_REPO` | `Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF` | Override the HuggingFace repository for the local LLM (e.g. `Qwen/Qwen2.5-Coder-3B-Instruct-GGUF` for the 3B variant). |
| `LLM_HF_MODEL_FILE` | `qwen2.5-coder-1.5b-instruct-q4_k_m.gguf` | Override the GGUF filename within the configured repo. |

The web portal Settings editor (`/settings`) reads and writes these knobs to
`~/.code-intelligence/server.toml`. Tier 2 retrieval tuning lives in new sections
`[retrieval]`, `[ranking]`, `[rrf]`, `[learning]`, and `[indexing] consent_required`;
these were previously hardcoded in `repo_config()` and the matching `*_WEIGHT` /
`HYBRID_ALPHA` env vars only ever affected the legacy `BASE_DIR` path, not the v4
daemon. Settings are daemon-global and apply on restart.

## Path Handling

**Standard:** Use camino for UTF-8 typed paths, centralized normalization.

This project uses `camino` for guaranteed UTF-8 paths at the type level. All file paths should use `Utf8PathBuf` (owned) or `&Utf8Path` (borrowed) instead of the standard library's `PathBuf` and `&Path`.

```rust
use crate::path::{PathNormalizer, Utf8Path, Utf8PathBuf, PathError};

// Create normalizer with base directory
let normalizer = PathNormalizer::new(base_dir);

// Normalize path for cross-platform comparison
let normalized = normalizer.normalize_for_compare(path)?;

// Convert to relative path within base
let relative = normalizer.relative_to_base(absolute_path)?;

// Security check against path escaping
normalizer.validate_within_base(user_input)?;
```

### Key Types

| Type | Use Case | Replaces |
|------|----------|----------|
| `Utf8PathBuf` | Owned UTF-8 path | `PathBuf` |
| `&Utf8Path` | Borrowed UTF-8 path | `&Path` |
| `PathNormalizer` | Centralized path operations | Manual path manipulation |
| `PathError` | Structured path errors | ad-hoc error handling |

### Migration Pattern

```rust
// OLD (don't use - scattered, error-prone):
let path = path.replace("\\", "/");
let relative = path.strip_prefix("/repo")?;

// NEW (use - centralized, tested):
let normalized = normalizer.normalize_for_compare(Utf8Path::new(path))?;
let relative = normalizer.relative_to_base(&normalized)?;
```

### Error Handling

```rust
use crate::path::PathError;

// PathError provides helpful error messages with context
match normalizer.relative_to_base(path) {
    Ok(rel) => /* use relative path */,
    Err(PathError::OutsideRepo { path, base }) => {
        anyhow::bail!("Path '{path}' is outside repository '{base}'")
    }
    Err(PathError::NonUtf8 { path }) => {
        anyhow::bail!("Path contains non-UTF-8 characters: {}", path.display())
    }
    Err(e) => return Err(e.into()),
}
```

### Platform

macOS-only (Apple Silicon). Paths are case-sensitive in code (APFS may be case-insensitive on disk).

The `src/path/mod.rs` module includes comprehensive parameterized tests using the `test-case` crate covering security validation, case sensitivity, and error message formatting.

## Adding a New Language

1. Add tree-sitter dependency to `Cargo.toml`
2. Create `src/indexer/extract/{lang}.rs` implementing symbol extraction
3. Register in `src/indexer/extract/mod.rs` dispatcher
4. Update `src/indexer/parser.rs` language detection

## Adding a New MCP Tool

1. Define tool with `#[macros::mcp_tool]` in `src/tools/mod.rs`
2. Implement handler in `src/handlers/mod.rs`
3. Add routing in `src/server/mod.rs`

## Ranking Signals

The retrieval pipeline uses a hybrid search approach: BM25 keyword search (via Tantivy) and vector semantic search (via LanceDB) are run in parallel, then merged using RRF (Reciprocal Rank Fusion) to produce a combined ranking. On top of this, the scoring system in `src/retrieval/ranking/score.rs` applies structural signals:

- Test file penalty (0.5x unless Intent::Test)
- Glue code filtering (index.ts barrel files deprioritized)
- Directory semantics (src/ boosted, dist/ penalized)
- Export status boost (exported/public symbols represent the primary API surface)
- Intent multipliers (Definition 1.5x, Schema 50-75x) — detected from query patterns
- Popularity boost by incoming edge count (PageRank-style graph signal)
- Score-gap detection — drops trailing results with a >2.5x score drop from the previous result
- Sub-query coverage — ensures multi-term queries have results matching each sub-query

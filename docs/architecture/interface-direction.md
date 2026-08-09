# Interface Direction: Engine First, CLI Primary, MCP Adapter

## Decision

Code Intelligence is a local code intelligence engine, not primarily an MCP server.

The durable architecture is:

```text
index engine + per-repo daemon
  -> stable JSON/HTTP API
  -> first-class CLI for humans, agents, hooks, and CI
  -> MCP adapter for client-native tool discovery and compatibility
```

MCP remains supported, but it should not be the only product boundary. The daemon owns indexing, retrieval, graph traversal, context assembly, background jobs, and local model lifecycle. The CLI and MCP surfaces should both call the same internal operations and return the same structured contracts.

## Why

Recent benchmark rounds showed that adding more MCP tool descriptions and stronger "use this tool" wording has reached diminishing returns. Agent behavior still depends heavily on the client harness, model tool-selection policy, and how much tool schema/result text lands in context. A CLI gives the user and agent a direct, debuggable path to the same grounded evidence without relying on MCP tool selection.

The wider market points in the same direction:

- Sourcegraph Cody combines keyword search, code search, and code graph context rather than relying on a single integration surface.
- Cursor treats semantic search/indexing as core infrastructure and uses dynamic context discovery to avoid loading large tool descriptions or results up front.
- Aider gets value from a compact, graph-ranked repo map exposed directly to the model, without an MCP dependency.
- Serena shows MCP works well for semantic code operations, especially when tools stay symbol-level and cache their backing indexes.
- Anthropic and Cloudflare's Code Mode pattern shows that large direct MCP tool surfaces can become inefficient; code/CLI execution can compose operations and filter large results before they reach the model.

## Product Surfaces

### Daemon

The daemon remains the source of truth:

- owns repo registration and workspace binding;
- maintains SQLite, Tantivy, and LanceDB indexes;
- runs local embedding, reranking, and description models;
- performs query classification, graph expansion, evidence assembly, and job tracking;
- exposes loopback-only HTTP endpoints for structured operations.

The daemon should provide stable internal request/response shapes that are independent of MCP.

### CLI

The CLI should become the main programmable interface:

```bash
code-intel ask --repo . "how does auth work?" --json
code-intel search --repo . "FastAPI route auth" --context snippets --json
code-intel investigate --repo . --mode impact --target authenticate_request --json "what breaks if this changes?"
code-intel hydrate --repo . --ids sym_1,sym_2 --json
code-intel repo-map --repo . --budget 4000
code-intel capabilities --json
```

CLI goals:

- deterministic JSON for agents, shell scripts, CI, and hooks;
- readable text output for humans;
- exit codes that distinguish no results, user/config errors, daemon unavailable, and internal failures;
- local daemon auto-discovery, with clear fallback messages when it is not running;
- no requirement that the caller is an MCP client.

The CLI should be thin. It should not duplicate retrieval logic; it should call the daemon API or a shared handler layer.

### Shared Handler Boundary

The CLI/API and MCP surfaces share the existing handler layer rather than routing MCP through the CLI command implementation. This keeps MCP transport concerns separate from shell behavior such as exit codes, stdout formatting, and timeout flags, while preserving one retrieval implementation for `ask`, `search`, `investigate`, `hydrate`, and `repo-map`.

### MCP

MCP remains a compatibility adapter for Claude Code, Cursor, Trae, OpenCode, Codex, Continue, Windsurf, and similar clients.

The long-term MCP surface should be smaller and more composite:

- `ask_code`: grounded answer/evidence contract;
- `investigate`: multi-hop evidence bundle;
- `search_code`: discovery only;
- `hydrate_symbols`: source bodies for known IDs;
- `repo_map`: compact project map;
- `bind_workspace`, `refresh_index`, and `get_index_stats` for lifecycle tasks.

Specialist tools can remain internally available, but the model-facing surface should prefer composite operations that return complete evidence in one call. This reduces tool-chaining errors and avoids large intermediate responses flowing through the model.

## CLI Contract

All agent-facing CLI commands should support:

- `--repo <path>`: absolute or relative workspace root. Defaults to the current working directory.
- `--json`: machine-readable output. Stable enough for hooks and tests.
- `--pretty`: pretty-printed JSON for debugging.
- `--limit <n>` where a result set is returned.
- `--timeout <duration>` for daemon calls.
- `--no-start`: fail if the daemon is not running, instead of attempting to start it.

Human text output can evolve. JSON output is the compatibility contract.

### Common Envelope

```json
{
  "ok": true,
  "command": "investigate",
  "repo": {
    "path": "/abs/path",
    "id": "repo_hash"
  },
  "index": {
    "version_unix_s": 1770000000,
    "fresh": true
  },
  "warnings": [],
  "result": {}
}
```

On failure:

```json
{
  "ok": false,
  "command": "search",
  "error": {
    "code": "daemon_unavailable",
    "message": "Code Intelligence daemon is not running",
    "hint": "Run `code-intel start` or retry without --no-start"
  }
}
```

Exit codes:

| Code | Meaning |
|---:|---|
| 0 | Success |
| 1 | Internal error |
| 2 | Invalid arguments |
| 3 | Daemon unavailable |
| 4 | Workspace not found or not bindable |
| 5 | No results, when the command is explicitly result-seeking |
| 124 | Timeout |

### `search`

Purpose: fast discovery. It should return ranked hits, not an answer.

```json
{
  "query": "FastAPI route auth",
  "hits": [
    {
      "symbol_id": "sym_...",
      "name": "authenticate_request",
      "kind": "function",
      "file_path": "src/auth.py",
      "range": { "start_line": 12, "end_line": 47 },
      "score": 0.91,
      "reasons": ["keyword", "vector", "framework_pattern"],
      "snippet": "def authenticate_request(...):"
    }
  ]
}
```

### `investigate`

Purpose: one-shot multi-hop evidence retrieval for an agent or hook. It should not rely on the caller to run follow-up graph tools.

```json
{
  "question": "what breaks if authenticate_request changes?",
  "mode": "impact",
  "evidence": [
    {
      "symbol_id": "sym_...",
      "file_path": "src/auth.py",
      "start_line": 12,
      "end_line": 47,
      "body": "def authenticate_request(...): ...",
      "role": "primary"
    }
  ],
  "verified_locations": [
    {
      "file_path": "src/auth.py",
      "line": 12,
      "symbol_id": "sym_..."
    }
  ],
  "context_chain": [
    {
      "from": "authenticate_request",
      "to": "login_handler",
      "edge": "call"
    }
  ]
}
```

### `ask`

Purpose: grounded answer mode. This can synthesize prose, but the evidence must remain first-class so a parent agent can decide whether to trust or rewrite the prose.

```json
{
  "question": "how does auth work?",
  "answer": "Authentication is handled by ...",
  "confidence": "medium",
  "citations": [
    {
      "file_path": "src/auth.py",
      "line": 12,
      "symbol_id": "sym_..."
    }
  ],
  "evidence": []
}
```

### `hydrate`

Purpose: fetch source bodies for symbols the caller already selected from `search`, `ask`, or `investigate`. This keeps discovery responses small while still giving agents a deterministic way to load exact source context.

```json
{
  "ids": ["sym_...", "sym_..."],
  "mode": "full",
  "items": [
    {
      "symbol_id": "sym_...",
      "file_path": "src/auth.py",
      "start_line": 12,
      "end_line": 47,
      "body": "def authenticate_request(...): ..."
    }
  ],
  "missing_ids": []
}
```

### `repo-map`

Purpose: compact static context. It should fit a caller-specified token budget and help the model choose files or symbols for follow-up.

```json
{
  "budget_tokens": 4000,
  "used_tokens": 3720,
  "files": [
    {
      "file_path": "src/auth.py",
      "importance": 0.87,
      "symbols": [
        {
          "name": "authenticate_request",
          "kind": "function",
          "signature": "authenticate_request(req: Request) -> User"
        }
      ]
    }
  ]
}
```

## Implementation Order

1. Add CLI subcommands that call daemon/handler logic: `ask`, `investigate`, `search`, `hydrate`, `repo-map`.
2. Add the shared JSON envelope and tests for stable serialization.
3. Wire MCP tools to the same command/result layer where practical.
4. Revisit the public MCP surface after the CLI contract is stable.

## Non-Goals

- Do not remove MCP support.
- Do not fork separate retrieval logic for CLI and MCP.
- Do not make JSON output depend on terminal formatting.
- Do not require cloud services.
- Do not optimize for a single MCP client behavior.

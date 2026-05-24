# Agent Query CLI

The query CLI is the stable non-MCP surface for agents, hooks, and scripts. It calls the local daemon API on the loopback dashboard/API port (`mcp_port + 2`, default `17802`) and prints JSON envelopes.

## Commands

```bash
code-intelligence-mcp-server repo-map --repo . --budget 4000 --json
code-intelligence-mcp-server ask --repo . --json "how does auth work?"
code-intelligence-mcp-server search --repo . --context snippets --json "auth handler"
code-intelligence-mcp-server hydrate --repo . --ids sym_1,sym_2 --json
code-intelligence-mcp-server investigate --repo . --mode impact --target authenticate_request --json "what breaks if this changes?"
```

Use `--port` when the daemon is running on a non-default MCP port. Use `--timeout 2s` to bound daemon calls. Use `--no-start` in scripts that require the daemon to already be running.

## Recommended Flows

- Initial orientation: `repo-map --budget 4000 --json`
- Broad question: `ask --json "question"`
- Discovery: `search --context snippets --json "query"`, then `hydrate --ids ... --json`
- Impact/debugging: `investigate --mode impact --target SYMBOL --json "question"`

## JSON Envelopes

Success:

```json
{
  "ok": true,
  "command": "search",
  "repo": { "path": "/abs/path", "id": "repo_hash" },
  "index": { "version_unix_s": 1770000000, "fresh": true },
  "warnings": [],
  "result": {}
}
```

Failure:

```json
{
  "ok": false,
  "command": "search",
  "error": {
    "code": "daemon_unavailable",
    "message": "failed to reach Code Intelligence daemon",
    "hint": "Run `code-intelligence-mcp-server start` or `code-intelligence-mcp-server install` first"
  }
}
```

## Exit Codes

| Code | Meaning |
|---:|---|
| 0 | Success |
| 1 | Internal error |
| 2 | Invalid arguments |
| 3 | Daemon unavailable |
| 4 | Workspace unavailable |
| 5 | No results for result-seeking commands |
| 124 | Timeout |

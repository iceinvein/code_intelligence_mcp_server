# Code Intelligence CLI

The query CLI is the stable non-MCP surface for agents, hooks, and scripts. It calls the local daemon API on the loopback dashboard/API port (`mcp_port + 2`, default `17802`) and prints JSON envelopes.

## Commands

```bash
code-intel repo-map --repo . --budget 4000 --json
code-intel ask --repo . --json "how does auth work?"
code-intel search --repo . --context snippets --json "auth handler"
code-intel hydrate --repo . --ids sym_1,sym_2 --json
code-intel investigate --repo . --mode impact --target authenticate_request --json "what breaks if this changes?"

code-intel definition --repo . --json authenticate_request
code-intel references --repo . --reference-type call --json authenticate_request
code-intel call-hierarchy --repo . --direction callers --depth 3 --json authenticate_request
code-intel type-graph --repo . --direction both --json UserService
code-intel dependency-graph --repo . --direction upstream --json auth

code-intel index status --repo . --json
code-intel index approve --repo . --json
code-intel index refresh --repo . --json
code-intel index jobs --json
code-intel capabilities --json
```

Use `--port` when the daemon is running on a non-default port. Use `--timeout 2s` to bound daemon calls. Query commands automatically start a registered launchd daemon when it is unavailable. Use `--no-start` in scripts that require it to already be running.

## Recommended Flows

- Initial orientation: `repo-map --budget 4000 --json`
- Broad question: `ask --json "question"`
- Discovery: `search --context snippets --json "query"`, then `hydrate --ids ... --json`
- Impact/debugging: `investigate --mode impact --target SYMBOL --json "question"`
- Exact navigation: `definition`, `references`, and the graph commands
- First index: `index status`, ask the user for approval, then `index approve`
- Dynamic discovery: `capabilities --json`

## JSON Envelopes

Query commands (`ask`, `search`, `investigate`, `hydrate`, `repo-map`, and the navigation commands) use the envelope below. The `index` subcommands return a slimmer `{"ok":…, "repo":"/abs/path", "repo_id":"…", "result":{…}}`.

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
    "message": "failed to reach Code Intelligence daemon at http://127.0.0.1:17802/api/query/search: error sending request: client error (Connect): tcp connect error: Connection refused (os error 61)",
    "hint": "Run `brew services start code-intelligence-mcp`, then `code-intel status` to confirm it came up"
  }
}
```

The `message` always leads with the transport error and its underlying cause,
so a busy daemon reads differently from an absent one. The `hint` names
whichever launchd service actually supervises this machine's daemon: our own
label, Homebrew's, or `code-intel install` when neither is registered.
Reinstalling over a Homebrew service would write a second service fighting it
for the port, so the hint never suggests it there.

A query command auto-starts a stopped daemon, with two exceptions: it never
restarts one that already holds the port (a slow daemon is not a dead one, and
`launchctl kickstart -k` would abort its in-flight index run), and it never
starts anything when `--port` names a daemon other than the registered one.

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

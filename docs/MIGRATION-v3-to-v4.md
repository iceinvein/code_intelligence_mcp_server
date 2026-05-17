# Migration: v3 → v4

v4.0 is a hard pivot to a single shared HTTP daemon. The v3 model (one
stdio process spawned per MCP client) is gone, along with leader
election and the embedded "follower" mode. This guide walks through the
upgrade for each MCP client and explains the new binding model.

## TL;DR

Pick one of:

```bash
# Homebrew (recommended on macOS)
brew tap iceinvein/tap
brew install code-intelligence-mcp
brew services start code-intelligence-mcp
code-intelligence-mcp-server migrate

# npm (keeps the v3-era npx muscle memory)
npx -y @iceinvein/code-intelligence-mcp install
npx -y @iceinvein/code-intelligence-mcp migrate
```

After install:

- Daemon listens on `http://127.0.0.1:17800/mcp`.
- Dashboard: `http://127.0.0.1:17802/`.
- Optional discovery endpoint: `http://127.0.0.1:17801/.well-known/mcp`.

> **Pick one install path.** Both produce the same daemon, but they manage launchd differently: Homebrew owns the service plist via `brew services`, while the binary's `install` subcommand (used by the npm path) writes its own `com.iceinvein.code-intelligence.plist`. Running both at once will fight over the port. The migration guide assumes whichever path you chose; commands below show both where they diverge.

## What changed

| | v3 (stdio) | v4 (HTTP daemon) |
|---|---|---|
| Process model | One server per client, spawned via `npx ...` | One launchd-managed daemon shared across all clients |
| Transport | stdio (jsonrpc-over-pipe) | Streamable HTTP at `/mcp` |
| Workspace binding | `BASE_DIR` env var per client | URL `?repo=`, MCP roots, or `bind_workspace` tool call |
| Leader election | Required (every client tried to be leader) | Deleted; there is no leader |
| Model load | Each stdio process loaded its own GGUF models | Loaded once by the daemon, shared by all sessions |
| Port | None (stdio) | 17800 (MCP), 17801 (discovery), 17802 (API + dashboard) |

The daemon binds 127.0.0.1 only. Every API route enforces an `Origin`
check rejecting non-localhost browsers as a DNS-rebinding defence.

## Binding hierarchy

When a tool call lands on the daemon, the session needs a bound
workspace. v4 tries four sources in order; first match wins.

1. **`?repo=/abs/path` URL query parameter** — *primary, works on every
   client.* The proxy captures the query, pairs it with the SDK-assigned
   `mcp-session-id`, binds the session before the first tool call lands.
2. **MCP `roots/list` response** — opportunistic. Claude Code is the
   only widely deployed client that implements this.
3. **Single-repo registry fallback** — when `repos/registry.json` has
   exactly one entry, an unbound session auto-binds to it. Disables
   itself the moment a second repo is added.
4. **Hard error** — if nothing binds, every tool call returns a JSON-RPC
   error pointing the user at the URL form, the `bind_workspace` tool,
   and the roots capability.

The `bind_workspace` tool is still supported as a manual escape hatch
but is no longer the primary mechanism. Prefer the URL form when you
control the client config.

## Per-client recipes

### Claude Code

The `migrate` subcommand rewrites stdio entries (`command: npx ...`) to
streamable-http entries. After running `install` + `migrate`, no
further action is needed: Claude Code negotiates `roots/list` so the
daemon picks up the workspace root automatically.

Optional: pass `--patch-claude-json` to `install` to skip the
interactive prompt. The original `~/.claude.json` is backed up as
`~/.claude.json.bak.<unix-ts>` (last three backups kept).

### Cursor

Cursor advertises the `roots` capability but returns `Method not found`
on `roots/list` (an upstream bug open for 8 months). Use the URL form
instead:

```json
{
  "mcpServers": {
    "code-intelligence": {
      "url": "http://127.0.0.1:17800/mcp?repo=/Users/me/projects/my-app",
      "transport": "streamable-http"
    }
  }
}
```

Replace `/Users/me/projects/my-app` with the absolute path you want
indexed. To use the same daemon with multiple Cursor projects, configure
a separate MCP server entry per project, each with its own `?repo=`.

### OpenCode

OpenCode does not implement `roots/list`. Same URL pattern:

```json
{
  "mcp": {
    "code-intelligence": {
      "type": "remote",
      "url": "http://127.0.0.1:17800/mcp?repo=/abs/path/to/repo",
      "enabled": true
    }
  }
}
```

### Codex CLI

Codex steers its workspace via `--cd` / `--add-dir` flags and does not
have a `ListRoots` handler. Configure the MCP server with the URL form
just like Cursor.

### Continue.dev

Continue's MCP client is constructed without capabilities (no
`ListRoots` handler in the source as of SHA `cb27309`). Use the URL
form.

### Windsurf and Trae

Closed-source, docs silent on roots. Use the URL form — it always
works.

### Fallback for any client

If your client config does not let you set the URL with a query string,
two manual options:

1. **`bind_workspace` tool call.** Tell the agent: `call bind_workspace
   with repo=/abs/path/to/your/workspace`. The daemon binds the session
   and every subsequent tool call hits the bound repo.
2. **Single-repo registry trick.** Delete every other repo from the
   dashboard (or `DELETE /api/repos/{id}`). With exactly one repo
   registered, sessions auto-bind to it.

## What broke and how to fix

### "Session not bound to a repo"

The session did not match any of the four binding sources. The error
message lists every option. Most common cause: forgot to add `?repo=`
to the URL on a non-Claude client.

### `BASE_DIR` no longer required

v3 read `BASE_DIR` per-process. v4 derives the repo from the URL or the
tool call; `BASE_DIR` is ignored. You can remove it from any wrapper
scripts.

### `--standalone` flag is a no-op

Standalone is the only mode now. The flag is accepted for one release
to soften the migration but logs a deprecation warning. Drop it from
scripts.

### Default port changed (3333 → 17800)

`install` writes the plist with port 17800. If you ran v3 with port
3333 hard-coded somewhere (custom shell aliases, dashboards), update
them. Pass `--port` to `install` to keep 3333 if needed.

### `instance_role` removed from `get_index_stats`

The field always reported `standalone` in v3.3 and is gone in v4 since
there is no other role. Parsers that check this field can drop the key.

### `@iceinvein/code-intelligence-mcp-standalone` is going away

The split npm package was a v3-era artifact. v4 ships everything in
`@iceinvein/code-intelligence-mcp`. The `-standalone` package will be
unpublished after v4.0 lands. If you depend on it explicitly, switch.

## Verifying the migration

After `install` + `migrate`:

```bash
# Daemon up and reachable
curl http://127.0.0.1:17802/api/version

# Plist bootstrapped (PID, port, uptime)
code-intelligence-mcp-server status

# Re-warm a repo via the URL pattern
curl -X POST 'http://127.0.0.1:17800/mcp?repo=/abs/path/to/your/repo' \
     -H 'content-type: application/json' \
     -H 'accept: application/json, text/event-stream' \
     -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"1.0"}}}'

# Then open the dashboard
open http://127.0.0.1:17802/
```

The dashboard's "MCP sessions" panel should show one bound session
within a few seconds.

## Rolling back to v3

The migrate command backs up `~/.claude.json` as
`~/.claude.json.bak.<unix-ts>` (last three kept). To revert:

```bash
# Homebrew install:
brew services stop code-intelligence-mcp
brew uninstall code-intelligence-mcp
brew untap iceinvein/tap

# OR npm install:
code-intelligence-mcp-server uninstall    # bootout + remove plist

# Restore the v3 config and reinstall the v3 package:
cp ~/.claude.json.bak.<ts> ~/.claude.json
npm install -g @iceinvein/code-intelligence-mcp-standalone@^3.3
```

Per-repo indexes under `~/.code-intelligence/repos/<hash>/` are
unchanged across the upgrade, so re-indexing is not required on
roll-forward or roll-back. Drop a repo via `DELETE /api/repos/{id}` (or
the dashboard's "Delete" button) if you want a clean slate.

## Session resilience (v4.0.1+)

The rust-mcp-sdk transport occasionally times out a session and returns
the `-32016` "session expired" envelope. In v4.0.1 the proxy detects
this envelope, re-initialises the session against the SDK, replays the
original request with the new `mcp-session-id`, and forwards the second
response to the client. Workspace bindings (`?repo=`, `roots/list`,
`bind_workspace`) survive the recovery, so the recovered session stays
pinned to the same repo without a re-bind round trip.

Concurrent recoveries for the same stale session id are deduplicated so
racing in-flight requests do not trigger duplicate upstream re-init
storms. Every successful recovery is logged at INFO; you can see them
live in the dashboard's log panel or under `~/.code-intelligence/logs/`.

Practical impact:

- Long-running MCP clients (Claude Code, Cursor, OpenCode, Codex, …)
  no longer surface `-32016` errors when the upstream SDK ages a
  session out.
- You do not need to reconnect, re-bind, or restart the daemon when a
  session is recycled.
- If a recovery itself fails (binary not reachable, upstream `init`
  rejected, …) the original error propagates as before.

## What's new in the daemon

The migration unlocks features that were impossible per-client:

- **Dashboard** at `http://127.0.0.1:17802/` — repo list with per-repo
  stats, MCP session view, live log stream, jobs panel with reindex
  progress.
- **JSON API** for scripting — `/api/repos`, `/api/sessions`,
  `/api/jobs`, `DELETE /api/repos/{id}`, `POST /api/repos/{id}/reindex`,
  SSE log stream at `/api/logs/stream`.
- **Cross-repo tools** — `search_across_repos` and
  `explore_cross_repo_dependencies` now hit a single shared
  registry instead of needing the v3 standalone leader.
- **Faster cold start** — model loads happen once at daemon start
  instead of once per MCP client.
- **Transparent session recovery** (covered above; new in v4.0.1).

# v4.0.0 Plan: Standalone-Only Pivot

Date: 2026-05-16
Status: Locked, ready to execute
Tracking branch: `v4-pivot`

## TL;DR

Drop stdio embedded mode. The MCP server runs as one HTTP daemon. Setup is a single command (`code-intelligence-mcp install`). Add a Notion-leaning web UI on the same port for dashboards, log streaming, and per-repo actions. Replace MCP `roots` as the primary session-binding mechanism with an explicit `?repo=<path>` URL query parameter, because only Claude Code actually implements roots.

## Goals

1. One operating mode: HTTP daemon on `127.0.0.1:<port>`.
2. One-command setup: `code-intelligence-mcp install` writes the plist, loads it, and offers an interactive `~/.claude.json` patch.
3. Predictable session-to-repo binding that works on every MCP client, not just Claude Code.
4. Web UI for users to see what the daemon is doing and act on it.
5. Less code: stdio handler, leader election, follower waits, embedded-mode worker startup all deleted.

## MCP client roots-support audit (evidence for binding redesign)

| Client | Roots support | Evidence |
|---|---|---|
| Claude Code | Yes | Live-verified during the roots-handshake fix (commit `a0d1a0b`) |
| Cursor | Broken | Advertises `roots` in `initialize` but returns `Method not found` on `roots/list`. Forum-confirmed, 8 months open |
| Continue.dev | No | Source at SHA `cb27309`: `new Client({...}, { capabilities: {} })`, no `setRequestHandler` for `ListRoots`. PR #4533 closed unmerged |
| OpenCode | No | Source at SHA `764c6bc` + empirical: client constructed without capabilities; connected to daemon, never responded to `roots/list`, disconnected after 12s |
| Codex CLI | No | No client-side `ListRoots` handler. Workspace steered via `--cd` / `--add-dir` flags |
| Windsurf | Likely no | Docs silent. Untested empirically (closed source) |
| Trae | Likely no | Docs silent. Untested empirically (closed source) |

**Conclusion:** 1 of 7 surveyed clients implements roots. Treating roots as the primary binding mechanism is wrong; it only helps Claude Code users.

## Locked decisions

- Version: **4.0.0**. No transitional 3.4 release.
- npm: **single package** `@iceinvein/code-intelligence-mcp`. Unpublish `@iceinvein/code-intelligence-mcp-standalone`.
- Default port: **17800**. Configurable via `--port`. Install probes and offers next free port on collision.
- `install` writes plist and bootstraps launchd. Patches `~/.claude.json` only after an interactive prompt unless `--patch-claude-json` or `--no-patch-claude-json` is passed.
- Autostart: enabled by default. Opt out with `--no-autostart`.
- UI: embedded SPA bundled into the Rust binary via `rust-embed`. React + Vite + Tailwind. Notion-ish neutral palette.
- UI scope: read-only dashboard, live log streaming, per-repo reindex/remove. Daemon config editing is out of scope.

## Binding hierarchy

Tried in order, first match wins:

1. **URL query parameter** `http://localhost:17800/mcp?repo=/abs/path`. Primary. Works on every HTTP MCP client. `install --patch-claude-json` writes per-workspace URLs.
2. **MCP `roots/list`** response from the client. Opportunistic upgrade. If client supplies roots and no `?repo=` was given, bind to the first root.
3. **Single registered repo.** If `repos/registry.json` contains exactly one repo, bind to it. Disables itself the moment a second repo is added.
4. **Hard error** with actionable message: `Session not bound to a repo. Add ?repo=/abs/path to the MCP server URL. See <docs-url>.`

The implicit most-recently-accessed fallback present in v3.3 is removed. Silent wrong-repo binds were a real source of confusion.

## Code-removal scope (Phase 1)

To delete:

- `src/server/stdio.rs` and its `ServerHandler` impl.
- `src/leader.rs` (leader election + follower wait machinery).
- `src/server/mod.rs` dispatch for embedded mode.
- `BASE_DIR` and `CIMCP_MODE` env handling in `src/config/*` and `src/main.rs`.
- All `mode == embedded` branches in description-worker startup.
- `--standalone` CLI flag (it is the only mode now; flag becomes a silent no-op for one release then deleted).
- Integration tests under `tests/integration_*` that spawn stdio servers.
- Any tests asserting leader-lock semantics.

Estimate: ~1.5 to 2 kLOC + tests.

## New work

### Phase 1: hard pivot (2 to 3 days)

- Delete the modules listed above.
- Add `?repo=` parsing in standalone.rs. Query parameter pulled from the HTTP request URL during MCP session setup; stored on `session_repos` before any `roots/list` is attempted.
- Tighten `resolve_state` to return the new error message when nothing binds. Remove silent fallbacks.
- Run full test suite. Fix breakage caused by deletions. Add unit tests for the URL parser.

### Phase 2: CLI subcommands (2 to 3 days)

CLI surface on the existing binary:

```
code-intelligence-mcp                                # run server (default)
code-intelligence-mcp install [--port N] [--patch-claude-json|--no-patch-claude-json]
                              [--no-autostart] [--no-launchd]
code-intelligence-mcp uninstall
code-intelligence-mcp start
code-intelligence-mcp stop
code-intelligence-mcp status
code-intelligence-mcp migrate
```

- Plist template baked into the binary; resolves self via `std::env::current_exe()`.
- Use modern launchctl API: `bootstrap gui/<uid>`, `bootout`, `kickstart`, `print`. Requires macOS 13+; refuse with a clear message on older.
- `~/.claude.json` patcher: atomic JSON read-mutate-rename, backup as `~/.claude.json.bak.<unix-ts>`, retain last 3.
- `migrate` rewrites stale stdio entries across `mcpServers`, `projects.*.mcpServers`, and `projects.*.enabledMcpjsonServers`.
- `status` reports daemon PID, port, uptime, sessions, repos, indexing progress.

### Phase 3: JSON API + SSE log stream (1 to 2 days)

Endpoints (all on the same port as MCP):

```
GET    /api/version            { version, uptime_s, started_at }
GET    /api/status             { daemon overview }
GET    /api/repos              [{ id, path, name, indexed_at, symbols, descriptions, undescribed }]
GET    /api/repos/:id          { full stats + recent runs }
POST   /api/repos/:id/reindex  202 { job_id }
DELETE /api/repos/:id          204
GET    /api/sessions           [{ session_id, bound_repo, connected_at }]
GET    /api/logs/stream        SSE: each event = one log line, with level + timestamp + module
GET    /api/jobs               [{ id, type, repo_id, progress, started_at }]
```

DNS-rebinding defence: reject requests whose `Origin` header is not `http://localhost:<port>` or `http://127.0.0.1:<port>`. No CSRF token; same-origin enforcement is sufficient on a 127.0.0.1 dashboard.

### Phase 4: Web UI (3 to 5 days)

Stack:
- React 18 + Vite + Tailwind 3.
- Vite project at `ui/`, built to `ui/dist/`, embedded via `rust-embed`.
- `cargo build --release` invokes `npm run build` in `ui/` via build.rs.

Routes:
- `/` Dashboard: daemon stats, sessions, recent jobs.
- `/repos` Repo list with sort and filter.
- `/repos/:id` Repo detail with reindex/remove actions and per-repo charts (symbols over time, description coverage).
- `/logs` Live tail with level filter, pause, copy.

Design: light mode, system sans, mono for paths and IDs, restrained Notion-leaning palette. The aim is "admin tool, not landing page."

### Phase 5: Docs (1 day)

- README rewrite (quickstart, migration, troubleshooting).
- Migration guide for v3 stdio users (rewrite their `~/.claude.json` entries).
- `CLAUDE.md` update: drop embedded mode references, document subcommands.
- Screenshot of dashboard.

### Phase 6: Distribution (1 to 2 days)

- GitHub Actions release workflow bundles binary + UI dist into a single arm64-darwin artifact.
- npm `@iceinvein/code-intelligence-mcp` becomes a thin wrapper: `postinstall` fetches matching binary from GitHub Releases; `bin/code-intelligence-mcp` proxies to it.
- Unpublish `@iceinvein/code-intelligence-mcp-standalone`.
- Homebrew tap deferred to 4.1 unless trivially bundled.

### Phase 7: Cut (0.5 day)

- Tag `v4.0.0`, publish to npm, draft GitHub release, smoke-test fresh Mac install end to end.

## Risks and mitigations

- **launchctl API age.** `bootstrap`/`bootout`/`kickstart` require macOS 13+. Detect at install, refuse on older with a clear pointer to the deprecated `load`/`unload` path.
- **Port 17800 collision.** Install probes the port; on conflict offers the next free port and uses it consistently across plist and `.claude.json`.
- **First post-clone build needs node/npm.** Document in CONTRIBUTING. Consider a `make ui` target that lives outside the cargo build for power users.
- **UI bundle size.** Budget under 500 KB embedded. Swap React for Preact if it grows past 700 KB.
- **MCP client that surprises us.** Windsurf and Trae are untested. If a user reports a binding issue, the URL `?repo=` path is the universal fallback; opportunistic roots remains available.
- **Daemon crash loop under KeepAlive.** Add `ThrottleInterval=30` and a per-process crash counter exposed at `/api/status` so the UI can warn the user.

## Out of scope for 4.0

- Linux/Windows daemons (macOS-only stays).
- Brew tap (slip to 4.1).
- Multi-user shared daemons.
- HTTP authentication (still 127.0.0.1 trust model).
- Auto-update of the daemon binary.
- Per-tool `repo` argument for cross-repo workflows (slip to 4.1 if real demand surfaces).
- UI: daemon config editing, multi-tenant, RBAC.

## Open questions to resolve during execution

- How to communicate the binding error to the user. The current plan is a structured JSON-RPC error plus a doc URL. Consider also surfacing it on the web UI's session list ("session X is unbound, click here to fix").
- Brand: pick a name/logo for the UI shell. Today the package is plain text "code-intelligence-mcp".
- Whether the embedded UI should be its own crate or a `mod web` in the existing binary.

## Effort summary

| Phase | Work | Effort |
|---|---|---|
| 1 | Hard pivot: delete stdio + leader code, add `?repo=` URL parsing, drop silent fallback | 2 to 3 days |
| 2 | CLI subcommands + plist generation + `.claude.json` patcher + migrate | 2 to 3 days |
| 3 | JSON API + SSE log stream + integration tests | 1 to 2 days |
| 4 | Web UI scaffold, dashboard, repo detail, logs page, embed | 3 to 5 days |
| 5 | Docs and migration guide | 1 day |
| 6 | GH release workflow + npm wrapper + unpublish alias | 1 to 2 days |
| 7 | Cut v4.0.0 and smoke test | 0.5 day |

Realistic total: 10 to 16 days of focused work.

## Sources

- MCP roots specification: <https://modelcontextprotocol.io/specification/2025-06-18/client/roots>
- Cursor roots bug: <https://forum.cursor.com/t/mcp-client-does-not-support-roots-list/77248>
- Continue.dev PR #4533: <https://github.com/continuedev/continue/pull/4533>
- OpenCode source (mcp client init): <https://github.com/sst/opencode/blob/764c6bc517d41b09b3608e6344d898013cee2c91/packages/opencode/src/mcp/index.ts#L286>
- Continue.dev source (mcp client init): <https://github.com/continuedev/continue/blob/cb273098d968906d25ee737b454f0b5f13ea2482/core/context/mcp/MCPConnection.ts#L88-L96>

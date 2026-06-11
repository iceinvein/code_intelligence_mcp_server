# `@iceinvein/code-intelligence-mcp-standalone` is deprecated

[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square)](LICENSE)

This package was the v3-era standalone-mode wrapper. v4 unified the embedded and standalone code paths into a single shared HTTP daemon, so the split package is no longer needed and will be unpublished after v4.0 stabilises.

## Migrate to v4

Pick whichever install path you prefer; both produce the same daemon listening on `http://127.0.0.1:17800/mcp`.

```bash
# Homebrew (recommended on macOS)
brew tap iceinvein/tap
brew install code-intelligence-mcp
brew services start code-intelligence-mcp
code-intelligence-mcp-server migrate    # rewrites v3 stdio entries in ~/.claude.json

# OR npm (keeps the v3-era npx muscle memory)
npx -y @iceinvein/code-intelligence-mcp install
npx -y @iceinvein/code-intelligence-mcp migrate
```

Then update any explicit dependency on `@iceinvein/code-intelligence-mcp-standalone` to `@iceinvein/code-intelligence-mcp`. The default port moved from `3333` (v3) to `17800` (v4); pass `--port 3333` to `install` if you need to keep the old port for compatibility.

### Bundled External Producers

Code Intelligence installs external producer entrypoints with the server binary. These helpers are resolved from the installed binary directory first, then from `PATH`, with `EXTERNAL_INDEX_<LANG>_COMMAND` still available as an explicit override.

Bundled producers do not make external indexing automatic. The default remains native Tree-sitter indexing:

```bash
EXTERNAL_INDEX_AUTO=false
EXTERNAL_INDEX_ON_REFRESH=disabled
```

Use `generate_external_index` or opt-in refresh configuration to run producers before benchmark-proven defaults are enabled.

## What changed in v4

| | v3 (this package) | v4 (unified) |
|---|---|---|
| Process model | Long-lived HTTP server you ran yourself with `npx ...-standalone` | launchd-managed daemon installed via `install` subcommand or `brew services` |
| Default port | 3333 | 17800 (MCP), 17801 (discovery), 17802 (API + dashboard) |
| Workspace binding | MCP `roots` capability only | URL `?repo=`, `roots/list`, `bind_workspace` tool, or single-repo registry fallback |
| Dashboard | None | Embedded dashboard at `http://127.0.0.1:17802/` |
| Cross-repo tools | Required this standalone process | Built into the unified daemon |
| Session resilience | None | Transparent recovery from `-32016` session-expired upstream errors (v4.0.1+) |

For per-client recipes (Cursor, OpenCode, Codex, Continue, Windsurf, Trae), the `?repo=` URL pattern, and a rollback procedure, see [docs/MIGRATION-v3-to-v4.md](https://github.com/iceinvein/code_intelligence_mcp_server/blob/main/docs/MIGRATION-v3-to-v4.md).

## License

MIT

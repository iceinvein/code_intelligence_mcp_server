# Agent Installer

`install-agent` installs the agent-facing guidance for Code Intelligence without owning a whole client config. It is meant to complement the daemon lifecycle commands:

```bash
code-intel install
code-intel install-agent --repo . --target codex
```

## Commands

```bash
code-intel install-agent [opts]
code-intel uninstall-agent [opts]
```

Common options:

| Option | Description |
|---|---|
| `--target LIST` | `auto`, `codex`, `claude`, `cursor`, `opencode`, `generic`, or `all`. Comma-separated values are accepted. |
| `--scope SCOPE` | `project` or `user`. Default: `project`. |
| `--repo PATH` | Project root for instruction files. Default: current directory. |
| `--port PORT` | MCP endpoint port in generated snippets. Default: `17800`. |
| `--print-config` | Print the instruction block without writing files. Add `--mcp` to include MCP config. |
| `--dry-run` | Print planned writes without changing files. |
| `--no-instructions` | Skip instruction file updates. |
| `--mcp` | Include optional MCP config/snippet output. |
| `--no-mcp` | Explicitly skip MCP config/snippet output. This is the default. |

## Project Writes

Project-scope installs update only a generated block bounded by:

```md
<!-- code-intelligence-agent:start -->
...
<!-- code-intelligence-agent:end -->
```

Targets map to files as follows:

| Target | File |
|---|---|
| `codex`, `generic`, `opencode` | `AGENTS.md` |
| `claude` | `CLAUDE.md` |
| `cursor` | `.cursor/rules/code-intelligence.mdc` |

`uninstall-agent` removes only that managed block and leaves human-written content intact.

When `install-agent` prints MCP config, it includes the project binding in the URL as `?repo=...`. This is the portable v4 setup for clients that do not negotiate MCP roots.

The generated instruction block separates work into two modes:

- Main session: use `repo-map`, `search`, `hydrate`, and `investigate --mode impact` for tight, low-noise grounding.
- Exploration subagent: use `ask` and open-ended `investigate` when a broader evidence pass is worth the extra context.

## Examples

Preview everything:

```bash
code-intel install-agent --repo . --target all --dry-run
```

Install Codex guidance in the current project:

```bash
code-intel install-agent --target codex
```

Install Claude project guidance and print the MCP snippet:

```bash
code-intel install-agent --target claude --repo . --mcp
```

Patch user-level Claude MCP config without writing project instructions:

```bash
code-intel install-agent --scope user --target claude --no-instructions --mcp
```

Print config snippets for manual setup in agents with custom config shapes:

```bash
code-intel install-agent --target cursor,opencode --print-config --mcp
```

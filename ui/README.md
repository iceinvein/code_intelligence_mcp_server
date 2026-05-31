# Code Intelligence Web Portal

React + TypeScript single-page app for the daemon's control plane and code
exploration. Built with Vite (bundler + dev server), managed by Bun.

## Develop

1. Start the daemon (serves the JSON API on port 17802 by default):
   `./target/release/code-intelligence-mcp-server`
2. In another shell: `cd ui && bun install && bun run dev`
3. Open the printed Vite URL (default http://localhost:5273).

Vite proxies `/api` to the daemon and rewrites the `Origin` header to the
daemon's own origin so the daemon's localhost-origin guard admits the request
via its legitimate matching-port path. Point at a non-default daemon with
`DAEMON_API=http://127.0.0.1:18002 bun run dev`.

## Build

`bun run build` emits `ui/dist/`. The release binary embeds that folder via
rust-embed; the daemon then serves the SPA on its own API port.

## Test

`bun test` runs unit + component tests (happy-dom via `bunfig.toml`).

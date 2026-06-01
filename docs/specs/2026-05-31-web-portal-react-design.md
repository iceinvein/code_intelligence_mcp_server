# Web Portal as a First-Class App Design

Date: 2026-05-31
Status: Approved (pre-implementation)

## Problem

The web portal is a single hand-rolled `ui/dashboard.html` (896 lines, embedded
CSS + vanilla JS) served as an `include_str!` string from `src/server/api.rs`. It
covers a useful slice (repo table, reindex/delete, jobs, sessions, live logs, a
bare query playground) but it is not built to grow. Adding real control surfaces
(settings, consent approval) and real exploration (search, graph, symbols) on top
of one inline file means hand-managing state, DOM, and styling with no component
model, no type safety, and no test surface.

We want to elevate the portal into a first-class local application: a developer's
code-intelligence console that manages repos, indexing, settings, and logs, and
also lets a human search and explore the index directly.

## Goals

- Replace the inline `dashboard.html` with a real React + TypeScript application
  built with Bun, styled with Tailwind + shadcn/Radix primitives.
- Keep the daemon's single self-contained-binary property: the built UI is
  embedded in the release binary, no runtime asset dependency for end users.
- Deliver four capabilities (search/investigate, settings editor, consent
  approval, graph/symbol exploration) on a shared app foundation, shipped in
  phases.
- Preserve the existing terminal/workshop identity (the deliberate aesthetic
  shipped on 2026-05-18), now expressed through reusable themed components.
- Preserve existing security posture: localhost-only binding, non-localhost
  `Origin` rejection (DNS-rebind defence).

## Non-Goals

- No authentication / multi-user access. The portal stays localhost-only.
- No remote hosting or public exposure.
- No change to the MCP tool surface or to how agents consume the index. The
  portal is an additional human-facing surface, not a replacement for MCP.
- No rewrite of the retrieval/indexing engine. The portal calls existing
  handlers/tools through the JSON API.

## Decisions

These were settled during brainstorming. Rationale is recorded so the plan does
not relitigate them.

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Portal role | Full local code-intel app | Control plane + first-class search + exploration. |
| Asset delivery (prod) | Embed in binary via `rust-embed` | Keeps the single self-contained binary; matches today's `include_str!` property. |
| Bundler + dev server | Vite, with Bun as package manager + runtime | Vite is the best-trodden React + Tailwind + shadcn path (shadcn ships first-class Vite docs); Bun stays the PM/runtime (`bun install`, `bun run`). Overrides the global "no Vite" note by explicit request. |
| Build of `ui/dist/` | Release CI only, not committed | Simpler than committing build output; safe because no consumer builds from source (see Distribution). |
| Aesthetic | Terminal / workshop (continue current identity) | Distinctive developer-console identity; not generic SaaS. |
| App shell | Persistent left sidebar + header + Cmd+K | Scales to ~8 destinations, surfaces ambient state (pending consent, running jobs) as sidebar badges, deep-linkable. |
| Settings apply model | Hot-apply cheap knobs, persist + restart-prompt heavy ones | Best UX without the risk of live model load/unload. |
| Router | React Router | Battle-tested, low risk for ~8 routes. Swappable. |
| Server state | TanStack Query | Caching, polling (`refetchInterval`) for repos/jobs/index progress; SSE for logs. |
| Palette | `cmdk` | Same command-palette behaviour as today, as a real component. |

### shadcn under a terminal aesthetic

Choosing the terminal aesthetic does not drop shadcn. shadcn/Radix are unstyled
behavioural primitives whose appearance is entirely CSS-variable + Tailwind
driven. We keep the accessible primitives (dialog, dropdown, tabs, tooltip,
command) and theme their tokens to the existing palette. Today's
`dashboard.html` OKLCH palette and mono/sans fonts (`--ink`, `--ink-dim`,
`--edge`, `--edge-soft`, `--accent`, `--warn`, `--surface-2`, `--label`,
`--font-mono`, `--font-sans`) are lifted into the Tailwind theme and the shadcn
CSS variables as the default token set (dark-first, light variant from the same
tokens). Primitives are restyled to match: sharp radii, mono for code/data,
uppercase micro-labels, dense rows.

## Architecture

### Frontend (`ui/`)

`ui/` becomes a Bun-managed Vite + React project building to `ui/dist/`
(Vite's `root` is `ui/`, `build.outDir` is `dist`). Bun is the package manager
and script runner; `bun run dev` invokes Vite's dev server, `bun run build`
invokes `vite build`.

```
ui/
  package.json            # bun-managed; scripts: dev (vite), build (vite build), test, lint
  vite.config.ts          # React plugin, Tailwind plugin, /api dev proxy (strips Origin)
  tsconfig.json
  index.html              # Vite SPA entry (at ui/ root)
  src/
    app/                  # shell: sidebar, header, palette, router, theme provider
    components/ui/         # themed shadcn primitives
    features/
      repos/              # list, stats, reindex, delete, add
      search/             # query, results, evidence, jump-to-def/refs
      settings/           # config editor (hot/restart split)
      consent/            # pending repos, approve/decline
      graph/              # call / type / dependency graph, symbol inspector, browse
      logs/               # SSE log stream
      jobs/               # jobs + sessions
    api/                  # typed client, one module per resource; types mirror Rust JSON
    lib/                  # theme tokens, query client, utils
  dist/
    .gitkeep              # committed so rust-embed compiles even when dist is empty
```

Module boundaries: each `features/<capability>/` folder is self-contained and
talks to the backend only through `src/api/`. The `api/` layer owns the typed
contract (request/response `type`s mirroring the Rust JSON shapes). Live data
uses TanStack Query polling, except logs which consume the existing SSE stream.

TypeScript conventions: prefer `type` over `interface` (interface only for class
contracts). No emdashes in any source, comment, or copy.

### Backend (`src/server/`)

`src/server/api.rs` is already 899 lines and will grow with new endpoints, so it
is split into a module (a targeted improvement justified by this work):

```
src/server/api/
  mod.rs        # router assembly + shared ApiState/ApiError
  assets.rs     # rust-embed static serving at / with SPA fallback
  repos.rs      # repo CRUD + reindex + stats (existing handlers moved here)
  query.rs      # /api/query/* (existing)
  jobs.rs       # jobs (existing)
  sessions.rs   # sessions (existing)
  logs.rs       # SSE log stream (existing)
  settings.rs   # GET/PUT runtime + persisted config (new)
  consent.rs    # list pending + approve/decline (new)
```

New / changed endpoints:

- `GET /` and non-`/api` routes -> `assets`: serve embedded SPA, fall back to
  `index.html` for client-side routes. Replaces `handle_dashboard`.
- `GET /api/settings`, `PUT /api/settings` -> read/update config.
- `GET /api/consent`, `POST /api/consent/{id}` -> list pending implicitly-bound
  repos and approve/decline (wraps the same logic as the `approve_indexing`
  tool).
- `POST /api/repos` -> add a repo to the registry.
- `GET /api/models` -> model presence/load status (embedding, reranker,
  description LLM).
- Symbol/graph endpoints wrap existing handlers/tools (`get_definition`,
  `find_references`, `get_call_hierarchy`, `get_type_graph`,
  `explore_dependency_graph`, `get_file_symbols`); exact shapes defined in the
  graph/symbols phase plan.

Existing `/api/query/*` and `/api/logs/stream` are reused as-is.

### Runtime-mutable config

A shared `Arc<RwLock<RuntimeConfig>>` holds the hot-applyable knobs (hybrid
alpha, ranking weights, max context bytes, learning boosts, watch mode, index
patterns). Query-path code reads from it instead of from a value frozen at boot.
`PUT /api/settings` updates the lock for hot knobs and applies immediately.

Heavy / load-time settings (reranker enabled, descriptions enabled, embeddings
backend/device, port) are written to `~/.code-intelligence/server.toml` and the
UI shows a "restart daemon to apply" banner; the daemon already reads
`server.toml` at boot via `StandaloneConfig::load`. A `server.toml` writer
serialises changes while preserving the existing priority
(CLI > env > toml > defaults) semantics on next boot.

## Build, CI, and Distribution

Distribution is prebuilt-binary only: the brew formula downloads the
sha256-pinned `code-intelligence-mcp-server-aarch64-apple-darwin.tar.gz` from the
GitHub release, and the npm package consumes the same release binary. No consumer
runs `cargo build` from source. Therefore building `ui/dist/` only in release CI
is safe.

- **Dev:** the daemon serves `/api` on `mcp_port+2`. `bun run dev` starts Vite's
  dev server with HMR on its own port; Vite's `server.proxy` forwards `/api`
  (including the `/api/logs/stream` SSE endpoint) to the daemon. The API client
  uses **relative** `/api` URLs so the same code is same-origin in production.
  The origin guard requires the `Origin` port to match the `Host` port, so the
  Vite proxy **rewrites the `Origin` header to the daemon's own origin** on
  proxied requests (`proxyReq.setHeader('origin', DAEMON_API)`); combined with
  `changeOrigin` (which rewrites `Host` to the target) the daemon sees matching
  origin and host ports and admits the request via its legitimate same-origin
  path, without relying on the "no `Origin` = allow" bypass. In debug builds
  `rust-embed` (`debug-embed` off) serves `ui/dist/` live from disk, so a debug
  `cargo run` also serves whatever was last `bun run build`.
- **Release CI:** `.github/workflows/release.yml`, "Build (macOS Silicon)" job
  gains a step before "Build Binary": install Bun, `bun install`, `bun run build`
  (runs `vite build`, populates `ui/dist/`). Then `cargo build --release` embeds
  it.
- **Empty-UI guard:** a release-build assertion (build.rs `cfg(not(debug))` check
  or an explicit CI step) fails the build if `ui/dist/index.html` is absent, so
  we can never ship a binary with an empty UI.
- `ui/dist/` is gitignored except a committed `ui/dist/.gitkeep` so the
  `rust-embed` derive compiles even when `dist` is otherwise empty.

## Phasing

All four capabilities ship; this is the delivery order. Each phase gets its own
implementation plan.

1. **Foundation + control-plane port.** Bun/React/TS/Tailwind/shadcn toolchain,
   lifted theme tokens, `rust-embed` serving + SPA fallback, app shell (sidebar,
   header, Cmd+K), `api/` client, TanStack Query. Port Repos (list, stats,
   reindex, delete, add), Jobs, Sessions, Logs to reach parity with today, then
   delete `ui/dashboard.html`. Shippable baseline.
2. **Consent approval UI.** Small and timely (the consent gate just landed):
   pending implicitly-bound repos surfaced with approve/decline.
3. **First-class search/investigate UI.** Query box, ranked results, expandable
   syntax-highlighted evidence, jump to definition/references. Upgrades the bare
   query playground into a daily-driver tool.
4. **[shipped] Settings editor.** Restart-only `server.toml` write-back + UI,
   including Tier 2 retrieval tuning previously hardcoded in `repo_config()`.
5. **Graph/symbol exploration.** Call/type/dependency graph visualisation, symbol
   inspector (definition + references + usage examples), index browse by file
   tree. Heaviest area; graph-visualisation library chosen in this phase's plan.

## Testing

- `bun test` for the frontend: component/unit tests and an API-client contract
  test (request/response shapes match the documented JSON).
- `cargo test` for new Rust endpoints: settings round-trip (hot vs persisted),
  consent list/approve, asset serving + SPA fallback smoke test.
- Per the repo commit protocol: run the relevant `bun test` and `cargo test`
  plus `cargo fmt` / `cargo clippy` before any commit; lint/format the frontend.

## Risks and Open Questions

- **Bundle size in the binary.** A React + shadcn app embedded in the binary adds
  to its size. Mitigate with code-splitting (per-route lazy loading) and keeping
  graph-viz deps in the last phase. Measure after Phase 1.
- **Graph-visualisation library.** Deferred to Phase 5. Candidates and the
  perf/interaction trade-offs are evaluated in that phase's plan.
- **Origin check on the dev port (resolved).** The origin guard rejects
  mismatched localhost ports, so a direct browser->daemon fetch from the Vite
  port would 403. Resolved by routing all dev traffic through Vite's `/api`
  proxy, which rewrites the `Origin` header to the daemon's own origin (see Dev
  above) so the request is admitted via the daemon's legitimate same-origin path
  rather than the Origin-less bypass. Production is same-origin and unaffected.
  The daemon's existing `origin.rs` tests already cover matching-port-allow and
  cross-port-reject; the proxy rewrite is verified by the Task 8 manual curl.
- **`server.toml` round-trip fidelity.** The writer must not clobber unrelated or
  comment content; decide between a structured re-serialise and a
  format-preserving edit in the settings phase.

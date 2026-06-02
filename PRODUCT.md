# Product

## Register

product

## Users

Two users sharing one surface, weighted equally:

1. **The daily operator.** A developer running the daemon locally on macOS (Apple Silicon). They open the dashboard mid-task to confirm the daemon is alive, the right repo is bound, indexing is making progress, and nothing in the log stream looks alarming. Glanceable. Often peripheral vision while another tool has focus.
2. **The evaluator.** A developer who just finished `brew install` or `npx ... install` and wants the dashboard to convince them the project is real, considered, and trustworthy. First impression in the first 10 seconds. They have not yet committed to relying on this tool.

Both users land at `http://127.0.0.1:17802/`. Both judge the project's quality through this page.

## Product Purpose

Show the live state of a local code-intelligence daemon: registered repositories, MCP sessions, background indexing jobs, and a streamed log tail. Provide the controls a local operator needs (re-index, drop a repo) inline with the data. The dashboard is the only graphical surface the project has; everything else is CLI plus MCP tools consumed by an agent.

Success looks like: a developer opens the page and within two seconds knows the daemon is healthy, knows which repos are indexed, and trusts the project enough to keep it running.

## Brand Personality

Precise, calm, technical. The dashboard reads like a measurement instrument, not a marketing dashboard. Confidence comes from accurate state and considered typography, not from accent color or imagery. Three words: *precise, quiet, considered*.

Voice for UI copy: short, factual, lowercase-friendly for system terms (`mcp-session-id`, `localhost`). No marketing copy. No exclamation marks. No emoji.

## Anti-references

- Generic SaaS admin dashboards (templated card grids, blue-on-white, gradient hero metric blocks).
- "AI startup" landing aesthetic (gradient text, glassmorphism, animated mesh backgrounds).
- Datadog / Grafana template appearance: strong information design, weak identity. The dashboard must not look like every other DevOps panel.
- Cramped tool UIs (Acrobat, traditional ops consoles) that pack everything into toolbars and tiny dense tables.
- Cyberpunk neon-on-near-black: the secondary dark theme is high-contrast slate plus warm ivory, never saturated neon. Calm and considered, not a hacker movie set.
- Hero-metric template: big number + small label + accent stripe + supporting stats, repeated four times across the top of the page. The overview's vital readout is one hairline-divided bar, not four floating stat cards.

## Design Principles

1. **Status is the headline.** The first thing a user sees must answer "is the daemon doing what I expect?" Health, repos indexed, sessions bound, active jobs. Everything else is supporting evidence.
2. **Earned density.** Start sparse where users land. Grow denser inside expanded sections (repo detail, log stream) where attention is focused. Never make the top of the page dense.
3. **Typography carries the design.** Hierarchy through scale, weight, and font choice; color never does hierarchy. Color is signal-only and stays well under ten percent of pixels: the state vocabulary plus the selection accent, nothing decorative.
4. **State changes are honest.** Running, succeeded, failed, idle. Each state has one clear visual treatment carried by a distinct glyph SHAPE, a text label, and a color (never color alone). No ambiguous tints, no badges that look the same at a glance.
5. **Precision instrument, light-primary.** The primary theme is a warm paper surface with near-black warm ink, hairline (1px) rules instead of card chrome, and monospace numerals for data. Type pairing: system-sans for chrome, mono for data/paths/code, a serif-italic wordmark as the one identity flourish. The aesthetic of a well-made desk instrument (Braun/Rams readout, a lab datasheet), not a devtool dashboard. Dark mode is a faithful secondary translation (deep cool slate, warm ivory, brighter teal), reachable via the explicit toggle; it is not the headline.

## Accessibility & Inclusion

- **WCAG 2.1 AA** for color contrast across both light and dark themes. Small monospace metadata (11px timestamps, counts, paths in `muted-foreground`) must meet AA on its background. Light-mode amber `state-run` is the tightest pair (~4.7:1); never lighten it.
- **prefers-reduced-motion**: respected. All transitions become instant or near-instant; the live log feed does not auto-animate-scroll.
- **prefers-color-scheme**: respected, plus an explicit `system / light / dark` toggle that overrides it.
- **Keyboard navigation**: every interactive element reachable by tab, with a visible `:focus-visible` ring tied to the accent token.
- **Screen readers**: status badges expose their state via accessible names, not color alone. Live regions for new log lines are polite, not assertive.
- **Color blindness**: state distinctions never rely on red-green alone. Each state has a unique badge shape, label, or icon as well as a color.

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
- Cyberpunk dev-tool dark mode (neon-on-near-black, terminal-mimic). Dark mode must feel like dim paper, not a hacker movie set.
- Hero-metric template: big number + small label + accent stripe + supporting stats, repeated four times across the top of the page.

## Design Principles

1. **Status is the headline.** The first thing a user sees must answer "is the daemon doing what I expect?" Health, repos indexed, sessions bound, active jobs. Everything else is supporting evidence.
2. **Earned density.** Start sparse where users land. Grow denser inside expanded sections (repo detail, log stream) where attention is focused. Never make the top of the page dense.
3. **Typography carries the design.** Hierarchy through scale, weight, and font choice. Color does not do hierarchy. Accent appears in less than ten percent of pixels.
4. **State changes are honest.** Running, succeeded, failed, idle. Each state has one clear visual treatment. No ambiguous tints, no badges that look the same at a glance.
5. **Dim paper in dark mode.** Dark mode is the same instrument viewed under low ambient light. Warm undertone preserved. Identity unchanged.

## Accessibility & Inclusion

- **WCAG 2.1 AA** for color contrast across both light and dark themes. The 12.5px monospace timestamp column must meet AA on its background, not just AAA-by-accident at 14px.
- **prefers-reduced-motion**: respected. All transitions become instant or near-instant; the live log feed does not auto-animate-scroll.
- **prefers-color-scheme**: respected, plus an explicit `system / light / dark` toggle that overrides it.
- **Keyboard navigation**: every interactive element reachable by tab, with a visible `:focus-visible` ring tied to the accent token.
- **Screen readers**: status badges expose their state via accessible names, not color alone. Live regions for new log lines are polite, not assertive.
- **Color blindness**: state distinctions never rely on red-green alone. Each state has a unique badge shape, label, or icon as well as a color.

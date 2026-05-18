# Dashboard redesign: terminal direction

Date: 2026-05-18
Replaces: `ui/dashboard.html` (5-view tabbed redesign shipped in v4.0.5, commit a303601)

## Goal

Replace the tabbed dashboard with a single-page, monospace, palette-driven surface. The page should read as one calm instrument in light mode and one high-contrast instrument in dark mode, both honouring the same identity (italic serif brand, mono body, single teal accent). Power actions live behind a `⌘K` command palette that opens, runs, and disappears.

Success: an operator opens the page and inside two seconds knows the daemon is healthy, which repos are indexed, and what just happened in the log tail. An evaluator opens the page and within ten seconds judges the project as precise, considered, and trustworthy. Hitting `⌘K` answers "is this a real tool?" without ambiguity.

## Users

Unchanged from PRODUCT.md. Two users on one surface:

1. Daily operator: glanceable mid-task confirmation.
2. Evaluator: first-impression credibility check.

## Information architecture

Top-down, no tabs. One scrolling page. Sections appear in this fixed order so the operator's eye lands in the same place every refresh.

1. **Header strip.** Italic serif brand on the left ("code intelligence"), daemon listening address as a crumb (e.g., `localhost:17800`), healthy/unhealthy pulse on the right. One line. Sparse.
2. **Status grid.** Two columns separated by a dashed seam.
   - Left column: key/value rows. `daemon`, `repos`, `sessions`, `jobs`, `embed`, `version`. Numbers in tabular figures.
   - Right column: live tail. Last six log lines with timestamp, level, message. Warn lines coloured.
3. **Repositories.** A flat table beneath the status grid. Columns: name (with subdued path), files, symbols, last index, sessions. Rows expand inline on click to show repo detail (last 10 jobs, bound sessions, drop control).
4. **Command palette.** Hidden by default. Opens as a centered overlay on `⌘K` (no `/` trigger; that key is left to ordinary typing). The palette is search-first: typing fuzzy-filters across repositories, sessions, and a small set of actions, presented in named sections. Modifier keys on a result execute scoped actions (`⌘↵` reindex a repo, `⌘⌫` start a two-step drop). Esc closes.

Earned density is honoured: the header is one line, status is glanceable, the repo table sits below the fold, the command palette is the densest surface and reveals itself only when summoned by `⌘K`.

## Visual identity

### Typography

- Brand: `Charter, Iowan Old Style, Georgia, serif`. Italic, 14 px.
- Body / data: `JetBrains Mono, Berkeley Mono, SF Mono, Menlo, ui-monospace`. 12 px / 1.5.
- Labels / metadata (small caps, tracked): system sans (`-apple-system, system-ui`). 9-10 px, letter-spacing 0.14-0.18 em, uppercase.

No webfont loads. Serif and mono both ship with macOS.

### Color tokens

Light theme (cream paper, warm ink, muted teal accent):

```
--bg:        #f4ede1
--ink:       #2a241a
--ink-dim:   #6d5f3f
--label:     #8a7a52
--edge:      #d9ccaf
--edge-soft: #c9bb9e
--accent:    #2f6b5e
--warn:      #8a3a1c
--surface-2: #e7dec9
```

Dark theme (high contrast, deep slate + crisp ivory + brighter teal):

```
--bg:        oklch(10% 0.010 250)
--ink:       oklch(96% 0.008 85)
--ink-dim:   oklch(72% 0.012 80)
--label:     oklch(64% 0.014 80)
--edge:      oklch(28% 0.012 250)
--edge-soft: oklch(22% 0.010 250)
--accent:    oklch(82% 0.150 195)
--warn:      oklch(82% 0.135 55)
--surface-2: oklch(14% 0.012 250)
```

Accent rule: teal appears only on `ok` state, the health pulse, the REPL caret, the jobs progress fill, and `:focus-visible` rings. Under 10 percent of pixels.

State colors: `ok` uses accent; `warn` uses warm orange (`oklch(82% 0.135 55)` in dark, `#8a3a1c` in light); `error` uses a deeper warm red (to be tokenised when first needed). State never relies on color alone; every state badge has a textual label (`ok`, `warn`, `error`) or a glyph.

### Note on PRODUCT.md

The dark palette intentionally pushes past PRODUCT.md's "dim paper, warm walnut" guidance toward higher contrast. This is a deliberate, owner-approved deviation captured in this spec. PRODUCT.md should be updated to reflect the new dark stance after this lands.

## Components

### Header strip

One line. Italic serif brand on the left. Dim crumb showing the daemon's listening address (e.g., `localhost:17800`). Pulse circle on the right: filled accent when daemon is healthy, hollow warn ring when not. Tooltip on the pulse names the state.

### Status grid

CSS grid, `minmax(0,1fr) 1px minmax(0,1.05fr)`, dashed vertical seam.

Left column: six key/value rows. Each value is one line, never wraps. Long lists (sessions, jobs) truncate with a count rollup ("3 / 4 cap, claude-code, trae, opencode").

Right column: live tail. Six rows, fixed height each, oldest at the bottom and newest pushed in at the top. New lines fade in over 120 ms (skipped when `prefers-reduced-motion`). Each row is a grid of timestamp / level / message.

### Repositories

Flat HTML table. Header row in small caps. Body rows in mono. Numeric columns use `font-variant-numeric: tabular-nums`. Row click toggles an inline expansion below the row showing recent jobs, bound sessions, and a drop control.

The drop control is a small text link (`drop repo`) that requires a confirmation step: clicking flips the link to `confirm drop` for three seconds; clicking again issues `DELETE /api/repos/{id}`. No modal.

### Command palette

Hidden by default. Opens on `⌘K` (Mac) and `Ctrl+K` (other platforms), pre-focused on its search input. Closes on `Esc` or on a click outside the palette panel.

Layout (centered overlay):

- A backdrop scrim (~45% black) dims the page underneath.
- A single panel, ~min(680 px, 80vw) wide, anchored ~16% from the top. Background `var(--surface-2)` with a 1 px `var(--edge)` border and a soft shadow.
- Top row: a single line with caret, search input, and an `Esc` kbd hint on the right. The input is mono, 13 px.
- Body: a scrollable list grouped into named sections (small caps, tracked). Sections are: `Repositories`, `Sessions`, `Actions`. Up to 8 rows per section visible at once; results scroll if more.
- Footer: a kbd hint strip showing the keystrokes available for the highlighted row (`↑ ↓ NAV`, `↵ RUN`, `⌘↵ REINDEX`, `⌘⌫ DROP`, `ESC CLOSE`). The strip swaps as the highlight moves between row types.

Each row layout:

- 20-px icon column (a tiny boxed glyph in `var(--ink-dim)`, switching to `var(--accent)` when highlighted).
- Primary label (the repo `name`, the session basename, or the action verb).
- Secondary subtitle in `var(--ink-dim)`, 9 px (the repo path tail, the session bind state, etc.).
- Right-aligned kbd badge showing the default action (`↵`).

Fuzzy match: a basic substring-then-subsequence scorer. Highlights the matched span on the primary label in `var(--accent)`. The query is split on whitespace; all tokens must match (AND semantics).

### Palette result types

| Section        | Source                        | Default `↵`               | `⌘↵`                       | `⌘⌫`                                  |
| -------------- | ----------------------------- | --------------------------- | ---------------------------- | -------------------------------------- |
| Repositories   | `/api/repos`                  | Expand inline in the table; smooth-scroll into view; close palette. | `POST /api/repos/{id}/reindex` and close. | Start two-step drop: keep palette open, swap the row hint to `confirm drop`, second `⌘⌫` issues `DELETE /api/repos/{id}`. 5 s window. |
| Sessions       | `/api/sessions`               | Scroll the session count in the status grid into view; close palette. | (n/a) | (n/a) |
| Actions        | Built-in static list (below)  | Per-action.                 | (n/a)                        | (n/a)                                  |

Built-in actions (V2):

- `Refresh data now` (re-runs all pollers immediately).
- `Cycle theme` (system → light → dark → system; mirrors the header toggle).

Bulk reindex and tail filtering were deliberately not shipped in V2. Per-repo reindex is reachable via `⌘↵` on a highlighted repo; the live tail's six-row window makes filtering low value, and full log history is at `~/.code-intelligence/logs/`.

Sections render in this order: Repositories, Sessions, Actions. Empty sections are omitted. When the search box is empty, all sections appear in full (with Repositories capped at 8 rows visible; the user can scroll). When the search box has text, only sections with at least one match render.

### Palette keyboard

- `⌘K` (or `Ctrl+K`): open the palette from anywhere on the page (does not fire while the user is typing in a real `<input>` or `<textarea>` that is NOT the palette itself).
- `Esc`: close the palette. If the palette is closed, do nothing.
- `↑` / `↓`: move the highlight. Skips section header rows. Wraps within the visible result set.
- `↵`: run the default action for the highlighted row, then close.
- `⌘↵` (or `Ctrl+↵`): on a Repository row, queue a reindex and close. Inactive on other row types.
- `⌘⌫`: on a Repository row, start the two-step drop. The first press swaps the row's right-side kbd badge to a warn-coloured `confirm drop` and arms `pendingDrop`. The second press within 5 s issues the DELETE and closes. The third option for a wary user is `Esc` to cancel.
- Typing: filters the current section list. Whitespace splits into AND tokens.

### Decommissioned commands

The V1 REPL commands `?`/`help`, `clear`, `status`, `version`, `tail --grep`, `tail --since`, plus the inline command-history walk (`↑`/`↓`) and `Tab` completion, are removed. Their UX functions are absorbed as follows:

- `help` and self-discovery → the palette itself is the help; the footer kbd hints document available shortcuts contextually.
- `clear` → no scrollback to clear.
- `status` → the status grid is permanently visible; "Refresh data now" forces a re-poll.
- `version` → shown in the existing status grid `version` row and on the health-pulse tooltip.
- `tail --grep` / `--since` → not shipped in V2; use the on-disk log file at `~/.code-intelligence/logs/` for ad-hoc searches.
- History (`↑`/`↓`) → unused in palette mode (you re-find the result by typing again, which is the Raycast convention).
- `Tab` completion → unused; typing already fuzzy-filters.

Out of scope for V2: multi-pane palette views, plugins, persistent recent-actions, fuzzy scorers more elaborate than substring + subsequence with token AND.

## Theming

Single CSS file (inline in `dashboard.html`, no build step). Themes via CSS custom properties on `:root` with three states:

- `:root.theme-light` and `:root.theme-dark` set explicitly.
- `@media (prefers-color-scheme: dark) { :root:not(.theme-light) { ... } }` honors system preference.
- Default with no class follows system preference.

A FOUC guard script runs before first paint to read `localStorage.cimcp.theme` and apply the class. Theme toggle is a single text control in the header strip (`system / light / dark`).

`prefers-reduced-motion: reduce` disables tail fade-in, pulse animation, and scrollback slide.

## Data sources

All endpoints already exist on the JSON API (`src/server/api.rs`). No backend changes required for V1.

- `GET /api/status`: daemon state, uptime, embed q/s, version.
- `GET /api/repos`: repo list.
- `GET /api/repos/{id}`: repo detail (for row expansion).
- `POST /api/repos/{id}/reindex`: reindex action.
- `DELETE /api/repos/{id}`: drop repo (palette `⌘⌫`).
- `GET /api/sessions`: bound MCP sessions.
- `GET /api/jobs`: running and finished background jobs.
- `GET /api/logs/stream`: SSE log feed for the live tail.
- `GET /api/version`: version string.

Polling cadence: status, repos, sessions, jobs every 5 s (matches v4.0.5 defaults). Logs stream is event-pushed; no polling.

## Accessibility

- WCAG 2.1 AA contrast across both themes. Spot-check the smallest type (9 px small caps labels, 11 px tail rows) against `--bg` in both themes.
- `prefers-reduced-motion: reduce` collapses transitions to instant; live tail does not animate.
- `prefers-color-scheme` respected; explicit toggle overrides.
- Every interactive element keyboard-reachable. `:focus-visible` ring uses the accent token at 2 px solid.
- Status badges expose state via `aria-label` (e.g., `aria-label="daemon healthy"`), never color alone.
- Live tail is announced as a polite ARIA live region (`aria-live="polite"`), not assertive.
- Color blindness: warn uses warm orange, ok uses teal, error will use warm red. Each state additionally carries a text label.

## Non-goals (V2)

- Charts and graphs. No sparklines on the dashboard. (Measure direction can be considered later.)
- Per-repo dashboards or deep links. Row expansion is enough.
- Configuration UI. Settings remain in `~/.code-intelligence/server.toml`.
- Multi-pane palette views, plugin extensions, or persistent recent-actions in the palette.
- A pinned bottom REPL. V1 shipped one; V2 replaces it with the palette overlay.
- Command history walk (`↑` / `↓`) and Tab completion inside the palette. The Raycast model relies on type-to-find; we follow it.

## Open questions

None blocking. Items to resolve during planning:

- Exact `aria-live` granularity for the tail (per row vs batch).
- Whether the palette should announce result count to screen readers as the query changes (e.g., `aria-live="polite"` on a hidden status node).

## Implementation notes

- Single file: replace `ui/dashboard.html`. Inline CSS, inline JS, no build step. Matches current project posture.
- Vanilla JS only; no framework. The current dashboard is vanilla; keep it that way for first-paint speed on localhost.
- Reuse the existing `localStorage.cimcp.theme` key and FOUC guard pattern from the v4.0.5 dashboard.
- Keep the page total under 80 KB inlined.
- No external font requests. Type stack falls back to system serif and mono.
- Tests: existing `tests/dashboard_markers.rs` is extended to assert the palette markers (`id="palette"`, `id="palette-input"`, the three section labels). Integration coverage of `POST /api/repos/{id}/reindex` and `DELETE /api/repos/{id}` already exists for the V1 row-link drop; the palette reuses those handlers.

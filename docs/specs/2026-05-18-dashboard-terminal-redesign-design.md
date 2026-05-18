# Dashboard redesign: terminal direction

Date: 2026-05-18
Replaces: `ui/dashboard.html` (5-view tabbed redesign shipped in v4.0.5, commit a303601)

## Goal

Replace the tabbed dashboard with a single-page, monospace, REPL-driven surface. The page should read as one calm instrument in light mode and one high-contrast instrument in dark mode, both honouring the same identity (italic serif brand, mono body, single teal accent).

Success: an operator opens the page and inside two seconds knows the daemon is healthy, which repos are indexed, and what just happened in the log tail. An evaluator opens the page and within ten seconds judges the project as precise, considered, and trustworthy. The REPL prompt at the bottom answers "is this a real tool?" without needing to be used.

## Users

Unchanged from PRODUCT.md. Two users on one surface:

1. Daily operator: glanceable mid-task confirmation.
2. Evaluator: first-impression credibility check.

## Information architecture

Top-down, no tabs. One scrolling page. Sections appear in this fixed order so the operator's eye lands in the same place every refresh.

1. **Header strip.** Italic serif brand on the left ("code intelligence"), bound repo path as a crumb, healthy/unhealthy pulse on the right. One line. Sparse.
2. **Status grid.** Two columns separated by a dashed seam.
   - Left column: key/value rows. `daemon`, `repos`, `sessions`, `jobs`, `embed`, `version`. Numbers in tabular figures.
   - Right column: live tail. Last six log lines with timestamp, level, message. Warn lines coloured.
3. **Repositories.** A flat table beneath the status grid. Columns: name (with subdued path), files, symbols, last index, sessions. Rows expand inline on click to show repo detail (last 10 jobs, bound sessions, drop control).
4. **REPL prompt.** Pinned at the bottom of the page. Single line: caret, input, keyboard hint. Slash key (`/`) focuses it from anywhere. History scrollback appears above the prompt when it has been used in this session.

Earned density is honoured: the header is one line, status is glanceable, the repo table sits below the fold, the REPL is the densest surface and reveals itself only when used.

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

One line. Italic serif brand on the left. Dim crumb showing the bound repo absolute path (truncated with ellipsis if it overflows). Pulse circle on the right: filled accent when daemon is healthy, hollow warn ring when not. Tooltip on the pulse names the state.

### Status grid

CSS grid, `minmax(0,1fr) 1px minmax(0,1.05fr)`, dashed vertical seam.

Left column: six key/value rows. Each value is one line, never wraps. Long lists (sessions, jobs) truncate with a count rollup ("3 / 4 cap, claude-code, trae, opencode").

Right column: live tail. Six rows, fixed height each, oldest at the bottom and newest pushed in at the top. New lines fade in over 120 ms (skipped when `prefers-reduced-motion`). Each row is a grid of timestamp / level / message.

### Repositories

Flat HTML table. Header row in small caps. Body rows in mono. Numeric columns use `font-variant-numeric: tabular-nums`. Row click toggles an inline expansion below the row showing recent jobs, bound sessions, and a drop control.

The drop control is a small text link (`drop repo`) that requires a confirmation step: clicking flips the link to `confirm drop` for three seconds; clicking again issues `DELETE /api/repos/{id}`. No modal.

### REPL prompt

Pinned to the bottom of the scroll container with a `border-top: 1px dashed var(--edge-soft)`. Layout: caret, input, hint. The hint shows the most relevant keybinding for current context (e.g., `tab complete` when input is non-empty, `/ to focus` when blurred).

Scrollback appears above the prompt when commands have been issued. It is a transient session log, not persistent across reloads. Scrollback is capped at 50 entries.

## REPL command set

Bounded set wired to existing endpoints. Unknown commands print `unknown command; try ?`.

| Command                | Action                                                              |
| ---------------------- | ------------------------------------------------------------------- |
| `?` or `help`          | List commands inline in scrollback.                                  |
| `clear`                | Clear scrollback.                                                    |
| `status`               | Reprint the status block as a scrollback entry.                      |
| `repos`                | Reprint the repo table; smooth-scroll the view to that section.      |
| `repos <name>`         | Expand the row for `<name>` (fuzzy match on basename).               |
| `reindex <name>`       | `POST /api/repos/{id}/reindex`. Print queued / failed result.        |
| `drop <name>`          | Two-step confirm. On second invocation, `DELETE /api/repos/{id}`.    |
| `tail [--since 5m]`    | Scroll to the right column and apply a filter on age or substring.   |
| `tail --grep <pat>`    | Filter the live tail by substring.                                   |
| `version`              | Print daemon version from `/api/version`.                            |

Keyboard:

- `/` focuses the input from anywhere on the page (unless a text input is already focused).
- `Esc` blurs the input.
- `Up` and `Down` walk command history within the current session.
- `Tab` completes the first unambiguous command keyword or repo name.
- `Ctrl+L` is an alias for `clear`.

Out of scope for V1: pipes, scripting, saved command files, plugins, multi-line input. The grammar is `<verb> [<arg>] [--flag value]` and nothing more.

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
- `DELETE /api/repos/{id}`: drop repo (REPL `drop`).
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

## Non-goals (V1)

- Charts and graphs. No sparklines on the dashboard. (Measure direction can be considered later.)
- Pipes and scripting in the REPL.
- Persistent REPL history across reloads.
- Drag-and-drop, multi-pane resizing, or tmux-like splits.
- Per-repo dashboards or deep links. Row expansion is enough.
- Configuration UI. Settings remain in `~/.code-intelligence/server.toml`.

## Open questions

None blocking. Items to resolve during planning:

- Exact `aria-live` granularity for the tail (per row vs batch).
- Whether `tab` completion should also complete fuzzy repo basenames or only command keywords.
- Whether `drop` confirmation should also accept typing the repo name (safer for destructive ops, slower).

## Implementation notes

- Single file: replace `ui/dashboard.html`. Inline CSS, inline JS, no build step. Matches current project posture.
- Vanilla JS only; no framework. The current dashboard is vanilla; keep it that way for first-paint speed on localhost.
- Reuse the existing `localStorage.cimcp.theme` key and FOUC guard pattern from the v4.0.5 dashboard.
- Keep the page total under 80 KB inlined.
- No external font requests. Type stack falls back to system serif and mono.
- Tests: Bun smoke test that fetches `/` and asserts the page contains the brand string, the section markers, and the REPL prompt; integration test that exercises one round trip of the REPL `version` command against a running daemon (already covered for backend by existing API tests).

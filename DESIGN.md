---
name: Code Intelligence
description: The local code-intelligence daemon dashboard – a precision desk instrument.
colors:
  paper: "oklch(96.8% 0.006 83)"
  surface: "oklch(99.2% 0.004 83)"
  ink: "oklch(23% 0.014 70)"
  ink-muted: "oklch(45% 0.012 75)"
  label: "oklch(47% 0.012 78)"
  hairline: "oklch(89% 0.006 83)"
  selection-teal: "oklch(48% 0.118 200)"
  state-ok: "oklch(50% 0.128 150)"
  state-run: "oklch(54% 0.112 68)"
  state-fail: "oklch(52% 0.175 27)"
  state-idle: "oklch(45% 0.012 75)"
typography:
  wordmark:
    fontFamily: "Charter, Iowan Old Style, Sitka Text, Georgia, serif"
    fontSize: "1.0625rem"
    fontWeight: 400
    letterSpacing: "-0.01em"
  chrome:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: 1.5
  data:
    fontFamily: "JetBrains Mono, Berkeley Mono, SF Mono, ui-monospace, Menlo, monospace"
    fontSize: "0.6875rem"
    fontWeight: 400
    fontFeature: "tabular-nums"
  readout:
    fontFamily: "JetBrains Mono, Berkeley Mono, SF Mono, ui-monospace, Menlo, monospace"
    fontSize: "1.375rem"
    fontWeight: 400
    fontFeature: "tabular-nums"
  eyebrow:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, system-ui, sans-serif"
    fontSize: "0.6875rem"
    fontWeight: 500
    letterSpacing: "0.13em"
rounded:
  sm: "2px"
  md: "3px"
  lg: "4px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "24px"
components:
  button-primary:
    backgroundColor: "{colors.selection-teal}"
    textColor: "{colors.surface}"
    rounded: "{rounded.md}"
    padding: "4px 12px"
  button-outline:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.md}"
    padding: "4px 12px"
  datasheet-row:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    padding: "10px 14px"
  nav-item-active:
    backgroundColor: "transparent"
    textColor: "{colors.selection-teal}"
    rounded: "{rounded.md}"
    padding: "6px 10px"
---

# Design System: Code Intelligence

## 1. Overview

**Creative North Star: "The Desk Instrument"**

This dashboard is read like a measurement instrument on a workbench, not a marketing surface. The page is a sheet of warm paper; data is set in monospace numerals the way a lab datasheet or a Braun/Rams readout sets a value. Structure comes from hairline rules, not card chrome or shadow. Confidence comes from accurate state and considered typography, not from accent color or imagery. Two readers share it: an operator glancing mid-task to confirm the daemon is doing what they expect, and an evaluator deciding in ten seconds whether the project is real and trustworthy.

The system is deliberately near-monochrome. **Color is signal-only**: the surface is warm-neutral ink-on-paper, and hue appears almost exclusively to encode state (ok / run / fail / idle) and the current selection. When everything is healthy the page is quiet; an amber or red mark is meant to be the only thing that catches the eye. Light is the primary theme (a daylit desk); dark is a faithful secondary translation reached by an explicit toggle, never the headline.

It explicitly rejects: generic SaaS admin dashboards (templated card grids, gradient hero-metric blocks), the "AI startup" aesthetic (gradient text, glassmorphism, animated mesh), the interchangeable Datadog/Grafana DevOps panel, and cyberpunk neon-on-black.

**Key Characteristics:**
- Warm paper surface, near-black warm ink, hairline (1px) rules instead of cards.
- Signal-only color: state vocabulary + one selection accent, under ~10% of pixels.
- Three type roles: serif-italic wordmark, system-sans chrome, monospace data.
- Light-primary; dark is a translation, not an inversion.
- Every state has a glyph SHAPE + label + color, never color alone.

## 2. Colors

A warm-neutral instrument palette: ink on paper, with a single cool selection accent and a four-state signal vocabulary. All values are OKLCH (canonical); neutrals are tinted warm (hue ~75–83) so the surface reads as paper, not grey.

### Primary
- **Selection Teal** (`oklch(48% 0.118 200)`): the one non-state hue. Primary buttons, active navigation, `:focus-visible` rings, current selection, links on hover. Never decorative. In dark mode it brightens to `oklch(80% 0.118 198)`.

### Neutral
- **Paper** (`oklch(96.8% 0.006 83)`): the page background. The desk.
- **Surface** (`oklch(99.2% 0.004 83)`): raised sheets – datasheet frames, inputs, popovers, code blocks.
- **Ink** (`oklch(23% 0.014 70)`): primary text. A warm near-black, never `#000`.
- **Ink Muted** (`oklch(45% 0.012 75)`): secondary text, paths, timestamps, counts. AA on paper.
- **Label** (`oklch(47% 0.012 78)`): all-caps section eyebrows and field labels.
- **Hairline** (`oklch(89% 0.006 83)`): every border and divider. The system's primary structural device.

### Signal (state vocabulary)
- **OK Green** (`oklch(50% 0.128 150)`): healthy, indexed, succeeded, bound.
- **Run Amber** (`oklch(54% 0.112 68)`): indexing, in-progress, reconnecting, pending decision.
- **Fail Red** (`oklch(52% 0.175 27)`): failed, unreachable. Also `--destructive`.
- **Idle** (`oklch(45% 0.012 75)`): idle, never-indexed, unbound. Carried by ink-muted, no hue.

### Named Rules
**The Signal-Only Rule.** Color encodes state and selection, nothing else. There is no decorative color anywhere on the surface. If a hue is not telling the reader about a state or a selection, it is wrong.

**The Tightest-Pair Rule.** Light-mode Run Amber sits at ~4.7:1 on paper – it passes AA with no headroom. Never lighten `state-run` in the light theme.

## 3. Typography

**Wordmark Font:** Charter (with Iowan Old Style, Sitka Text, Georgia, serif)
**Chrome Font:** system-ui stack (`-apple-system, BlinkMacSystemFont, Segoe UI, system-ui`)
**Data/Mono Font:** JetBrains Mono (with Berkeley Mono, SF Mono, ui-monospace, Menlo)

**Character:** A three-role split with strict jobs. The serif-italic wordmark is the single identity flourish. Native system-sans carries all chrome (nav, headings, labels, prose, buttons) – fast, legible, unpretentious. Monospace carries every piece of data (numerals, paths, IDs, code) with `tabular-nums` so columns align like a printed datasheet. Fixed rem scale, never fluid.

### Hierarchy
- **Wordmark** (serif italic, 1.0625rem, tracking -0.01em): the brand mark in the header, only.
- **Readout** (mono, 1.375rem, tabular-nums): the big vital numerals on the overview.
- **Body / Chrome** (sans, 0.875rem, 1.5): default UI text, prose, controls. Prose capped 65–75ch.
- **Data** (mono, 0.6875rem, tabular-nums): paths, timestamps, counts, references, log lines.
- **Eyebrow / Label** (sans, 0.6875rem, weight 500, uppercase, tracking 0.13em): section headers and field labels.

### Named Rules
**The Mono-For-Data Rule.** If it is a number, a path, an identifier, or code, it is monospace with tabular numerals. If it is a sentence, a label, or a control, it is sans. The wordmark is the only serif, and it appears once.

## 4. Elevation

Flat by default. Depth is conveyed by hairline rules and a single tone step (Paper → Surface), not by shadow. The instrument is a flat sheet; nothing floats at rest.

### Shadow Vocabulary (the only shadows in the system)
- **Overlay drop** (`box-shadow: 0 12px 40px -16px oklch(20% 0.02 250 / 0.45)`): used only on genuinely floating layers – dialogs, the command palette, the symbol-picker dropdown. Soft and low.

### Named Rules
**The Flat-By-Default Rule.** Surfaces are flat. The only shadow in the system lifts a true overlay (modal, palette, dropdown) off the page. Resting cards, rows, and panels get a hairline border and a tone step – never a shadow.

## 5. Components

### Buttons
- **Shape:** crisp, near-square (3px radius). Never pill.
- **Primary:** Selection Teal fill, surface-colored text, `hover:opacity-90`. Used for the one primary action in a context (search, add repository, approve).
- **Outline:** transparent with a hairline `input` border, ink text, `hover:bg-muted`. The default secondary.
- **Ghost:** no border, `hover:bg-muted`. For low-emphasis actions.
- **Destructive:** hairline red border, red text, `hover:bg-destructive/10`. Drop / decline.
- **States:** 150ms color transition; focus shows the global 2px teal `:focus-visible` outline.

### Datasheet (signature component)
- **Replaces card lists everywhere.** A hairline-framed (`rounded-md border`) container with `divide-y` rows over a Surface background. A list of fifty reads as one calm table, never fifty floating cards.
- **Row:** `flex items-center gap-3`, padding ~10px 14px. Leading status glyph, sans primary text, mono secondary (path/meta), right-aligned mono metadata.

### Status Glyph (signature component)
- The carrier of the signal-only system. Four states, each a **distinct SVG shape** plus color plus text label: ok = filled disc, run = disc + pinging ring, fail = solid triangle, idle = hollow ring.
- Exposes an accessible name (`role="img"` + `aria-label`) when no visible label is shown. Meaning survives greyscale and color-blindness.

### Vital Readout (signature component)
- The overview's headline. Four cells (daemon / repositories / sessions / jobs) in ONE hairline-divided bar (`grid gap-px bg-border`), each cell a small uppercase label + restrained mono numeral + a sub-line. It is an instrument readout, **not** four floating stat cards.

### Inputs / Fields
- **Style:** hairline `input` border, Surface background, 3px radius, sans or mono per content.
- **Focus:** 2px teal ring (`focus-visible:ring-ring`).

### Navigation
- Left rail, system-sans labels with small Lucide icons. **Active = Selection-Teal text on a `bg-primary/10` tint** – never a colored side-stripe. Collapses to an icon-only rail below the `sm` breakpoint.

## 6. Do's and Don'ts

### Do:
- **Do** keep the surface near-monochrome and reserve color for state and selection (the Signal-Only Rule).
- **Do** use the Datasheet (hairline frame + `divide-y` rows) for every list. Spacing and rules group content, not cards.
- **Do** set all numbers, paths, IDs, and code in monospace with `tabular-nums`.
- **Do** give every state a glyph shape AND a text label AND a color – so it survives greyscale and color-blindness.
- **Do** keep the overview's vitals as one hairline-divided bar.
- **Do** write copy short, factual, lowercase-friendly for system terms. No exclamation marks, no emoji.

### Don't:
- **Don't** build the hero-metric template: big number + small label + accent stripe + supporting stats, repeated four times. The vital readout is one bar, not four cards.
- **Don't** use generic SaaS admin styling: templated card grids, blue-on-white, gradient hero-metric blocks.
- **Don't** use the "AI startup" aesthetic: gradient text, glassmorphism, animated mesh backgrounds.
- **Don't** look like Datadog/Grafana – strong information design, weak identity. This must not read as every other DevOps panel.
- **Don't** make the dark theme cyberpunk neon-on-near-black; it is high-contrast slate plus warm ivory, never saturated neon.
- **Don't** use a `border-left`/`border-right` greater than 1px as a colored accent stripe on rows, cards, or callouts. Active nav is a tint + accent text, not a stripe.
- **Don't** float resting surfaces on shadows. Hairline + tone step only; shadow is for overlays.
- **Don't** lighten light-mode Run Amber – it passes AA with zero headroom.

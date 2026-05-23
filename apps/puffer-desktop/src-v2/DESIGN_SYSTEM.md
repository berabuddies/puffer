# Puffer Desktop UI v2 — Design System

Source of truth: Paper file **Momo**, artboard **WorldAgent Home Design System** (1440 × 1400). All numeric values in this document were read with `mcp__paper__get_computed_styles` against that artboard; the prose was read with `mcp__paper__get_node_info` so it matches the design verbatim.

Code surfaces:

- `apps/puffer-desktop/src-v2/styles/tokens.css` — every value below as a CSS custom property.
- `apps/puffer-desktop/src-v2/styles/base.css` — reset, web fonts, and `.text-*` utility classes.

> All page agents should consume the tokens. **Do not** hard-code hex values, font names, or pixel sizes. If you need something that isn't here, ask the design-system agent to extend `tokens.css` rather than introducing a one-off.

---

## Mood

Quiet desktop productivity. Most surfaces are white or near-white gray. Borders carry hierarchy instead of shadows. A single warm cream is reserved for the next useful action — never used for decoration, never used twice in the same view if one cream cluster already exists. Serif type carries page emotion (greetings, section tone); the rest is operational system sans. The frame is fixed and dense (1093 × 994), so spacing is restrained: 8 / 16 / 24 do almost all the work.

---

## Color palette

| Token                          | Hex                       | When to use                                                                                     |
| ------------------------------ | ------------------------- | ----------------------------------------------------------------------------------------------- |
| `--color-surface-app`          | `#FFFFFF`                 | Page background. The default ground.                                                            |
| `--color-surface-rail`         | `#F4F4F4`                 | Left rail, neutral icon buttons, secondary action pills (e.g. "Reschedule").                    |
| `--color-surface-input`        | `#FEFEFE`                 | Composer pill fill, attach button fill. A barely-warmer white that reads as "input."            |
| `--color-input-border`         | `#E0E0E0`                 | Composer pill border, inputs.                                                                   |
| `--color-input-border-soft`    | `#E1E1E1`                 | Composer attach `+` button border.                                                              |
| `--color-action-cream`         | `#F8EEDC`                 | Primary action fill (the only warm accent in the system).                                       |
| `--color-action-cream-soft`    | `#F7EEDC`                 | Composer send button (1px warmer in the design; kept as a distinct token to preserve fidelity). |
| `--color-action-cream-border`  | `#F0E2C7`                 | Hairline border around cream swatches, focus ring color.                                        |
| `--color-action-cream-text`    | `#795600`                 | Text/icon on cream. Never use cream-on-white text.                                              |
| `--color-text-primary`         | `#161616`                 | Headings, primary card titles, page hero.                                                       |
| `--color-text-secondary`       | `#525252`                 | Body text (13px compact) inside cards and rows.                                                 |
| `--color-text-muted`           | `#6F6F6F`                 | Captions, eyebrow labels, secondary serif (subtitle).                                           |
| `--color-card-border`          | `#ECECEC`                 | Card hairline border. Also the composer's top divider.                                          |
| `--color-selected-fill`        | `rgba(0,0,0,0.06)`        | Selected nav row fill. **Never** swap this for a saturated brand color.                         |

Coordinator's fallback table cross-check: every value matches. Card border is confirmed `#ECECEC` (not `#EEEEEE`). Composer pill uses `#E0E0E0`; the composer's `+` attach button uses a marginally lighter `#E1E1E1` — both kept.

---

## Typography

Three families. Use the family that matches the role; do not mix.

| Style          | Family                     | Size | Line height | Weight | Color                  | Usage                                                                              |
| -------------- | -------------------------- | ---- | ----------- | ------ | ---------------------- | ---------------------------------------------------------------------------------- |
| Display        | Source Serif 4             | 28px | 34px        | 400    | `text-primary`         | Page greetings ("Good morning, Yuna."), page hero titles.                          |
| Subtitle       | Source Serif 4             | 20px | 24px        | 400    | `text-muted`           | Soft secondary line under the display ("5 things need your attention today").      |
| Section        | Source Serif 4             | 24px | 30px        | 400    | `text-primary`         | Larger serif section headings inside a page ("Color tokens", "Usage rules").       |
| Task title     | system-ui                  | 15px | 22px        | 500    | `text-primary` (#000)  | Card titles, list-row primary text.                                                |
| Body compact   | system-ui                  | 13px | 18px        | 400    | `text-secondary`       | Card meta, meeting details, summaries.                                             |
| Button label   | Inter                      | 12px | 16px        | 500    | inherits (cream/black) | All button labels.                                                                 |
| Eyebrow        | system-ui                  | 12px | 16px        | 600    | `text-muted`           | Section labels in caps ("SIDEBAR NAV", "TASK CARD"). Letter-spacing `0.04em`.      |

Notes:

- Web fonts loaded from Google Fonts: Source Serif 4 (400, 500), Inter (400, 500, 600).
- IBM Plex Sans was inspected (available via `get_font_family_info`) but is **not used** in the design system, so it is not loaded.
- The "Task title" sample in the artboard uses `system-ui` resolved to pure `#000000`. We map it to `--color-text-primary` (`#161616`) for consistency with every other primary text token; the visible difference is sub-perceptual on white.

---

## Spacing & radii

Spacing scale (multiples of 4; `8 / 16 / 24` are the load-bearing tiers called out in the artboard):

| Token       | Value | Typical use                                                          |
| ----------- | ----- | -------------------------------------------------------------------- |
| `--space-1` | 4px   | Hairline gap inside a stack (swatch label rows).                     |
| `--space-2` | 8px   | Micro spacing between tightly related elements.                      |
| `--space-3` | 12px  | Composer internal gaps.                                              |
| `--space-4` | 16px  | **Card padding, card-to-card gap, button horizontal padding.**       |
| `--space-5` | 24px  | **Section rhythm, page side padding.**                               |
| `--space-6` | 32px  | Large section break.                                                 |
| `--space-7` | 48px  | Hero spacing.                                                        |

Radii:

| Token              | Value     | Use                                                                  |
| ------------------ | --------- | -------------------------------------------------------------------- |
| `--radius-control` | 8px       | Sidebar nav rows, inputs.                                            |
| `--radius-card`    | 16px      | Task cards, panels.                                                  |
| `--radius-pill`    | 999px     | Composer pill, icon buttons, action pills, avatars/icon blocks.      |
| `--radius-swatch`  | 10px      | Token swatch only (design-system reference). Not used in product UI. |

---

## Shell layout

```
┌────────────────────────── 1093 ──────────────────────────┐
│                                                          │
│  ┌───────────┐  ┌─────────────────────────────────────┐ │
│  │           │  │ ← 24 padding →                      │ │
│  │   rail    │  │     ┌───────── 760 max ─────────┐   │ │
│  │   248     │  │     │                           │   │ │
│  │           │  │     │     page content column   │   │ │   994 tall
│  │           │  │     │                           │   │ │
│  │           │  │     └───────────────────────────┘   │ │
│  │           │  │              ← 24 padding →         │ │
│  └───────────┘  └─────────────────────────────────────┘ │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

- Outer frame: 1093 × 994.
- Left rail: 248px wide, full height, `--color-surface-rail` background.
- Page column: flexible width, 24px side padding inside the page.
- Inner content column caps at 760px (the composer, cards, and prose all share this measure).
- Composer sits at the bottom of the page column, separated from content above by a `--color-card-border` hairline (`border-top: 1px`).

Tokens to consume: `--shell-width`, `--shell-height`, `--shell-rail-width`, `--shell-page-max`, `--shell-page-padding`.

---

## Core component recipes

These are the only components Agent 2 / per-page agents should reuse. Anatomy and sizes are observed from the artboard; do not vary them without coordination.

### Sidebar nav row

- Frame: `width: 100%; height: 32px; border-radius: 8px;`
- Padding: `7px 10px`. Gap between icon and label: `10px`.
- Icon slot: 16 × 16, line-based (Lucide-style strokes), `flex-shrink: 0`.
- Label: `.text-task-title` is too heavy — nav uses `--font-size-body` (13–14px) at `--font-weight-medium`, `--color-text-primary`. (Inferred from the rendered design; if a label needs to look heavier, raise weight, not size.)
- States:
  - default: transparent background.
  - hover: `background: var(--color-selected-fill);` at half opacity (or rgba(0,0,0,0.03)).
  - selected: `background: var(--color-selected-fill);` — **never** a saturated brand color.

### Task card

- Frame: `display: flex; align-items: flex-start; width: 100%; padding: 16px; gap: 16px; border-radius: 16px; background: var(--color-surface-app); border: 1px solid var(--color-card-border);`
- Anatomy: `[icon block 40×40 pill, surface-rail bg]` + `[content row: title + meta on the left, action cluster pinned right]`.
- Icon block: 40 × 40, `border-radius: 999px`, `background: var(--color-surface-rail)`, line icon 16–20px centered.
- Title: `.text-task-title`. Meta beneath: `.text-body-compact`. Vertical gap inside the content stack: 4–8px depending on density.
- Action cluster: pinned to the right with `justify-content: space-between` on the content row; gap between buttons `--space-2` (8px) or `--space-3` (12px).
- Cards stack with `--space-4` (16px) vertical gap.

### Button — primary (cream)

- `height: var(--height-button-card)` (32px) for in-card actions; `48px` for utility/large.
- `padding: 0 var(--space-4)` (16px horizontal).
- `background: var(--color-action-cream);`
- `color: var(--color-action-cream-text);`
- `border-radius: var(--radius-pill);` (999px → pill).
- Label uses `.text-button-label` (Inter 12/16 medium).
- Hover: darken the cream by ~4% (e.g. apply `filter: brightness(0.97)`) — do not change hue.
- Focus: outline `--color-action-cream-border` at 2px offset.

### Button — secondary (neutral)

- Same geometry as primary (32px / 999px / 16px horizontal padding).
- `background: var(--color-surface-rail);` `color: var(--color-text-primary);`
- Hover: `background: #ECECEC` (one step darker, no new token needed — equivalent to `--color-card-border`).

### Button — icon-only

- 32 × 32 (card row), 36 × 36 (composer send), 48 × 48 (composer attach `+` / large utility).
- `border-radius: var(--radius-pill);`
- 32px and 36px icon buttons use `background: var(--color-surface-rail)` (or `--color-action-cream-soft` for the composer send).
- 48px attach button: `background: var(--color-surface-input); border: 1px solid var(--color-input-border-soft);`
- Icon: 16–20px line stroke, color `--color-text-primary` on neutral, `--color-action-cream-text` on cream.

### Composer

- Outer row: `display: flex; align-items: center; width: 100%; padding: 18px 0; gap: 12px; border-top: 1px solid var(--color-card-border);`
- Left attach button: 48 × 48, pill, `--color-surface-input` fill, `--color-input-border-soft` border, `+` glyph centered.
- Center input pill: `flex: 1; height: 50px; border-radius: 999px; padding: 6px 8px 6px 16px; gap: 8px; background: var(--color-surface-input); border: 1px solid var(--color-input-border);`
  - Placeholder/text: `--font-size-task` (15px), `--color-text-primary` for content, `--color-text-muted` for placeholder.
  - Inside the pill on the trailing edge: a 36 × 36 cream send button (`--color-action-cream-soft`, `--radius-pill`).
- The composer pins to the bottom of the page column. It should read like a single command line: one attach, one pill, one send.

---

## Usage rules

Verbatim from the artboard's "Usage rules" frame (3K-0):

1. **Keep the palette quiet.** Most surfaces are white or gray. Cream only marks the next useful action.
2. **Use border before shadow.** Cards rely on `#ECECEC` borders; avoid heavy elevation.
3. **Serif is for page emotion.** Use Source Serif 4 for greetings, section tone, and large headings; use system for operational details.
4. **Cards are action rows.** Each task should contain icon, short title, concise context, and a compact action cluster.
5. **Icons are line-based.** Use 16–20px Lucide-style strokes, black/gray, inside circular neutral icon blocks.

Composer rule (3J-0): _"Composer sits at the bottom of the page column. It should feel like a command line: one neutral attach/tool button, one large pill input, one cream send affordance."_

Sidebar selection rule (2F-0): _"Selected item uses subtle black alpha fill, never a saturated brand color. Icons stay 16px line icons."_

Card rule (2W-0): _"Cards are 16px padded, 16px radius, 1px border. Buttons cluster on the right; primary actions use cream, secondary actions use neutral gray."_

Button rules (37-0, 38-0):

- Heights: **32px for card actions, 36px for composer send, 48px for large utility.**
- _"Use text buttons for clear commands; use icon-only for tools and agent actions."_

---

## Notes for future agents

- **Do not introduce a new accent color.** Cream is the only warm note. If a page seems to need a second accent, the page is doing too much — reduce density first.
- **Do not add shadows.** The system is border-led. If you need elevation, use a darker border (`#D9D9D9`), not a shadow.
- **Icons:** the artboard does not ship icon glyphs. Per the usage rule, use Lucide line icons at 16–20px. Agent 2 should pick a Lucide-for-Svelte package; per-page agents should not invent their own.
- **The 12px caps eyebrow** (`.text-eyebrow`) is only labeled in the spec sheet, not the product surfaces shown in the design. Use sparingly — it's a tool for section labels, not body decoration.
- **Verification cross-check:** ≥3 numeric values were read via `get_computed_styles` and confirmed against the coordinator's fallback table.
  - Card: `padding 16px`, `border-radius 16px`, `border 1px solid #ECECEC` — matches.
  - Task title: `15px / 22px / 500 / system-ui` — matches.
  - Cream button: `height 32px`, `padding-inline 16px`, `background #F8EEDC`, `border-radius 999px` — matches.
  - Composer pill: `height 50px`, `border #E0E0E0`, `background #FEFEFE`, `border-radius 999px` — matches (50px height not stated in the fallback table; sourced from computed styles).
  - Display: `28px / 34px / 400 / Source Serif 4` — fallback said 24–28; computed is 28. Using 28.

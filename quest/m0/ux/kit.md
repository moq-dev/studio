# [L] The studio's look

## Goal

The studio has a look of its own: its colors, type, spacing, and icons come
from one set of tokens, and its controls are its own widgets, drawn as the
[mock-ups](/quest/m0/ux/mock/shell-live.webp) draw them. Today's panels wear
it at once, and a gallery view shows every widget in every state, in the
[screenshots](/quest/m0/ux/shots.md).

Today the app sets no fonts, style, or visuals, has no icon anywhere, and
hard-codes a handful of colors.

## Plan

`mock/kit.css` is the reference: its custom properties are the tokens, and
its classes are the widgets.

- **Tokens:** near-black surfaces in three steps, one line color, text in
  three strengths. Red is the program and nothing else. Amber is
  everything that is not right and not on air: cued, off air, degraded, or
  needing attention. Green is a healthy level. Blue is selection.
- **Type:** three bundled faces under an open license: a condensed face for
  state and names (ON AIR, scene names, page names), a plain face for
  everything else, and a monospace for clocks and rates, whose digits must
  not jitter. The mock-ups use Barlow Condensed, Inter, and JetBrains Mono;
  keep them unless one fails to render well in the chosen toolkit. Bundled,
  never the machine's, as overlays already do.
- **Icons:** one open-licensed set in a single stroke weight (the mock-ups
  draw in Lucide's style), named by role in one place. Every icon-only
  control has a tooltip that names it and its key.
- **Widgets:** the scene pad, the pill, the icon button, the toggle, the
  slider, the fader, the segmented meter, the number field that drags, the
  popover, the tooltip, the sheet, the keycap, and the tally frame.
- **Effects:** today's rune-fx effects (`rs/moq-studio/src/fx.rs`) read the
  tokens, and stop registering spots while panels draw, which ties them to
  the layout. The Effects setting and reduced motion keep working.

Measure the UI's frame time before and after, under a real compositor.

Recommendation: keep each widget's state in the caller, so the pages can
draw from a snapshot of the engine rather than holding its lock while they
draw, as `Studio::ui` does today.

Human check: taste, on the gallery's screenshots, and readability of red
against amber.

## Required

- [Toolkit spike](/quest/m0/ux/spike.md) - the toolkit the widgets are written for
- [UI screenshots](/quest/m0/ux/shots.md) - the gallery's pictures

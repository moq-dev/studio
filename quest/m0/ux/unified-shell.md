# [L] Unified Studio shell, slim dock, and inline audio mixer

## Goal

Replace the permanent four-page split (`LIVE`, `BUILD`, `MIX`, `REVIEW`) with a
single unified Studio workspace ([mock-up](/quest/m0/ux/mock/overhaul-studio.webp)
and [audio popover](/quest/m0/ux/mock/overhaul-popovers.webp)):

- **Always-editable canvas:** merge `LIVE` and `BUILD` so you can click, drag,
  resize, and crop layers directly on the main stage at any time. A compact
  `Editable` / `Locked` toggle in the title bar locks canvas dragging during a
  broadcast when desired.
- **Collapsible Layers rail:** a toggleable `Layers` button in the title bar
  opens/closes the left layers rail without shrinking the canvas when not
  needed. Remove the permanent empty `"ALWAYS ON TOP"` placeholder when no
  pinned layers exist.
- **No default page tabs:** remove `LIVE | BUILD | MIX | REVIEW` from the title
  bar by default. When a user enables the `Audio Scenes & Routes` or
  `Replay & Review` mod in Settings > Mods, only those extra tabs appear beside
  `Studio`.
- **Slim bottom dock (~86 to 112 px):**
  - Left: scene pads with live thumbnails. Clicking `+` creates a new scene and
    **switches to it immediately** (no toast telling the user to click it and
    switch to Build). Remove the confusing `FIRST` badge on the active scene.
  - Right: **inline horizontal volume sliders** with live level meters, mute
    toggle, dB readout, and a per-source gear button that opens a compact
    options popover (`Monitor in headphones`, `Delay`, `Pan`, `+ Filter`), so
    everyday audio adjustments never require leaving the main workspace.
  - Off-air clutter removed: hide the permanent Panic (`BRB`), Save Clip, and
    Drop Marker column when off air or unused (Clip/Marker belong to the
    `Replay & Review` mod; Panic appears only when a Safe scene is configured
    or on air).

## Plan

Decided 2026-11-22 with the maintainer (/quest-plan):

- **Page consolidation:** ✅ single unified Studio workspace merging LIVE +
  BUILD, with inline audio sliders and Mod-gated MIX/REVIEW (rejected:
  two-page Studio + Mix layout; keeping all four permanent tabs).
- **Audio dock:** ✅ horizontal volume slider + live meter + mute + per-source
  popover (`Monitor in headphones`, `Delay`, `Pan`, `+ Filter`) directly in
  the bottom dock, moving multi-bus `Audio Scenes & Routes` to Settings > Mods
  (rejected: mute-only bottom list forcing trips to MIX; keeping Audio Scenes
  visible by default; inline expanding drawers that squish neighboring strips).
- **Scene `+` behavior:** ✅ clicking `+` adds a scene and makes it active
  immediately, showing the empty-canvas quick-start prompt (rejected: adding
  in the background with a toast).
- **Bottom dock height & off-air chrome:** ✅ reduce dock height from 150px to
  ~96-112px and omit Clip/Marker/Panic controls when off air or unconfigured
  (rejected: permanently reserving 112px of middle dock space off air).
- **Title bar:** ✅ trim to Show menu (with Undo/Redo), Layers toggle +
  Editable/Locked toggle, quiet Health pill, clear `REC` button, primary
  `GO LIVE` / `LIVE <clock>` button, and Settings gear (dropping the search
  icon since `Ctrl+K` and Show menu cover it).
- **Shots & docs:** update `just shots` views (`studio`, `studio_locked`,
  `audio_popover`, and compact/portrait layouts) and update `README.md`.

## Required

- [Mods](/quest/m0/ux/mods.md) - gates Audio Scenes & Routes, Replay & Review, Studio Mode, and other power features

## Related

- [Essential layer inspector](/quest/m0/ux/essential-inspector.md) - contextual right-hand inspector for selected layers
- [Streamlined Go Live and Settings](/quest/m0/ux/golive-settings.md) - title bar Go Live / End Stream and Settings polish
- The quick start on an empty picture (`rs/moq-studio/src/ui/start.rs`), drawn on LIVE's and BUILD's pictures today, moves onto the unified canvas

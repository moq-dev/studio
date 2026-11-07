# [L] Effects in the studio UI

## Goal

The studio's own UI comes alive with GPU effects from
[rune-fx](https://github.com/anomalyco/rune/tree/dev/crates/rune-fx), drawn
inside egui:

- **On-air state:** the Program monitor glows or carries an electric border
  while the studio publishes, and recording shows a pulse.
- **Cut feedback:** a Cut or transition, from a button or a hotkey, fires a
  one-shot burst (shockwave, sparkles, or a zap) on the Program monitor.
- **Buttons and chrome:** the Cut button, the Hotkeys panel's keycaps, and
  panel and first-launch backgrounds get rune-fx fills (liquid metal,
  plastic, mesh or grain gradients).
- **Health alerts:** a failing uplink or a poor grade shows on the Health
  panel and its tab with electric flicker or a warning pulse, scaled to how
  bad it is.

UI effects never touch the program, a published scene, or any encode. They
are not scene effects and travel in no scene file or layout.

## Plan

Decided 2026-11-07 with the maintainer (/quest-plan):

- ✅ **rune-fx inside egui.** Depend on `rune-fx` at a pinned rev of rune's
  `dev` branch (Rune is pre-release and asks for a rev pin; `dev` is never
  rewritten), declared once in the root `Cargo.toml`. Draw its layers through
  `egui_wgpu` paint callbacks on the widgets above. rune-fx is on wgpu 30, as
  the studio is. Rejected: moving the UI to Rune now, which
  [the Rune spike](/quest/m1/rune-spike.md) evaluates instead.
- ✅ **All four areas** above are in scope. Rejected for now: scene-level
  effects (animated background sources and item decorations), video
  filters, and user-loadable shaders.
- ✅ **Turning effects down:** follow the OS reduced-motion setting by
  default, plus an Effects setting (Full, Reduced, Off) kept with the other
  editor settings. Reduced uses rune-fx's reduced mode, which settles one
  pose and stops animating.
- Rune takes no pull requests; report a rune-fx bug as an issue there and
  bump the rev once it is fixed.

Recommendations:

- Keep each effect quiet until its state changes: rune-fx skips a layer that
  is idle, finished, or off screen, so an idle studio repaints no more than
  today.
- Measure the UI's frame time and GPU cost with effects Full, Reduced, and
  Off, and show the program's render time is unchanged.
- `README.md` documents the Effects setting.

# [L] Interactive agent control and headless driving harness

## Goal

Let coding agents and automated tests launch, inspect, and interactively drive
`moq-studio` as a real user without hijacking a desktop session or editing
hardcoded `#[cfg(test)]` Rust enums:

1. **Real accessibility tree:** enable `eframe`'s `"accesskit"` feature in
   `rs/moq-studio/Cargo.toml` and ensure every interactive widget across
   `rs/moq-studio/src/kit/` and `rs/moq-studio/src/ui/` emits a clean,
   non-ambiguous AccessKit `WidgetInfo` role and label.
2. **Embedded control server (`--control <socket>` and `--headless`):**
   - `moq-studio --control <unix-socket>` exposes a local JSON-RPC (or newline
     JSON) control endpoint on a running window.
   - `moq-studio --headless --control <unix-socket>` runs the full app and
     compositor off-screen on wgpu (lavapipe or GPU) with no display server,
     stepping frames on demand and on engine ticks.
3. **CLI driver (`moq-studio ctl --socket <path> <cmd>`):**
   - `snapshot`: returns the live AccessKit tree (node IDs, roles, labels,
     enabled/checked states, bounding rects) plus structured JSON of current
     engine/show state (active scene, layers, selection, audio levels/mutes,
     on-air/recording status).
   - `screenshot <out.png>`: renders and writes the current live frame to PNG.
   - `click`, `right-click`, `double-click`, `hover`, `drag`, `type`, `key`,
     `scroll`: target any element by accessible label, AccessKit node ID, or
     `(x, y)` coordinates and step frames until settled.
   - `wait`: block until sources settle, a label appears/disappears, or state
     matches a condition (with timeout).
4. **Headless Wayland compositor in dev shell:** add `sway` and `grim` (and
   input helper if appropriate) to `flake.nix` so agents can also run real
   Wayland compositor frame-time measurements as `AGENTS.md` requires.

## Plan

Decided 2026-11-22 with the maintainer (/quest-plan) during the UX audit:

- **Control architecture:** ✅ in-process AccessKit + Unix-socket control
  server supporting both live windows (`--control`) and off-screen headless
  runs (`--headless --control`), paired with a thin `moq-studio ctl` subcommand
  (rejected: relying only on static `shots.rs` `#[cfg(test)]` enums; relying
  exclusively on external OS screen-scraping/ydotool which breaks across
  Wayland compositors and cannot inspect widget semantics).
- **Security boundary:** ✅ bind only to a user-owned Unix domain socket path
  explicitly passed via `--control` (never open a TCP port or enable control
  by default in normal user launches).
- **Widget audit:** ✅ verify every clickable/draggable control (layer rows,
  canvas selection handles, scene pads and `+`, inline audio sliders and gear
  buttons, modal cards, Settings toggles) has a distinct, predictable AccessKit
  label so `ctl snapshot` and `ctl click` work without brittle pixel guesses.
- **Verification:** add an integration test that launches `moq-studio
  --headless --control <tmp.sock>`, drives a full multi-step user journey via
  `moq-studio ctl` (open empty show -> add source -> select & inspect -> adjust
  audio slider -> take screenshot -> verify non-empty PNG and engine state),
  and exits cleanly.
- **Dev shell:** ✅ add `sway` and `grim` to Linux `devShells.default` in
  `flake.nix` so headless compositor benchmarks (`WLR_BACKENDS=headless sway`)
  work out of the box in `nix develop`.

## Related

- [Broadcaster UX](/quest/m0/ux/README.md) - the UX overhaul quests that agents can interactively verify using this harness

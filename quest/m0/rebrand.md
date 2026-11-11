# [S] Rebrand to kixel.live

## Goal

The product carries the kixel.live brand everywhere a user or a viewer sees
its name, renamed in one PR so the wire breaks once. The app and bundle ID is
already `live.kixel.studio`.

## Plan

Decided 2026-11-11 (maintainer, on
[moq-dev/studio#132](https://github.com/moq-dev/studio/pull/132); rejected:
renaming only the layout track then, renaming both wire names then).

Rename together:

- The "MoQ Studio" product name: the README title, the window title
  (`rs/moq-studio/src/main.rs`), the setup heading (`rs/moq-studio/src/setup.rs`),
  the PipeWire and v4l2loopback virtual camera names
  (`rs/moq-studio/src/vcam/pipewire.rs`, the README's `modprobe` line), the
  flake description, `js/join/package.json`, and `AGENTS.md`.
- The `dev.moq.studio` catalog section (`rs/moq-scene/src/publish.rs`).
- The `dev.moq.studio.layers` layout track (`rs/moq-scene/src/layout.rs`), and
  every quest and README line that names either wire name.

Recommended names: the product "kixel.live Studio" (virtual camera "kixel.live
Studio"), the catalog section `live.kixel.studio`, and the track
`live.kixel.studio.layers`, matching the bundle ID. The crate names
(`moq-scene`, `moq-studio`) and the repository stay, since they describe the
MoQ stack, not the product; rename them only if the maintainer asks.

No reader of the old names needs to keep working: no player ships yet, so the
rename takes no compatibility alias. Note the break in the PR's Impact.

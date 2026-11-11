# [S] The studio publishes hang's layer layout

## Goal

The `moq` submodule includes hang's layer layout track and `moq play`'s layer
compositing. The studio publishes hang's format instead of the provisional
`dev.moq.studio.layers`, and the `moq-studio --watch` prototype is gone, with
`README.md` pointing viewers at `moq play` instead.

## Plan

Decided 2026-11-08 (maintainer, /quest-plan): the layer layout becomes
official moq, so `moq play` and `@moq/watch` play a studio broadcast. Once
the wait clears, advance the gitlink with `just upgrade-moq` in this PR, move
the studio to hang's types, and delete the
`moq-studio --watch` prototype (`rs/moq-studio/src/watch.rs`) and its flag.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ the wait names moq's three
quests (rejected: "both quests", unnamed, which left out `@moq/watch`).

## Waiting

- moq-dev/moq: its three layer quests land, planned in moq's m3 from the
  `moq-studio --watch` prototype's format, the README's "Layer layout":
  [hang specifies a layer layout track](https://github.com/moq-dev/moq/blob/main/quest/m3/layers/spec.md),
  [moq play composites layers](https://github.com/moq-dev/moq/blob/main/quest/m3/layers/play.md),
  and
  [The watch player composites layers](https://github.com/moq-dev/moq/blob/main/quest/m3/layers/watch.md),
  which the [epic](/quest/m2/compose/README.md)'s test plays in. Check: all
  three quests' PRs have merged on moq's `main`, deleting their files, hang
  exposes a layer layout whose audio layers carry gain, mute, pan, and delay,
  and both players apply all four. A quest file that moved, or went away
  without its work landing, does not count.

## Required

- [Viewer controls](/quest/m2/compose/controls.md) - toggles and priorities the spec carries
- [The studio is caught up to moq's main](/quest/m0/pin-main.md) - moq's breaking changes since moq-dev/moq#8272, so this pin takes only its own

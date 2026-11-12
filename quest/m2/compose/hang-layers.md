# [S] The studio publishes hang's composition

## Goal

The `moq` submodule includes hang's composition and `moq play`'s compositing
of it. The studio publishes hang's format instead of its provisional one, and the `moq-studio --watch` prototype is gone, with
`README.md` pointing viewers at `moq play` instead.

## Plan

Decided 2026-11-08 (maintainer, /quest-plan): the layer layout becomes
official moq, so `moq play` and `@moq/watch` play a studio broadcast. Once
the wait clears, advance the gitlink with `just upgrade-moq` in this PR, move
the studio to hang's types, and delete the
`moq-studio --watch` prototype (`rs/moq-studio/src/watch.rs`) and its flag.

Decided 2026-11-09 (maintainer, /quest-audit): ✅ the wait names moq's three
quests (rejected: "both quests", unnamed, which left out `@moq/watch`).

Decided 2026-11-12 (maintainer, /quest-unblock): hang specifies a
composition, a broadcast listing a catalog per source, rather than a layer
layout track. The studio's prototype, which moq's spec starts from, is
`rs/moq-scene/src/composition.rs`, documented in the README's "Composition". moq's three quests below were planned for a
layer layout track and are replanned in moq's tree.

## Waiting

- moq-dev/moq: hang specifies the composition, and `moq play` and
  `@moq/watch` composite it, which the [epic](/quest/m2/compose/README.md)'s
  test plays in. moq's m3 plans this today as three layer quests,
  [hang specifies a layer layout track](https://github.com/moq-dev/moq/blob/main/quest/m3/layers/spec.md),
  [moq play composites layers](https://github.com/moq-dev/moq/blob/main/quest/m3/layers/play.md),
  and
  [The watch player composites layers](https://github.com/moq-dev/moq/blob/main/quest/m3/layers/watch.md),
  to be replanned there for the composition. Check: moq's `main` has hang's
  composition, whose sound entries carry gain, mute, pan, and delay, and both
  players apply all four. A quest file that moved or was replanned, without
  that work landing, does not count.

## Required

- [Viewer controls](/quest/m2/compose/controls.md) - toggles and priorities the spec carries
- [The studio is caught up to moq's main](/quest/m0/pin-main.md) - moq's breaking changes since moq-dev/moq#8272, so this pin takes only its own

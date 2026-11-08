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
[watch prototype](/quest/m2/compose/watch.md) and its flag.

## Waiting

- moq-dev/moq: hang's layer layout spec and `moq play`'s layer compositing
  land, planned in moq's m3 from the
  [watch prototype](/quest/m2/compose/watch.md)'s format. Check: both quests'
  PRs have merged on moq's `main`, deleting their files, and hang exposes the
  layer layout. A quest file that moved, or went away without its work
  landing, does not count.

## Required

- [moq-studio watch prototype](/quest/m2/compose/watch.md) - the format moq specifies
- [Viewer controls](/quest/m2/compose/controls.md) - toggles and priorities the spec carries
